#!/usr/bin/env python3
"""
Pilot F (89 tasks): wg-native with surveillance loops — scaled from pilot-f-5x1.

For each of 18 unique benchmark tasks × 5 replicas = 90 trials:
  1. Create an isolated per-trial wg graph in a temp dir
  2. Configure native executor with M2.7 model + graph context + WG Quick Guide
  3. Create an INIT task (completed immediately) → external predecessor for cycle header fix
  4. Create a WORK task (depends on init → becomes cycle header, not surv)
  5. Create a SURVEILLANCE task (depends on work)
  6. Close the cycle: work → surv → work (max 3 iterations, 1m delay)
  7. Start wg service, poll until terminal state, stop
  8. Collect results (pass/fail, timing, surveillance loop stats, token metrics)

Key fixes from pilot-f-5x1 learnings:
  - Init task as external predecessor ensures work (not surv) becomes cycle header
  - Surveillance checks: output exists, is valid, verify command passes
  - If valid → wg done --converged; if invalid → wg done (not converged) → cycle iterates

Task set: 8 calibration tasks + 10 hard benchmark tasks = 18 unique tasks
Replicas: 5 per task → 90 total trials (>= 89 required)

Produces:
  terminal-bench/results/pilot-f-89/summary.json
  terminal-bench/results/pilot-f-89/<trial-id>/  (per-trial graph state)
"""

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

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
RESULTS_DIR = os.path.join(SCRIPT_DIR, "results", "pilot-f-89")
WG_BIN = shutil.which("wg") or os.path.expanduser("~/.cargo/bin/wg")

MODEL = "openrouter:minimax/minimax-m2.7"
REPLICAS = 5
MAX_ITERATIONS = 3
CYCLE_DELAY = "1m"
TRIAL_TIMEOUT = 1800  # 30 min per trial
POLL_INTERVAL = 5.0

# WG Quick Guide for condition F distilled context injection
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
# Task definitions — ALL 18 terminal-bench tasks
# ---------------------------------------------------------------------------

# 8 calibration tasks (easy, medium, hard)
CALIBRATION_TASKS = [
    {
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
    {
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
    {
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
    {
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
    {
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
    {
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
    {
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
    {
        "id": "sysadmin",
        "title": "Sysadmin: rate-limited HTTP server",
        "instruction_file": "tasks/condition-a-calibration/08-sysadmin-hard.txt",
        "verify_cmd": (
            "test -f /tmp/ratelimit_server.py && "
            "python3 -c \"import ast; ast.parse(open('/tmp/ratelimit_server.py').read())\" && "
            "grep -q '8765' /tmp/ratelimit_server.py && "
            "grep -q '429' /tmp/ratelimit_server.py && "
            "grep -qi 'rate' /tmp/ratelimit_server.py"
        ),
        "difficulty": "hard",
        "tmp_paths": ["/tmp/ratelimit_server.py"],
    },
]

# 10 hard benchmark tasks
HARD_BENCHMARK_TASKS = [
    {
        "id": "configure-git-webserver",
        "title": "Configure Git Webserver: bare repo + post-receive hook + HTTP server",
        "instruction_file": "tasks/hard-benchmarks/01-configure-git-webserver.txt",
        "verify_cmd": (
            "test -d /tmp/git-server/repo.git && "
            "test -x /tmp/git-server/repo.git/hooks/post-receive && "
            "test -f /tmp/web/html/index.html && "
            "grep -q 'Version 2' /tmp/web/html/index.html && "
            "test -f /tmp/web/deploy.log && "
            "test $(wc -l < /tmp/web/deploy.log) -ge 2"
        ),
        "difficulty": "hard",
        "tmp_paths": ["/tmp/git-server", "/tmp/web", "/tmp/git-client"],
    },
    {
        "id": "mailman",
        "title": "Mailman: local mail system with mailing list manager",
        "instruction_file": "tasks/hard-benchmarks/02-mailman.txt",
        "verify_cmd": (
            "test -f /tmp/mailman/list_manager.py && "
            "test -f /tmp/mailman/cli.py && "
            "python3 -c \""
            "import json; "
            "members = json.load(open('/tmp/mailman/lists/test-list/members.json')); "
            "assert len(members) == 2, f'Expected 2 members, got {len(members)}'"
            "\" && "
            "python3 -c \""
            "import os; "
            "archive = '/tmp/mailman/lists/test-list/archive'; "
            "count = len([f for f in os.listdir(archive) if os.path.isfile(os.path.join(archive, f))]); "
            "assert count == 3, f'Expected 3 archive messages, got {count}'"
            "\""
        ),
        "difficulty": "hard",
        "tmp_paths": ["/tmp/mailman"],
    },
    {
        "id": "multi-source-data-merger",
        "title": "Multi-Source Data Merger: 3 formats -> merge -> conflict report",
        "instruction_file": "tasks/hard-benchmarks/03-multi-source-data-merger.txt",
        "verify_cmd": (
            "test -f /tmp/merger/merge.py && "
            "python3 /tmp/merger/merge.py && "
            "python3 -c \""
            "import csv; "
            "rows = list(csv.DictReader(open('/tmp/merger/output/merged.csv'))); "
            "assert len(rows) == 7, f'Expected 7 rows, got {len(rows)}'"
            "\" && "
            "python3 -c \""
            "import json; "
            "conflicts = json.load(open('/tmp/merger/output/conflicts.json')); "
            "assert len(conflicts) >= 4, f'Expected >= 4 conflicts, got {len(conflicts)}'"
            "\""
        ),
        "difficulty": "hard",
        "tmp_paths": ["/tmp/merger"],
    },
    {
        "id": "financial-document-processor",
        "title": "Financial Document Processor: classify -> extract -> summarize",
        "instruction_file": "tasks/hard-benchmarks/04-financial-document-processor.txt",
        "verify_cmd": (
            "test -f /tmp/finproc/processor.py && "
            "test -f /tmp/finproc/summarizer.py && "
            "python3 /tmp/finproc/processor.py && "
            "python3 /tmp/finproc/summarizer.py && "
            "python3 -c \""
            "import os; "
            "extracted = [f for f in os.listdir('/tmp/finproc/extracted') if f.endswith('.json')]; "
            "assert len(extracted) == 5, f'Expected 5 extracted, got {len(extracted)}'"
            "\" && "
            "python3 -c '"
            "import json; "
            "d = json.load(open(\"/tmp/finproc/output/totals.json\")); "
            "assert abs(d[\"grand_total\"] - 6089.25) < 0.01"
            "'"
        ),
        "difficulty": "hard",
        "tmp_paths": ["/tmp/finproc"],
    },
    {
        "id": "cobol-modernization",
        "title": "COBOL Modernization: payroll COBOL -> Python with identical output",
        "instruction_file": "tasks/hard-benchmarks/05-cobol-modernization.txt",
        "verify_cmd": (
            "test -f /tmp/cobol-modern/python/payroll.py && "
            "test -f /tmp/cobol-modern/python/test_payroll.py && "
            "cd /tmp/cobol-modern && python3 python/payroll.py && "
            "cd /tmp/cobol-modern && python3 -m pytest python/test_payroll.py -v"
        ),
        "difficulty": "hard",
        "tmp_paths": ["/tmp/cobol-modern"],
    },
    {
        "id": "build-cython-ext",
        "title": "Build Cython Extension: numpy integration, build, test",
        "instruction_file": "tasks/hard-benchmarks/06-build-cython-ext.txt",
        "verify_cmd": (
            "cd /tmp/cython-ext && "
            "python3 -c 'from fastmath import dot_product, matrix_multiply, moving_average, euclidean_distance; print(\"OK\")' && "
            "python3 -m pytest tests/ -v"
        ),
        "difficulty": "hard",
        "tmp_paths": ["/tmp/cython-ext"],
    },
    {
        "id": "fix-code-vulnerability",
        "title": "Fix Code Vulnerabilities: analyze -> report -> fix -> test",
        "instruction_file": "tasks/hard-benchmarks/07-fix-code-vulnerability.txt",
        "verify_cmd": (
            "test -f /tmp/vuln-app/vulnerability_report.json && "
            "test -f /tmp/vuln-app/app_fixed.py && "
            "python3 -c \""
            "import json; "
            "r = json.load(open('/tmp/vuln-app/vulnerability_report.json')); "
            "assert len(r) >= 6, f'Only {len(r)} findings'"
            "\""
        ),
        "difficulty": "hard",
        "tmp_paths": ["/tmp/vuln-app"],
    },
    {
        "id": "constraints-scheduling",
        "title": "Constraints Scheduling: ICS parsing + slot finding + meeting generation",
        "instruction_file": "tasks/hard-benchmarks/08-constraints-scheduling.txt",
        "verify_cmd": (
            "test -f /tmp/scheduler/find_slots.py && "
            "test -f /tmp/scheduler/schedule_meeting.py && "
            "python3 /tmp/scheduler/find_slots.py --date 2024-01-22 --duration 60 --participants alice,bob,carol && "
            "test -f /tmp/scheduler/output/meeting.ics && "
            "cd /tmp/scheduler && python3 -m pytest test_scheduler.py -v"
        ),
        "difficulty": "hard",
        "tmp_paths": ["/tmp/scheduler"],
    },
    {
        "id": "multi-module-type-migration",
        "title": "Multi-Module Type Migration: UserId str -> dataclass across 6 modules",
        "instruction_file": "tasks/hard-benchmarks/09-multi-module-type-migration.txt",
        "verify_cmd": (
            "cd /tmp/type_migration && "
            "python3 -c 'from core.types import UserId; assert not isinstance(UserId, type(str))' && "
            "python3 -m pytest tests/ -v && "
            "python3 main.py"
        ),
        "difficulty": "hard",
        "tmp_paths": ["/tmp/type_migration"],
    },
    {
        "id": "iterative-test-fix",
        "title": "Iterative Test Fix: 6 interrelated bugs, 15 tests, fix all",
        "instruction_file": "tasks/hard-benchmarks/10-iterative-test-fix.txt",
        "verify_cmd": (
            "cd /tmp/iterative_fix && "
            "python3 -m pytest tests/ -v 2>&1 | "
            "grep -c 'PASSED' | "
            "python3 -c 'import sys; n=int(sys.stdin.read().strip()); "
            "sys.exit(0 if n >= 15 else 1)'"
        ),
        "difficulty": "hard",
        "tmp_paths": ["/tmp/iterative_fix"],
    },
]

# All tasks combined
ALL_TASKS = CALIBRATION_TASKS + HARD_BENCHMARK_TASKS


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

async def exec_wg(wg_dir: str, subcmd: list[str], timeout: float = 120) -> str:
    """Execute a wg command against a specific graph directory."""
    cmd = [WG_BIN, "--dir", wg_dir] + subcmd
    # Strip WG_* and CLAUDECODE env vars to prevent parent agent leakage
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
            try:
                os.remove(p)
            except OSError:
                pass


async def poll_completion(
    wg_dir: str,
    task_id: str,
    timeout_secs: float,
    poll_interval: float = POLL_INTERVAL,
) -> tuple[str, float, dict]:
    """Poll until task reaches terminal state. Returns (status, elapsed, loop_info)."""
    start = time.monotonic()
    terminal = {"done", "failed", "abandoned"}
    last_status = "unknown"
    loop_info = {"iterations_observed": 0, "last_loop_iteration": 0}

    while True:
        elapsed = time.monotonic() - start
        if elapsed > timeout_secs:
            return "timeout", elapsed, loop_info

        result = await exec_wg(wg_dir, ["show", task_id])
        for line in result.splitlines():
            s = line.strip()
            if s.startswith("Status:"):
                status = s.split(":", 1)[1].strip().lower()
                if status != last_status:
                    last_status = status
                if status in terminal:
                    return status, elapsed, loop_info
                break
            if "loop_iteration" in s.lower() or "Loop iteration" in s:
                try:
                    iter_val = int(s.split(":")[-1].strip())
                    if iter_val > loop_info["last_loop_iteration"]:
                        loop_info["last_loop_iteration"] = iter_val
                        loop_info["iterations_observed"] = iter_val
                except (ValueError, IndexError):
                    pass

        await asyncio.sleep(poll_interval)


async def poll_all_done(
    wg_dir: str,
    timeout_secs: float,
    poll_interval: float = POLL_INTERVAL,
) -> tuple[str, float]:
    """Poll until all tasks in the graph are in terminal state."""
    start = time.monotonic()
    terminal = {"done", "failed", "abandoned"}

    while True:
        elapsed = time.monotonic() - start
        if elapsed > timeout_secs:
            return "timeout", elapsed

        result = await exec_wg(wg_dir, ["list", "--json"])
        try:
            tasks = json.loads(result.split("\n")[0]) if result.strip() else []
            if isinstance(tasks, list) and len(tasks) > 0:
                all_terminal = all(
                    t.get("status", "").lower() in terminal
                    for t in tasks
                )
                if all_terminal:
                    return "done", elapsed
        except (json.JSONDecodeError, IndexError):
            pass

        await asyncio.sleep(poll_interval)


async def collect_metrics(wg_dir: str) -> dict:
    """Read agent logs to extract token counts and cost."""
    agents_dir = os.path.join(wg_dir, "agents")
    metrics = {
        "total_input_tokens": 0,
        "total_output_tokens": 0,
        "total_cost_usd": 0.0,
        "total_turns": 0,
        "model_used": None,
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
                    if event.get("type") == "init":
                        model = event.get("model")
                        if model:
                            metrics["model_used"] = model
                    elif event.get("type") == "turn":
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


def extract_surveillance_stats(wg_dir: str, surveillance_id: str) -> dict:
    """Extract surveillance loop stats from the graph."""
    stats = {
        "iterations_completed": 0,
        "issues_caught": [],
        "converged": False,
    }
    graph_path = os.path.join(wg_dir, "graph.jsonl")
    if not os.path.isfile(graph_path):
        return stats

    try:
        with open(graph_path) as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    entry = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if entry.get("id") == surveillance_id:
                    cycle_cfg = entry.get("cycle_config", {})
                    if cycle_cfg:
                        stats["iterations_completed"] = cycle_cfg.get("current_iteration", 0)
                    status = entry.get("status", "")
                    if status == "done":
                        stats["converged"] = True
                if entry.get("type") == "log" and entry.get("task_id") == surveillance_id:
                    msg = entry.get("message", "")
                    if "invalid" in msg.lower() or "failed" in msg.lower() or "issue" in msg.lower():
                        stats["issues_caught"].append(msg)
    except Exception:
        pass
    return stats


# ---------------------------------------------------------------------------
# Single trial runner
# ---------------------------------------------------------------------------

async def run_trial(task_def: dict, replica: int) -> dict:
    """Run a single trial with work task + surveillance loop."""
    task_id = task_def["id"]
    trial_id = f"f-{task_id}-r{replica}"
    init_task_id = f"init-{task_id}"
    work_task_id = f"work-{task_id}"
    surv_task_id = f"surv-{task_id}"

    result = {
        "trial_id": trial_id,
        "condition": "F",
        "task": task_id,
        "difficulty": task_def["difficulty"],
        "replica": replica,
        "model": MODEL,
        "status": "not_started",
        "elapsed_s": 0.0,
        "model_verified": False,
        "wg_context_available": True,
        "surveillance": {
            "created": False,
            "cycle_edge": False,
            "iterations": 0,
            "issues_caught": [],
            "converged": False,
        },
        "metrics": None,
        "verify_output": None,
        "error": None,
    }

    # Clean up tmp paths from previous runs
    cleanup_tmp_paths(task_def.get("tmp_paths", []))

    tmpdir = tempfile.mkdtemp(prefix=f"tb-pilot-f89-{trial_id}-")
    wg_dir = os.path.join(tmpdir, ".workgraph")
    start = time.monotonic()

    trial_num = getattr(run_trial, '_counter', 0) + 1
    run_trial._counter = trial_num
    total = len(ALL_TASKS) * REPLICAS

    print(f"\n{'='*60}", flush=True)
    print(f"  Trial {trial_num}/{total}: {trial_id} ({task_def['difficulty']})", flush=True)
    print(f"  Task: {task_def['title']}", flush=True)
    print(f"{'='*60}", flush=True)

    try:
        # 1. Init graph
        init_out = await exec_wg(wg_dir, ["init"])
        if "error" in init_out.lower() and "already" not in init_out.lower():
            raise RuntimeError(f"Init failed: {init_out}")
        print(f"  [1/8] Graph initialized", flush=True)

        # 2. Write config (condition F: graph context, native executor, full wg tools)
        config_lines = [
            "[coordinator]",
            "max_agents = 2",  # Need 2: one for work, one for surveillance
            'executor = "native"',
            f'model = "{MODEL}"',
            "worktree_isolation = false",
            "",
            "[agent]",
            f'model = "{MODEL}"',
            'context_scope = "graph"',
            'exec_mode = "full"',
            "",
            "[agency]",
            "auto_assign = false",
            "auto_evaluate = false",
        ]
        with open(os.path.join(wg_dir, "config.toml"), "w") as f:
            f.write("\n".join(config_lines) + "\n")
        print(f"  [2/8] Config written (model={MODEL}, context=graph, exec=full)", flush=True)

        # 3a. Create INIT task (one-shot, completes immediately)
        # CRITICAL FIX from pilot-f-5x1: This gives the work task an external predecessor
        # from outside the cycle, which ensures work (not surv) becomes the cycle header.
        add_init = await exec_wg(wg_dir, [
            "add", f"Init: {task_def['title']}",
            "--id", init_task_id,
            "-d", "One-shot trigger task. Provides external predecessor for cycle header.",
            "--no-place",
        ])
        await exec_wg(wg_dir, ["done", init_task_id])
        print(f"  [3a/8] Init task created and completed: {init_task_id}", flush=True)

        # 3b. Create WORK task (depends on init -> gets external predecessor -> becomes cycle header)
        instruction = load_instruction(task_def)
        work_description = (
            f"## Terminal Bench Trial (Condition F — wg-native)\n\n"
            f"**Task:** {task_id} ({task_def['difficulty']})\n\n"
            f"{WG_QUICK_GUIDE}\n\n"
            f"## Instructions\n\n{instruction}\n"
        )
        add_work = await exec_wg(wg_dir, [
            "add", f"Work: {task_def['title']}",
            "--id", work_task_id,
            "--after", init_task_id,
            "-d", work_description,
            "--verify", task_def["verify_cmd"],
            "--exec-mode", "full",
            "--context-scope", "graph",
            "--model", MODEL,
            "--no-place",
        ])
        if "[exit code:" in add_work and work_task_id not in add_work:
            raise RuntimeError(f"Work task creation failed: {add_work}")
        print(f"  [3b/8] Work task created: {work_task_id} (after {init_task_id})", flush=True)

        # 4. Create SURVEILLANCE task (depends on work task)
        surv_description = (
            f"## Surveillance Task for: {task_id}\n\n"
            f"You are a surveillance agent. Your job is to verify that the work task "
            f"completed correctly.\n\n"
            f"### What to check\n"
            f"Run the following verification command:\n"
            f"```bash\n{task_def['verify_cmd']}\n```\n\n"
            f"### Decision logic\n"
            f"1. Run the verify command above\n"
            f"2. If it passes (exit code 0): the work is valid. Run:\n"
            f"   ```bash\n"
            f"   wg log {surv_task_id} 'Verification passed — output is valid'\n"
            f"   wg done {surv_task_id} --converged\n"
            f"   ```\n"
            f"3. If it fails: log what went wrong, then signal for retry:\n"
            f"   ```bash\n"
            f"   wg log {surv_task_id} 'Verification FAILED: <describe issue>'\n"
            f"   wg done {surv_task_id}\n"
            f"   ```\n"
            f"   (Using plain `wg done` without `--converged` causes the cycle to iterate,\n"
            f"    which resets the work task so it can be retried.)\n\n"
            f"### Important\n"
            f"- You are in a cycle with max {MAX_ITERATIONS} iterations\n"
            f"- Check `wg show {surv_task_id}` for your current loop_iteration\n"
            f"- If this is iteration {MAX_ITERATIONS} and it still fails, use `wg done --converged` anyway\n"
            f"  and log the failure details\n"
        )
        add_surv = await exec_wg(wg_dir, [
            "add", f"Surveil: {task_def['title']}",
            "--id", surv_task_id,
            "--after", work_task_id,
            "-d", surv_description,
            "--exec-mode", "light",
            "--context-scope", "graph",
            "--model", MODEL,
            "--no-place",
        ])
        if "[exit code:" in add_surv and surv_task_id not in add_surv:
            raise RuntimeError(f"Surveillance task creation failed: {add_surv}")
        result["surveillance"]["created"] = True
        print(f"  [4/8] Surveillance task created: {surv_task_id}", flush=True)

        # 5. Close the cycle: work -> surv -> work (back-edge)
        edit_out = await exec_wg(wg_dir, [
            "edit", work_task_id,
            "--add-after", surv_task_id,
            "--max-iterations", str(MAX_ITERATIONS),
            "--cycle-delay", CYCLE_DELAY,
        ])
        if "[exit code:" in edit_out:
            print(f"  WARNING: Cycle edge creation returned: {edit_out}", flush=True)
        else:
            result["surveillance"]["cycle_edge"] = True
        print(f"  [5/8] Cycle edge created: {work_task_id} -> {surv_task_id} -> {work_task_id} "
              f"(max {MAX_ITERATIONS} iters, {CYCLE_DELAY} delay)", flush=True)

        # Verify the cycle was detected and work is the header
        cycles_out = await exec_wg(wg_dir, ["cycles"])
        if "work-" in cycles_out.lower():
            print(f"  Cycles: work task is header (correct)", flush=True)
        else:
            print(f"  Cycles: {cycles_out.strip()[:200]}", flush=True)

        # 6. Start wg service
        service_out = await exec_wg(wg_dir, [
            "service", "start",
            "--max-agents", "2",
            "--executor", "native",
            "--model", MODEL,
            "--no-coordinator-agent",
            "--force",
        ])
        daemon_registry.register(wg_dir, WG_BIN)
        print(f"  [6/8] Service started, polling for completion...", flush=True)

        # 7. Poll for completion
        status, elapsed, loop_info = await poll_completion(
            wg_dir, surv_task_id, TRIAL_TIMEOUT
        )

        # If surveillance never reached terminal, check overall graph
        if status not in ("done", "failed", "abandoned"):
            overall_status, overall_elapsed = await poll_all_done(
                wg_dir, max(0, TRIAL_TIMEOUT - elapsed)
            )
            status = overall_status
            elapsed = time.monotonic() - start

        result["status"] = status
        result["elapsed_s"] = round(elapsed, 2)
        print(f"  [7/8] Trial completed: {status.upper()} in {elapsed:.1f}s", flush=True)

        # Stop service (daemon_registry.stop_one in finally is the safety net)
        daemon_registry.stop_one(wg_dir)

        # Collect surveillance stats
        surv_show = await exec_wg(wg_dir, ["show", surv_task_id])
        for line in surv_show.splitlines():
            s = line.strip().lower()
            if "loop_iteration" in s or "iteration" in s:
                try:
                    val = int(s.split(":")[-1].strip())
                    result["surveillance"]["iterations"] = val
                except (ValueError, IndexError):
                    pass

        graph_stats = extract_surveillance_stats(wg_dir, surv_task_id)
        if graph_stats["iterations_completed"] > 0:
            result["surveillance"]["iterations"] = graph_stats["iterations_completed"]
        result["surveillance"]["converged"] = graph_stats["converged"]
        result["surveillance"]["issues_caught"] = graph_stats["issues_caught"]

        # Collect agent metrics (token counts, model verification)
        result["metrics"] = await collect_metrics(wg_dir)
        if result["metrics"].get("model_used"):
            model_used = result["metrics"]["model_used"]
            result["model_verified"] = "minimax" in model_used.lower() or "m2.7" in model_used.lower()
            print(f"  Model used: {model_used} (verified: {result['model_verified']})", flush=True)

        # Run verify ourselves to check final state
        try:
            verify_proc = await asyncio.create_subprocess_shell(
                task_def["verify_cmd"],
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            v_stdout, v_stderr = await asyncio.wait_for(
                verify_proc.communicate(), timeout=60
            )
            verify_passed = verify_proc.returncode == 0
            result["verify_output"] = (
                (v_stdout.decode(errors="replace") + v_stderr.decode(errors="replace"))[:500]
            )
            if verify_passed and result["status"] != "done":
                result["status"] = "done"
                print(f"  Verify passed (external check confirms work is correct)", flush=True)
            elif not verify_passed:
                print(f"  Verify failed (external check)", flush=True)
        except Exception as e:
            result["verify_output"] = f"Error running verify: {e}"

        # Show brief final status
        list_out = await exec_wg(wg_dir, ["list"])
        print(f"\n  Final graph state:\n{list_out}", flush=True)

    except Exception as e:
        result["status"] = "error"
        result["error"] = str(e)
        print(f"  ERROR: {e}", flush=True)
    finally:
        # Always stop the daemon before cleanup (handles both normal and error paths)
        daemon_registry.stop_one(wg_dir)
        result["elapsed_s"] = round(time.monotonic() - start, 2)
        # Save WG state for analysis
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
# Summary generation
# ---------------------------------------------------------------------------

def write_summary(results: list[dict]) -> dict:
    """Write summary.json with all required fields."""
    passed = [r for r in results if r["status"] == "done"]
    failed = [r for r in results if r["status"] in ("failed", "error", "timeout")]

    # Surveillance loop stats
    total_surv_iterations = sum(r["surveillance"]["iterations"] for r in results)
    surv_created = sum(1 for r in results if r["surveillance"]["created"])
    surv_cycles = sum(1 for r in results if r["surveillance"]["cycle_edge"])
    surv_converged = sum(1 for r in results if r["surveillance"]["converged"])
    all_issues = []
    for r in results:
        for issue in r["surveillance"]["issues_caught"]:
            all_issues.append({"trial": r["trial_id"], "issue": issue})

    # Model verification
    model_verified = sum(1 for r in results if r["model_verified"])

    times = [r["elapsed_s"] for r in results if r["elapsed_s"] > 0]

    # Token aggregates
    total_input_tokens = sum(
        (r.get("metrics") or {}).get("total_input_tokens", 0) for r in results
    )
    total_output_tokens = sum(
        (r.get("metrics") or {}).get("total_output_tokens", 0) for r in results
    )
    total_cost = sum(
        (r.get("metrics") or {}).get("total_cost_usd", 0.0) for r in results
    )
    total_turns = sum(
        (r.get("metrics") or {}).get("total_turns", 0) for r in results
    )
    total_agents = sum(
        (r.get("metrics") or {}).get("num_agents_spawned", 0) for r in results
    )

    # Per-difficulty stats
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
                "mean_time_s": round(sum(d_times) / len(d_times), 2) if d_times else 0,
            }

    # Per-task stats
    task_stats = {}
    for task_def in ALL_TASKS:
        task_results = [r for r in results if r.get("task") == task_def["id"]]
        if task_results:
            t_passed = sum(1 for r in task_results if r["status"] == "done")
            t_times = [r["elapsed_s"] for r in task_results if r["elapsed_s"] > 0]
            t_surv_iters = sum(r["surveillance"]["iterations"] for r in task_results)
            task_stats[task_def["id"]] = {
                "total": len(task_results),
                "passed": t_passed,
                "pass_rate": t_passed / len(task_results),
                "mean_time_s": round(sum(t_times) / len(t_times), 2) if t_times else 0,
                "total_surveillance_iterations": t_surv_iters,
            }

    summary = {
        "run_id": "pilot-f-89",
        "condition": "F",
        "description": "Condition F at scale: wg-native with surveillance loops, 18 tasks x 5 replicas",
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "model": MODEL,
        "replicas": REPLICAS,
        "unique_tasks": len(ALL_TASKS),
        "max_iterations": MAX_ITERATIONS,
        "cycle_delay": CYCLE_DELAY,
        "total_trials": len(results),
        "passed": len(passed),
        "failed": len(failed),
        "pass_rate": len(passed) / len(results) if results else 0,
        "mean_time_s": round(sum(times) / len(times), 2) if times else 0,
        "total_wall_clock_s": round(sum(times), 2) if times else 0,
        "model_verified_count": model_verified,
        "claude_fallback_detected": model_verified < len(results),
        "wg_context_available": True,
        "token_stats": {
            "total_input_tokens": total_input_tokens,
            "total_output_tokens": total_output_tokens,
            "total_tokens": total_input_tokens + total_output_tokens,
            "total_cost_usd": round(total_cost, 4),
            "total_turns": total_turns,
            "total_agents_spawned": total_agents,
            "mean_tokens_per_trial": round(
                (total_input_tokens + total_output_tokens) / len(results), 0
            ) if results else 0,
        },
        "surveillance_loop_stats": {
            "loops_created": surv_created,
            "cycle_edges_created": surv_cycles,
            "total_iterations_across_trials": total_surv_iterations,
            "trials_converged_first_try": sum(
                1 for r in results
                if r["surveillance"]["converged"] and r["surveillance"]["iterations"] <= 1
            ),
            "trials_needed_retry": sum(
                1 for r in results
                if r["surveillance"]["iterations"] > 1
            ),
            "issues_detected": all_issues,
            "issues_detected_count": len(all_issues),
        },
        "difficulty_stats": difficulty_stats,
        "task_stats": task_stats,
        "trials": [
            {
                "trial_id": r["trial_id"],
                "task": r["task"],
                "difficulty": r["difficulty"],
                "replica": r["replica"],
                "status": r["status"],
                "elapsed_s": r["elapsed_s"],
                "model_verified": r["model_verified"],
                "surveillance_iterations": r["surveillance"]["iterations"],
                "surveillance_converged": r["surveillance"]["converged"],
                "surveillance_issues": r["surveillance"]["issues_caught"],
                "metrics": r["metrics"],
                "verify_output": r["verify_output"],
                "error": r["error"],
            }
            for r in results
        ],
    }

    os.makedirs(RESULTS_DIR, exist_ok=True)
    summary_path = os.path.join(RESULTS_DIR, "summary.json")
    with open(summary_path, "w") as f:
        json.dump(summary, f, indent=2)

    return summary


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

async def main():
    total_trials = len(ALL_TASKS) * REPLICAS

    print(f"Pilot F (89-task scale): wg-native with surveillance loops")
    print(f"  Model: {MODEL}")
    print(f"  Unique tasks: {len(ALL_TASKS)} (8 calibration + 10 hard benchmarks)")
    print(f"  Replicas per task: {REPLICAS}")
    print(f"  Total trials: {total_trials}")
    print(f"  Max surveillance iterations per loop: {MAX_ITERATIONS}")
    print(f"  Cycle delay: {CYCLE_DELAY}")
    print(f"  wg binary: {WG_BIN}")
    print(f"  Results: {RESULTS_DIR}")
    print()

    # Clean old results
    if os.path.isdir(RESULTS_DIR):
        shutil.rmtree(RESULTS_DIR)
    os.makedirs(RESULTS_DIR, exist_ok=True)

    # Reset trial counter
    run_trial._counter = 0

    results = []
    overall_start = time.monotonic()

    # Run sequentially: each task's replicas share /tmp paths, so no concurrency
    for task_def in ALL_TASKS:
        for replica in range(REPLICAS):
            r = await run_trial(task_def, replica)
            results.append(r)

            # Write intermediate summary after each trial (crash recovery)
            intermediate = write_summary(results)

            # Progress report every 10 trials
            if len(results) % 10 == 0:
                elapsed_total = time.monotonic() - overall_start
                rate = elapsed_total / len(results) if results else 0
                remaining = (total_trials - len(results)) * rate
                passed_so_far = sum(1 for r in results if r["status"] == "done")
                print(f"\n  === PROGRESS: {len(results)}/{total_trials} complete, "
                      f"{passed_so_far} passed, "
                      f"~{remaining/60:.0f}min remaining ===\n", flush=True)

    # Write final summary
    summary = write_summary(results)

    total_wall = time.monotonic() - overall_start

    # Print final report
    print(f"\n{'='*60}")
    print(f"  PILOT F (89-TASK SCALE) RESULTS")
    print(f"{'='*60}")
    print(f"  Wall clock: {total_wall:.1f}s ({total_wall/60:.1f}min)")
    print(f"  Passed: {summary['passed']}/{summary['total_trials']} "
          f"({summary['pass_rate']:.1%})")
    print(f"  Mean time: {summary['mean_time_s']:.1f}s per trial")
    print(f"  Model verified: {summary['model_verified_count']}/{summary['total_trials']}")
    print(f"  Claude fallback: {'YES' if summary['claude_fallback_detected'] else 'NO'}")
    print(f"  Total tokens: {summary['token_stats']['total_tokens']:,}")
    print(f"  Total cost: ${summary['token_stats']['total_cost_usd']:.4f}")
    surv = summary['surveillance_loop_stats']
    print(f"  Surveillance loops created: {surv['loops_created']}/{summary['total_trials']}")
    print(f"  Cycle edges: {surv['cycle_edges_created']}/{summary['total_trials']}")
    print(f"  Total surveillance iterations: {surv['total_iterations_across_trials']}")
    print(f"  Issues caught by surveillance: {surv['issues_detected_count']}")
    print(f"\n  Results: {os.path.join(RESULTS_DIR, 'summary.json')}")

    # Per-difficulty breakdown
    print(f"\n  Per-difficulty:")
    for diff in ("easy", "medium", "hard"):
        d = summary.get("difficulty_stats", {}).get(diff, {})
        if d:
            print(f"    {diff}: {d['passed']}/{d['total']} ({d['pass_rate']:.0%}), "
                  f"mean {d['mean_time_s']:.1f}s")

    print(f"{'='*60}")

    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
