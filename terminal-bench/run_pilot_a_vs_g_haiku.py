#!/usr/bin/env python3
"""
Pilot: Condition A vs G — Claude Haiku, 5 diverse tasks.

Condition A: Workgraph-coordinated execution (isolated wg service + native-exec agent)
Condition G: Raw Claude Code (claude CLI in print mode, no workgraph)

Both conditions use Claude Haiku as the execution model.

Usage:
    python run_pilot_a_vs_g_haiku.py
    python run_pilot_a_vs_g_haiku.py --smoke   # single task, quick validation
"""

import argparse
import asyncio
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

MODEL = "claude:haiku"  # Claude Haiku — provider:model format for wg config/commands
CLAUDE_MODEL_FLAG = "haiku"  # For claude CLI --model flag (alias format, uses OAuth)
EXECUTOR = "claude"  # Uses claude CLI (OAuth) — not "native" which needs API keys
MAX_AGENTS_CONDITION_A = 1  # single agent for fair comparison
MAX_CONCURRENT_TRIALS = 4
DEFAULT_TIMEOUT = 1800  # 30 min per trial
POLL_INTERVAL = 5.0

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
RESULTS_DIR = os.path.join(SCRIPT_DIR, "results", "pilot-a-vs-g-haiku")

WG_BIN = shutil.which("wg") or os.path.expanduser("~/.cargo/bin/wg")
CLAUDE_BIN = shutil.which("claude") or os.path.expanduser("~/.local/bin/claude")

# 5 diverse tasks: easy(1), medium(2), hard(2)
SELECTED_TASKS = {
    "text-processing": {
        "id": "text-processing",
        "title": "Text Processing: word frequency counter",
        "instruction_file": "tasks/condition-a-calibration/02-text-processing-easy.txt",
        "verify_cmd": (
            "test -f /tmp/wordfreq.py && "
            "echo 'the the the dog dog cat' | python3 /tmp/wordfreq.py | head -1 | grep -q 'the'"
        ),
        "difficulty": "easy",
        "category": "text-processing",
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
        "category": "debugging",
        "tmp_paths": ["/tmp/buggy_sort.py"],
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
        "category": "data-processing",
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
        "category": "algorithm",
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
        "category": "ml-inference",
        "tmp_paths": ["/tmp/kmeans.py"],
    },
}


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

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


def run_verify(verify_cmd: str, timeout: float = 30) -> bool:
    """Run a verification command and return True if it passes."""
    try:
        result = subprocess.run(
            ["bash", "-c", verify_cmd],
            capture_output=True, text=True, timeout=timeout,
        )
        return result.returncode == 0
    except (subprocess.TimeoutExpired, Exception):
        return False


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


async def poll_completion(
    wg_dir: str, task_id: str, timeout_secs: float,
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
        await asyncio.sleep(POLL_INTERVAL)


def write_trial_config(wg_dir: str, model: str, executor: str = "claude",
                       max_agents: int = 1) -> None:
    """Write config.toml for Condition A trial."""
    config = f"""[coordinator]
max_agents = {max_agents}
executor = "{executor}"
model = "{model}"
worktree_isolation = false
agent_timeout = "30m"
max_verify_failures = 0
max_spawn_failures = 0

[agent]
model = "{model}"
context_scope = "clean"
exec_mode = "full"

[agency]
auto_assign = false
auto_evaluate = false
"""
    with open(os.path.join(wg_dir, "config.toml"), "w") as f:
        f.write(config)


async def collect_metrics(wg_dir: str) -> dict:
    """Collect token/turn metrics from agent stream.jsonl files."""
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
# Condition A: With Workgraph
# ---------------------------------------------------------------------------

async def run_condition_a(
    task_def: dict, timeout: float,
) -> dict:
    """Run a single Condition A trial (workgraph-coordinated)."""
    trial_id = f"condA-{task_def['id']}"
    result = {
        "trial_id": trial_id,
        "task_id": task_def["id"],
        "condition": "A",
        "difficulty": task_def["difficulty"],
        "category": task_def.get("category", "unknown"),
        "model": MODEL,
        "status": "not_started",
        "elapsed_s": 0.0,
        "reward": 0.0,
        "failure_mode": None,
        "metrics": None,
        "error": None,
    }

    cleanup_tmp_paths(task_def.get("tmp_paths", []))
    tmpdir = tempfile.mkdtemp(prefix=f"tb-condA-{task_def['id']}-")
    wg_dir = os.path.join(tmpdir, ".workgraph")
    start = time.monotonic()

    print(f"  [{trial_id}] Starting (workgraph, model={MODEL})...", flush=True)

    try:
        # Init isolated graph
        await exec_wg(wg_dir, ["init"])
        write_trial_config(wg_dir, MODEL, EXECUTOR, MAX_AGENTS_CONDITION_A)

        # Create root task
        instruction = load_instruction(task_def)
        root_task_id = f"tb-{trial_id}"
        description = (
            f"## Terminal Bench Trial (Condition A)\n\n"
            f"**Task:** {task_def['id']} ({task_def['difficulty']})\n\n"
            f"## Instructions\n\n{instruction}\n"
        )
        add_out = await exec_wg(wg_dir, [
            "add", f"A: {task_def['title']}",
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
            result["status"] = "error"
            result["failure_mode"] = "crash"
            print(f"  [{trial_id}] TASK CREATION FAILED: {add_out[:200]}", flush=True)
            return result

        # Start service
        await exec_wg(wg_dir, [
            "service", "start",
            "--max-agents", str(MAX_AGENTS_CONDITION_A),
            "--executor", EXECUTOR,
            "--model", MODEL,
            "--no-coordinator-agent",
            "--force",
        ])
        print(f"  [{trial_id}] Service started, polling...", flush=True)

        # Poll for completion
        status, elapsed = await poll_completion(wg_dir, root_task_id, timeout)
        result["status"] = status
        result["elapsed_s"] = round(elapsed, 2)

        if status == "done":
            result["reward"] = 1.0
        elif status == "timeout":
            result["failure_mode"] = "timeout"
        elif status == "failed":
            result["failure_mode"] = "wrong_answer"
        else:
            result["failure_mode"] = "crash"

        print(f"  [{trial_id}] {status.upper()} in {elapsed:.1f}s (reward={result['reward']})", flush=True)

        # Stop service
        await exec_wg(wg_dir, ["service", "stop", "--kill-agents"])

        # Collect metrics
        result["metrics"] = await collect_metrics(wg_dir)

    except Exception as e:
        result["status"] = "error"
        result["error"] = str(e)
        result["failure_mode"] = "crash"
        print(f"  [{trial_id}] Error: {e}", flush=True)
        try:
            await exec_wg(wg_dir, ["service", "stop", "--kill-agents"])
        except Exception:
            pass
    finally:
        result["elapsed_s"] = round(time.monotonic() - start, 2)
        # Save wg state for analysis
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
# Condition G: Raw Claude Code (no workgraph)
# ---------------------------------------------------------------------------

async def run_condition_g(
    task_def: dict, timeout: float,
) -> dict:
    """Run a single Condition G trial (raw Claude Code, no workgraph)."""
    trial_id = f"condG-{task_def['id']}"
    result = {
        "trial_id": trial_id,
        "task_id": task_def["id"],
        "condition": "G",
        "difficulty": task_def["difficulty"],
        "category": task_def.get("category", "unknown"),
        "model": MODEL,
        "status": "not_started",
        "elapsed_s": 0.0,
        "reward": 0.0,
        "failure_mode": None,
        "metrics": None,
        "error": None,
    }

    cleanup_tmp_paths(task_def.get("tmp_paths", []))
    tmpdir = tempfile.mkdtemp(prefix=f"tb-condG-{task_def['id']}-")
    start = time.monotonic()

    print(f"  [{trial_id}] Starting (raw Claude Code, model={MODEL})...", flush=True)

    try:
        instruction = load_instruction(task_def)

        # Run claude CLI in print mode — no workgraph, just raw Claude Code
        # --model haiku: use Haiku model
        # --print: non-interactive
        # --dangerously-skip-permissions: allow tool execution without prompts
        # --bare: skip hooks, CLAUDE.md, etc. for clean baseline
        cmd = [
            CLAUDE_BIN,
            "--model", CLAUDE_MODEL_FLAG,
            "--print",
            "--dangerously-skip-permissions",
            "--output-format", "json",
        ]

        # Strip WG_* env vars so claude doesn't pick up workgraph context
        clean_env = {
            k: v for k, v in os.environ.items()
            if not k.startswith("WG_") and k != "CLAUDECODE"
        }
        # Set working directory to tmpdir
        clean_env["HOME"] = os.environ.get("HOME", "/home/erik")

        proc = await asyncio.create_subprocess_exec(
            *cmd,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=clean_env,
            cwd=tmpdir,
        )

        try:
            stdout, stderr = await asyncio.wait_for(
                proc.communicate(input=instruction.encode()),
                timeout=timeout,
            )
            elapsed = time.monotonic() - start

            # Parse JSON output for metrics
            claude_output = stdout.decode(errors="replace")
            try:
                output_json = json.loads(claude_output)
                usage = output_json.get("usage", {})
                result["metrics"] = {
                    "total_input_tokens": usage.get("input_tokens", 0),
                    "total_output_tokens": usage.get("output_tokens", 0),
                    "total_cost_usd": output_json.get("total_cost_usd", 0.0),
                    "total_turns": output_json.get("num_turns", 0),
                    "num_agents_spawned": 0,
                }
            except json.JSONDecodeError:
                result["metrics"] = {
                    "total_input_tokens": 0,
                    "total_output_tokens": 0,
                    "total_cost_usd": 0.0,
                    "total_turns": 0,
                    "num_agents_spawned": 0,
                }

            if proc.returncode != 0:
                print(f"  [{trial_id}] Claude exited with code {proc.returncode}", flush=True)

        except asyncio.TimeoutError:
            proc.kill()
            await proc.wait()
            elapsed = time.monotonic() - start
            result["status"] = "timeout"
            result["failure_mode"] = "timeout"
            result["elapsed_s"] = round(elapsed, 2)
            print(f"  [{trial_id}] TIMEOUT after {elapsed:.1f}s", flush=True)
            return result

        # Verify the result
        passed = run_verify(task_def["verify_cmd"])
        result["elapsed_s"] = round(elapsed, 2)

        if passed:
            result["status"] = "done"
            result["reward"] = 1.0
        else:
            result["status"] = "failed"
            result["failure_mode"] = "wrong_answer"

        print(f"  [{trial_id}] {'DONE' if passed else 'FAILED'} in {elapsed:.1f}s (reward={result['reward']})", flush=True)

    except Exception as e:
        result["status"] = "error"
        result["error"] = str(e)
        result["failure_mode"] = "crash"
        result["elapsed_s"] = round(time.monotonic() - start, 2)
        print(f"  [{trial_id}] Error: {e}", flush=True)
    finally:
        # Save claude output for analysis
        output_dst = os.path.join(RESULTS_DIR, trial_id)
        try:
            os.makedirs(output_dst, exist_ok=True)
        except Exception:
            pass
        shutil.rmtree(tmpdir, ignore_errors=True)

    return result


# ---------------------------------------------------------------------------
# Main orchestrator
# ---------------------------------------------------------------------------

async def run_trial_with_limit(
    semaphore: asyncio.Semaphore,
    cond: str,
    task_def: dict,
    timeout: float,
) -> dict:
    async with semaphore:
        if cond == "A":
            return await run_condition_a(task_def, timeout)
        else:
            return await run_condition_g(task_def, timeout)


def build_comparison_table(results: list[dict]) -> str:
    """Build a markdown comparison table."""
    # Group by task
    tasks = {}
    for r in results:
        tid = r["task_id"]
        if tid not in tasks:
            tasks[tid] = {"A": None, "G": None}
        tasks[tid][r["condition"]] = r

    lines = []
    lines.append("| Task | Difficulty | Cond A (wg) | Cond A Time | Cond G (raw) | Cond G Time | Delta |")
    lines.append("|------|-----------|-------------|------------|--------------|------------|-------|")

    a_wins = 0
    g_wins = 0
    ties = 0

    for tid, conds in tasks.items():
        a = conds.get("A", {})
        g = conds.get("G", {})
        a_reward = a.get("reward", 0) if a else 0
        g_reward = g.get("reward", 0) if g else 0
        a_time = a.get("elapsed_s", 0) if a else 0
        g_time = g.get("elapsed_s", 0) if g else 0
        difficulty = (a or g or {}).get("difficulty", "?")

        a_status = "PASS" if a_reward == 1.0 else (a.get("failure_mode", "fail") if a else "N/A")
        g_status = "PASS" if g_reward == 1.0 else (g.get("failure_mode", "fail") if g else "N/A")

        if a_reward > g_reward:
            delta = "A wins"
            a_wins += 1
        elif g_reward > a_reward:
            delta = "G wins"
            g_wins += 1
        else:
            delta = "tie"
            ties += 1

        lines.append(
            f"| {tid} | {difficulty} | {a_status} | {a_time:.1f}s | {g_status} | {g_time:.1f}s | {delta} |"
        )

    lines.append("")
    lines.append(f"**Summary:** A wins {a_wins}, G wins {g_wins}, ties {ties}")

    return "\n".join(lines)


async def main(task_names: list[str] | None, timeout: float, smoke: bool):
    tasks = task_names or list(SELECTED_TASKS.keys())
    if smoke:
        tasks = [tasks[0]]

    total_trials = len(tasks) * 2  # 2 conditions
    print(f"Pilot: Condition A vs G (Claude Haiku)")
    print(f"  Model: {MODEL}")
    print(f"  Tasks: {tasks}")
    print(f"  Total trials: {total_trials}")
    print(f"  Max concurrent: {MAX_CONCURRENT_TRIALS}")
    print(f"  Timeout: {timeout}s per trial")
    print(f"  wg binary: {WG_BIN}")
    print(f"  claude binary: {CLAUDE_BIN}")
    print()

    os.makedirs(RESULTS_DIR, exist_ok=True)

    semaphore = asyncio.Semaphore(MAX_CONCURRENT_TRIALS)
    overall_start = time.monotonic()

    # Build trial list: all Condition A, then all Condition G
    trial_coros = []
    for cond in ["A", "G"]:
        for task_name in tasks:
            if task_name not in SELECTED_TASKS:
                print(f"  WARNING: Unknown task '{task_name}', skipping")
                continue
            trial_coros.append(
                run_trial_with_limit(semaphore, cond, SELECTED_TASKS[task_name], timeout)
            )

    print(f"Launching {len(trial_coros)} trials...\n")
    all_results = await asyncio.gather(*trial_coros)
    results = list(all_results)

    total_wall_clock = time.monotonic() - overall_start

    # Build comparison table
    table = build_comparison_table(results)

    # Compute per-condition stats
    cond_stats = {}
    for cond in ["A", "G"]:
        cond_results = [r for r in results if r["condition"] == cond]
        passed = sum(1 for r in cond_results if r["reward"] == 1.0)
        times = [r["elapsed_s"] for r in cond_results if r["elapsed_s"] > 0]
        total_tokens = sum(
            (r.get("metrics") or {}).get("total_input_tokens", 0)
            + (r.get("metrics") or {}).get("total_output_tokens", 0)
            for r in cond_results
        )
        total_cost = sum(
            (r.get("metrics") or {}).get("total_cost_usd", 0.0) for r in cond_results
        )
        cond_stats[cond] = {
            "total": len(cond_results),
            "passed": passed,
            "pass_rate": passed / len(cond_results) if cond_results else 0,
            "mean_time_s": round(sum(times) / len(times), 2) if times else 0,
            "total_tokens": total_tokens,
            "total_cost_usd": round(total_cost, 4),
        }

    # Write JSON summary
    summary = {
        "run_id": "pilot-a-vs-g-haiku",
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "model": MODEL,
        "tasks": tasks,
        "total_wall_clock_s": round(total_wall_clock, 2),
        "condition_stats": cond_stats,
        "trials": results,
    }
    json_path = os.path.join(RESULTS_DIR, "summary.json")
    with open(json_path, "w") as f:
        json.dump(summary, f, indent=2)

    # Write markdown report
    md_path = os.path.join(RESULTS_DIR, "comparison-report.md")
    with open(md_path, "w") as f:
        f.write("# Pilot: Condition A vs G — Claude Haiku\n\n")
        f.write(f"**Date:** {datetime.now(timezone.utc).strftime('%Y-%m-%d %H:%M UTC')}\n")
        f.write(f"**Model:** {MODEL}\n")
        f.write(f"**Tasks:** {len(tasks)}\n")
        f.write(f"**Total wall clock:** {total_wall_clock:.1f}s ({total_wall_clock/60:.1f}min)\n\n")

        f.write("## Conditions\n\n")
        f.write("- **Condition A (wg):** Workgraph-coordinated. Isolated wg service + native-exec agent.\n")
        f.write("- **Condition G (raw):** Raw Claude Code. `claude --model haiku -p` with no workgraph.\n\n")

        f.write("## Results\n\n")
        f.write(table + "\n\n")

        f.write("## Per-Condition Summary\n\n")
        f.write("| Condition | Pass Rate | Mean Time | Total Tokens | Cost |\n")
        f.write("|-----------|-----------|-----------|-------------|------|\n")
        for cond, stats in cond_stats.items():
            label = "A (wg)" if cond == "A" else "G (raw)"
            f.write(
                f"| {label} | {stats['passed']}/{stats['total']} ({stats['pass_rate']:.0%}) | "
                f"{stats['mean_time_s']:.1f}s | {stats['total_tokens']:,} | "
                f"${stats['total_cost_usd']:.4f} |\n"
            )

        f.write("\n## Conclusion\n\n")
        a_pass = cond_stats["A"]["pass_rate"]
        g_pass = cond_stats["G"]["pass_rate"]
        if a_pass > g_pass:
            f.write("Workgraph coordination (Condition A) **improved** performance over raw Claude Code.\n")
        elif g_pass > a_pass:
            f.write("Raw Claude Code (Condition G) performed **better** than workgraph coordination.\n")
        else:
            f.write("Both conditions performed **equally** — workgraph was neutral on this task set.\n")

        a_time = cond_stats["A"]["mean_time_s"]
        g_time = cond_stats["G"]["mean_time_s"]
        if a_time > 0 and g_time > 0:
            if a_time < g_time:
                f.write(f"Condition A was faster on average ({a_time:.1f}s vs {g_time:.1f}s).\n")
            elif g_time < a_time:
                f.write(f"Condition G was faster on average ({g_time:.1f}s vs {a_time:.1f}s).\n")

        f.write("\n## Trial Details\n\n")
        f.write("| Trial | Condition | Task | Status | Time | Tokens | Failure Mode |\n")
        f.write("|-------|-----------|------|--------|------|--------|--------------|\n")
        for r in results:
            m = r.get("metrics") or {}
            tokens = m.get("total_input_tokens", 0) + m.get("total_output_tokens", 0)
            status = "PASS" if r["reward"] == 1.0 else r["status"].upper()
            cond_label = "A (wg)" if r["condition"] == "A" else "G (raw)"
            f.write(
                f"| {r['trial_id']} | {cond_label} | {r['task_id']} | "
                f"{status} | {r['elapsed_s']:.1f}s | {tokens:,} | "
                f"{r.get('failure_mode') or 'N/A'} |\n"
            )

    # Print to stdout
    print(f"\n{'='*60}")
    print(f"Pilot: Condition A vs G — Claude Haiku")
    print(f"{'='*60}")
    print(f"  Wall clock: {total_wall_clock:.1f}s ({total_wall_clock/60:.1f}min)")
    print()
    print(table)
    print()
    for cond, stats in cond_stats.items():
        label = "A (wg)" if cond == "A" else "G (raw)"
        print(f"  {label}: {stats['passed']}/{stats['total']} ({stats['pass_rate']:.0%}), "
              f"mean {stats['mean_time_s']:.1f}s, ${stats['total_cost_usd']:.4f}")
    print(f"\n  Results: {json_path}")
    print(f"  Report:  {md_path}")

    return summary


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        description="Pilot: Condition A (wg) vs G (raw Claude Code), Claude Haiku"
    )
    parser.add_argument("--tasks", type=str, default=None,
                        help="Comma-separated task names (default: 5 diverse)")
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT,
                        help=f"Timeout per trial (default: {DEFAULT_TIMEOUT}s)")
    parser.add_argument("--smoke", action="store_true",
                        help="Smoke test: single task only")
    args = parser.parse_args()

    task_names = args.tasks.split(",") if args.tasks else None
    summary = asyncio.run(main(task_names, args.timeout, args.smoke))

    # Print final pass rates
    a_rate = summary["condition_stats"]["A"]["pass_rate"]
    g_rate = summary["condition_stats"]["G"]["pass_rate"]
    print(f"\nFinal: A={a_rate:.0%}, G={g_rate:.0%}")
