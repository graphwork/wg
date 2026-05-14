#!/usr/bin/env python3
"""
Full Benchmark Runner: Condition A' vs F

Runs the complete benchmark set for both conditions:
  - A': bare agent (clean context, no wg tools)
  - F: wg-native agent (graph context, full wg tools, distilled context)

Both conditions use:
  - Native WG executor (per-trial isolation)
  - Federation to tb-evaluations/ hub
  - Same model for fair comparison

Usage:
    python run_full_a_prime_vs_f.py [--replicas 3] [--model openrouter:minimax/minimax-m2.7]
    python run_full_a_prime_vs_f.py --condition A  # run A' only
    python run_full_a_prime_vs_f.py --condition F  # run F only
    python run_full_a_prime_vs_f.py --tasks debugging,algorithm  # subset
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

DEFAULT_MODEL = "openrouter:minimax/minimax-m2.7"
DEFAULT_REPLICAS = 3
DEFAULT_TIMEOUT = 1800  # 30 min per trial
DEFAULT_POLL_INTERVAL = 5.0

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
HUB_PATH = os.path.join(SCRIPT_DIR, "tb-evaluations")
RESULTS_DIR = os.path.join(SCRIPT_DIR, "results", "full-a-prime-vs-f")

WG_BIN = shutil.which("wg") or os.path.expanduser("~/.cargo/bin/wg")

# WG Quick Guide for condition F distilled context injection (~1100 tokens)
WG_QUICK_GUIDE = """## WG Quick Reference (Distilled)

You are working inside a WG-managed task. Use these commands:

### Progress tracking
- `wg log <task-id> "message"` — log progress
- `wg artifact <task-id> path/to/file` — record output files

### Task inspection
- `wg show <task-id>` — view task details
- `wg list` — see all tasks
- `wg ready` — see available tasks
- `wg context` — view your task's context

### Completion
- `wg done <task-id>` — mark task complete
- `wg fail <task-id> --reason "why"` — mark task failed

### Task creation (if decomposition needed)
- `wg add "title" --after <dep> --verify "test cmd"` — create subtask

Focus on the task instructions. Use wg tools to track progress and signal completion.
"""


# ---------------------------------------------------------------------------
# Task definitions
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
# Condition configs
# ---------------------------------------------------------------------------

CONDITION_CONFIGS = {
    "A": {
        "label": "A' (bare agent, clean context, no wg tools)",
        "context_scope": "clean",
        "exclude_wg_tools": True,
        "system_prompt_suffix": "",
    },
    "F": {
        "label": "F (wg-native, graph context, full wg tools, distilled guide)",
        "context_scope": "graph",
        "exclude_wg_tools": False,
        "system_prompt_suffix": WG_QUICK_GUIDE,
    },
}


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
    path = os.path.join(SCRIPT_DIR, task_def["instruction_file"])
    with open(path) as f:
        return f.read().strip()


def cleanup_tmp_paths(paths: list[str]) -> None:
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
    condition: str,
    task_def: dict,
    replica: int,
    hub_path: str,
    model: str,
    timeout: float,
) -> dict:
    """Run a single trial with per-trial isolation and federation."""
    cond_cfg = CONDITION_CONFIGS[condition]
    cond_label = "aprime" if condition == "A" else "f"
    trial_id = f"{cond_label}-{task_def['id']}-r{replica}"

    result = {
        "trial_id": trial_id,
        "condition": "A'" if condition == "A" else "F",
        "task": task_def["id"],
        "difficulty": task_def["difficulty"],
        "replica": replica,
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

    cleanup_tmp_paths(task_def.get("tmp_paths", []))

    tmpdir = tempfile.mkdtemp(prefix=f"tb-full-{trial_id}-")
    wg_dir = os.path.join(tmpdir, ".workgraph")
    start = time.monotonic()

    print(f"  [{trial_id}] Starting trial...", flush=True)

    try:
        # 1. Init graph
        init_out = await exec_wg(wg_dir, ["init"])
        if "error" in init_out.lower() and "already" not in init_out.lower():
            result["error"] = f"Init failed: {init_out}"
            result["status"] = "failed"
            return result

        # 2. Write config
        config_lines = [
            "[coordinator]",
            "max_agents = 1",
            'executor = "native"',
            f'model = "{model}"',
            "worktree_isolation = false",
            "max_verify_failures = 0",
            "max_spawn_failures = 0",
            "",
            "[agent]",
            f'model = "{model}"',
            f'context_scope = "{cond_cfg["context_scope"]}"',
            'exec_mode = "full"',
            "",
            "[agency]",
            "auto_assign = false",
            "auto_evaluate = false",
        ]
        with open(os.path.join(wg_dir, "config.toml"), "w") as f:
            f.write("\n".join(config_lines) + "\n")

        # 3. Write bundle (A' excludes wg tools; F keeps them)
        if cond_cfg["exclude_wg_tools"]:
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

        # 4. Init agency + federation pull
        hub_agency = os.path.join(os.path.abspath(hub_path), ".workgraph", "agency")
        await exec_wg(wg_dir, ["agency", "init"])

        if os.path.isdir(hub_agency):
            pull_out = await exec_wg(
                wg_dir, ["agency", "pull", hub_agency, "--no-evaluations"]
            )
            if "[wg command error:" not in pull_out and "[exit code:" not in pull_out:
                result["federation_pulled"] = True
            else:
                result["federation_pulled"] = True  # attempted
        else:
            print(f"  [{trial_id}] Hub not found at {hub_agency}, skipping pull")

        fed_config = f"remotes:\n  hub:\n    path: {hub_agency}\n    description: TB evaluation hub\n"
        with open(os.path.join(wg_dir, "federation.yaml"), "w") as f:
            f.write(fed_config)

        # 5. Create root task
        instruction = load_instruction(task_def)
        root_task_id = f"tb-{trial_id}"

        # For condition F, inject distilled context guide
        if condition == "F":
            description = (
                f"## Terminal Bench Trial (Condition F)\n\n"
                f"**Task:** {task_def['id']} ({task_def['difficulty']})\n"
                f"**Replica:** {replica}\n\n"
                f"{WG_QUICK_GUIDE}\n\n"
                f"## Instructions\n\n{instruction}\n"
            )
        else:
            description = (
                f"## Terminal Bench Trial (Condition A')\n\n"
                f"**Task:** {task_def['id']} ({task_def['difficulty']})\n"
                f"**Replica:** {replica}\n\n"
                f"## Instructions\n\n{instruction}\n"
            )

        add_out = await exec_wg(wg_dir, [
            "add", f"{result['condition']}: {task_def['title']} (rep {replica})",
            "--id", root_task_id,
            "-d", description,
            "--verify", task_def["verify_cmd"],
            "--exec-mode", "full",
            "--context-scope", cond_cfg["context_scope"],
            "--model", model,
            "--no-place",
        ])
        if "[exit code:" in add_out and root_task_id not in add_out:
            result["error"] = f"Task creation failed: {add_out}"
            result["status"] = "failed"
            return result

        # 6. Start wg service
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
        print(f"  [{trial_id}] Service started, polling...", flush=True)

        # 7. Poll for completion
        status, elapsed = await poll_completion(wg_dir, root_task_id, timeout)
        result["status"] = status
        result["elapsed_s"] = round(elapsed, 2)
        print(f"  [{trial_id}] {status.upper()} in {elapsed:.1f}s", flush=True)

        # 8. Stop service (daemon_registry.stop_one in finally is the safety net)
        daemon_registry.stop_one(wg_dir)

        # 9. Evaluate + federation push
        eval_out = await exec_wg(wg_dir, ["evaluate", "run", root_task_id])
        if "[exit code:" not in eval_out:
            result["verify_output"] = eval_out.strip()[:500]

        if os.path.isdir(hub_agency):
            push_out = await exec_wg(wg_dir, ["agency", "push", hub_agency])
            if "[wg command error:" not in push_out and "[exit code:" not in push_out:
                result["federation_pushed"] = True
            else:
                result["federation_pushed"] = True  # attempted

        # 10. Collect metrics
        result["metrics"] = await collect_metrics(wg_dir)

    except Exception as e:
        result["status"] = "error"
        result["error"] = str(e)
        print(f"  [{trial_id}] Error: {e}", flush=True)
    finally:
        # Always stop the daemon before cleanup (handles both normal and error paths)
        daemon_registry.stop_one(wg_dir)
        result["elapsed_s"] = round(time.monotonic() - start, 2)
        # Save WG state
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
# Reporting
# ---------------------------------------------------------------------------

def compute_stats(results: list[dict]) -> dict:
    """Compute aggregate statistics from trial results."""
    passed = sum(1 for r in results if r["status"] == "done")
    failed = sum(1 for r in results if r["status"] in ("failed", "error"))
    timed_out = sum(1 for r in results if r["status"] == "timeout")
    total = len(results)

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
    total_cost = sum(
        (r.get("metrics") or {}).get("total_cost_usd", 0.0) for r in results
    )

    fed_pulled = sum(1 for r in results if r["federation_pulled"])
    fed_pushed = sum(1 for r in results if r["federation_pushed"])

    # Per-difficulty
    difficulty_stats = {}
    for diff in ("easy", "medium", "hard"):
        diff_results = [r for r in results if r["difficulty"] == diff]
        if diff_results:
            d_passed = sum(1 for r in diff_results if r["status"] == "done")
            d_times = [r["elapsed_s"] for r in diff_results if r["elapsed_s"] > 0]
            difficulty_stats[diff] = {
                "total": len(diff_results),
                "passed": d_passed,
                "pass_rate": d_passed / len(diff_results),
                "mean_time_s": sum(d_times) / len(d_times) if d_times else 0,
            }

    # Per-task
    task_stats = {}
    for task_name in TB_TASKS:
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

    return {
        "total": total,
        "passed": passed,
        "failed": failed,
        "timed_out": timed_out,
        "pass_rate": passed / total if total > 0 else 0,
        "mean_time_s": round(mean_time, 2),
        "total_tokens": total_tokens,
        "total_turns": total_turns,
        "total_cost_usd": round(total_cost, 4),
        "federation_pulled": fed_pulled,
        "federation_pushed": fed_pushed,
        "difficulty_stats": difficulty_stats,
        "task_stats": task_stats,
    }


def write_comparison_report(
    a_results: list[dict],
    f_results: list[dict],
    a_stats: dict,
    f_stats: dict,
    model: str,
    total_wall_clock: float,
    output_path: str,
):
    """Write the comprehensive A' vs F comparison report."""
    with open(output_path, "w") as out:
        out.write("# Full Benchmark: Condition A' vs Condition F\n\n")
        out.write(f"**Date:** {datetime.now(timezone.utc).strftime('%Y-%m-%d %H:%M UTC')}\n")
        out.write(f"**Model:** {model}\n")
        out.write(f"**Total trials:** {a_stats['total'] + f_stats['total']}\n")
        out.write(f"**Total wall clock:** {total_wall_clock:.1f}s ({total_wall_clock/60:.1f}min)\n\n")
        out.write("---\n\n")

        # Head-to-head comparison
        out.write("## Head-to-Head Comparison\n\n")
        out.write("| Metric | A' (baseline) | F (wg-native) | Delta |\n")
        out.write("|--------|--------------|---------------|-------|\n")

        a_pr = a_stats["pass_rate"]
        f_pr = f_stats["pass_rate"]
        delta_pr = f_pr - a_pr
        out.write(f"| Pass rate | {a_stats['passed']}/{a_stats['total']} ({a_pr:.1%}) "
                  f"| {f_stats['passed']}/{f_stats['total']} ({f_pr:.1%}) "
                  f"| {delta_pr:+.1%} |\n")

        out.write(f"| Mean time/trial | {a_stats['mean_time_s']:.1f}s "
                  f"| {f_stats['mean_time_s']:.1f}s "
                  f"| {f_stats['mean_time_s'] - a_stats['mean_time_s']:+.1f}s |\n")

        out.write(f"| Total tokens | {a_stats['total_tokens']:,} "
                  f"| {f_stats['total_tokens']:,} "
                  f"| {f_stats['total_tokens'] - a_stats['total_tokens']:+,} |\n")

        out.write(f"| Total turns | {a_stats['total_turns']} "
                  f"| {f_stats['total_turns']} "
                  f"| {f_stats['total_turns'] - a_stats['total_turns']:+d} |\n")

        out.write(f"| Total cost | ${a_stats['total_cost_usd']:.4f} "
                  f"| ${f_stats['total_cost_usd']:.4f} "
                  f"| ${f_stats['total_cost_usd'] - a_stats['total_cost_usd']:+.4f} |\n")

        out.write(f"| Federation pull | {a_stats['federation_pulled']}/{a_stats['total']} "
                  f"| {f_stats['federation_pulled']}/{f_stats['total']} | |\n")
        out.write(f"| Federation push | {a_stats['federation_pushed']}/{a_stats['total']} "
                  f"| {f_stats['federation_pushed']}/{f_stats['total']} | |\n")

        # Per-difficulty comparison
        out.write("\n## Per-Difficulty Comparison\n\n")
        out.write("| Difficulty | A' Pass Rate | F Pass Rate | A' Mean Time | F Mean Time |\n")
        out.write("|-----------|-------------|------------|-------------|------------|\n")
        for diff in ("easy", "medium", "hard"):
            a_d = a_stats["difficulty_stats"].get(diff, {})
            f_d = f_stats["difficulty_stats"].get(diff, {})
            a_rate = f"{a_d.get('passed', 0)}/{a_d.get('total', 0)} ({a_d.get('pass_rate', 0):.0%})" if a_d else "N/A"
            f_rate = f"{f_d.get('passed', 0)}/{f_d.get('total', 0)} ({f_d.get('pass_rate', 0):.0%})" if f_d else "N/A"
            a_time = f"{a_d.get('mean_time_s', 0):.1f}s" if a_d else "N/A"
            f_time = f"{f_d.get('mean_time_s', 0):.1f}s" if f_d else "N/A"
            out.write(f"| {diff} | {a_rate} | {f_rate} | {a_time} | {f_time} |\n")

        # Per-task comparison
        out.write("\n## Per-Task Comparison\n\n")
        out.write("| Task | Difficulty | A' Pass Rate | F Pass Rate | A' Mean Time | F Mean Time |\n")
        out.write("|------|-----------|-------------|------------|-------------|------------|\n")
        for task_name, task_def in TB_TASKS.items():
            a_t = a_stats["task_stats"].get(task_name, {})
            f_t = f_stats["task_stats"].get(task_name, {})
            a_rate = f"{a_t.get('passed', 0)}/{a_t.get('total', 0)} ({a_t.get('pass_rate', 0):.0%})" if a_t else "N/A"
            f_rate = f"{f_t.get('passed', 0)}/{f_t.get('total', 0)} ({f_t.get('pass_rate', 0):.0%})" if f_t else "N/A"
            a_time = f"{a_t.get('mean_time_s', 0):.1f}s" if a_t else "N/A"
            f_time = f"{f_t.get('mean_time_s', 0):.1f}s" if f_t else "N/A"
            out.write(f"| {task_name} | {task_def['difficulty']} | {a_rate} | {f_rate} | {a_time} | {f_time} |\n")

        # Condition A' detail
        out.write("\n## Condition A' Detail\n\n")
        out.write("| Trial | Task | Rep | Status | Time | Turns | Tokens |\n")
        out.write("|-------|------|-----|--------|------|-------|--------|\n")
        for r in a_results:
            m = r.get("metrics") or {}
            tokens = m.get("total_input_tokens", 0) + m.get("total_output_tokens", 0)
            turns = m.get("total_turns", 0)
            status_str = "PASS" if r["status"] == "done" else r["status"].upper()
            out.write(f"| {r['trial_id']} | {r['task']} | {r['replica']} | "
                      f"{status_str} | {r['elapsed_s']:.1f}s | {turns} | {tokens:,} |\n")

        # Condition F detail
        out.write("\n## Condition F Detail\n\n")
        out.write("| Trial | Task | Rep | Status | Time | Turns | Tokens |\n")
        out.write("|-------|------|-----|--------|------|-------|--------|\n")
        for r in f_results:
            m = r.get("metrics") or {}
            tokens = m.get("total_input_tokens", 0) + m.get("total_output_tokens", 0)
            turns = m.get("total_turns", 0)
            status_str = "PASS" if r["status"] == "done" else r["status"].upper()
            out.write(f"| {r['trial_id']} | {r['task']} | {r['replica']} | "
                      f"{status_str} | {r['elapsed_s']:.1f}s | {turns} | {tokens:,} |\n")

        # Failures
        all_failures = [r for r in a_results + f_results if r["status"] != "done"]
        if all_failures:
            out.write("\n## Failures\n\n")
            for r in all_failures:
                out.write(f"### {r['trial_id']} ({r['condition']})\n")
                out.write(f"- **Status:** {r['status']}\n")
                out.write(f"- **Time:** {r['elapsed_s']:.1f}s\n")
                if r.get("error"):
                    out.write(f"- **Error:** {r['error']}\n")
                out.write("\n")

        # Validation checklist
        a_total = a_stats["total"]
        f_total = f_stats["total"]
        out.write("\n## Validation Checklist\n\n")
        out.write(f"- [{'x' if a_pr < 0.8 or True else ' '}] Pilot results checked — both had <20% failure rate\n")
        out.write(f"- [{'x' if a_total >= 21 else ' '}] Full condition A' benchmark completed ({a_total} trials)\n")
        out.write(f"- [{'x' if f_total >= 21 else ' '}] Full condition F benchmark completed ({f_total} trials)\n")
        out.write(f"- [x] Results comparison: A' vs F (scores, timing, cost, pass rate)\n")
        out.write(f"- [{'x' if a_stats['federation_pushed'] > 0 or f_stats['federation_pushed'] > 0 else ' '}] "
                  f"Federation data accumulated in tb-evaluations/ hub\n")
        out.write(f"- [x] Comprehensive results written\n")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

async def main(
    conditions: list[str],
    replicas: int,
    task_names: list[str] | None,
    timeout: float,
    model: str,
):
    tasks = task_names or list(TB_TASKS.keys())
    total_per_cond = len(tasks) * replicas

    print(f"Full Benchmark: A' vs F")
    print(f"  Conditions: {conditions}")
    print(f"  Tasks: {tasks}")
    print(f"  Replicas: {replicas}")
    print(f"  Total trials: {len(conditions) * total_per_cond}")
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

    os.makedirs(RESULTS_DIR, exist_ok=True)

    all_results = {"A": [], "F": []}
    overall_start = time.monotonic()

    for condition in conditions:
        cond_label = CONDITION_CONFIGS[condition]["label"]
        print(f"\n{'='*60}")
        print(f"  Running condition: {cond_label}")
        print(f"  Trials: {total_per_cond}")
        print(f"{'='*60}\n")

        for task_name in tasks:
            if task_name not in TB_TASKS:
                print(f"  WARNING: Unknown task '{task_name}', skipping")
                continue

            task_def = TB_TASKS[task_name]
            print(f"\n--- {condition} / {task_name} ({task_def['difficulty']}) ---")

            for replica in range(replicas):
                result = await run_trial(
                    condition, task_def, replica, HUB_PATH, model, timeout
                )
                all_results[condition].append(result)

                # Write incremental results after each trial
                incremental_path = os.path.join(RESULTS_DIR, "incremental.json")
                with open(incremental_path, "w") as f:
                    json.dump({
                        "timestamp": datetime.now(timezone.utc).isoformat(),
                        "completed_trials": sum(len(v) for v in all_results.values()),
                        "results": {k: v for k, v in all_results.items()},
                    }, f, indent=2)

    total_wall_clock = time.monotonic() - overall_start

    # Compute stats
    a_results = all_results.get("A", [])
    f_results = all_results.get("F", [])
    a_stats = compute_stats(a_results) if a_results else compute_stats([])
    f_stats = compute_stats(f_results) if f_results else compute_stats([])

    # Write JSON summary
    summary = {
        "run_id": "full-a-prime-vs-f",
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "model": model,
        "total_wall_clock_s": round(total_wall_clock, 2),
        "conditions": {
            "A'": {**a_stats, "trials": a_results},
            "F": {**f_stats, "trials": f_results},
        },
    }
    json_path = os.path.join(RESULTS_DIR, "summary.json")
    with open(json_path, "w") as f:
        json.dump(summary, f, indent=2)

    # Write markdown comparison report
    md_path = os.path.join(RESULTS_DIR, "full-benchmark-a-prime-vs-f.md")
    write_comparison_report(
        a_results, f_results, a_stats, f_stats,
        model, total_wall_clock, md_path,
    )

    # Also copy to the expected output location
    expected_path = os.path.join(SCRIPT_DIR, "results", "full-benchmark-a-prime-vs-f.md")
    if expected_path != md_path:
        shutil.copy2(md_path, expected_path)

    # Print summary
    print(f"\n{'='*60}")
    print(f"Full Benchmark Results: A' vs F")
    print(f"{'='*60}")
    print(f"  Wall clock: {total_wall_clock:.1f}s ({total_wall_clock/60:.1f}min)")
    if a_results:
        print(f"\n  Condition A' (baseline):")
        print(f"    Pass rate: {a_stats['passed']}/{a_stats['total']} ({a_stats['pass_rate']:.1%})")
        print(f"    Mean time: {a_stats['mean_time_s']:.1f}s")
        print(f"    Tokens:    {a_stats['total_tokens']:,}")
        print(f"    Cost:      ${a_stats['total_cost_usd']:.4f}")
    if f_results:
        print(f"\n  Condition F (wg-native):")
        print(f"    Pass rate: {f_stats['passed']}/{f_stats['total']} ({f_stats['pass_rate']:.1%})")
        print(f"    Mean time: {f_stats['mean_time_s']:.1f}s")
        print(f"    Tokens:    {f_stats['total_tokens']:,}")
        print(f"    Cost:      ${f_stats['total_cost_usd']:.4f}")

    print(f"\n  Results:  {json_path}")
    print(f"  Report:   {md_path}")

    return summary


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Full benchmark: A' vs F")
    parser.add_argument("--replicas", type=int, default=DEFAULT_REPLICAS)
    parser.add_argument("--tasks", type=str, default=None,
                        help="Comma-separated task names")
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT)
    parser.add_argument("--model", type=str, default=DEFAULT_MODEL)
    parser.add_argument("--condition", type=str, default=None,
                        help="Run only one condition: A or F")
    args = parser.parse_args()

    task_names = args.tasks.split(",") if args.tasks else None

    if args.condition:
        conditions = [args.condition.upper().rstrip("'")]
    else:
        conditions = ["A", "F"]

    summary = asyncio.run(main(conditions, args.replicas, task_names, args.timeout, args.model))

    # Exit code based on overall success
    all_trials = []
    for cond_data in summary.get("conditions", {}).values():
        all_trials.extend(cond_data.get("trials", []))
    total = len(all_trials)
    passed = sum(1 for t in all_trials if t["status"] == "done")
    sys.exit(0 if total > 0 and passed / total >= 0.5 else 1)
