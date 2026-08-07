#!/usr/bin/env python3
"""Measure WG control-plane authority without truncating production at cfg(test) imports.

The 2026-08-06 deletion audit truncated each Rust file at its first `#[cfg(test)]`.
That is unsafe: several production files put a test-only import near the top and
continue with production code. This scanner masks each cfg(test)-annotated item
instead, leaving later production visible.
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

CONTROL_PATHS = [
    # Graph/readiness.
    "src/graph.rs", "src/parser.rs", "src/query.rs", "src/cron.rs",
    # Attempt kernels/broker.
    "src/lifecycle.rs", "src/lifecycle_protocol.rs", "src/attempt_runtime.rs",
    "src/worker_control.rs",
    # Completion/finalization.
    "src/save_transaction.rs", "src/completion_evidence.rs",
    "src/completion_manifest.rs", "src/completion_review.rs",
    "src/completion_review_model.rs", "src/completion_task.rs",
    "src/finalization/mod.rs", "src/work_save.rs", "src/commands/done.rs",
    "src/commands/finalize.rs", "src/commands/completion_submit.rs",
    "src/commands/completion_land.rs", "src/commands/completion_done.rs",
    "src/commands/completion_repair.rs", "src/commands/work_save.rs",
    # Daemon/dispatch.
    "src/service/planner.rs", "src/service/convergence.rs",
    "src/commands/service/coordinator.rs", "src/commands/service/ipc.rs",
    "src/commands/service/mod.rs", "src/commands/spawn/execution.rs",
    "src/commands/spawn/worktree.rs",
    # Liveness/observers/provider.
    "src/service/registry.rs", "src/service/provider_health.rs",
    "src/telemetry/mod.rs", "src/worktree_observer.rs", "src/pi_watchdog/mod.rs",
    "src/stream_event.rs", "src/commands/heartbeat.rs", "src/commands/reap.rs",
    "src/commands/pi_stream_bridge.rs", "src/commands/pi_watchdog.rs",
    "src/commands/worktree_observer.rs", "src/commands/classify_failure.rs",
    "src/commands/service/triage.rs",
    "src/commands/service/zero_output.rs",
    # Agency/synthetic control tasks.
    "src/assignment_eligibility.rs", "src/eval_lifecycle.rs",
    "src/commands/service/assignment.rs", "src/commands/evaluate.rs",
    "src/commands/eval_scaffold.rs", "src/commands/assign.rs",
    # Other terminal/recovery/remote paths.
    "src/commands/claim_lifecycle.rs", "src/commands/fail.rs",
    "src/commands/incomplete.rs", "src/commands/retry.rs", "src/commands/reset.rs",
    "src/commands/recover.rs", "src/commands/requeue.rs", "src/commands/sweep.rs",
    "src/commands/abandon.rs", "src/commands/kill.rs",
    "src/commands/dead_agents.rs", "src/commands/exec_fed_cmd.rs",
]

CFG_TEST = re.compile(r"^\s*#\s*\[\s*cfg\s*\([^]]*\btest\b[^]]*\)\s*\]\s*$")
TASK_STATUS_ASSIGNMENT = re.compile(
    r"\.status\s*=(?!=)\s*(?:(?:worksgood|crate)::graph::)?Status::"
)
TEST_ATTRIBUTE = re.compile(r"^\s*#\s*\[\s*test\s*\]\s*$")


def _brace_delta(line: str) -> int:
    """Count structural braces, ignoring strings and line comments."""
    delta = 0
    quote: str | None = None
    escaped = False
    i = 0
    while i < len(line):
        char = line[i]
        if quote:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == quote:
                quote = None
            i += 1
            continue
        if char in ('"', "'"):
            quote = char
        elif char == "/" and i + 1 < len(line) and line[i + 1] == "/":
            break
        elif char == "{":
            delta += 1
        elif char == "}":
            delta -= 1
        i += 1
    return delta


def production_mask(lines: list[str]) -> list[bool]:
    """Return True for lines belonging to production items.

    A cfg(test) attribute suppresses only the following Rust item, not the rest
    of the file. Masking that item avoids the audit's first-attribute truncation
    bug while remaining dependency-free.
    """
    keep = [True] * len(lines)
    i = 0
    while i < len(lines):
        if not (CFG_TEST.match(lines[i]) or TEST_ATTRIBUTE.match(lines[i])):
            i += 1
            continue
        start = i
        keep[i] = False
        i += 1
        while i < len(lines) and (
            not lines[i].strip()
            or lines[i].lstrip().startswith("#")
            or lines[i].lstrip().startswith("//")
        ):
            keep[i] = False
            i += 1
        if i >= len(lines):
            break

        # Mask through the end of the annotated item. Most test items are a
        # module/function block; cfg(test) imports end at a semicolon.
        depth = 0
        saw_brace = False
        while i < len(lines):
            keep[i] = False
            delta = _brace_delta(lines[i])
            if delta != 0 or "{" in lines[i]:
                saw_brace = True
            depth += delta
            item_ended = (saw_brace and depth <= 0) or (
                not saw_brace and ";" in lines[i]
            )
            i += 1
            if item_ended:
                break
    return keep


def scan_file(path: Path) -> tuple[int, list[int]]:
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    keep = production_mask(lines)
    production_loc = sum(1 for active in keep if active)
    assignments = [
        number
        for number, (line, active) in enumerate(zip(lines, keep), 1)
        if active and TASK_STATUS_ASSIGNMENT.search(line)
    ]
    return production_loc, assignments


def collect(root: Path) -> dict[str, object]:
    loc = 0
    assignments: dict[str, list[int]] = {}
    missing: list[str] = []
    for relative in CONTROL_PATHS:
        path = root / relative
        if not path.exists():
            missing.append(relative)
            continue
        file_loc, _ = scan_file(path)
        loc += file_loc

    # Status authority is a repository-wide property, not a property of the
    # fixed LOC manifest. A writer outside that manifest is still a bypass.
    for path in sorted((root / "src").rglob("*.rs")):
        _, hits = scan_file(path)
        if hits:
            assignments[str(path.relative_to(root))] = hits
    return {
        "schema": "wg-control-plane-metrics/v1",
        "control_path_count": len(CONTROL_PATHS),
        "missing_paths": missing,
        "production_control_plane_loc": loc,
        "direct_task_status_assignment_count": sum(map(len, assignments.values())),
        "direct_task_status_assignment_file_count": len(assignments),
        "direct_task_status_assignments": assignments,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--check-baseline", type=Path)
    args = parser.parse_args()
    result = collect(args.root.resolve())
    if args.check_baseline:
        expected = json.loads(args.check_baseline.read_text())
        for key in (
            "production_control_plane_loc",
            "direct_task_status_assignment_count",
            "direct_task_status_assignment_file_count",
        ):
            if result[key] != expected[key]:
                raise SystemExit(f"{key}: expected {expected[key]}, got {result[key]}")
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
