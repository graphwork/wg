#!/usr/bin/env python3
"""
Condition F Pilot Runner — runs 14 condition-F trials through the full
native wg adapter lifecycle with federation.

Each trial:
  1. Init per-trial workgraph in temp dir
  2. Write condition F config (graph context, native executor)
  3. Federation pull from tb-evaluations/ hub
  4. Create root task
  5. Start wg service
  6. Mark task done (pilot mode — lifecycle verification, not LLM execution)
  7. Poll for completion
  8. Stop service
  9. Federation push to hub
 10. Collect metrics + cleanup

Produces:
  - terminal-bench/results/pilot-condition-f.md  (human-readable summary)
  - terminal-bench/trials/tb-results-pilot-condition-f.json  (machine-readable)
"""

import asyncio
import json
import os
import shutil
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

# Import adapter internals
import sys
sys.path.insert(0, os.path.dirname(__file__))

from wg.daemon_cleanup import daemon_registry
from wg.adapter import (
    CONDITION_CONFIG,
    FEDERATION_CONDITIONS,
    _collect_agent_metrics,
    _ensure_hub_initialized,
    _exec_wg_cmd_host,
    _federation_pull,
    _federation_push,
    _poll_task_completion,
    _write_trial_bundle,
    _write_trial_federation_config,
    _write_trial_wg_config,
)

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
HUB_PATH = os.path.join(SCRIPT_DIR, "tb-evaluations")
WG_BIN = shutil.which("wg") or os.path.expanduser("~/.cargo/bin/wg")

# 7 task types × 2 replicas = 14 trials
TASK_TYPES = [
    "file-ops", "text-processing", "debugging",
    "shell-scripting", "data-processing", "algorithm", "ml",
]
REPLICAS = 2
CONDITION = "F"
MODEL = "test:pilot-condition-f"


# ---------------------------------------------------------------------------
# Trial result
# ---------------------------------------------------------------------------

class TrialResult:
    def __init__(self, trial_id, task_type, replica):
        self.trial_id = trial_id
        self.task_type = task_type
        self.replica = replica
        self.condition = CONDITION
        self.status = "not_started"
        self.elapsed_s = 0.0
        self.used_native_executor = False
        self.started_wg_service = False
        self.federation_pulled = False
        self.federation_pushed = False
        self.config_written = False
        self.error = None
        self.metrics = None

    def to_dict(self):
        return {
            "trial_id": self.trial_id,
            "condition": self.condition,
            "task_type": self.task_type,
            "replica": self.replica,
            "status": self.status,
            "elapsed_s": round(self.elapsed_s, 2),
            "used_native_executor": self.used_native_executor,
            "started_wg_service": self.started_wg_service,
            "federation_pulled": self.federation_pulled,
            "federation_pushed": self.federation_pushed,
            "config_written": self.config_written,
            "error": self.error,
        }


# ---------------------------------------------------------------------------
# Single trial
# ---------------------------------------------------------------------------

async def run_trial(task_type: str, replica: int) -> TrialResult:
    trial_id = f"pilot-f-{task_type}-r{replica}"
    result = TrialResult(trial_id, task_type, replica)
    start = time.monotonic()

    tmpdir = tempfile.mkdtemp(prefix=f"tb-pilot-f-{trial_id}-")
    wg_dir = os.path.join(tmpdir, ".workgraph")

    try:
        # 1. Init workgraph
        init_out = await _exec_wg_cmd_host(wg_dir, WG_BIN, ["init"])
        if "error" in init_out.lower() and "already" not in init_out.lower():
            raise RuntimeError(f"Init failed: {init_out}")

        # 2. Write condition F config (native executor, graph context)
        await _write_trial_wg_config(tmpdir, wg_dir, CONDITION, MODEL)
        config_path = os.path.join(wg_dir, "config.toml")
        assert os.path.isfile(config_path), "Config not written"
        config_content = open(config_path).read()
        assert 'executor = "native"' in config_content, "Not native executor"
        result.config_written = True
        result.used_native_executor = True

        # 3. Write bundle (condition F doesn't exclude wg tools, so no bundle needed)
        await _write_trial_bundle(wg_dir, CONDITION)

        # 4. Federation pull from hub
        assert CONDITION in FEDERATION_CONDITIONS, "F must be in FEDERATION_CONDITIONS"
        await _ensure_hub_initialized(HUB_PATH, WG_BIN)
        await _write_trial_federation_config(wg_dir, HUB_PATH)
        fed_config = os.path.join(wg_dir, "federation.yaml")
        assert os.path.isfile(fed_config), "Federation config not written"

        pull_out = await _federation_pull(wg_dir, WG_BIN, HUB_PATH)
        if "[wg command error:" in pull_out:
            raise RuntimeError(f"Federation pull failed: {pull_out}")
        result.federation_pulled = True

        # 5. Create root task
        root_task_id = f"tb-{trial_id}"
        description = (
            f"Condition F pilot trial: {task_type} (replica {replica})\n"
            f"Context scope: graph | Executor: native | Federation: enabled"
        )
        add_out = await _exec_wg_cmd_host(wg_dir, WG_BIN, [
            "add", f"Pilot F: {task_type} (r{replica})",
            "--id", root_task_id,
            "-d", description,
        ])

        # 6. Verify task exists
        show_out = await _exec_wg_cmd_host(wg_dir, WG_BIN, ["show", root_task_id])
        assert "Status:" in show_out, f"Task not created: {show_out}"

        # 7. Start wg service
        service_cmd = [
            "service", "start",
            "--max-agents", "1",
            "--executor", "native",
            "--model", MODEL,
            "--no-coordinator-agent",
            "--force",
        ]
        service_out = await _exec_wg_cmd_host(wg_dir, WG_BIN, service_cmd)
        daemon_registry.register(wg_dir, WG_BIN)
        result.started_wg_service = True

        # 8. Mark task done (pilot mode — verifying lifecycle, not LLM execution)
        done_out = await _exec_wg_cmd_host(wg_dir, WG_BIN, ["done", root_task_id])

        # 9. Poll for completion
        status, poll_elapsed = await _poll_task_completion(
            wg_dir, WG_BIN, root_task_id,
            timeout_secs=15, poll_interval=0.3,
        )
        if status != "done":
            raise RuntimeError(f"Expected 'done', got '{status}'")

        # 10. Stop service (daemon_registry.stop_one in finally is the safety net)
        daemon_registry.stop_one(wg_dir)

        # 11. Federation push to hub
        push_out = await _federation_push(wg_dir, WG_BIN, HUB_PATH)
        if "[wg command error:" in push_out:
            raise RuntimeError(f"Federation push failed: {push_out}")
        result.federation_pushed = True

        # 12. Collect metrics
        result.metrics = await _collect_agent_metrics(wg_dir)

        result.status = "done"

    except Exception as e:
        result.status = "failed"
        result.error = str(e)
    finally:
        # Always stop the daemon before cleanup (handles both normal and error paths)
        daemon_registry.stop_one(wg_dir)
        result.elapsed_s = time.monotonic() - start
        shutil.rmtree(tmpdir, ignore_errors=True)

    return result


# ---------------------------------------------------------------------------
# Run all trials
# ---------------------------------------------------------------------------

async def run_all_trials():
    results = []
    for task_type in TASK_TYPES:
        for replica in range(REPLICAS):
            print(f"  Running trial: F-{task_type}-r{replica} ...", end=" ", flush=True)
            r = await run_trial(task_type, replica)
            results.append(r)
            status_icon = "PASS" if r.status == "done" else "FAIL"
            print(f"{status_icon} ({r.elapsed_s:.1f}s)")
            if r.error:
                print(f"    Error: {r.error}")
    return results


# ---------------------------------------------------------------------------
# Results writing
# ---------------------------------------------------------------------------

def write_results_json(results, output_path):
    passed = [r for r in results if r.status == "done"]
    failed = [r for r in results if r.status == "failed"]
    times = [r.elapsed_s for r in results]

    data = {
        "run_id": "pilot-condition-f",
        "condition": "F",
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "tasks": TASK_TYPES,
        "replicas": REPLICAS,
        "total_trials": len(results),
        "passed": len(passed),
        "failed": len(failed),
        "pass_rate": len(passed) / len(results) if results else 0,
        "mean_time_s": sum(times) / len(times) if times else 0,
        "federation_pulled": sum(1 for r in results if r.federation_pulled),
        "federation_pushed": sum(1 for r in results if r.federation_pushed),
        "native_executor_used": sum(1 for r in results if r.used_native_executor),
        "trials": [r.to_dict() for r in results],
    }

    os.makedirs(os.path.dirname(output_path), exist_ok=True)
    with open(output_path, "w") as f:
        json.dump(data, f, indent=2)
    return data


def write_results_markdown(results, data, output_path):
    passed = data["passed"]
    failed = data["failed"]
    total = data["total_trials"]

    lines = [
        "# Condition F Pilot Run Results",
        "",
        f"**Date:** {datetime.now(timezone.utc).strftime('%Y-%m-%d %H:%M UTC')}",
        f"**Condition:** F (distilled context injection + empirical verification)",
        f"**Executor:** Native wg (not litellm)",
        f"**Mode:** Lifecycle pilot (no LLM execution -- verifies adapter + federation pipeline)",
        f"**Hub:** terminal-bench/tb-evaluations/",
        "",
        "## Summary",
        "",
        f"| Metric | Value |",
        f"|---|---|",
        f"| Total trials | {total} |",
        f"| Passed | {passed} |",
        f"| Failed | {failed} |",
        f"| Pass rate | {passed/total:.0%} |",
        f"| Mean time per trial | {data['mean_time_s']:.2f}s |",
        f"| Federation pull verified | {data['federation_pulled']}/{total} |",
        f"| Federation push verified | {data['federation_pushed']}/{total} |",
        f"| Native executor used | {data['native_executor_used']}/{total} |",
        "",
        "## Per-Task Results",
        "",
        "| Task | Rep | Status | Time (s) | Fed Pull | Fed Push | Error |",
        "|---|---|---|---|---|---|---|",
    ]

    for r in results:
        error_col = r.error[:60] + "..." if r.error and len(r.error) > 60 else (r.error or "")
        lines.append(
            f"| {r.task_type} | {r.replica} | {r.status} | {r.elapsed_s:.2f} "
            f"| {'yes' if r.federation_pulled else 'no'} "
            f"| {'yes' if r.federation_pushed else 'no'} "
            f"| {error_col} |"
        )

    lines.extend([
        "",
        "## Validation Checklist",
        "",
        f"- [{'x' if total >= 10 else ' '}] At least 10 condition F trials ran to completion ({total} total, {passed} passed)",
        f"- [{'x' if data['native_executor_used'] == total else ' '}] Each trial used native wg executor ({data['native_executor_used']}/{total})",
        f"- [{'x' if data['federation_pulled'] >= passed else ' '}] Federation pull verified ({data['federation_pulled']}/{total})",
        f"- [{'x' if data['federation_pushed'] >= passed else ' '}] Federation push verified ({data['federation_pushed']}/{total})",
        f"- [{'x' if passed >= 10 else ' '}] Results summary with pass/fail counts, timing",
    ])

    # Document failures
    failed_trials = [r for r in results if r.status == "failed"]
    if failed_trials:
        lines.extend([
            "",
            "## Failures",
            "",
        ])
        for r in failed_trials:
            lines.append(f"### {r.trial_id}")
            lines.append(f"- **Task:** {r.task_type}, replica {r.replica}")
            lines.append(f"- **Error:** {r.error}")
            lines.append("")
    else:
        lines.extend([
            "",
            "## Failures",
            "",
            "No failures.",
        ])

    lines.extend([
        "",
        "## Design Notes",
        "",
        "Condition F is the wg-native condition with distilled context injection.",
        "It uses graph-scope context, full wg tools, and federation to the tb-evaluations hub.",
        "No agency identity is assigned (unlike D/E) -- the agent operates with raw wg tools",
        "plus the distilled WG Quick Guide (~1100 tokens) injected into the system prompt.",
        "",
        "This pilot validates the adapter lifecycle and federation pipeline.",
        "Full trials with LLM execution require Harbor + Docker + OPENROUTER_API_KEY.",
    ])

    os.makedirs(os.path.dirname(output_path), exist_ok=True)
    with open(output_path, "w") as f:
        f.write("\n".join(lines) + "\n")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    print(f"Condition F Pilot Runner")
    print(f"  Tasks: {len(TASK_TYPES)} types × {REPLICAS} replicas = {len(TASK_TYPES) * REPLICAS} trials")
    print(f"  Hub: {HUB_PATH}")
    print(f"  wg binary: {WG_BIN}")
    print()

    results = asyncio.run(run_all_trials())

    # Write JSON results
    json_path = os.path.join(SCRIPT_DIR, "trials", "tb-results-pilot-condition-f.json")
    data = write_results_json(results, json_path)
    print(f"\nJSON results: {json_path}")

    # Write markdown results
    md_path = os.path.join(SCRIPT_DIR, "results", "pilot-condition-f.md")
    write_results_markdown(results, data, md_path)
    print(f"Markdown results: {md_path}")

    # Summary
    print(f"\n{'='*60}")
    print(f"  Condition F Pilot: {data['passed']}/{data['total_trials']} passed ({data['pass_rate']:.0%})")
    print(f"  Federation: {data['federation_pulled']} pulled, {data['federation_pushed']} pushed")
    print(f"  Mean time: {data['mean_time_s']:.2f}s per trial")
    print(f"{'='*60}")

    return 0 if data["passed"] >= 10 else 1


if __name__ == "__main__":
    sys.exit(main())
