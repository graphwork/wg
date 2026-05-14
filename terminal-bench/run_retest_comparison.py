#!/usr/bin/env python3
"""
TB Retest: Smart Fanout G vs Original G vs A (10-task comparison).

Runs the same 10-task subset under three conditions on the same model
(Qwen3-Coder-30B on lambda01) and produces a comparison table.

Conditions:
  A (baseline): Already have results — loaded from qwen3-hard-20-a/
  G-original:   Unconditional decomposition (always creates wg subtasks)
  G-smart:      Try-first smart fanout (direct impl default, decompose only if needed)

Task selection (10 tasks):
  A-failed (context overflow, G's theoretical win):
    cobol-modernization, constraints-scheduling, multi-source-data-merger
  A-passed (hard, tests G overhead):
    algorithm, ml, fix-code-vulnerability, configure-git-webserver
  A-passed (medium/easy, calibration):
    debugging, data-processing, file-ops

Usage:
    python run_retest_comparison.py                     # run G-original then G-smart
    python run_retest_comparison.py --condition g       # G-original only
    python run_retest_comparison.py --condition g-smart # G-smart only
    python run_retest_comparison.py --analyze-only      # skip runs, just analyze existing results
    python run_retest_comparison.py --smoke             # single task quick check
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

from wg.daemon_cleanup import daemon_registry
from wg.tasks import TASKS_BY_ID, ALL_TASKS

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

MODEL = "local:qwen3-coder-30b"
SGLANG_BASE_URL = "http://lambda01:30000/v1"
CONTEXT_WINDOW = 32768
DEFAULT_POLL_INTERVAL = 5.0

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
RESULTS_BASE = os.path.join(SCRIPT_DIR, "tb-results")
WG_BIN = shutil.which("wg") or os.path.expanduser("~/.cargo/bin/wg")

# 10 representative tasks
RETEST_TASKS = [
    # A-failed (context overflow) — G's theoretical win
    "cobol-modernization",          # hard: COBOL -> Python, heavy context
    "constraints-scheduling",       # hard: ICS parsing + constraint solving
    "multi-source-data-merger",     # hard: 3 formats -> merge -> conflicts
    # A-passed (hard) — tests G overhead
    "algorithm",                    # hard: kv store with transactions (simple, fast)
    "ml",                           # hard: k-means from scratch
    "fix-code-vulnerability",       # hard: analyze -> report -> fix
    "configure-git-webserver",      # hard: pipeline git + hooks + webserver
    # A-passed (calibration)
    "debugging",                    # medium: fix merge sort bugs
    "data-processing",              # medium: JSON -> CSV
    "file-ops",                     # easy: create project structure
]

# Per-condition timeout and max_agents config
CONDITION_PARAMS = {
    "g": {
        "max_agents": 4,
        "timeouts": {"easy": 600, "medium": 900, "hard": 1200},  # 20min hard — enough to show failure pattern
        "run_id": "retest-g-original",
        "label": "G-original (always decompose)",
        "smart": False,
        "worktree_isolation": False,
    },
    "g-smart": {
        "max_agents": 4,
        "timeouts": {"easy": 600, "medium": 1200, "hard": 2400},
        "run_id": "retest-g-smart",
        "label": "G-smart (try-first fanout)",
        "smart": True,
        "worktree_isolation": True,
    },
}


# ---------------------------------------------------------------------------
# Meta-prompts (duplicated from run_qwen3_hard_20_g.py for self-containment)
# ---------------------------------------------------------------------------

CONDITION_G_META_PROMPT = """You are a graph architect. You do NOT implement solutions yourself.

Your job:
1. Read the task below and understand what needs to be done
2. Explore the working directory (`ls`, `cat`) to understand the codebase
3. Build a WG task graph that solves the problem, then mark YOUR task done

DO NOT write code. DO NOT modify files. Only create wg tasks.

## Graph design

Create tasks using `wg add`, then wire them into a self-correcting cycle:

```bash
# 1. Work tasks (parallelize where possible — up to {max_agents} agents run concurrently)
wg add "Implement the solution" --no-place -d "Description of what to do..."

# 2. Verify task (runs after work, checks if tests pass)
wg add "Run tests and verify" --after implement-the-solution --no-place \\
  -d "Run the test suite: <test command>.
If tests PASS: wg done <your-task-id> --converged
If tests FAIL: wg log <your-task-id> 'what failed and why', then wg done <your-task-id>"

# 3. Close the loop: work task cycles back through verify
wg edit implement-the-solution --add-after run-tests-and-verify --max-iterations 5
```

The verify agent signals `--converged` when tests pass (stops the loop) or
plain `wg done` when tests fail (triggers another iteration with failure
context visible to the next work agent via `wg context`).

## Context management — CRITICAL

Your context window is only {context_window} tokens. This is SHORT.
Decomposition is your main tool for managing context pressure:
- Each subtask gets a FRESH context window
- Break complex multi-file tasks into focused sub-problems
- Don't try to fit everything into one agent's context

## Important details for sub-task descriptions

Worker agents don't see this prompt. They only see the description you write
in `wg add -d "..."`. So put ALL necessary context in each task's description:
- What files to read/modify
- What the expected output is
- How to verify (test command)
- IMPORTANT: Remind them their context window is only {context_window} tokens
- For the verify task: EXACTLY when to use `--converged` vs plain `wg done`

## After building the graph

Call `wg done {seed_task_id}` to mark this seed task complete. The
coordinator dispatches worker agents to your tasks automatically.

"""

CONDITION_G_SMART_META_PROMPT = """You are solving a programming task. You have two strategies available:

**Strategy 1 — Direct Implementation (default)**
Implement the solution yourself. This is fastest for most tasks.

**Strategy 2 — Decomposition (only when needed)**
Break the task into subtasks and let other agents implement them in parallel.
Only use this if direct implementation won't work.

## Step 1: Triage (spend < 2 minutes here)

Read the task. Scan the working directory (`ls`, `ls tests/`). Then decide:

**Use DIRECT IMPLEMENTATION if ANY of these are true:**
- The instruction is under ~300 words
- You need to modify 2 or fewer files
- The test suite has 5 or fewer tests
- The task is a single logical unit of work (even if complex)
- You're not sure → default to direct implementation

**Use DECOMPOSITION only if ALL of these are true:**
- The instruction is over ~500 words
- You need to modify 3+ distinct files
- The work splits into 2-4 independent sub-problems (different files, no ordering)
- Each sub-problem is substantial enough to benefit from a fresh context window

**Log your decision:**
```bash
wg log {seed_task_id} "FANOUT_DECISION: <direct|decompose> — <reason>"
```

## Context management — CRITICAL

Your context window is only {context_window} tokens. This is SHORT.
If you choose direct implementation, be aware of context pressure:
- If you start re-reading files you already read, or losing track of earlier edits
- Switch to decomposition for the REMAINING work only
```bash
wg log {seed_task_id} "FANOUT_SWITCH: direct→decompose — context pressure after N turns"
```

## If Direct Implementation

Implement the solution. Write code, modify files, run tests.

If tests pass → `wg done {seed_task_id}`

## If Decomposition

1. **Serialize your exploration** — everything you learned during triage goes
   into the subtask descriptions. File paths, test commands, patterns, edge cases.
   Workers only see what you write in `wg add -d "..."`.

2. **Create 2-4 focused subtasks** (NEVER more than 4):
```bash
wg add "Part 1: <specific scope>" --no-place -d "## What to do
<concrete instructions>

## Files to modify
- path/to/file1.py

## How to verify
Run: <test command>

## Context
Your context window is only {context_window} tokens.

## IMPORTANT
Implement directly. Do NOT create subtasks. Do NOT decompose further."
```

3. **Wire in a verify task**:
```bash
wg add "Verify: run full test suite" --after part-1,part-2 --no-place \\
  -d "Run the test suite: <test command>.
If ALL tests pass: wg done <your-task-id> --converged
If tests fail: wg log <your-task-id> 'what failed' then wg done <your-task-id>"
```

4. **Create the retry loop** (if the task warrants iteration):
```bash
wg edit part-1 --add-after verify --max-iterations 3
```

5. **Mark your seed task done**:
```bash
wg done {seed_task_id}
```

## Hard constraints
- NEVER create more than 4 subtasks
- Subtasks must NOT create their own subtasks (1 level max)
- If two subtasks would modify the same file, merge them or serialize with --after
- Always include a verify task at the end

"""


# ---------------------------------------------------------------------------
# Helpers (adapted from run_qwen3_hard_20_g.py)
# ---------------------------------------------------------------------------

async def exec_wg(wg_dir: str, subcmd: list[str], timeout: float = 120,
                  extra_env: dict | None = None) -> str:
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
    path = os.path.join(SCRIPT_DIR, task_def["instruction_file"])
    with open(path) as f:
        return f.read().strip()


def cleanup_tmp_paths(task_def: dict) -> None:
    """Clean up /tmp artifacts from previous runs of the same task.

    Parses the verify_cmd for /tmp paths and removes them.
    Also handles explicit tmp_paths if present.
    """
    import re
    for p in task_def.get("tmp_paths", []):
        if os.path.isdir(p):
            shutil.rmtree(p, ignore_errors=True)
        elif os.path.isfile(p):
            os.remove(p)

    # Parse verify_cmd for /tmp paths
    verify_cmd = task_def.get("verify_cmd", "")
    tmp_paths = set(re.findall(r'/tmp/[\w./_-]+', verify_cmd))
    for p in tmp_paths:
        # Only clean files/dirs, not /tmp itself
        if p == "/tmp" or p == "/tmp/":
            continue
        # Get the top-level /tmp/X directory or file
        parts = p.split("/")
        if len(parts) >= 3:
            top = "/".join(parts[:3])  # /tmp/something
            if os.path.isdir(top):
                shutil.rmtree(top, ignore_errors=True)
            elif os.path.isfile(top):
                os.remove(top)


async def poll_graph_quiescence(
    wg_dir: str,
    timeout_secs: float,
    poll_interval: float = DEFAULT_POLL_INTERVAL,
) -> tuple[str, float]:
    """Poll until all non-internal tasks reach terminal status."""
    start = time.monotonic()
    while True:
        elapsed = time.monotonic() - start
        if elapsed > timeout_secs:
            return "timeout", elapsed

        has_active = False
        for check_status in ("open", "in-progress", "blocked"):
            result = await exec_wg(wg_dir, ["list", "--status", check_status])
            if "[exit code:" in result:
                continue
            for line in result.strip().splitlines():
                stripped = line.strip()
                if not stripped or "no tasks" in stripped.lower() or "tasks found" in stripped.lower():
                    continue
                parts = stripped.split()
                if not parts:
                    continue
                if len(parts) >= 2 and parts[0].startswith("["):
                    task_id_col = parts[1]
                else:
                    task_id_col = parts[0]
                if task_id_col.startswith("─") or task_id_col == "ID":
                    continue
                if task_id_col.startswith("."):
                    continue
                has_active = True
                break
            if has_active:
                break

        if not has_active:
            done_result = await exec_wg(wg_dir, ["list", "--status", "done"])
            if "[exit code:" not in done_result and done_result.strip():
                for line in done_result.strip().splitlines():
                    stripped = line.strip()
                    if not stripped or "no tasks" in stripped.lower():
                        continue
                    parts = stripped.split()
                    if not parts:
                        continue
                    if len(parts) >= 2 and parts[0].startswith("["):
                        task_id_col = parts[1]
                    else:
                        task_id_col = parts[0]
                    if task_id_col.startswith("─") or task_id_col == "ID":
                        continue
                    if not task_id_col.startswith("."):
                        return "done", elapsed
            return "failed", elapsed

        await asyncio.sleep(poll_interval)


async def collect_metrics(wg_dir: str) -> dict:
    """Read agent stream.jsonl files to extract token counts."""
    agents_dir = os.path.join(wg_dir, "agents")
    metrics = {
        "total_input_tokens": 0,
        "total_output_tokens": 0,
        "total_turns": 0,
        "num_agents_spawned": 0,
        "max_input_tokens_single_turn": 0,
        "context_truncation_events": 0,
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
                            in_tok = usage.get("input_tokens", 0)
                            out_tok = usage.get("output_tokens", 0)
                            metrics["total_input_tokens"] += in_tok
                            metrics["total_output_tokens"] += out_tok
                            if in_tok > metrics["max_input_tokens_single_turn"]:
                                metrics["max_input_tokens_single_turn"] = in_tok
                            if in_tok > CONTEXT_WINDOW * 0.8:
                                metrics["context_truncation_events"] += 1
        except Exception:
            pass

    return metrics


async def count_subtasks(wg_dir: str) -> dict:
    """Count tasks in the graph."""
    graph_path = os.path.join(wg_dir, "graph.jsonl")
    counts = {
        "total_tasks": 0,
        "user_tasks": 0,
        "internal_tasks": 0,
        "done_tasks": 0,
        "failed_tasks": 0,
        "task_ids": [],
    }

    if not os.path.isfile(graph_path):
        return counts

    try:
        with open(graph_path) as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    task = json.loads(line)
                except json.JSONDecodeError:
                    continue

                task_id = task.get("id", "")
                status = task.get("status", "")
                counts["total_tasks"] += 1

                if task_id.startswith("."):
                    counts["internal_tasks"] += 1
                else:
                    counts["user_tasks"] += 1
                    counts["task_ids"].append(task_id)
                    if status == "done":
                        counts["done_tasks"] += 1
                    elif status in ("failed", "abandoned"):
                        counts["failed_tasks"] += 1
    except Exception:
        pass

    return counts


def extract_fanout_decisions(wg_dir: str) -> dict:
    """Extract FANOUT_DECISION and FANOUT_SWITCH log entries from the graph."""
    graph_path = os.path.join(wg_dir, "graph.jsonl")
    decisions = {
        "initial_decision": None,
        "switched": False,
        "switch_reason": None,
        "all_entries": [],
    }

    if not os.path.isfile(graph_path):
        return decisions

    try:
        with open(graph_path) as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    task = json.loads(line)
                except json.JSONDecodeError:
                    continue

                # Check log entries for FANOUT_DECISION/FANOUT_SWITCH
                for log_entry in task.get("log", []):
                    msg = log_entry.get("message", "") if isinstance(log_entry, dict) else str(log_entry)
                    if "FANOUT_DECISION" in msg:
                        decisions["initial_decision"] = msg
                        decisions["all_entries"].append(msg)
                    elif "FANOUT_SWITCH" in msg:
                        decisions["switched"] = True
                        decisions["switch_reason"] = msg
                        decisions["all_entries"].append(msg)
    except Exception:
        pass

    # Also check daemon log for fanout entries
    daemon_log = os.path.join(wg_dir, "service", "daemon.log")
    if os.path.isfile(daemon_log):
        try:
            with open(daemon_log) as f:
                for line in f:
                    if "FANOUT" in line:
                        decisions["all_entries"].append(line.strip()[:200])
        except Exception:
            pass

    return decisions


def analyze_daemon_log(wg_dir: str) -> dict:
    """Parse daemon log for errors."""
    daemon_log = os.path.join(wg_dir, "service", "daemon.log")
    analysis = {"error_patterns": [], "log_tail": ""}

    if not os.path.isfile(daemon_log):
        return analysis

    try:
        with open(daemon_log) as f:
            log_content = f.read()

        for pattern, category in [
            ("rate limit", "rate_limit"), ("429", "rate_limit"),
            ("context length", "context_overflow"), ("maximum context", "context_overflow"),
            ("out of memory", "oom"), ("OOM", "oom"),
            ("connection refused", "endpoint_down"),
        ]:
            if pattern.lower() in log_content.lower():
                analysis["error_patterns"].append(category)

        analysis["log_tail"] = log_content[-3000:]
    except Exception:
        pass

    return analysis


def write_trial_config(wg_dir: str, max_agents: int, worktree_isolation: bool = False) -> None:
    """Write config.toml for a trial."""
    worktree_val = "true" if worktree_isolation else "false"
    config = f"""[coordinator]
max_agents = {max_agents}
executor = "native"
model = "{MODEL}"
worktree_isolation = {worktree_val}
agent_timeout = "40m"
max_verify_failures = 0
max_spawn_failures = 0
coordinator_agent = true
heartbeat_interval = 30

[agent]
model = "{MODEL}"
context_scope = "graph"
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
    condition: str,
    params: dict,
    results_dir: str,
) -> dict:
    """Run a single trial."""
    task_id = task_def["id"]
    trial_id = f"{params['run_id']}-{task_id}"
    smart = params["smart"]
    max_agents = params["max_agents"]
    timeout = params["timeouts"].get(task_def["difficulty"], 2400)

    result = {
        "trial_id": trial_id,
        "task": task_id,
        "difficulty": task_def["difficulty"],
        "condition": condition,
        "condition_label": params["label"],
        "smart_fanout": smart,
        "model": MODEL,
        "max_agents": max_agents,
        "timeout_s": timeout,
        "status": "not_started",
        "elapsed_s": 0.0,
        "reward": 0.0,
        "failure_mode": None,
        "metrics": None,
        "subtask_counts": None,
        "fanout_decisions": None,
        "error": None,
    }

    cleanup_tmp_paths(task_def)
    tmpdir = tempfile.mkdtemp(prefix=f"tb-retest-{condition}-{task_id}-")
    wg_dir = os.path.join(tmpdir, ".workgraph")
    start = time.monotonic()

    # Initialize a git repo so worktree isolation can work if enabled,
    # and so agents can use git commands inside the trial.
    subprocess.run(["git", "init", tmpdir], capture_output=True)
    subprocess.run(["git", "-C", tmpdir, "commit", "--allow-empty", "-m", "init"],
                   capture_output=True)

    print(f"  [{trial_id}] Starting in {tmpdir} (timeout={timeout}s)...")

    try:
        # 1. Init graph
        init_out = await exec_wg(wg_dir, ["init"])
        if "error" in init_out.lower() and "already" not in init_out.lower():
            result["error"] = f"Init failed: {init_out}"
            result["status"] = "failed"
            result["failure_mode"] = "init_error"
            return result

        # 2. Write config
        write_trial_config(wg_dir, max_agents, params.get("worktree_isolation", False))

        # 3. Build task description with meta-prompt
        instruction = load_instruction(task_def)
        root_task_id = f"tb-{task_id}"

        base_prompt = CONDITION_G_SMART_META_PROMPT if smart else CONDITION_G_META_PROMPT
        meta = base_prompt.replace("{seed_task_id}", root_task_id)
        meta = meta.replace("{max_agents}", str(max_agents))
        meta = meta.replace("{context_window}", str(CONTEXT_WINDOW))

        if task_def.get("verify_cmd"):
            meta += (
                f"\n## Test command\n"
                f"The test command that determines pass/fail is:\n"
                f"```\n{task_def['verify_cmd']}\n```\n"
                f"Include this command in your verify task's description "
                f"so it knows exactly what to run.\n\n"
            )

        full_instruction = meta + instruction
        description = (
            f"## Terminal Bench Retest: {params['label']}\n\n"
            f"**Task:** {task_id} ({task_def['difficulty']})\n"
            f"**Model:** {MODEL}\n"
            f"**Context Window:** {CONTEXT_WINDOW} tokens\n\n"
            f"## Instructions\n\n{full_instruction}\n"
        )

        add_out = await exec_wg(wg_dir, [
            "add", f"TB: {task_def['title']}",
            "--id", root_task_id,
            "-d", description,
            "--exec-mode", "full",
            "--context-scope", "graph",
            "--model", MODEL,
            "--no-place",
        ])
        if "[exit code:" in add_out and root_task_id not in add_out:
            result["error"] = f"Task creation failed: {add_out}"
            result["status"] = "failed"
            result["failure_mode"] = "task_create_error"
            return result

        # 4. Start service
        service_out = await exec_wg(wg_dir, [
            "service", "start",
            "--max-agents", str(max_agents),
            "--executor", "native",
            "--model", MODEL,
            "--force",
        ])
        daemon_registry.register(wg_dir, WG_BIN)
        print(f"  [{trial_id}] Service started, polling for quiescence...")

        # 5. Poll
        status, elapsed = await poll_graph_quiescence(wg_dir, timeout)
        result["status"] = status
        result["elapsed_s"] = round(elapsed, 2)

        # 6. Run verify command
        try:
            verify_result = subprocess.run(
                ["bash", "-c", task_def["verify_cmd"]],
                capture_output=True, text=True, timeout=60,
            )
            verify_passed = verify_result.returncode == 0
        except (subprocess.TimeoutExpired, Exception):
            verify_passed = False

        if verify_passed:
            result["reward"] = 1.0
            result["failure_mode"] = None
            if status == "timeout":
                result["status"] = "done_after_timeout"
            elif status != "done":
                result["status"] = "done"
        elif status == "timeout":
            result["failure_mode"] = "timeout"
        elif status == "failed":
            result["failure_mode"] = "wrong_answer"
        else:
            result["failure_mode"] = f"status_{status}"

        print(f"  [{trial_id}] Completed: {status} in {elapsed:.1f}s "
              f"(reward={result['reward']}, verify={'PASS' if verify_passed else 'FAIL'})")

        # 7. Stop service (daemon_registry.stop_one in finally is the safety net)
        daemon_registry.stop_one(wg_dir)

        # 8. Collect metrics
        result["metrics"] = await collect_metrics(wg_dir)
        result["subtask_counts"] = await count_subtasks(wg_dir)

        # 9. Extract fanout decisions (G-smart specific)
        if smart:
            result["fanout_decisions"] = extract_fanout_decisions(wg_dir)

        # 10. Analyze daemon log
        log_analysis = analyze_daemon_log(wg_dir)
        if result["failure_mode"] and log_analysis.get("error_patterns"):
            patterns = log_analysis["error_patterns"]
            if "context_overflow" in patterns:
                result["failure_mode"] = "context_overflow"
            elif "rate_limit" in patterns:
                result["failure_mode"] = "rate_limit"
            elif "oom" in patterns:
                result["failure_mode"] = "oom"

    except Exception as e:
        result["status"] = "error"
        result["error"] = str(e)
        result["failure_mode"] = "exception"
        print(f"  [{trial_id}] Error: {e}")
    finally:
        # Always stop the daemon before cleanup (handles both normal and error paths)
        daemon_registry.stop_one(wg_dir)
        result["elapsed_s"] = round(time.monotonic() - start, 2)
        # Save WG state
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
# Run a full condition
# ---------------------------------------------------------------------------

async def run_condition(condition: str, task_names: list[str]) -> dict:
    """Run all tasks for a single condition, return summary."""
    params = CONDITION_PARAMS[condition]
    results_dir = os.path.join(RESULTS_BASE, params["run_id"])
    os.makedirs(results_dir, exist_ok=True)

    total = len(task_names)
    results = []
    start_time = time.monotonic()

    print(f"\n{'='*80}")
    print(f"Running: {params['label']}")
    print(f"  Tasks ({total}): {task_names}")
    print(f"  Max agents: {params['max_agents']}")
    print(f"  Smart fanout: {params['smart']}")
    print(f"{'='*80}\n")

    for i, task_name in enumerate(task_names, 1):
        if task_name not in TASKS_BY_ID:
            print(f"  WARNING: Unknown task '{task_name}', skipping")
            continue

        task_def = TASKS_BY_ID[task_name]
        timeout = params["timeouts"].get(task_def["difficulty"], 2400)
        print(f"\n--- [{i}/{total}] {task_name} ({task_def['difficulty']}, timeout={timeout}s) ---")

        result = await run_trial(task_def, condition, params, results_dir)
        results.append(result)

        # Print running tally
        passed = sum(1 for r in results if r["reward"] > 0)
        metrics = result.get("metrics") or {}
        subtasks = (result.get("subtask_counts") or {}).get("user_tasks", 0)
        agents = metrics.get("num_agents_spawned", 0)
        turns = metrics.get("total_turns", 0)
        fanout = ""
        if (result.get("fanout_decisions") or {}).get("initial_decision"):
            fanout = f"  decision={result['fanout_decisions']['initial_decision'][:60]}"
        print(f"  Running: {passed}/{len(results)} passed | "
              f"subtasks={subtasks} agents={agents} turns={turns} "
              f"failure={result.get('failure_mode', 'none')}{fanout}")

    total_time = time.monotonic() - start_time
    passed = sum(1 for r in results if r["reward"] > 0)
    total_trials = len(results)
    times = [r["elapsed_s"] for r in results if r["elapsed_s"] > 0]
    median_time = sorted(times)[len(times) // 2] if times else 0

    # Decomposition stats
    decomposed = sum(1 for r in results
                     if (r.get("subtask_counts") or {}).get("user_tasks", 0) > 1)
    total_subtasks = sum((r.get("subtask_counts") or {}).get("user_tasks", 0)
                         for r in results)

    summary = {
        "run_id": params["run_id"],
        "condition": condition,
        "label": params["label"],
        "model": MODEL,
        "context_window": CONTEXT_WINDOW,
        "max_agents": params["max_agents"],
        "smart_fanout": params["smart"],
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "total_trials": total_trials,
        "passed": passed,
        "pass_rate": round(passed / total_trials, 4) if total_trials > 0 else 0,
        "median_time_s": round(median_time, 2),
        "total_wall_clock_s": round(total_time, 2),
        "decomposed_trials": decomposed,
        "decomposition_rate": round(decomposed / total_trials, 4) if total_trials else 0,
        "total_subtasks_created": total_subtasks,
        "avg_subtasks_per_trial": round(total_subtasks / total_trials, 2) if total_trials else 0,
        "trials": results,
    }

    json_path = os.path.join(results_dir, "summary.json")
    with open(json_path, "w") as f:
        json.dump(summary, f, indent=2)

    print(f"\n  {params['label']} complete: {passed}/{total_trials} passed "
          f"({summary['pass_rate']:.0%}), median={median_time:.0f}s, "
          f"decomposed={decomposed}/{total_trials}")
    print(f"  Results: {json_path}")

    return summary


# ---------------------------------------------------------------------------
# Load existing Condition A results
# ---------------------------------------------------------------------------

def load_condition_a_results(task_names: list[str]) -> dict:
    """Load Condition A results for the retest task subset.

    Merges data from two sources:
    - summary.json: has elapsed_s and full metrics dict, but may be incomplete
    - combined_summary.json: has all 18 tasks with reward/turns but no timing
    """
    base_dir = os.path.join(SCRIPT_DIR, "results", "qwen3-hard-20-a")

    # Load both files
    summary_trials = {}
    summary_path = os.path.join(base_dir, "summary.json")
    if os.path.isfile(summary_path):
        with open(summary_path) as f:
            data = json.load(f)
            summary_trials = {t["task"]: t for t in data.get("trials", [])}

    combined_trials = {}
    combined_path = os.path.join(base_dir, "combined_summary.json")
    if os.path.isfile(combined_path):
        with open(combined_path) as f:
            data = json.load(f)
            combined_trials = {t["task"]: t for t in data.get("trials", [])}

    if not summary_trials and not combined_trials:
        print(f"WARNING: Condition A results not found in {base_dir}")
        return None

    subset_trials = []
    for name in task_names:
        # Prefer summary.json (has timing), fall back to combined
        st = summary_trials.get(name)
        ct = combined_trials.get(name)
        t = st or ct
        if not t:
            continue

        metrics = t.get("metrics") or {}
        elapsed = t.get("elapsed_s", 0)

        # If from combined_summary (no elapsed_s), try to reconstruct from
        # WG state or leave as 0
        if not elapsed and ct and not st:
            elapsed = 0  # No timing data available for this task

        subset_trials.append({
            "trial_id": f"condition-a-{name}",
            "task": name,
            "difficulty": t.get("difficulty", "?"),
            "condition": "a",
            "condition_label": "A (baseline, single agent)",
            "smart_fanout": False,
            "model": MODEL,
            "max_agents": 1,
            "status": t.get("status", "?"),
            "elapsed_s": elapsed,
            "reward": t.get("reward", 0),
            "failure_mode": t.get("failure_mode"),
            "metrics": {
                "total_turns": metrics.get("total_turns", t.get("turns", 0)),
                "max_input_tokens_single_turn": metrics.get("max_input_tokens_single_turn",
                                                             t.get("max_in_single_turn", 0)),
                "num_agents_spawned": 1,
            },
            "subtask_counts": {"user_tasks": 1},
        })

    passed = sum(1 for t in subset_trials if t["reward"] > 0)
    times = [t["elapsed_s"] for t in subset_trials if t.get("elapsed_s", 0) > 0]
    median_time = sorted(times)[len(times) // 2] if times else 0

    return {
        "run_id": "condition-a-baseline",
        "condition": "a",
        "label": "A (baseline, single agent)",
        "model": MODEL,
        "total_trials": len(subset_trials),
        "passed": passed,
        "pass_rate": round(passed / len(subset_trials), 4) if subset_trials else 0,
        "median_time_s": round(median_time, 2),
        "decomposed_trials": 0,
        "decomposition_rate": 0,
        "total_subtasks_created": len(subset_trials),
        "avg_subtasks_per_trial": 1.0,
        "trials": subset_trials,
    }


# ---------------------------------------------------------------------------
# Comparison analysis
# ---------------------------------------------------------------------------

def write_comparison(a_summary: dict, g_summary: dict | None, gs_summary: dict | None,
                     task_names: list[str]) -> str:
    """Write the comparison table and analysis to tb-results/."""
    lines = []
    lines.append("# TB Retest: Smart Fanout G vs Original G vs A")
    lines.append(f"\n**Date:** {datetime.now(timezone.utc).strftime('%Y-%m-%d %H:%M UTC')}")
    lines.append(f"**Model:** {MODEL}")
    lines.append(f"**Endpoint:** {SGLANG_BASE_URL}")
    lines.append(f"**Context window:** {CONTEXT_WINDOW} tokens")
    lines.append(f"**Task count:** {len(task_names)}")
    lines.append(f"**Tasks:** {', '.join(task_names)}")
    lines.append("")

    # Summary table
    lines.append("## Summary Comparison")
    lines.append("")
    lines.append("| Metric | Condition A | Condition G-orig | Condition G-smart |")
    lines.append("|--------|------------|------------------|-------------------|")

    def _val(summary, key, fmt="{}", default="—"):
        if summary is None:
            return default
        v = summary.get(key)
        return fmt.format(v) if v is not None else default

    a, g, gs = a_summary, g_summary, gs_summary
    lines.append(f"| Pass rate | {_val(a, 'pass_rate', '{:.0%}')} ({_val(a, 'passed')}/{_val(a, 'total_trials')}) "
                 f"| {_val(g, 'pass_rate', '{:.0%}')} ({_val(g, 'passed')}/{_val(g, 'total_trials')}) "
                 f"| {_val(gs, 'pass_rate', '{:.0%}')} ({_val(gs, 'passed')}/{_val(gs, 'total_trials')}) |")
    lines.append(f"| Median time (s) | {_val(a, 'median_time_s')} | {_val(g, 'median_time_s')} | {_val(gs, 'median_time_s')} |")
    lines.append(f"| Decomposition rate | 0% | {_val(g, 'decomposition_rate', '{:.0%}')} | {_val(gs, 'decomposition_rate', '{:.0%}')} |")
    lines.append(f"| Avg subtasks/trial | 1.0 | {_val(g, 'avg_subtasks_per_trial')} | {_val(gs, 'avg_subtasks_per_trial')} |")
    lines.append(f"| Max agents | 1 | {_val(g, 'max_agents')} | {_val(gs, 'max_agents')} |")
    lines.append("")

    # Per-task head-to-head
    lines.append("## Per-Task Head-to-Head")
    lines.append("")
    lines.append("| Task | Diff | A | G-orig | G-smart | A time | G-orig time | G-smart time | G-smart subtasks | G-smart decision |")
    lines.append("|------|------|---|--------|---------|--------|-------------|--------------|------------------|------------------|")

    a_trials = {t["task"]: t for t in (a.get("trials", []) if a else [])}
    g_trials = {t["task"]: t for t in (g.get("trials", []) if g else [])}
    gs_trials = {t["task"]: t for t in (gs.get("trials", []) if gs else [])}

    for task_name in task_names:
        at = a_trials.get(task_name, {})
        gt = g_trials.get(task_name, {})
        gst = gs_trials.get(task_name, {})

        a_r = "PASS" if at.get("reward", 0) > 0 else ("FAIL" if at else "—")
        g_r = "PASS" if gt.get("reward", 0) > 0 else ("FAIL" if gt else "—")
        gs_r = "PASS" if gst.get("reward", 0) > 0 else ("FAIL" if gst else "—")

        a_t = f"{at.get('elapsed_s', 0):.0f}s" if at else "—"
        g_t = f"{gt.get('elapsed_s', 0):.0f}s" if gt else "—"
        gs_t = f"{gst.get('elapsed_s', 0):.0f}s" if gst else "—"

        gs_subs = str((gst.get("subtask_counts") or {}).get("user_tasks", "—")) if gst else "—"
        gs_decision = ""
        if gst.get("fanout_decisions", {}).get("initial_decision"):
            dec = gst["fanout_decisions"]["initial_decision"]
            # Extract just the decision type
            if "direct" in dec.lower():
                gs_decision = "direct"
            elif "decompose" in dec.lower():
                gs_decision = "decompose"
            if gst.get("fanout_decisions", {}).get("switched"):
                gs_decision += " → switched"

        diff = at.get("difficulty", gst.get("difficulty", gt.get("difficulty", "?")))

        lines.append(f"| {task_name} | {diff} | {a_r} | {g_r} | {gs_r} | {a_t} | {g_t} | {gs_t} | {gs_subs} | {gs_decision} |")

    lines.append("")

    # Failure mode breakdown
    lines.append("## Failure Mode Breakdown")
    lines.append("")
    for label, summary in [("A", a), ("G-original", g), ("G-smart", gs)]:
        if summary is None:
            continue
        modes = {}
        for t in summary.get("trials", []):
            mode = t.get("failure_mode") or "success"
            modes[mode] = modes.get(mode, 0) + 1
        lines.append(f"**{label}:** {modes}")
        lines.append("")

    # G-smart fanout decisions log
    if gs:
        lines.append("## G-smart Fanout Decisions (detailed)")
        lines.append("")
        for t in gs.get("trials", []):
            fd = t.get("fanout_decisions") or {}
            lines.append(f"**{t['task']}** (reward={t['reward']}):")
            if fd.get("initial_decision"):
                lines.append(f"  - Initial: {fd['initial_decision']}")
            if fd.get("switched"):
                lines.append(f"  - Switch: {fd['switch_reason']}")
            if not fd.get("initial_decision") and not fd.get("switched"):
                lines.append(f"  - No FANOUT_DECISION logged")
            lines.append("")

    # Verdict
    lines.append("## Analysis & Verdict")
    lines.append("")
    if gs and a:
        gs_pass = gs.get("pass_rate", 0)
        a_pass = a.get("pass_rate", 0)
        delta = gs_pass - a_pass
        lines.append(f"- **G-smart vs A delta:** {delta:+.0%}")
        if g:
            g_pass = g.get("pass_rate", 0)
            lines.append(f"- **G-original vs A delta:** {g_pass - a_pass:+.0%}")
            lines.append(f"- **G-smart vs G-original delta:** {gs_pass - g_pass:+.0%}")

        lines.append("")

        # Check if smart fanout fixed the overhead problem
        gs_median = gs.get("median_time_s", 0)
        a_median = a.get("median_time_s", 0)
        g_median = g.get("median_time_s", 0) if g else 0

        if g_median > 0:
            overhead_reduction = (g_median - gs_median) / g_median * 100
            lines.append(f"- **Overhead reduction (G-smart vs G-original):** "
                         f"{overhead_reduction:.0f}% (median time {g_median:.0f}s → {gs_median:.0f}s)")
        lines.append(f"- **G-smart overhead vs A:** {gs_median - a_median:+.0f}s median")

    report = "\n".join(lines)
    report_path = os.path.join(RESULTS_BASE, "comparison-report.md")
    with open(report_path, "w") as f:
        f.write(report)

    # Also write structured JSON
    comparison_data = {
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "model": MODEL,
        "context_window": CONTEXT_WINDOW,
        "tasks": task_names,
        "condition_a": a,
        "condition_g_original": g,
        "condition_g_smart": gs,
    }
    json_path = os.path.join(RESULTS_BASE, "comparison-data.json")
    with open(json_path, "w") as f:
        json.dump(comparison_data, f, indent=2)

    print(f"\nComparison report: {report_path}")
    print(f"Comparison data: {json_path}")
    return report


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

async def main():
    parser = argparse.ArgumentParser(description="TB Retest: A vs G-original vs G-smart")
    parser.add_argument("--condition", choices=["g", "g-smart", "both"], default="both",
                        help="Which condition(s) to run (default: both)")
    parser.add_argument("--tasks", nargs="*", help="Override task list")
    parser.add_argument("--smoke", action="store_true", help="Single task quick check")
    parser.add_argument("--analyze-only", action="store_true",
                        help="Skip runs, just analyze existing results")
    args = parser.parse_args()

    tasks = args.tasks or RETEST_TASKS
    if args.smoke:
        tasks = ["algorithm"]

    # Verify endpoint
    import urllib.request
    try:
        with urllib.request.urlopen(f"{SGLANG_BASE_URL}/models", timeout=10) as resp:
            models_data = json.loads(resp.read())
            model_ids = [m["id"] for m in models_data.get("data", [])]
            if "qwen3-coder-30b" not in model_ids:
                print(f"ERROR: qwen3-coder-30b not found at {SGLANG_BASE_URL}")
                sys.exit(1)
            print(f"Endpoint OK: qwen3-coder-30b at {SGLANG_BASE_URL}")
    except Exception as e:
        print(f"ERROR: Cannot reach {SGLANG_BASE_URL}: {e}")
        sys.exit(1)

    os.makedirs(RESULTS_BASE, exist_ok=True)

    # Load Condition A baseline
    a_summary = load_condition_a_results(tasks)
    if a_summary:
        print(f"\nCondition A (baseline): {a_summary['passed']}/{a_summary['total_trials']} "
              f"({a_summary['pass_rate']:.0%})")

    g_summary = None
    gs_summary = None

    if not args.analyze_only:
        # Run requested conditions
        if args.condition in ("g", "both"):
            g_summary = await run_condition("g", tasks)

        if args.condition in ("g-smart", "both"):
            gs_summary = await run_condition("g-smart", tasks)
    else:
        # Load existing results
        for cond, key in [("g", "g_summary"), ("g-smart", "gs_summary")]:
            params = CONDITION_PARAMS[cond]
            path = os.path.join(RESULTS_BASE, params["run_id"], "summary.json")
            if os.path.isfile(path):
                with open(path) as f:
                    data = json.load(f)
                if cond == "g":
                    g_summary = data
                else:
                    gs_summary = data
                print(f"Loaded {params['label']}: {data['passed']}/{data['total_trials']} "
                      f"({data['pass_rate']:.0%})")

    # Write comparison
    report = write_comparison(a_summary, g_summary, gs_summary, tasks)
    print(f"\n{'='*80}")
    print(report)
    print(f"{'='*80}")


if __name__ == "__main__":
    asyncio.run(main())
