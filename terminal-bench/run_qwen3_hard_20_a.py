#!/usr/bin/env python3
"""
TB Stress Test: Qwen3-Coder-30B, hardest tasks, Condition A (agent-only).

Runs all 18 available local TB tasks against Qwen3-Coder-30B on lambda01,
ordered hardest-first. The full TB 2.0 catalog has 89 tasks but only 18
have local runner definitions with instruction files and verify commands.

Goal: Find the limits of a 32k context window model on hard tasks.
The 10-task pilot went 10/10 (100%) including 5/5 hard tasks.

Key stress factors:
  - 32768 token context window (SHORT — will hit limits on complex tasks)
  - Multi-file, multi-step tasks that require long conversation histories
  - Context management: truncation events, token utilization tracked

Usage:
    python run_qwen3_hard_20_a.py
    python run_qwen3_hard_20_a.py --smoke          # single task quick check
    python run_qwen3_hard_20_a.py --hard-only      # only hard-rated tasks (13)
    python run_qwen3_hard_20_a.py --tasks mailman,cobol-modernization
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
from wg.tasks import TASKS_BY_ID, ALL_TASKS

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

MODEL = "local:qwen3-coder-30b"
SGLANG_BASE_URL = "http://lambda01:30000/v1"
CONTEXT_WINDOW = 32768
MAX_AGENTS = 1
DEFAULT_TIMEOUT = 2400  # 40 min per trial
DEFAULT_POLL_INTERVAL = 5.0

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
RUN_ID = "qwen3-hard-20-a"
RESULTS_DIR = os.path.join(SCRIPT_DIR, "results", RUN_ID)

WG_BIN = shutil.which("wg") or os.path.expanduser("~/.cargo/bin/wg")

# All 18 tasks ordered hardest-first.
# 10 hard-benchmark tasks (most challenging — multi-file, multi-step):
HARD_BENCHMARK = [
    "configure-git-webserver",     # pipeline: git + hooks + webserver
    "mailman",                     # pipeline: postfix + mailman3 + config
    "multi-source-data-merger",    # multi-file: 3 formats -> merge -> conflicts
    "financial-document-processor", # multi-file: classify -> extract -> summarize
    "cobol-modernization",         # multi-file: COBOL -> Python migration
    "build-cython-ext",            # pipeline: clone -> fix compat -> compile
    "fix-code-vulnerability",      # multi-file: analyze -> report -> fix
    "constraints-scheduling",      # algorithm: ICS parsing + constraint solving
    "multi-module-type-migration", # cascading: type change across 6 modules
    "iterative-test-fix",          # iterative: 6 bugs, 15 tests, fix all
]

# 3 hard calibration tasks:
HARD_CALIBRATION = [
    "algorithm",     # key-value store with transactions
    "ml",            # k-means clustering from scratch
    "sysadmin",      # rate-limited HTTP server
]

# 3 medium calibration tasks:
MEDIUM_CALIBRATION = [
    "debugging",         # fix merge sort bugs
    "shell-scripting",   # log file analyzer
    "data-processing",   # JSON to CSV department summary
]

# 2 easy calibration tasks (included for completeness):
EASY_CALIBRATION = [
    "file-ops",          # create project structure
    "text-processing",   # word frequency counter
]

# Ordered: hardest first
ALL_STRESS_TASKS = HARD_BENCHMARK + HARD_CALIBRATION + MEDIUM_CALIBRATION + EASY_CALIBRATION


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


def cleanup_tmp_paths(task_def: dict) -> None:
    """Remove /tmp files from a previous trial to ensure isolation."""
    for p in task_def.get("tmp_paths", []):
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
    """Read agent stream.jsonl files to extract token counts and context events."""
    agents_dir = os.path.join(wg_dir, "agents")
    metrics = {
        "total_input_tokens": 0,
        "total_output_tokens": 0,
        "total_cost_usd": 0.0,
        "total_turns": 0,
        "max_input_tokens_single_turn": 0,
        "context_truncation_events": 0,
        "token_counts_per_turn": [],
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
                            in_tok = usage.get("input_tokens", 0)
                            out_tok = usage.get("output_tokens", 0)
                            metrics["total_input_tokens"] += in_tok
                            metrics["total_output_tokens"] += out_tok
                            metrics["token_counts_per_turn"].append({
                                "turn": metrics["total_turns"],
                                "input_tokens": in_tok,
                                "output_tokens": out_tok,
                            })
                            if in_tok > metrics["max_input_tokens_single_turn"]:
                                metrics["max_input_tokens_single_turn"] = in_tok
                            # Context pressure: approaching 32k window
                            if in_tok > CONTEXT_WINDOW * 0.8:
                                metrics["context_truncation_events"] += 1
                    elif event.get("type") == "result":
                        usage = event.get("usage", {})
                        cost = usage.get("cost_usd")
                        if cost:
                            metrics["total_cost_usd"] += cost
        except Exception:
            pass

    return metrics


def analyze_daemon_log(wg_dir: str) -> dict:
    """Parse daemon log for context management events and errors."""
    daemon_log = os.path.join(wg_dir, "service", "daemon.log")
    analysis = {
        "error_patterns": [],
        "context_events": [],
        "log_tail": "",
    }

    if not os.path.isfile(daemon_log):
        return analysis

    try:
        with open(daemon_log) as f:
            log_content = f.read()

        # Check for known error patterns
        error_patterns = [
            ("OOM", "out_of_memory"),
            ("out of memory", "out_of_memory"),
            ("CUDA", "cuda_error"),
            ("connection refused", "endpoint_down"),
            ("Connection refused", "endpoint_down"),
            ("context length", "context_overflow"),
            ("maximum context", "context_overflow"),
            ("token limit", "token_limit"),
            ("truncat", "truncation"),
            ("rate limit", "rate_limit"),
            ("429", "rate_limit"),
        ]
        for pattern, category in error_patterns:
            if pattern.lower() in log_content.lower():
                analysis["error_patterns"].append(category)

        # Look for context management events
        for line in log_content.splitlines():
            lower = line.lower()
            if any(kw in lower for kw in ["truncat", "context", "token", "overflow", "sliding"]):
                if len(analysis["context_events"]) < 50:
                    analysis["context_events"].append(line.strip()[:200])

        # Save last 5000 chars
        analysis["log_tail"] = log_content[-5000:]
    except Exception:
        pass

    return analysis


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
        "context_window": CONTEXT_WINDOW,
        "status": "not_started",
        "elapsed_s": 0.0,
        "reward": 0.0,
        "failure_mode": None,
        "metrics": None,
        "context_analysis": None,
        "error": None,
    }

    # Clean up /tmp paths from previous runs of same task
    cleanup_tmp_paths(task_def)

    tmpdir = tempfile.mkdtemp(prefix=f"tb-qwen3-hard-{task_id}-")
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
            f"## Terminal Bench Trial: Qwen3-Coder-30B Stress Test\n\n"
            f"**Task:** {task_id} ({task_def['difficulty']})\n"
            f"**Model:** {MODEL}\n"
            f"**Endpoint:** {SGLANG_BASE_URL}\n"
            f"**Context Window:** {CONTEXT_WINDOW} tokens\n\n"
            f"**CRITICAL:** Your context window is only {CONTEXT_WINDOW} tokens. "
            f"Be concise. Avoid reading large files in full — use head/tail/grep. "
            f"Keep your responses short and focused.\n\n"
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

        print(f"  [{trial_id}] Completed: {status} in {elapsed:.1f}s "
              f"(reward={result['reward']})")

        # 6. Stop service (daemon_registry.stop_one in finally is the safety net)
        daemon_registry.stop_one(wg_dir)

        # 7. Collect metrics (token counts, context events)
        result["metrics"] = await collect_metrics(wg_dir)

        # 8. Analyze daemon log for context management and errors
        result["context_analysis"] = analyze_daemon_log(wg_dir)

        # Classify failure mode based on analysis
        if result["failure_mode"] == "task_failed" and result["context_analysis"]:
            patterns = result["context_analysis"].get("error_patterns", [])
            if "context_overflow" in patterns:
                result["failure_mode"] = "context_overflow"
            elif "out_of_memory" in patterns:
                result["failure_mode"] = "oom"
            elif "endpoint_down" in patterns:
                result["failure_mode"] = "endpoint_error"
            elif "rate_limit" in patterns:
                result["failure_mode"] = "rate_limit"

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
    task_names = tasks or ALL_STRESS_TASKS
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

    print(f"\nTB Stress Test: Qwen3-Coder-30B — Hardest Tasks — Condition A")
    print(f"  Model: {MODEL}")
    print(f"  Endpoint: {SGLANG_BASE_URL}")
    print(f"  Context window: {CONTEXT_WINDOW} tokens")
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

        # Print running tally with context stats
        passed_so_far = sum(1 for r in results if r["reward"] > 0)
        metrics = result.get("metrics") or {}
        max_tok = metrics.get("max_input_tokens_single_turn", 0)
        ctx_events = metrics.get("context_truncation_events", 0)
        turns = metrics.get("total_turns", 0)
        print(f"  Running: {passed_so_far}/{len(results)} passed | "
              f"turns={turns} | max_input_tok={max_tok} | "
              f"ctx_pressure_events={ctx_events} | "
              f"failure={result.get('failure_mode', 'none')}")

    total_time = time.monotonic() - start_time

    # Compute statistics
    passed = sum(1 for r in results if r["reward"] > 0)
    total_trials = len(results)
    times = [r["elapsed_s"] for r in results if r["elapsed_s"] > 0]
    mean_time = sum(times) / len(times) if times else 0
    median_time = sorted(times)[len(times) // 2] if times else 0

    # Token throughput
    total_input = sum(r.get("metrics", {}).get("total_input_tokens", 0)
                      for r in results if r.get("metrics"))
    total_output = sum(r.get("metrics", {}).get("total_output_tokens", 0)
                       for r in results if r.get("metrics"))
    total_turns = sum(r.get("metrics", {}).get("total_turns", 0)
                      for r in results if r.get("metrics"))

    # Context stress analysis
    ctx_truncations = sum(r.get("metrics", {}).get("context_truncation_events", 0)
                          for r in results if r.get("metrics"))
    max_input_any = max((r.get("metrics", {}).get("max_input_tokens_single_turn", 0)
                         for r in results if r.get("metrics")), default=0)

    # Failure mode breakdown
    failure_modes = {}
    for r in results:
        mode = r.get("failure_mode") or "success"
        failure_modes[mode] = failure_modes.get(mode, 0) + 1

    # By difficulty breakdown
    by_difficulty = {}
    for r in results:
        diff = r["difficulty"]
        if diff not in by_difficulty:
            by_difficulty[diff] = {"passed": 0, "total": 0}
        by_difficulty[diff]["total"] += 1
        if r["reward"] > 0:
            by_difficulty[diff]["passed"] += 1
    for d in by_difficulty.values():
        d["pass_rate"] = d["passed"] / d["total"] if d["total"] > 0 else 0

    summary = {
        "run_id": RUN_ID,
        "model": MODEL,
        "endpoint": SGLANG_BASE_URL,
        "context_window": CONTEXT_WINDOW,
        "serving_engine": "SGLang",
        "gpu": "RTX 6000 Ada 48GB (lambda01)",
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "condition": "A (agent-only, no workgraph decomposition)",
        "task_selection": "All 18 available local TB tasks, ordered hardest-first",
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
        "context_stress": {
            "total_truncation_events": ctx_truncations,
            "max_input_tokens_any_turn": max_input_any,
            "context_window": CONTEXT_WINDOW,
            "utilization_pct": round(max_input_any / CONTEXT_WINDOW * 100, 1) if CONTEXT_WINDOW > 0 else 0,
        },
        "failure_modes": failure_modes,
        "by_difficulty": by_difficulty,
        "trials": results,
    }

    # Write results
    os.makedirs(RESULTS_DIR, exist_ok=True)

    json_path = os.path.join(RESULTS_DIR, "summary.json")
    with open(json_path, "w") as f:
        json.dump(summary, f, indent=2)

    # Print summary
    print(f"\n{'='*80}")
    print(f"SUMMARY: {RUN_ID} — Qwen3-Coder-30B Stress Test (Condition A)")
    print(f"{'='*80}")
    print(f"  Model: {MODEL}")
    print(f"  Endpoint: {SGLANG_BASE_URL}")
    print(f"  Context window: {CONTEXT_WINDOW} tokens")
    print(f"  Pass rate: {passed}/{total_trials} ({passed/total_trials:.0%})" if total_trials else "  No trials")
    print(f"  Mean time per trial: {mean_time:.1f}s")
    print(f"  Median time per trial: {median_time:.1f}s")
    print(f"  Total wall clock: {total_time:.1f}s ({total_time/60:.1f}m)")
    print(f"  Total tokens: {total_input:,} in + {total_output:,} out")
    print(f"  Total turns: {total_turns}")
    print(f"  Effective throughput: {summary['tokens_per_second']:.1f} tok/s (output)")
    print(f"\n  Context stress:")
    print(f"    Truncation events (>80% ctx): {ctx_truncations}")
    print(f"    Max input tokens any turn: {max_input_any:,}")
    print(f"    Context utilization: {summary['context_stress']['utilization_pct']:.1f}%")
    print(f"\n  Failure modes: {failure_modes}")
    print(f"\n  By difficulty:")
    for diff, stats in by_difficulty.items():
        print(f"    {diff}: {stats['passed']}/{stats['total']} ({stats['pass_rate']:.0%})")
    print()
    print(f"  Per-task results:")
    for r in results:
        metrics = r.get("metrics") or {}
        turns = metrics.get("total_turns", 0)
        max_tok = metrics.get("max_input_tokens_single_turn", 0)
        ctx_ev = metrics.get("context_truncation_events", 0)
        print(f"    {r['task']:35s} {r['difficulty']:8s} "
              f"reward={r['reward']:.1f}  time={r['elapsed_s']:7.1f}s  "
              f"turns={turns:3d}  max_tok={max_tok:6d}  "
              f"ctx_press={ctx_ev:2d}  "
              f"failure={r.get('failure_mode', 'none')}")
    print()
    print(f"  Results written to: {json_path}")

    # Comparison with pilot
    pilot_path = os.path.join(SCRIPT_DIR, "results", "pilot-qwen3-local-10", "summary.json")
    if os.path.isfile(pilot_path):
        with open(pilot_path) as f:
            pilot = json.load(f)
        pilot_tasks = {t["task"]: t for t in pilot.get("trials", [])}
        print(f"\n  Comparison with pilot (10-task, all passed):")
        for r in results:
            if r["task"] in pilot_tasks:
                pt = pilot_tasks[r["task"]]
                delta_time = r["elapsed_s"] - pt.get("elapsed_s", 0)
                print(f"    {r['task']:35s} "
                      f"pilot: {pt.get('reward', 0):.0f} in {pt.get('elapsed_s', 0):.0f}s / "
                      f"stress: {r['reward']:.0f} in {r['elapsed_s']:.0f}s "
                      f"(Δ{delta_time:+.0f}s)")

    return summary


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        description="TB Stress Test: Qwen3-Coder-30B, hardest tasks, Condition A")
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT,
                        help="Per-trial timeout in seconds (default: 2400)")
    parser.add_argument("--tasks", nargs="*", help="Override task list")
    parser.add_argument("--smoke", action="store_true",
                        help="Run single hard task for quick validation")
    parser.add_argument("--hard-only", action="store_true",
                        help="Only run hard-rated tasks (13 tasks)")
    args = parser.parse_args()

    tasks = args.tasks
    if args.smoke:
        tasks = ["iterative-test-fix"]
    elif args.hard_only:
        tasks = HARD_BENCHMARK + HARD_CALIBRATION

    summary = asyncio.run(main(
        timeout=args.timeout,
        tasks=tasks,
    ))
    sys.exit(0 if summary["passed"] > 0 else 1)
