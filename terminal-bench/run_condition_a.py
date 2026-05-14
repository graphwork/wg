#!/usr/bin/env python3
"""
Condition A Runner — isolated wg service per terminalbench problem, 8 parallel agents.

Each terminalbench problem runs in its own wg service instance with up to 8
parallel agents, using WG's native executor (no litellm/harbor fallback).

Design: terminal-bench/DESIGN-condition-a-isolation.md

Usage:
    python run_condition_a.py
    python run_condition_a.py --replicas 3 --model openrouter:minimax/minimax-m2.7
    python run_condition_a.py --tasks debugging,algorithm
    python run_condition_a.py --max-agents 4 --max-concurrent 2
    python run_condition_a.py --smoke  # single easy task, 1 replica, quick validation
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
DEFAULT_MAX_AGENTS = 8
DEFAULT_MAX_CONCURRENT_TRIALS = 4
DEFAULT_TIMEOUT = 1800  # 30 min per trial
DEFAULT_POLL_INTERVAL = 5.0

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
RESULTS_DIR = os.path.join(SCRIPT_DIR, "results", "condition-a")

WG_BIN = shutil.which("wg") or os.path.expanduser("~/.cargo/bin/wg")


# ---------------------------------------------------------------------------
# Task definitions (same as existing runners)
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

    Strips all WG_* and CLAUDECODE env vars to prevent parent-service leakage.
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
    """Poll task status until terminal or timeout."""
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
    """Collect token/turn/cost metrics from agent stream.jsonl files."""
    agents_dir = os.path.join(wg_dir, "agents")
    metrics = {
        "total_input_tokens": 0,
        "total_output_tokens": 0,
        "total_cost_usd": 0.0,
        "total_turns": 0,
        "num_agents_spawned": 0,
    }
    if not os.path.isdir(agents_dir):
        return metrics
    for agent_id in os.listdir(agents_dir):
        agent_dir = os.path.join(agents_dir, agent_id)
        if not os.path.isdir(agent_dir):
            continue
        metrics["num_agents_spawned"] += 1
        stream_path = os.path.join(agent_dir, "stream.jsonl")
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
# Verification (3-layer)
# ---------------------------------------------------------------------------

def verify_executor_path(wg_dir: str, expected_model: str) -> dict:
    """Layer 2: Verify every agent used native executor + expected model via stream.jsonl."""
    agents_dir = os.path.join(wg_dir, "agents")
    verification = {
        "all_native": True,
        "all_correct_model": True,
        "agents": [],
    }
    if not os.path.isdir(agents_dir):
        verification["error"] = "No agents directory"
        return verification

    for agent_id in os.listdir(agents_dir):
        stream_path = os.path.join(agents_dir, agent_id, "stream.jsonl")
        if not os.path.isfile(stream_path):
            continue
        agent_info = {"agent_id": agent_id, "executor": None, "model": None}
        with open(stream_path) as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    event = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if event.get("type") == "init":
                    agent_info["executor"] = event.get("executor_type")
                    agent_info["model"] = event.get("model")
                    break
        verification["agents"].append(agent_info)
        if agent_info["executor"] != "native":
            verification["all_native"] = False
        # Model comparison: strip provider prefix for matching
        actual = (agent_info.get("model") or "").replace(":", "/", 1)
        expected = expected_model.replace(":", "/", 1)
        if actual != expected and agent_info.get("model") != expected_model:
            verification["all_correct_model"] = False

    return verification


def audit_trial_config(wg_dir: str, expected_model: str) -> dict:
    """Layer 3: Structural audit — verify config.toml matches intent."""
    config_path = os.path.join(wg_dir, "config.toml")
    audit = {"config_exists": False, "executor_native": False,
             "model_matches": False}
    if not os.path.isfile(config_path):
        return audit
    audit["config_exists"] = True
    with open(config_path) as f:
        content = f.read()
    audit["executor_native"] = 'executor = "native"' in content
    audit["model_matches"] = f'model = "{expected_model}"' in content
    return audit


def build_verification(wg_dir: str, model: str) -> dict:
    """Build full 3-layer verification dict for a trial."""
    return {
        "env_sanitized": True,  # Always true by construction (exec_wg strips WG_*)
        "executor_audit": audit_trial_config(wg_dir, model),
        "stream_verification": verify_executor_path(wg_dir, model),
    }


# ---------------------------------------------------------------------------
# Config generation
# ---------------------------------------------------------------------------

def write_trial_config(wg_dir: str, model: str, max_agents: int = 8,
                       context_scope: str = "clean") -> None:
    """Write config.toml that locks executor to native + target model."""
    config = f"""[coordinator]
max_agents = {max_agents}
executor = "native"
model = "{model}"
worktree_isolation = false
agent_timeout = "30m"
max_verify_failures = 0
max_spawn_failures = 0

[agent]
model = "{model}"
context_scope = "{context_scope}"
exec_mode = "full"

[agency]
auto_assign = false
auto_evaluate = false
"""
    with open(os.path.join(wg_dir, "config.toml"), "w") as f:
        f.write(config)


# ---------------------------------------------------------------------------
# Trial runner
# ---------------------------------------------------------------------------

async def run_trial(
    task_def: dict,
    replica: int,
    model: str,
    max_agents: int = DEFAULT_MAX_AGENTS,
    timeout: float = DEFAULT_TIMEOUT,
    results_dir: str = RESULTS_DIR,
) -> dict:
    """Run a single condition A trial with full isolation and verification."""
    trial_id = f"condA-{task_def['id']}-r{replica}"
    result = {
        "trial_id": trial_id,
        "problem_id": task_def["id"],
        "condition": "A",
        "difficulty": task_def["difficulty"],
        "replica": replica,
        "model": model,
        "max_agents": max_agents,
        "status": "not_started",
        "elapsed_s": 0.0,
        "metrics": None,
        "verification": None,
        "error": None,
    }

    # Clean up any leftover tmp paths from prior runs
    cleanup_tmp_paths(task_def.get("tmp_paths", []))

    tmpdir = tempfile.mkdtemp(prefix=f"tb-condA-{trial_id}-")
    wg_dir = os.path.join(tmpdir, ".workgraph")
    start = time.monotonic()

    print(f"  [{trial_id}] Starting (max_agents={max_agents})...", flush=True)

    try:
        # 1. Init graph
        init_out = await exec_wg(wg_dir, ["init"])
        if "error" in init_out.lower() and "already" not in init_out.lower():
            result["error"] = f"Init failed: {init_out}"
            result["status"] = "error"
            return result

        # 2. Write locked config
        write_trial_config(wg_dir, model, max_agents, context_scope="clean")

        # 3. Create root task
        instruction = load_instruction(task_def)
        root_task_id = f"tb-{trial_id}"

        description = (
            f"## Terminal Bench Trial (Condition A)\n\n"
            f"**Task:** {task_def['id']} ({task_def['difficulty']})\n"
            f"**Replica:** {replica}\n\n"
            f"## Instructions\n\n{instruction}\n"
        )

        add_out = await exec_wg(wg_dir, [
            "add", f"A: {task_def['title']} (rep {replica})",
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
            result["status"] = "error"
            return result

        # 4. Start isolated service
        service_out = await exec_wg(wg_dir, [
            "service", "start",
            "--max-agents", str(max_agents),
            "--executor", "native",
            "--model", model,
            "--no-coordinator-agent",
            "--force",
        ])
        daemon_registry.register(wg_dir, WG_BIN)
        print(f"  [{trial_id}] Service started, polling...", flush=True)

        # 5. Poll for completion
        status, elapsed = await poll_completion(wg_dir, root_task_id, timeout)
        result["status"] = status
        result["elapsed_s"] = round(elapsed, 2)
        print(f"  [{trial_id}] {status.upper()} in {elapsed:.1f}s", flush=True)

        # 6. Stop service (daemon_registry.stop_one in finally is the safety net)
        daemon_registry.stop_one(wg_dir)

        # 7. Verify executor path (3-layer)
        result["verification"] = build_verification(wg_dir, model)

        # 8. Collect metrics
        result["metrics"] = await collect_metrics(wg_dir)

    except Exception as e:
        result["status"] = "error"
        result["error"] = str(e)
        print(f"  [{trial_id}] Error: {e}", flush=True)
    finally:
        # Always stop the daemon before cleanup (handles both normal and error paths)
        daemon_registry.stop_one(wg_dir)
        result["elapsed_s"] = round(time.monotonic() - start, 2)
        # Preserve graph state for post-hoc analysis
        state_dst = os.path.join(results_dir, trial_id, "workgraph_state")
        try:
            os.makedirs(os.path.dirname(state_dst), exist_ok=True)
            if os.path.isdir(wg_dir):
                shutil.copytree(wg_dir, state_dst)
        except Exception:
            pass
        shutil.rmtree(tmpdir, ignore_errors=True)

    return result


# ---------------------------------------------------------------------------
# Parallel execution with semaphore
# ---------------------------------------------------------------------------

async def run_trial_with_limit(
    semaphore: asyncio.Semaphore,
    task_def: dict,
    replica: int,
    model: str,
    max_agents: int,
    timeout: float,
    results_dir: str,
) -> dict:
    """Run a trial, gated by semaphore to limit concurrent trials."""
    async with semaphore:
        return await run_trial(
            task_def, replica, model, max_agents, timeout, results_dir
        )


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
    total_agents = sum(
        (r.get("metrics") or {}).get("num_agents_spawned", 0) for r in results
    )

    # Verification rollup
    all_verified = all(
        r.get("verification", {}).get("stream_verification", {}).get("all_native", False)
        and r.get("verification", {}).get("stream_verification", {}).get("all_correct_model", False)
        for r in results
    ) if results else False

    # Per-difficulty
    difficulty_stats = {}
    for diff in ("easy", "medium", "hard"):
        diff_results = [r for r in results if r.get("difficulty") == diff]
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
        task_results = [r for r in results if r.get("problem_id") == task_name]
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
        "total_agents_spawned": total_agents,
        "all_executor_verified": all_verified,
        "difficulty_stats": difficulty_stats,
        "task_stats": task_stats,
    }


def write_report(
    results: list[dict],
    stats: dict,
    model: str,
    max_agents: int,
    max_concurrent: int,
    total_wall_clock: float,
    output_path: str,
):
    """Write markdown report for condition A results."""
    with open(output_path, "w") as out:
        out.write("# Condition A Benchmark Results\n\n")
        out.write(f"**Date:** {datetime.now(timezone.utc).strftime('%Y-%m-%d %H:%M UTC')}\n")
        out.write(f"**Model:** {model}\n")
        out.write(f"**Max agents per trial:** {max_agents}\n")
        out.write(f"**Max concurrent trials:** {max_concurrent}\n")
        out.write(f"**Total trials:** {stats['total']}\n")
        out.write(f"**Total wall clock:** {total_wall_clock:.1f}s ({total_wall_clock/60:.1f}min)\n\n")

        # Executor verification
        out.write("## Executor Verification\n\n")
        if stats["all_executor_verified"]:
            out.write("All agents used native executor with correct model.\n\n")
        else:
            out.write("**WARNING:** Some agents may not have used the expected executor/model.\n")
            out.write("Check per-trial verification details in summary.json.\n\n")

        # Summary
        out.write("## Summary\n\n")
        out.write(f"| Metric | Value |\n")
        out.write(f"|--------|-------|\n")
        out.write(f"| Pass rate | {stats['passed']}/{stats['total']} ({stats['pass_rate']:.1%}) |\n")
        out.write(f"| Mean time/trial | {stats['mean_time_s']:.1f}s |\n")
        out.write(f"| Total tokens | {stats['total_tokens']:,} |\n")
        out.write(f"| Total turns | {stats['total_turns']} |\n")
        out.write(f"| Total cost | ${stats['total_cost_usd']:.4f} |\n")
        out.write(f"| Total agents spawned | {stats['total_agents_spawned']} |\n")

        # Per-difficulty
        out.write("\n## Per-Difficulty Results\n\n")
        out.write("| Difficulty | Pass Rate | Mean Time |\n")
        out.write("|-----------|-----------|----------|\n")
        for diff in ("easy", "medium", "hard"):
            d = stats["difficulty_stats"].get(diff, {})
            if d:
                out.write(f"| {diff} | {d['passed']}/{d['total']} ({d['pass_rate']:.0%}) | {d['mean_time_s']:.1f}s |\n")
            else:
                out.write(f"| {diff} | N/A | N/A |\n")

        # Per-task
        out.write("\n## Per-Task Results\n\n")
        out.write("| Task | Difficulty | Pass Rate | Mean Time |\n")
        out.write("|------|-----------|-----------|----------|\n")
        for task_name, task_def in TB_TASKS.items():
            t = stats["task_stats"].get(task_name, {})
            if t:
                out.write(f"| {task_name} | {task_def['difficulty']} | "
                          f"{t['passed']}/{t['total']} ({t['pass_rate']:.0%}) | "
                          f"{t['mean_time_s']:.1f}s |\n")
            else:
                out.write(f"| {task_name} | {task_def['difficulty']} | N/A | N/A |\n")

        # Trial details
        out.write("\n## Trial Details\n\n")
        out.write("| Trial | Task | Rep | Status | Time | Agents | Turns | Tokens |\n")
        out.write("|-------|------|-----|--------|------|--------|-------|--------|\n")
        for r in results:
            m = r.get("metrics") or {}
            tokens = m.get("total_input_tokens", 0) + m.get("total_output_tokens", 0)
            turns = m.get("total_turns", 0)
            agents = m.get("num_agents_spawned", 0)
            status_str = "PASS" if r["status"] == "done" else r["status"].upper()
            out.write(f"| {r['trial_id']} | {r['problem_id']} | {r['replica']} | "
                      f"{status_str} | {r['elapsed_s']:.1f}s | {agents} | {turns} | {tokens:,} |\n")

        # Failures
        failures = [r for r in results if r["status"] != "done"]
        if failures:
            out.write("\n## Failures\n\n")
            for r in failures:
                out.write(f"### {r['trial_id']}\n")
                out.write(f"- **Status:** {r['status']}\n")
                out.write(f"- **Time:** {r['elapsed_s']:.1f}s\n")
                if r.get("error"):
                    out.write(f"- **Error:** {r['error']}\n")
                v = r.get("verification", {})
                sv = v.get("stream_verification", {})
                if not sv.get("all_native", True):
                    out.write(f"- **Executor mismatch:** agents did not all use native executor\n")
                if not sv.get("all_correct_model", True):
                    out.write(f"- **Model mismatch:** agents did not all use expected model\n")
                out.write("\n")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

async def main(
    task_names: list[str] | None,
    replicas: int,
    max_agents: int,
    max_concurrent: int,
    timeout: float,
    model: str,
    results_dir: str,
):
    tasks = task_names or list(TB_TASKS.keys())
    total_trials = len(tasks) * replicas

    print(f"Condition A Benchmark")
    print(f"  Tasks: {tasks}")
    print(f"  Replicas: {replicas}")
    print(f"  Total trials: {total_trials}")
    print(f"  Max agents per trial: {max_agents}")
    print(f"  Max concurrent trials: {max_concurrent}")
    print(f"  Max concurrent agents: {max_concurrent * max_agents}")
    print(f"  Model: {model}")
    print(f"  Timeout: {timeout}s per trial")
    print(f"  wg binary: {WG_BIN}")
    print()

    os.makedirs(results_dir, exist_ok=True)

    semaphore = asyncio.Semaphore(max_concurrent)
    overall_start = time.monotonic()

    # Build list of all trial coroutines
    trial_coros = []
    for task_name in tasks:
        if task_name not in TB_TASKS:
            print(f"  WARNING: Unknown task '{task_name}', skipping")
            continue
        task_def = TB_TASKS[task_name]
        for replica in range(replicas):
            trial_coros.append(
                run_trial_with_limit(
                    semaphore, task_def, replica, model,
                    max_agents, timeout, results_dir,
                )
            )

    print(f"Launching {len(trial_coros)} trials (up to {max_concurrent} concurrent)...\n")

    # Run all trials with semaphore-gated concurrency
    all_results = await asyncio.gather(*trial_coros)
    results = list(all_results)

    total_wall_clock = time.monotonic() - overall_start

    # Compute stats
    stats = compute_stats(results)

    # Write JSON summary
    summary = {
        "run_id": "condition-a",
        "condition": "A",
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "model": model,
        "max_agents": max_agents,
        "max_concurrent_trials": max_concurrent,
        "total_wall_clock_s": round(total_wall_clock, 2),
        "stats": stats,
        "trials": results,
    }
    json_path = os.path.join(results_dir, "summary.json")
    with open(json_path, "w") as f:
        json.dump(summary, f, indent=2)

    # Write markdown report
    md_path = os.path.join(results_dir, "condition-a-results.md")
    write_report(
        results, stats, model, max_agents, max_concurrent,
        total_wall_clock, md_path,
    )

    # Print summary
    print(f"\n{'='*60}")
    print(f"Condition A Results")
    print(f"{'='*60}")
    print(f"  Wall clock: {total_wall_clock:.1f}s ({total_wall_clock/60:.1f}min)")
    print(f"  Pass rate:  {stats['passed']}/{stats['total']} ({stats['pass_rate']:.1%})")
    print(f"  Mean time:  {stats['mean_time_s']:.1f}s")
    print(f"  Tokens:     {stats['total_tokens']:,}")
    print(f"  Cost:       ${stats['total_cost_usd']:.4f}")
    print(f"  Agents:     {stats['total_agents_spawned']}")
    print(f"  Verified:   {'YES' if stats['all_executor_verified'] else 'NO'}")
    print(f"\n  Results: {json_path}")
    print(f"  Report:  {md_path}")

    return summary


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        description="Condition A benchmark: isolated wg service per problem, 8 parallel agents"
    )
    parser.add_argument("--replicas", type=int, default=DEFAULT_REPLICAS)
    parser.add_argument("--tasks", type=str, default=None,
                        help="Comma-separated task names")
    parser.add_argument("--max-agents", type=int, default=DEFAULT_MAX_AGENTS,
                        help=f"Max agents per trial (default: {DEFAULT_MAX_AGENTS})")
    parser.add_argument("--max-concurrent", type=int, default=DEFAULT_MAX_CONCURRENT_TRIALS,
                        help=f"Max concurrent trials (default: {DEFAULT_MAX_CONCURRENT_TRIALS})")
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT,
                        help=f"Timeout per trial in seconds (default: {DEFAULT_TIMEOUT})")
    parser.add_argument("--model", type=str, default=DEFAULT_MODEL)
    parser.add_argument("--results-dir", type=str, default=RESULTS_DIR)
    parser.add_argument("--smoke", action="store_true",
                        help="Smoke test: 1 easy task, 1 replica")
    args = parser.parse_args()

    if args.smoke:
        task_names = ["file-ops"]
        replicas = 1
        max_concurrent = 1
    else:
        task_names = args.tasks.split(",") if args.tasks else None
        replicas = args.replicas
        max_concurrent = args.max_concurrent

    summary = asyncio.run(main(
        task_names, replicas, args.max_agents, max_concurrent,
        args.timeout, args.model, args.results_dir,
    ))

    # Exit code based on overall success
    total = len(summary.get("trials", []))
    passed = sum(1 for t in summary.get("trials", []) if t["status"] == "done")
    sys.exit(0 if total > 0 and passed / total >= 0.5 else 1)
