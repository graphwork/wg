#!/usr/bin/env python3
"""
Pilot F (5×1): wg-native with surveillance loops.

For each of 5 benchmark tasks:
  1. Create an isolated per-trial wg graph in a temp dir
  2. Configure native executor with M2.7 model + graph context + WG Quick Guide
  3. Create a WORK task (the actual benchmark problem)
  4. Create a SURVEILLANCE task that depends on the work task
  5. Close the cycle: work → surveillance → work (max 3 iterations, 1m delay)
  6. Start wg service, poll until terminal state, stop
  7. Collect results (pass/fail, timing, surveillance loop stats)

Surveillance loop semantics:
  - After the work agent marks its task done, the surveillance task becomes ready
  - The surveillance agent checks if the output actually exists + passes verify
  - If valid: `wg done --converged` → stops the cycle
  - If invalid: `wg done` (not converged) → cycle iterates, work task resets to open
  - Max 3 iterations prevents infinite loops

Produces:
  terminal-bench/results/pilot-f-5x1/summary.json
  terminal-bench/results/pilot-f-5x1/<trial-id>/  (per-trial graph state)
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
RESULTS_DIR = os.path.join(SCRIPT_DIR, "results", "pilot-f-5x1")
WG_BIN = shutil.which("wg") or os.path.expanduser("~/.cargo/bin/wg")

MODEL = "openrouter:minimax/minimax-m2.7"
MAX_ITERATIONS = 3
CYCLE_DELAY = "1m"
TRIAL_TIMEOUT = 1800  # 30 min per trial
POLL_INTERVAL = 5.0

# WG Quick Guide for condition F distilled context injection
WG_QUICK_GUIDE = """## WG Quick Reference (Distilled)

You are working inside a workgraph-managed task. Use these commands:

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
# Task definitions (same 5 as pilot-a-5x1)
# ---------------------------------------------------------------------------

TB_TASKS = [
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
]


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
            os.remove(p)


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

        # Check the surveillance task status (it's the terminal signal)
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
            # Track cycle iteration
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
                # Look for the surveillance task's cycle info
                if entry.get("id") == surveillance_id:
                    cycle_cfg = entry.get("cycle_config", {})
                    if cycle_cfg:
                        stats["iterations_completed"] = cycle_cfg.get("current_iteration", 0)
                    status = entry.get("status", "")
                    if status == "done":
                        stats["converged"] = True
                # Look for log entries that mention issues
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

async def run_trial(task_def: dict) -> dict:
    """Run a single trial with work task + surveillance loop."""
    task_id = task_def["id"]
    trial_id = f"f-{task_id}-r0"
    init_task_id = f"init-{task_id}"
    work_task_id = f"work-{task_id}"
    surv_task_id = f"surv-{task_id}"

    result = {
        "trial_id": trial_id,
        "condition": "F",
        "task": task_id,
        "difficulty": task_def["difficulty"],
        "replica": 0,
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

    tmpdir = tempfile.mkdtemp(prefix=f"tb-pilot-f5x1-{trial_id}-")
    wg_dir = os.path.join(tmpdir, ".workgraph")
    start = time.monotonic()

    print(f"\n{'='*60}", flush=True)
    print(f"  Trial: {trial_id} ({task_def['difficulty']})", flush=True)
    print(f"  Task: {task_def['title']}", flush=True)
    print(f"{'='*60}", flush=True)

    try:
        # 1. Init graph
        init_out = await exec_wg(wg_dir, ["init"])
        if "error" in init_out.lower() and "already" not in init_out.lower():
            raise RuntimeError(f"Init failed: {init_out}")
        print(f"  [1/7] Graph initialized", flush=True)

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
        print(f"  [2/7] Config written (model={MODEL}, context=graph, exec=full)", flush=True)

        # 3a. Create INIT task (one-shot, completes immediately)
        # This gives the work task an external predecessor from outside the cycle,
        # which ensures work (not surv) becomes the cycle header.
        # Without this, string-ID ordering makes surv the header (s < w),
        # causing the surveillance task to run before the work task.
        add_init = await exec_wg(wg_dir, [
            "add", f"Init: {task_def['title']}",
            "--id", init_task_id,
            "-d", "One-shot trigger task. Provides external predecessor for cycle header.",
            "--no-place",
        ])
        # Mark init done immediately
        await exec_wg(wg_dir, ["done", init_task_id])
        print(f"  [3a/8] Init task created and completed: {init_task_id}", flush=True)

        # 3b. Create WORK task (depends on init → gets external predecessor → becomes cycle header)
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

        # 5. Close the cycle: work → surv → work (back-edge)
        #    Edit the work task to add a back-edge from surveillance + cycle config
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
        print(f"  [5/8] Cycle edge created: {work_task_id} → {surv_task_id} → {work_task_id} "
              f"(max {MAX_ITERATIONS} iters, {CYCLE_DELAY} delay)", flush=True)

        # Verify the cycle was detected and work is the header
        cycles_out = await exec_wg(wg_dir, ["cycles"])
        print(f"  Cycles: {cycles_out.strip()[:300]}", flush=True)

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
        # We poll the surveillance task — when it's done (converged), the trial is complete.
        # But we also need to handle the case where all tasks finish.
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

        # Parse graph for surveillance details
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
                verify_proc.communicate(), timeout=30
            )
            verify_passed = verify_proc.returncode == 0
            result["verify_output"] = (
                (v_stdout.decode(errors="replace") + v_stderr.decode(errors="replace"))[:500]
            )
            if verify_passed and result["status"] != "done":
                # Work was correct but maybe the service didn't mark it properly
                result["status"] = "done"
                print(f"  Verify passed (external check confirms work is correct)", flush=True)
            elif not verify_passed:
                print(f"  Verify failed (external check)", flush=True)
        except Exception as e:
            result["verify_output"] = f"Error running verify: {e}"

        # Dump final graph state for analysis
        list_out = await exec_wg(wg_dir, ["list"])
        print(f"\n  Final graph state:\n{list_out}", flush=True)

        # Show logs for both tasks
        for tid in [work_task_id, surv_task_id]:
            show_out = await exec_wg(wg_dir, ["show", tid])
            # Extract just the log section
            in_log = False
            log_lines = []
            for line in show_out.splitlines():
                if line.strip().startswith("Log:"):
                    in_log = True
                    continue
                if in_log:
                    if line.strip() and not line.startswith(" "):
                        break
                    log_lines.append(line)
            if log_lines:
                print(f"\n  Logs for {tid}:", flush=True)
                for ll in log_lines[:20]:
                    print(f"    {ll.strip()}", flush=True)

    except Exception as e:
        result["status"] = "error"
        result["error"] = str(e)
        print(f"  ERROR: {e}", flush=True)
    finally:
        # Always stop the daemon before cleanup (handles both normal and error paths)
        daemon_registry.stop_one(wg_dir)
        result["elapsed_s"] = round(time.monotonic() - start, 2)
        # Save graph state for analysis
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

    summary = {
        "run_id": "pilot-f-5x1",
        "condition": "F",
        "description": "Condition F pilot: wg-native with surveillance loops",
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "model": MODEL,
        "max_iterations": MAX_ITERATIONS,
        "cycle_delay": CYCLE_DELAY,
        "total_trials": len(results),
        "passed": len(passed),
        "failed": len(failed),
        "pass_rate": len(passed) / len(results) if results else 0,
        "mean_time_s": round(sum(times) / len(times), 2) if times else 0,
        "model_verified_count": model_verified,
        "wg_context_available": True,
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
        "per_trial": [
            {
                "trial_id": r["trial_id"],
                "task": r["task"],
                "difficulty": r["difficulty"],
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
    print(f"Pilot F (5×1): wg-native with surveillance loops")
    print(f"  Model: {MODEL}")
    print(f"  Tasks: {len(TB_TASKS)}")
    print(f"  Max iterations per surveillance loop: {MAX_ITERATIONS}")
    print(f"  Cycle delay: {CYCLE_DELAY}")
    print(f"  wg binary: {WG_BIN}")
    print(f"  Results: {RESULTS_DIR}")
    print()

    # Clean old results
    if os.path.isdir(RESULTS_DIR):
        shutil.rmtree(RESULTS_DIR)
    os.makedirs(RESULTS_DIR, exist_ok=True)

    results = []
    for task_def in TB_TASKS:
        r = await run_trial(task_def)
        results.append(r)

    # Write summary
    summary = write_summary(results)

    # Print final report
    print(f"\n{'='*60}")
    print(f"  PILOT F (5×1) RESULTS")
    print(f"{'='*60}")
    print(f"  Passed: {summary['passed']}/{summary['total_trials']} "
          f"({summary['pass_rate']:.0%})")
    print(f"  Mean time: {summary['mean_time_s']:.1f}s")
    print(f"  Model verified: {summary['model_verified_count']}/{summary['total_trials']}")
    print(f"  Surveillance loops created: "
          f"{summary['surveillance_loop_stats']['loops_created']}/{summary['total_trials']}")
    print(f"  Cycle edges: "
          f"{summary['surveillance_loop_stats']['cycle_edges_created']}/{summary['total_trials']}")
    print(f"  Issues detected by surveillance: "
          f"{summary['surveillance_loop_stats']['issues_detected_count']}")
    print(f"  Results: {os.path.join(RESULTS_DIR, 'summary.json')}")
    print(f"{'='*60}")

    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
