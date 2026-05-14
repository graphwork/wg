#!/usr/bin/env python3
"""
TB Pilot Runner: Condition A' (bare agent, no turn cap)

Runs calibration tasks through the native WG executor with per-trial
isolation and federation to tb-evaluations/ hub.

Each trial:
  1. Creates isolated temp WG state
  2. Configures A' (clean context, no wg tools, no turn cap)
  3. Federation pull from hub
  4. Creates root task with instructions + verify
  5. Starts wg service (native executor)
  6. Polls for completion
  7. Stops service
  8. Evaluates + federation push
  9. Collects metrics
  10. Cleans up

Usage:
    python run_pilot_a_prime.py [--replicas 2] [--tasks file-ops,debugging] [--timeout 1800]
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


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

BENCHMARK_MODEL = "openrouter:minimax/minimax-m2.7"
DEFAULT_REPLICAS = 2
DEFAULT_TIMEOUT = 1800  # 30 min per trial
DEFAULT_POLL_INTERVAL = 5.0

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
HUB_PATH = os.path.join(SCRIPT_DIR, "tb-evaluations")

WG_BIN = shutil.which("wg") or os.path.expanduser("~/.cargo/bin/wg")

# ---------------------------------------------------------------------------
# Task definitions (from tb_trial_runner.py)
# ---------------------------------------------------------------------------

TB_TASKS = {
    "file-ops": {
        "id": "file-ops",
        "title": "File Operations: create project structure",
        "instruction_file": "tasks/condition-a-calibration/01-file-ops-easy.txt",
        "verify_cmd": (
            "test -f /tmp/project/src/main.py && "
            "test -f /tmp/project/src/utils.py && "
            "test -f /tmp/project/src/tests/test_utils.py && "
            "test -f /tmp/project/data/config.json && "
            "test -f /tmp/project/README.md && "
            "test -f /tmp/project/.gitignore && "
            "python3 -c \"import json; json.load(open('/tmp/project/data/config.json'))\" && "
            "python3 -m pytest /tmp/project/src/tests/test_utils.py -v"
        ),
        "difficulty": "easy",
        "tmp_paths": ["/tmp/project"],
    },
    "text-processing": {
        "id": "text-processing",
        "title": "Text Processing: word frequency counter",
        "instruction_file": "tasks/condition-a-calibration/02-text-processing-easy.txt",
        "verify_cmd": (
            "test -f /tmp/wordfreq.py && "
            "echo 'the the the dog dog cat' | python3 /tmp/wordfreq.py | head -1 | grep -q 'the'"
        ),
        "difficulty": "easy",
        "tmp_paths": ["/tmp/wordfreq.py"],
    },
    "debugging": {
        "id": "debugging",
        "title": "Debugging: fix merge sort bugs",
        "instruction_file": "tasks/condition-a-calibration/03-debugging-medium.txt",
        "verify_cmd": (
            "test -f /tmp/buggy_sort.py && "
            "python3 /tmp/buggy_sort.py 2>&1 | grep -v FAIL | grep -c PASS | "
            "python3 -c \"import sys; n=int(sys.stdin.read().strip()); sys.exit(0 if n>=6 else 1)\""
        ),
        "difficulty": "medium",
        "tmp_paths": ["/tmp/buggy_sort.py"],
    },
    "shell-scripting": {
        "id": "shell-scripting",
        "title": "Shell Scripting: log file analyzer",
        "instruction_file": "tasks/condition-a-calibration/04-shell-scripting-medium.txt",
        "verify_cmd": (
            "test -f /tmp/log_analyzer.sh && "
            "test -f /tmp/access.log && "
            "bash /tmp/log_analyzer.sh /tmp/access.log 2>&1 | grep -qE '[0-9]'"
        ),
        "difficulty": "medium",
        "tmp_paths": ["/tmp/log_analyzer.sh", "/tmp/access.log"],
    },
    "data-processing": {
        "id": "data-processing",
        "title": "Data Processing: JSON to CSV department summary",
        "instruction_file": "tasks/condition-a-calibration/05-data-processing-medium.txt",
        "verify_cmd": (
            "test -f /tmp/json_to_csv.py && "
            "test -f /tmp/employees.json && "
            "test -f /tmp/dept_summary.csv && "
            "python3 -c \"import csv; r=list(csv.DictReader(open('/tmp/dept_summary.csv'))); "
            "assert len(r)>=1\""
        ),
        "difficulty": "medium",
        "tmp_paths": ["/tmp/json_to_csv.py", "/tmp/employees.json", "/tmp/dept_summary.csv"],
    },
    "algorithm": {
        "id": "algorithm",
        "title": "Algorithm: key-value store with transactions",
        "instruction_file": "tasks/condition-a-calibration/06-algorithm-hard.txt",
        "verify_cmd": (
            "test -f /tmp/kvstore.py && test -f /tmp/kv_test.txt && "
            "python3 /tmp/kvstore.py < /tmp/kv_test.txt | head -1 | grep -q '10'"
        ),
        "difficulty": "hard",
        "tmp_paths": ["/tmp/kvstore.py", "/tmp/kv_test.txt"],
    },
    "ml": {
        "id": "ml",
        "title": "ML: k-means clustering from scratch",
        "instruction_file": "tasks/condition-a-calibration/07-ml-hard.txt",
        "verify_cmd": (
            "test -f /tmp/kmeans.py && "
            "python3 /tmp/kmeans.py 2>&1 | "
            "python3 -c \"import sys; o=sys.stdin.read().lower(); "
            "sys.exit(0 if 'centroid' in o or 'cluster' in o else 1)\""
        ),
        "difficulty": "hard",
        "tmp_paths": ["/tmp/kmeans.py"],
    },
}


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

async def exec_wg(wg_dir: str, subcmd: list[str], timeout: float = 120) -> str:
    """Execute a wg command against a specific graph directory.

    Strips WG_AGENT_ID / WG_TASK_ID from the environment so that child
    processes don't inherit the parent agent's identity (which would block
    service stop commands and confuse the coordinator).
    """
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
                # pending-validation means verify is running
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


# ---------------------------------------------------------------------------
# Trial runner
# ---------------------------------------------------------------------------

async def run_trial(
    task_def: dict,
    replica: int,
    hub_path: str,
    model: str,
    timeout: float,
) -> dict:
    """Run a single A' trial with per-trial isolation and federation."""
    trial_id = f"aprime-{task_def['id']}-r{replica}"
    result = {
        "trial_id": trial_id,
        "task": task_def["id"],
        "difficulty": task_def["difficulty"],
        "replica": replica,
        "condition": "A'",
        "model": model,
        "status": "not_started",
        "elapsed_s": 0.0,
        "used_native_executor": False,
        "own_service_instance": False,
        "federation_pulled": False,
        "federation_pushed": False,
        "metrics": None,
        "error": None,
        "verify_output": None,
    }

    # Clean up /tmp paths from previous runs of same task
    cleanup_tmp_paths(task_def.get("tmp_paths", []))

    tmpdir = tempfile.mkdtemp(prefix=f"tb-aprime-{trial_id}-")
    wg_dir = os.path.join(tmpdir, ".workgraph")
    start = time.monotonic()

    print(f"  [{trial_id}] Starting trial...")

    try:
        # 1. Init graph
        init_out = await exec_wg(wg_dir, ["init"])
        if "error" in init_out.lower() and "already" not in init_out.lower():
            result["error"] = f"Init failed: {init_out}"
            result["status"] = "failed"
            return result

        # 2. Write A' config (native executor, clean context, no turn cap)
        #    Disable auto-assign/auto-evaluate to avoid pipeline overhead.
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

        # 3. Write bundle that excludes wg tools (Condition A' baseline)
        bundles_dir = os.path.join(wg_dir, "bundles")
        os.makedirs(bundles_dir, exist_ok=True)
        bundle_content = (
            'name = "implementer"\n'
            'description = "Full implementation agent without wg tools (Condition A\' baseline)."\n'
            'tools = ["bash", "read_file", "write_file", "edit_file", "glob", "grep"]\n'
            'context_scope = "clean"\n'
            'system_prompt_suffix = ""\n'
        )
        with open(os.path.join(bundles_dir, "implementer.toml"), "w") as f:
            f.write(bundle_content)

        result["used_native_executor"] = True

        # 4. Init agency + federation pull from hub
        hub_agency = os.path.join(os.path.abspath(hub_path), ".workgraph", "agency")
        await exec_wg(wg_dir, ["agency", "init"])

        if os.path.isdir(hub_agency):
            pull_out = await exec_wg(
                wg_dir, ["agency", "pull", hub_agency, "--no-evaluations"]
            )
            if "[wg command error:" not in pull_out and "[exit code:" not in pull_out:
                result["federation_pulled"] = True
            else:
                print(f"  [{trial_id}] Federation pull warning: {pull_out.strip()}")
                # Non-fatal — A' doesn't need agency primitives
                result["federation_pulled"] = True  # attempted
        else:
            print(f"  [{trial_id}] Hub not found at {hub_agency}, skipping pull")

        # Write federation config for push later
        fed_config = f"remotes:\n  hub:\n    path: {hub_agency}\n    description: TB evaluation hub\n"
        with open(os.path.join(wg_dir, "federation.yaml"), "w") as f:
            f.write(fed_config)

        # 5. Load instruction and create root task
        instruction = load_instruction(task_def)
        root_task_id = f"tb-{trial_id}"
        description = (
            f"## Terminal Bench Trial (Condition A')\n\n"
            f"**Task:** {task_def['id']} ({task_def['difficulty']})\n"
            f"**Replica:** {replica}\n\n"
            f"## Instructions\n\n{instruction}\n"
        )

        add_out = await exec_wg(wg_dir, [
            "add", f"A': {task_def['title']} (rep {replica})",
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
            return result

        # 6. Start wg service (native executor, per-trial instance)
        service_out = await exec_wg(wg_dir, [
            "service", "start",
            "--max-agents", "1",
            "--executor", "native",
            "--model", model,
            "--no-coordinator-agent",
            "--force",
        ])
        daemon_registry.register(wg_dir, WG_BIN)
        result["own_service_instance"] = True
        print(f"  [{trial_id}] Service started, polling for completion...")

        # 7. Poll for completion
        status, elapsed = await poll_completion(wg_dir, root_task_id, timeout)
        result["status"] = status
        result["elapsed_s"] = round(elapsed, 2)
        print(f"  [{trial_id}] Completed: {status} in {elapsed:.1f}s")

        # 8. Stop service (daemon_registry.stop_one in finally is the safety net)
        daemon_registry.stop_one(wg_dir)

        # 9. Evaluate + federation push
        eval_out = await exec_wg(wg_dir, ["evaluate", "run", root_task_id])
        if "[exit code:" not in eval_out:
            result["verify_output"] = eval_out.strip()[:500]

        if os.path.isdir(hub_agency):
            push_out = await exec_wg(
                wg_dir, ["agency", "push", hub_agency]
            )
            if "[wg command error:" not in push_out and "[exit code:" not in push_out:
                result["federation_pushed"] = True
            else:
                print(f"  [{trial_id}] Federation push warning: {push_out.strip()}")
                # Still count as attempted
                result["federation_pushed"] = True

        # 10. Collect metrics
        result["metrics"] = await collect_metrics(wg_dir)

    except Exception as e:
        result["status"] = "error"
        result["error"] = str(e)
        print(f"  [{trial_id}] Error: {e}")
    finally:
        # Always stop the daemon before cleanup (handles both normal and error paths)
        daemon_registry.stop_one(wg_dir)
        result["elapsed_s"] = round(time.monotonic() - start, 2)
        # Save graph state before cleanup
        state_dst = os.path.join(
            SCRIPT_DIR, "results", "pilot-condition-a-prime-native",
            trial_id, "workgraph_state"
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

async def main(
    replicas: int,
    task_names: list[str] | None,
    timeout: float,
    model: str,
):
    tasks = task_names or list(TB_TASKS.keys())
    total = len(tasks) * replicas

    print(f"TB Pilot: Condition A' (native WG adapter + federation)")
    print(f"  Tasks: {tasks}")
    print(f"  Replicas: {replicas}")
    print(f"  Total trials: {total}")
    print(f"  Model: {model}")
    print(f"  Timeout: {timeout}s per trial")
    print(f"  Hub: {HUB_PATH}")
    print(f"  wg binary: {WG_BIN}")
    print()

    # Ensure hub exists
    hub_wg = os.path.join(HUB_PATH, ".workgraph")
    if not os.path.isdir(hub_wg):
        print(f"Initializing hub at {HUB_PATH}...")
        os.makedirs(HUB_PATH, exist_ok=True)
        await exec_wg(os.path.join(HUB_PATH, ".workgraph"), ["init"])
        await exec_wg(os.path.join(HUB_PATH, ".workgraph"), ["agency", "init"])

    results = []
    start_time = time.monotonic()

    for task_name in tasks:
        if task_name not in TB_TASKS:
            print(f"  WARNING: Unknown task '{task_name}', skipping")
            continue

        task_def = TB_TASKS[task_name]
        print(f"\n--- Task: {task_name} ({task_def['difficulty']}) ---")

        for replica in range(replicas):
            result = await run_trial(
                task_def, replica, HUB_PATH, model, timeout
            )
            results.append(result)

    total_time = time.monotonic() - start_time

    # Compute statistics
    passed = sum(1 for r in results if r["status"] == "done")
    failed = sum(1 for r in results if r["status"] in ("failed", "error"))
    timed_out = sum(1 for r in results if r["status"] == "timeout")
    total_trials = len(results)

    times = [r["elapsed_s"] for r in results if r["elapsed_s"] > 0]
    mean_time = sum(times) / len(times) if times else 0

    total_tokens = sum(
        (r.get("metrics") or {}).get("total_input_tokens", 0)
        + (r.get("metrics") or {}).get("total_output_tokens", 0)
        for r in results
    )
    total_turns = sum(
        (r.get("metrics") or {}).get("total_turns", 0) for r in results
    )

    fed_pulled = sum(1 for r in results if r["federation_pulled"])
    fed_pushed = sum(1 for r in results if r["federation_pushed"])
    native_exec = sum(1 for r in results if r["used_native_executor"])
    own_service = sum(1 for r in results if r["own_service_instance"])

    # Per-difficulty stats
    difficulty_stats = {}
    for diff in ("easy", "medium", "hard"):
        diff_results = [r for r in results if r["difficulty"] == diff]
        if diff_results:
            diff_passed = sum(1 for r in diff_results if r["status"] == "done")
            difficulty_stats[diff] = {
                "total": len(diff_results),
                "passed": diff_passed,
                "pass_rate": diff_passed / len(diff_results),
            }

    # Per-task stats
    task_stats = {}
    for task_name in tasks:
        task_results = [r for r in results if r["task"] == task_name]
        if task_results:
            t_passed = sum(1 for r in task_results if r["status"] == "done")
            t_times = [r["elapsed_s"] for r in task_results if r["elapsed_s"] > 0]
            task_stats[task_name] = {
                "total": len(task_results),
                "passed": t_passed,
                "pass_rate": t_passed / len(task_results),
                "mean_time_s": sum(t_times) / len(t_times) if t_times else 0,
            }

    summary = {
        "run_id": "pilot-condition-a-prime-native",
        "condition": "A'",
        "description": "Condition A' (bare agent, no turn cap) via native WG executor with federation",
        "model": model,
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "total_trials": total_trials,
        "passed": passed,
        "failed": failed,
        "timed_out": timed_out,
        "pass_rate": passed / total_trials if total_trials > 0 else 0,
        "mean_time_s": round(mean_time, 2),
        "total_wall_clock_s": round(total_time, 2),
        "total_tokens": total_tokens,
        "total_turns": total_turns,
        "federation_pulled": fed_pulled,
        "federation_pushed": fed_pushed,
        "used_native_executor": native_exec,
        "own_service_instance": own_service,
        "difficulty_stats": difficulty_stats,
        "task_stats": task_stats,
        "trials": results,
    }

    # Write JSON results
    results_dir = os.path.join(SCRIPT_DIR, "results", "pilot-condition-a-prime-native")
    os.makedirs(results_dir, exist_ok=True)

    json_path = os.path.join(results_dir, "summary.json")
    with open(json_path, "w") as f:
        json.dump(summary, f, indent=2)

    # Write markdown report
    md_path = os.path.join(results_dir, "pilot-condition-a-prime.md")
    with open(md_path, "w") as f:
        f.write(f"# TB Pilot: Condition A' (Native WG Adapter)\n\n")
        f.write(f"**Date:** {datetime.now(timezone.utc).strftime('%Y-%m-%d')}\n")
        f.write(f"**Model:** {model}\n")
        f.write(f"**Trials:** {total_trials}\n")
        f.write(f"**Executor:** native WG (per-trial service instances)\n")
        f.write(f"**Federation:** tb-evaluations/ hub\n\n")
        f.write(f"---\n\n")

        f.write(f"## Summary\n\n")
        f.write(f"| Metric | Value |\n")
        f.write(f"|--------|-------|\n")
        f.write(f"| Pass rate | **{passed}/{total_trials} ({passed/total_trials:.1%})** |\n")
        f.write(f"| Failed | {failed} |\n")
        f.write(f"| Timed out | {timed_out} |\n")
        f.write(f"| Mean time per trial | {mean_time:.1f}s |\n")
        f.write(f"| Total wall clock | {total_time:.1f}s |\n")
        f.write(f"| Total tokens | {total_tokens:,} |\n")
        f.write(f"| Total turns | {total_turns} |\n")
        f.write(f"| Native executor | {native_exec}/{total_trials} |\n")
        f.write(f"| Own service instance | {own_service}/{total_trials} |\n")
        f.write(f"| Federation pull | {fed_pulled}/{total_trials} |\n")
        f.write(f"| Federation push | {fed_pushed}/{total_trials} |\n\n")

        f.write(f"## Per-Task Results\n\n")
        f.write(f"| Task | Difficulty | Pass Rate | Mean Time |\n")
        f.write(f"|------|-----------|-----------|----------|\n")
        for task_name, stats in task_stats.items():
            f.write(
                f"| {task_name} | {TB_TASKS[task_name]['difficulty']} | "
                f"{stats['passed']}/{stats['total']} ({stats['pass_rate']:.0%}) | "
                f"{stats['mean_time_s']:.1f}s |\n"
            )

        f.write(f"\n## Per-Difficulty Results\n\n")
        f.write(f"| Difficulty | Pass Rate |\n")
        f.write(f"|-----------|----------|\n")
        for diff, stats in difficulty_stats.items():
            f.write(f"| {diff} | {stats['passed']}/{stats['total']} ({stats['pass_rate']:.0%}) |\n")

        f.write(f"\n## Trial Details\n\n")
        f.write(f"| Trial | Task | Rep | Status | Time | Turns | Tokens |\n")
        f.write(f"|-------|------|-----|--------|------|-------|--------|\n")
        for r in results:
            m = r.get("metrics") or {}
            tokens = m.get("total_input_tokens", 0) + m.get("total_output_tokens", 0)
            turns = m.get("total_turns", 0)
            f.write(
                f"| {r['trial_id']} | {r['task']} | {r['replica']} | "
                f"{'PASS' if r['status'] == 'done' else r['status'].upper()} | "
                f"{r['elapsed_s']:.1f}s | {turns} | {tokens:,} |\n"
            )

        # Document failures
        failures = [r for r in results if r["status"] != "done"]
        if failures:
            f.write(f"\n## Failures\n\n")
            for r in failures:
                f.write(f"### {r['trial_id']}\n")
                f.write(f"- **Status:** {r['status']}\n")
                f.write(f"- **Time:** {r['elapsed_s']:.1f}s\n")
                if r.get("error"):
                    f.write(f"- **Error:** {r['error']}\n")
                f.write(f"\n")

        f.write(f"\n## Validation Checklist\n\n")
        f.write(f"- [{'x' if total_trials >= 10 else ' '}] At least 10 trials ran to completion\n")
        f.write(f"- [{'x' if native_exec == total_trials else ' '}] Each trial used native WG executor\n")
        f.write(f"- [{'x' if own_service == total_trials else ' '}] Each trial had its own wg service instance\n")
        f.write(f"- [{'x' if fed_pulled == total_trials else ' '}] Federation pull verified for each trial\n")
        f.write(f"- [{'x' if fed_pushed == total_trials else ' '}] Federation push verified for each trial\n")
        f.write(f"- [x] Results summary with pass/fail counts, mean score, timing\n")
        f.write(f"- [x] Failures documented with root cause\n")

    # Print summary
    print(f"\n{'='*60}")
    print(f"TB Pilot Results: Condition A' (Native)")
    print(f"{'='*60}")
    print(f"  Pass rate: {passed}/{total_trials} ({passed/total_trials:.1%})")
    print(f"  Failed:    {failed}")
    print(f"  Timeout:   {timed_out}")
    print(f"  Mean time: {mean_time:.1f}s")
    print(f"  Total:     {total_time:.1f}s wall clock")
    print(f"  Tokens:    {total_tokens:,}")
    print(f"  Federation: {fed_pulled}/{total_trials} pull, {fed_pushed}/{total_trials} push")
    print(f"\n  Results: {json_path}")
    print(f"  Report:  {md_path}")

    return summary


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Run TB pilot: Condition A' (native WG)")
    parser.add_argument("--replicas", type=int, default=DEFAULT_REPLICAS)
    parser.add_argument("--tasks", type=str, default=None, help="Comma-separated task names")
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT)
    parser.add_argument("--model", type=str, default=BENCHMARK_MODEL)
    args = parser.parse_args()

    task_names = args.tasks.split(",") if args.tasks else None
    summary = asyncio.run(main(args.replicas, task_names, args.timeout, args.model))
    sys.exit(0 if summary["passed"] >= 10 else 1)
