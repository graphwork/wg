#!/usr/bin/env python3
"""
TB Pilot: Qwen3-Coder-30B local (SGLang on lambda01), 10 diverse tasks.

Condition A (agent-only, no workgraph decomposition).
Uses the native wg executor with local:qwen3-coder-30b model spec,
routing to http://lambda01:30000/v1 via [native_executor] api_base config.

Usage:
    python run_pilot_qwen3_local_10.py
    python run_pilot_qwen3_local_10.py --smoke    # single task quick check
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

from wg.tasks import TASKS_BY_ID, ALL_TASKS

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

MODEL = "local:qwen3-coder-30b"
SGLANG_BASE_URL = "http://lambda01:30000/v1"
CONTEXT_WINDOW = 32768
MAX_AGENTS = 1
DEFAULT_TIMEOUT = 2400  # 40 min per trial (longer for slow local model)
DEFAULT_POLL_INTERVAL = 5.0

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
RUN_ID = "pilot-qwen3-local-10"
RESULTS_DIR = os.path.join(SCRIPT_DIR, "results", RUN_ID)

WG_BIN = shutil.which("wg") or os.path.expanduser("~/.cargo/bin/wg")

# 10 diverse tasks: 2 easy, 3 medium, 5 hard
# Spanning file ops, text, debugging, scripting, data, algorithms, ML, systems
PILOT_TASKS = [
    # Easy (2)
    "file-ops",
    "text-processing",
    # Medium (3)
    "debugging",
    "shell-scripting",
    "data-processing",
    # Hard (5)
    "algorithm",
    "ml",
    "sysadmin",
    "configure-git-webserver",
    "mailman",
]


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

async def exec_wg(wg_dir: str, subcmd: list[str], timeout: float = 120,
                  extra_env: dict | None = None) -> str:
    """Execute a wg command against a specific graph directory."""
    cmd = [WG_BIN, "--dir", wg_dir] + subcmd
    env = {k: v for k, v in os.environ.items()
           if not k.startswith("WG_") and k != "CLAUDECODE"}
    if extra_env:
        env.update(extra_env)
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


def write_trial_config(wg_dir: str) -> None:
    """Write config.toml for a Condition A trial against lambda01 SGLang."""
    config = f"""[coordinator]
max_agents = {MAX_AGENTS}
executor = "native"
model = "{MODEL}"
worktree_isolation = false
agent_timeout = "40m"
max_verify_failures = 0
max_spawn_failures = 0

[agent]
model = "{MODEL}"
context_scope = "clean"
exec_mode = "full"

[agency]
auto_assign = false
auto_evaluate = false

[native_executor]
api_base = "{SGLANG_BASE_URL}"
context_window = {CONTEXT_WINDOW}
"""
    with open(os.path.join(wg_dir, "config.toml"), "w") as f:
        f.write(config)


# ---------------------------------------------------------------------------
# Trial runner
# ---------------------------------------------------------------------------

async def run_trial(
    task_def: dict,
    timeout: float,
) -> dict:
    """Run a single Condition A trial with per-trial isolation."""
    task_id = task_def["id"]
    trial_id = f"{RUN_ID}-{task_id}"
    result = {
        "trial_id": trial_id,
        "task": task_id,
        "difficulty": task_def["difficulty"],
        "model": MODEL,
        "endpoint": SGLANG_BASE_URL,
        "status": "not_started",
        "elapsed_s": 0.0,
        "reward": 0.0,
        "failure_mode": None,
        "metrics": None,
        "error": None,
    }

    # Clean up /tmp paths from previous runs of same task
    cleanup_tmp_paths(task_def.get("tmp_paths", []))

    tmpdir = tempfile.mkdtemp(prefix=f"tb-qwen3-{task_id}-")
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

        # 2. Write config
        write_trial_config(wg_dir)

        # 3. Load instruction and create root task
        instruction = load_instruction(task_def)
        root_task_id = f"tb-{task_id}"
        description = (
            f"## Terminal Bench Trial (Qwen3-Coder-30B Local)\n\n"
            f"**Task:** {task_id} ({task_def['difficulty']})\n"
            f"**Model:** {MODEL}\n"
            f"**Endpoint:** {SGLANG_BASE_URL}\n"
            f"**Context Window:** {CONTEXT_WINDOW}\n\n"
            f"## Instructions\n\n{instruction}\n"
        )

        add_out = await exec_wg(wg_dir, [
            "add", f"TB: {task_def['title']}",
            "--id", root_task_id,
            "-d", description,
            "--verify", task_def["verify_cmd"],
            "--exec-mode", "full",
            "--context-scope", "clean",
            "--model", MODEL,
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
            "--model", MODEL,
            "--no-coordinator-agent",
            "--force",
        ])
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

        print(f"  [{trial_id}] Completed: {status} in {elapsed:.1f}s "
              f"(reward={result['reward']})")

        # 6. Stop service
        await exec_wg(wg_dir, ["service", "stop", "--kill-agents"])

        # 7. Collect metrics
        result["metrics"] = await collect_metrics(wg_dir)

        # 8. Check daemon log for errors
        daemon_log = os.path.join(wg_dir, "service", "daemon.log")
        if os.path.isfile(daemon_log):
            try:
                with open(daemon_log) as f:
                    log_content = f.read()
                # Check for OOM or endpoint errors
                for pattern in ["OOM", "out of memory", "CUDA", "connection refused",
                                "Connection refused"]:
                    if pattern.lower() in log_content.lower():
                        result["failure_mode"] = f"endpoint_error:{pattern}"
                        result["error"] = f"{pattern} detected in daemon log"
                        break
                # Save last 3000 chars of log
                result["daemon_log_tail"] = log_content[-3000:]
            except Exception:
                pass

    except Exception as e:
        result["status"] = "error"
        result["error"] = str(e)
        result["failure_mode"] = "exception"
        print(f"  [{trial_id}] Error: {e}")
        try:
            await exec_wg(wg_dir, ["service", "stop", "--kill-agents"])
        except Exception:
            pass
    finally:
        result["elapsed_s"] = round(time.monotonic() - start, 2)
        # Save workgraph state before cleanup
        state_dst = os.path.join(RESULTS_DIR, trial_id, "workgraph_state")
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

async def main(timeout: float, tasks: list[str] | None = None):
    task_names = tasks or PILOT_TASKS
    total = len(task_names)

    # Verify endpoint is reachable
    import urllib.request
    try:
        with urllib.request.urlopen(f"{SGLANG_BASE_URL}/models", timeout=10) as resp:
            models_data = json.loads(resp.read())
            model_ids = [m["id"] for m in models_data.get("data", [])]
            if "qwen3-coder-30b" not in model_ids:
                print(f"ERROR: qwen3-coder-30b not found at {SGLANG_BASE_URL}")
                print(f"  Available models: {model_ids}")
                sys.exit(1)
            print(f"Endpoint OK: qwen3-coder-30b available at {SGLANG_BASE_URL}")
    except Exception as e:
        print(f"ERROR: Cannot reach {SGLANG_BASE_URL}: {e}")
        sys.exit(1)

    print(f"\nTB Pilot: Qwen3-Coder-30B Local (SGLang)")
    print(f"  Model: {MODEL}")
    print(f"  Endpoint: {SGLANG_BASE_URL}")
    print(f"  Context window: {CONTEXT_WINDOW}")
    print(f"  Run ID: {RUN_ID}")
    print(f"  Tasks ({total}): {task_names}")
    print(f"  Timeout: {timeout}s per trial")
    print(f"  wg binary: {WG_BIN}")
    print()

    results = []
    start_time = time.monotonic()

    for i, task_name in enumerate(task_names, 1):
        if task_name not in TASKS_BY_ID:
            print(f"  WARNING: Unknown task '{task_name}', skipping")
            continue

        task_def = TASKS_BY_ID[task_name]
        print(f"\n--- [{i}/{total}] Task: {task_name} ({task_def['difficulty']}) ---")

        result = await run_trial(task_def, timeout)
        results.append(result)

        # Print running tally
        passed_so_far = sum(1 for r in results if r["reward"] > 0)
        print(f"  Running: {passed_so_far}/{len(results)} passed")

    total_time = time.monotonic() - start_time

    # Compute statistics
    passed = sum(1 for r in results if r["reward"] > 0)
    total_trials = len(results)
    times = [r["elapsed_s"] for r in results if r["elapsed_s"] > 0]
    mean_time = sum(times) / len(times) if times else 0
    median_time = sorted(times)[len(times) // 2] if times else 0

    # Token throughput
    total_input = sum(r.get("metrics", {}).get("total_input_tokens", 0) for r in results if r.get("metrics"))
    total_output = sum(r.get("metrics", {}).get("total_output_tokens", 0) for r in results if r.get("metrics"))
    total_turns = sum(r.get("metrics", {}).get("total_turns", 0) for r in results if r.get("metrics"))

    # Failure mode breakdown
    failure_modes = {}
    for r in results:
        mode = r.get("failure_mode") or "success"
        failure_modes[mode] = failure_modes.get(mode, 0) + 1

    summary = {
        "run_id": RUN_ID,
        "model": MODEL,
        "endpoint": SGLANG_BASE_URL,
        "context_window": CONTEXT_WINDOW,
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "tasks": task_names,
        "total_trials": total_trials,
        "passed": passed,
        "pass_rate": passed / total_trials if total_trials > 0 else 0,
        "mean_time_s": round(mean_time, 2),
        "median_time_s": round(median_time, 2),
        "total_wall_clock_s": round(total_time, 2),
        "total_input_tokens": total_input,
        "total_output_tokens": total_output,
        "total_turns": total_turns,
        "tokens_per_second": round(total_output / total_time, 2) if total_time > 0 else 0,
        "failure_modes": failure_modes,
        "trials": results,
    }

    # Write results
    os.makedirs(RESULTS_DIR, exist_ok=True)

    json_path = os.path.join(RESULTS_DIR, "summary.json")
    with open(json_path, "w") as f:
        json.dump(summary, f, indent=2)

    # Print summary
    print(f"\n{'='*70}")
    print(f"SUMMARY: {RUN_ID}")
    print(f"{'='*70}")
    print(f"  Model: {MODEL}")
    print(f"  Endpoint: {SGLANG_BASE_URL}")
    print(f"  Context window: {CONTEXT_WINDOW}")
    print(f"  Pass rate: {passed}/{total_trials} ({passed/total_trials:.0%})" if total_trials else "  No trials")
    print(f"  Mean time per trial: {mean_time:.1f}s")
    print(f"  Median time per trial: {median_time:.1f}s")
    print(f"  Total wall clock: {total_time:.1f}s ({total_time/60:.1f}m)")
    print(f"  Total tokens: {total_input} in + {total_output} out = {total_input + total_output}")
    print(f"  Total turns: {total_turns}")
    print(f"  Effective throughput: {summary['tokens_per_second']:.1f} tok/s (output)")
    print(f"  Failure modes: {failure_modes}")
    print()
    print(f"  Per-task results:")
    for r in results:
        turns = r.get("metrics", {}).get("total_turns", 0) if r.get("metrics") else 0
        print(f"    {r['task']:30s} reward={r['reward']:.1f}  time={r['elapsed_s']:7.1f}s  "
              f"turns={turns:2d}  status={r['status']:8s}  "
              f"failure={r.get('failure_mode', 'none')}")
    print()
    print(f"  Results written to: {json_path}")

    return summary


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="TB Pilot: Qwen3-Coder-30B Local")
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT,
                        help="Per-trial timeout in seconds (default: 2400)")
    parser.add_argument("--tasks", nargs="*", help="Override task list")
    parser.add_argument("--smoke", action="store_true",
                        help="Run single task for quick validation")
    args = parser.parse_args()

    tasks = args.tasks
    if args.smoke:
        tasks = ["text-processing"]

    summary = asyncio.run(main(
        timeout=args.timeout,
        tasks=tasks,
    ))
    sys.exit(0 if summary["passed"] > 0 else 1)
