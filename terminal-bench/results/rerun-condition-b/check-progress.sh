#!/bin/bash
# Monitor progress of the rerun-condition-b experiment
# Usage: ./check-progress.sh [--wg-stats]

RESULTS_DIR="/home/erik/workgraph/terminal-bench/results/rerun-condition-b/rerun-condition-b"
SHOW_WG_STATS="${1:-}"

if [ ! -d "$RESULTS_DIR" ]; then
    echo "Results directory not found: $RESULTS_DIR"
    echo "Run may not have started yet."
    exit 1
fi

python3 << 'PYEOF'
import json, os, sys
from pathlib import Path

results_dir = Path("/home/erik/workgraph/terminal-bench/results/rerun-condition-b/rerun-condition-b")
total_expected = 267  # 89 tasks × 3 trials
show_wg_stats = "--wg-stats" in sys.argv

# Count trial dirs
trial_dirs = [d for d in results_dir.iterdir() if d.is_dir()]
started = len(trial_dirs)

pass_count = 0
fail_count = 0
error_count = 0
errors = {}
completed = 0

# wg usage tracking
wg_log_count = 0
wg_done_count = 0
wg_add_count = 0
wg_artifact_count = 0
wg_any_count = 0
has_wg_state = 0
trials_with_agent_data = 0

for td in sorted(trial_dirs):
    rf = td / "result.json"
    if rf.exists():
        completed += 1
        try:
            d = json.loads(rf.read_text())
            exc = d.get("exception_info")
            vr = d.get("verifier_result")
            if exc:
                etype = exc.get("exception_type", "Unknown")
                errors[etype] = errors.get(etype, 0) + 1
                error_count += 1
            elif vr:
                reward = vr.get("rewards", {}).get("reward", 0)
                if reward >= 1.0:
                    pass_count += 1
                else:
                    fail_count += 1
            else:
                fail_count += 1
        except Exception as e:
            errors[f"ParseError: {e}"] = errors.get(f"ParseError: {e}", 0) + 1

    # Check wg usage in agent logs
    agent_dir = td / "agent"
    if agent_dir.exists():
        trials_with_agent_data += 1
        agent_log = agent_dir / "agent_loop.ndjson"
        if agent_log.exists():
            try:
                text = agent_log.read_text()
                used_any = False
                if '"wg_log"' in text:
                    wg_log_count += 1
                    used_any = True
                if '"wg_done"' in text:
                    wg_done_count += 1
                    used_any = True
                if '"wg_add"' in text:
                    wg_add_count += 1
                    used_any = True
                if '"wg_artifact"' in text:
                    wg_artifact_count += 1
                    used_any = True
                if used_any:
                    wg_any_count += 1
            except Exception:
                pass
        wg_state = agent_dir / "workgraph_state"
        if wg_state.exists():
            has_wg_state += 1

pct = f"{completed/total_expected*100:.1f}%" if total_expected else "N/A"
pass_rate = f"{pass_count/(pass_count+fail_count)*100:.1f}%" if (pass_count + fail_count) > 0 else "N/A"

print("╔══════════════════════════════════════════════════════╗")
print("║ Rerun Condition B (with skill injection) Progress   ║")
print("╠══════════════════════════════════════════════════════╣")
print(f"║ Total expected:   {total_expected:>4}")
print(f"║ Trials started:   {started:>4}")
print(f"║ Trials complete:  {completed:>4} / {total_expected} ({pct})")
print(f"║ Pass (reward=1):  {pass_count:>4}")
print(f"║ Fail (reward=0):  {fail_count:>4}")
print(f"║ Pass rate:        {pass_rate:>6}")
print(f"║ Errors:           {error_count:>4}")
print("╠══════════════════════════════════════════════════════╣")
print("║ Workgraph Usage (target: 80%+)                      ║")
print("╠══════════════════════════════════════════════════════╣")
if trials_with_agent_data > 0:
    wg_pct = f"{wg_any_count/trials_with_agent_data*100:.1f}%"
    print(f"║ Any wg tool:      {wg_any_count:>4} / {trials_with_agent_data} ({wg_pct})")
    print(f"║ wg_log:           {wg_log_count:>4} / {trials_with_agent_data}")
    print(f"║ wg_done:          {wg_done_count:>4} / {trials_with_agent_data}")
    print(f"║ wg_add:           {wg_add_count:>4} / {trials_with_agent_data}")
    print(f"║ wg_artifact:      {wg_artifact_count:>4} / {trials_with_agent_data}")
    print(f"║ wg state saved:   {has_wg_state:>4} / {trials_with_agent_data}")
else:
    print("║ (no agent data yet)")
print("╚══════════════════════════════════════════════════════╝")

if errors:
    print("\nError breakdown:")
    for etype, count in sorted(errors.items(), key=lambda x: -x[1]):
        print(f"  {count:3d}  {etype}")

# Per-task pass rates
if show_wg_stats:
    print("\n\nPer-task results:")
    task_results = {}
    for td in sorted(trial_dirs):
        name = td.name.rsplit("__", 1)[0]
        rf = td / "result.json"
        if rf.exists():
            try:
                d = json.loads(rf.read_text())
                exc = d.get("exception_info")
                vr = d.get("verifier_result")
                if exc:
                    status = "error"
                elif vr and vr.get("rewards", {}).get("reward", 0) >= 1.0:
                    status = "pass"
                else:
                    status = "fail"
            except Exception:
                status = "error"
        else:
            status = "pending"
        task_results.setdefault(name, []).append(status)

    for task, results in sorted(task_results.items()):
        passes = results.count("pass")
        total = len(results)
        indicator = "✓" if passes == total else ("~" if passes > 0 else "✗")
        print(f"  {indicator} {task}: {passes}/{total} ({', '.join(results)})")
PYEOF

# Check if process is still running
PID_FILE="/home/erik/workgraph/terminal-bench/results/rerun-condition-b/run.pid"
if [ -f "$PID_FILE" ]; then
    PID=$(cat "$PID_FILE")
    if ps -p "$PID" > /dev/null 2>&1; then
        ELAPSED=$(ps -p "$PID" -o etime= | tr -d ' ')
        echo ""
        echo "Process $PID is running (elapsed: $ELAPSED)."
    else
        echo ""
        echo "Process $PID has finished."
    fi
fi
