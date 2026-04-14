#!/usr/bin/env python3
"""
TB Pilot Runner: Free model smoke test (5 diverse tasks)

Runs 5 diverse terminalbench tasks through a free OpenRouter model using
the native wg executor. Each trial gets its own isolated workgraph.

Designed for comparability across free models — all free-model pilots
should use the SAME 5 tasks (defined in SMOKE_TASKS below).

Usage:
    python run_pilot_free_smoke.py --model "openrouter:qwen/qwen3-coder:free" \
        --run-id pilot-qwen3-coder-free
"""

import argparse
import asyncio
import json
import os
import shutil
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

from wg.daemon_cleanup import daemon_registry
from wg.tasks import TASKS_BY_ID

# ---------------------------------------------------------------------------
# Canonical 5-task set for free-model smoke tests
# Chosen for diversity: algorithm, data processing, systems, debugging, ML
# ---------------------------------------------------------------------------
SMOKE_TASKS = ["algorithm", "data-processing", "sysadmin", "debugging", "ml"]

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
DEFAULT_TIMEOUT = 1800  # 30 minutes per trial
DEFAULT_POLL_INTERVAL = 5.0

WG_BIN = shutil.which("wg") or os.path.expanduser("~/.cargo/bin/wg")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

async def exec_wg(wg_dir: str, subcmd: list[str], timeout: float = 120) -> str:
    """Execute a wg command against a specific graph directory."""
    cmd = [WG_BIN, "--dir", wg_dir] + subcmd
    env = {k: v for k, v in os.environ.items()
           if not k.startswith("WG_") and k != "CLAUDECODE"}
    try:
        proc = await asyncio.create_subprocess_exec(
            *cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=env,
        )
        stdout, stderr = await asyncio.wait_for(proc.communicate(), timeout=timeout)
        parts = []
        if stdout:
            parts.append(stdout.decode(errors="replace"))
        if stderr:
            parts.append(stderr.decode(errors="replace"))
        if proc.returncode != 0:
            parts.append(f"[exit code: {proc.returncode}]")
        return "\n".join(parts) if parts else "(no output)"
    except asyncio.TimeoutError:
        return f"[wg command timed out after {timeout}s]"
    except Exception as e:
        return f"[wg command error: {e}]"


def load_instruction(task_def: dict) -> str:
    """Load task instruction from file."""
    path = os.path.join(SCRIPT_DIR, task_def["instruction_file"])
    with open(path) as f:
        return f.read().strip()


def cleanup_tmp_paths(paths: list[str]) -> None:
    """Remove /tmp files from a previous trial to ensure isolation."""
    for p in paths:
        if os.path.isdir(p):
            shutil.rmtree(p, ignore_errors=True)
        elif os.path.isfile(p):
            os.remove(p)


async def poll_completion(
    wg_dir: str,
    task_id: str,
    timeout_secs: float,
    poll_interval: float = DEFAULT_POLL_INTERVAL,
) -> tuple[str, float]:
    """Poll `wg show` until the task reaches a terminal status."""
    start = time.monotonic()
    terminal = {"done", "failed", "abandoned"}

    while True:
        elapsed = time.monotonic() - start
        if elapsed > timeout_secs:
            return "timeout", elapsed

        result = await exec_wg(wg_dir, ["show", task_id])
        for line in result.splitlines():
            s = line.strip()
            if s.startswith("Status:"):
                status = s.split(":", 1)[1].strip().lower()
                if status in terminal:
                    return status, elapsed
                break

        await asyncio.sleep(poll_interval)


async def collect_metrics(wg_dir: str) -> dict:
    """Read agent stream.jsonl files to extract token counts."""
    agents_dir = os.path.join(wg_dir, "agents")
    metrics = {
        "total_input_tokens": 0,
        "total_output_tokens": 0,
        "total_cost_usd": 0.0,
        "total_turns": 0,
    }

    if not os.path.isdir(agents_dir):
        return metrics

    for agent_id in os.listdir(agents_dir):
        stream_path = os.path.join(agents_dir, agent_id, "stream.jsonl")
        if not os.path.isfile(stream_path):
            continue

        try:
            with open(stream_path) as f:
                for line in f:
                    line = line.strip()
                    if not line:
                        continue
                    try:
                        event = json.loads(line)
                    except json.JSONDecodeError:
                        continue

                    if event.get("type") == "turn":
                        metrics["total_turns"] += 1
                        usage = event.get("usage")
                        if usage:
                            metrics["total_input_tokens"] += usage.get("input_tokens", 0)
                            metrics["total_output_tokens"] += usage.get("output_tokens", 0)
                    elif event.get("type") == "result":
                        usage = event.get("usage", {})
                        cost = usage.get("cost_usd")
                        if cost:
                            metrics["total_cost_usd"] += cost
        except Exception:
            pass

    return metrics


async def check_verify(wg_dir: str, task_id: str) -> tuple[bool, str]:
    """Check if the verify command passed by looking at task logs."""
    result = await exec_wg(wg_dir, ["show", task_id])
    status = "unknown"
    for line in result.splitlines():
        s = line.strip()
        if s.startswith("Status:"):
            status = s.split(":", 1)[1].strip().lower()
            break
    return status == "done", result


# ---------------------------------------------------------------------------
# Trial runner
# ---------------------------------------------------------------------------

async def run_trial(
    task_def: dict,
    model: str,
    run_id: str,
    timeout: float,
) -> dict:
    """Run a single free-model trial with per-trial isolation."""
    task_id = task_def["id"]
    trial_id = f"{run_id}-{task_id}"
    result = {
        "trial_id": trial_id,
        "task": task_id,
        "difficulty": task_def["difficulty"],
        "model": model,
        "status": "not_started",
        "elapsed_s": 0.0,
        "reward": 0.0,
        "failure_mode": None,
        "metrics": None,
        "error": None,
    }

    # Clean up /tmp paths from previous runs of same task
    cleanup_tmp_paths(task_def.get("tmp_paths", []))

    tmpdir = tempfile.mkdtemp(prefix=f"tb-free-{trial_id}-")
    wg_dir = os.path.join(tmpdir, ".workgraph")
    start = time.monotonic()

    print(f"  [{trial_id}] Starting trial in {tmpdir}...")

    try:
        # 1. Init graph
        init_out = await exec_wg(wg_dir, ["init"])
        if "error" in init_out.lower() and "already" not in init_out.lower():
            result["error"] = f"Init failed: {init_out}"
            result["status"] = "failed"
            result["failure_mode"] = "init_error"
            return result

        # 2. Write config (native executor, clean context)
        config_content = (
            "[coordinator]\n"
            f'max_agents = 1\n'
            f'executor = "native"\n'
            f'model = "{model}"\n'
            f'worktree_isolation = false\n'
            f'max_verify_failures = 0\n'
            f'max_spawn_failures = 0\n'
            "\n"
            "[agent]\n"
            f'model = "{model}"\n'
            f'context_scope = "clean"\n'
            f'exec_mode = "full"\n'
            "\n"
            "[agency]\n"
            f'auto_assign = false\n'
            f'auto_evaluate = false\n'
        )
        with open(os.path.join(wg_dir, "config.toml"), "w") as f:
            f.write(config_content)

        # 3. Load instruction and create root task
        instruction = load_instruction(task_def)
        root_task_id = f"tb-{task_id}"
        description = (
            f"## Terminal Bench Trial (Free Model Smoke)\n\n"
            f"**Task:** {task_id} ({task_def['difficulty']})\n"
            f"**Model:** {model}\n\n"
            f"## Instructions\n\n{instruction}\n"
        )

        add_out = await exec_wg(wg_dir, [
            "add", f"TB: {task_def['title']}",
            "--id", root_task_id,
            "-d", description,
            "--verify", task_def["verify_cmd"],
            "--exec-mode", "full",
            "--context-scope", "clean",
            "--model", model,
            "--no-place",
        ])
        if "[exit code:" in add_out and root_task_id not in add_out:
            result["error"] = f"Task creation failed: {add_out}"
            result["status"] = "failed"
            result["failure_mode"] = "task_create_error"
            return result

        # 4. Start wg service
        service_out = await exec_wg(wg_dir, [
            "service", "start",
            "--max-agents", "1",
            "--executor", "native",
            "--model", model,
            "--no-coordinator-agent",
            "--force",
        ])
        daemon_registry.register(wg_dir, WG_BIN)
        print(f"  [{trial_id}] Service started, polling for completion...")

        # 5. Poll for completion
        status, elapsed = await poll_completion(wg_dir, root_task_id, timeout)
        result["status"] = status
        result["elapsed_s"] = round(elapsed, 2)

        if status == "done":
            result["reward"] = 1.0
            result["failure_mode"] = None
        elif status == "timeout":
            result["failure_mode"] = "timeout"
        elif status == "failed":
            result["failure_mode"] = "task_failed"
        else:
            result["failure_mode"] = f"status_{status}"

        print(f"  [{trial_id}] Completed: {status} in {elapsed:.1f}s (reward={result['reward']})")

        # 6. Stop service (daemon_registry.stop_one in finally is the safety net)
        daemon_registry.stop_one(wg_dir)

        # 7. Collect metrics
        result["metrics"] = await collect_metrics(wg_dir)

        # 8. Grab daemon log for debugging
        daemon_log = os.path.join(wg_dir, "service", "daemon.log")
        if os.path.isfile(daemon_log):
            try:
                with open(daemon_log) as f:
                    log_content = f.read()
                # Check for rate-limit or billing errors
                if "429" in log_content or "402" in log_content:
                    result["failure_mode"] = "rate_limit_or_billing"
                    result["error"] = "API rate limit (429) or billing (402) error detected in daemon log"
                # Save last 2000 chars of log
                result["daemon_log_tail"] = log_content[-2000:]
            except Exception:
                pass

    except Exception as e:
        result["status"] = "error"
        result["error"] = str(e)
        result["failure_mode"] = "exception"
        print(f"  [{trial_id}] Error: {e}")
    finally:
        # Always stop the daemon before cleanup (handles both normal and error paths)
        daemon_registry.stop_one(wg_dir)
        result["elapsed_s"] = round(time.monotonic() - start, 2)
        # Save workgraph state before cleanup
        state_dst = os.path.join(
            SCRIPT_DIR, "results", run_id, trial_id, "workgraph_state"
        )
        try:
            os.makedirs(os.path.dirname(state_dst), exist_ok=True)
            if os.path.isdir(wg_dir):
                shutil.copytree(wg_dir, state_dst)
        except Exception:
            pass

        shutil.rmtree(tmpdir, ignore_errors=True)

    return result


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

async def main(model: str, run_id: str, timeout: float, tasks: list[str] | None = None):
    task_names = tasks or SMOKE_TASKS
    total = len(task_names)

    print(f"TB Pilot: Free Model Smoke Test")
    print(f"  Model: {model}")
    print(f"  Run ID: {run_id}")
    print(f"  Tasks ({total}): {task_names}")
    print(f"  Timeout: {timeout}s per trial")
    print(f"  wg binary: {WG_BIN}")
    print()

    results = []
    start_time = time.monotonic()

    for task_name in task_names:
        if task_name not in TASKS_BY_ID:
            print(f"  WARNING: Unknown task '{task_name}', skipping")
            continue

        task_def = TASKS_BY_ID[task_name]
        print(f"\n--- Task: {task_name} ({task_def['difficulty']}) ---")

        result = await run_trial(task_def, model, run_id, timeout)
        results.append(result)

    total_time = time.monotonic() - start_time

    # Compute statistics
    passed = sum(1 for r in results if r["reward"] > 0)
    total_trials = len(results)
    times = [r["elapsed_s"] for r in results if r["elapsed_s"] > 0]
    mean_time = sum(times) / len(times) if times else 0

    # Failure mode breakdown
    failure_modes = {}
    for r in results:
        mode = r.get("failure_mode") or "success"
        failure_modes[mode] = failure_modes.get(mode, 0) + 1

    summary = {
        "run_id": run_id,
        "model": model,
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "tasks": task_names,
        "total_trials": total_trials,
        "passed": passed,
        "pass_rate": passed / total_trials if total_trials > 0 else 0,
        "mean_time_s": round(mean_time, 2),
        "total_wall_clock_s": round(total_time, 2),
        "failure_modes": failure_modes,
        "trials": results,
    }

    # Write results
    results_dir = os.path.join(SCRIPT_DIR, "results", run_id)
    os.makedirs(results_dir, exist_ok=True)

    json_path = os.path.join(results_dir, "summary.json")
    with open(json_path, "w") as f:
        json.dump(summary, f, indent=2)

    # Print summary
    print(f"\n{'='*60}")
    print(f"SUMMARY: {run_id}")
    print(f"{'='*60}")
    print(f"  Model: {model}")
    print(f"  Pass rate: {passed}/{total_trials} ({passed/total_trials:.0%})")
    print(f"  Mean time per trial: {mean_time:.1f}s")
    print(f"  Total wall clock: {total_time:.1f}s")
    print(f"  Failure modes: {failure_modes}")
    print()
    print(f"  Per-task results:")
    for r in results:
        print(f"    {r['task']:25s} reward={r['reward']:.1f}  time={r['elapsed_s']:.1f}s  "
              f"status={r['status']}  failure={r.get('failure_mode', 'none')}")
    print()
    print(f"  Results written to: {json_path}")

    return summary


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="TB Free Model Smoke Test")
    parser.add_argument("--model", required=True, help="Model in provider:name format")
    parser.add_argument("--run-id", required=True, help="Unique run identifier")
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT, help="Per-trial timeout")
    parser.add_argument("--tasks", nargs="*", help="Override task list")
    args = parser.parse_args()

    summary = asyncio.run(main(
        model=args.model,
        run_id=args.run_id,
        timeout=args.timeout,
        tasks=args.tasks,
    ))
    sys.exit(0 if summary["passed"] > 0 else 1)
