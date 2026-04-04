#!/usr/bin/env bash
# Monitor progress of the Pilot Condition A' experiment
RESULTS_DIR="/home/erik/workgraph/terminal-bench/results/pilot-a-prime/pilot-a-prime"

if [ ! -d "$RESULTS_DIR" ]; then
    echo "Results directory not found: $RESULTS_DIR"
    echo "(The job may not have started yet)"
    exit 1
fi

python3 << 'PYEOF'
import json, os, sys
from pathlib import Path

results_dir = Path("/home/erik/workgraph/terminal-bench/results/pilot-a-prime/pilot-a-prime")
total_expected = 30  # 10 tasks * 3 trials

# Count trial dirs
trial_dirs = [d for d in results_dir.iterdir() if d.is_dir()]
started = len(trial_dirs)

pass_count = 0
fail_count = 0
error_count = 0
running_count = 0
errors = {}
completed = 0

# Per-trial stats
turn_counts = []
duration_secs = []
token_counts = []

for td in sorted(trial_dirs):
    rf = td / "result.json"
    if rf.exists():
        completed += 1
        try:
            d = json.loads(rf.read_text())
            exc = d.get("exception_info")
            vr = d.get("verifier_result")
            ar = d.get("agent_result", {})
            meta = ar.get("metadata", {})

            # Collect stats
            turns = meta.get("turns", 0)
            if turns:
                turn_counts.append(turns)
            n_in = ar.get("n_input_tokens", 0) or 0
            n_out = ar.get("n_output_tokens", 0) or 0
            if n_in + n_out > 0:
                token_counts.append(n_in + n_out)

            ae = d.get("agent_execution", {})
            if ae.get("started_at") and ae.get("finished_at"):
                from datetime import datetime
                t0 = datetime.fromisoformat(ae["started_at"].rstrip("Z"))
                t1 = datetime.fromisoformat(ae["finished_at"].rstrip("Z"))
                dur = (t1 - t0).total_seconds()
                duration_secs.append(dur)

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
    else:
        running_count += 1

pct = f"{completed/total_expected*100:.1f}%" if total_expected else "N/A"
valid = pass_count + fail_count
pass_rate = f"{pass_count/valid*100:.1f}%" if valid > 0 else "N/A"
error_rate = f"{error_count/completed*100:.1f}%" if completed > 0 else "N/A"

avg_turns = f"{sum(turn_counts)/len(turn_counts):.1f}" if turn_counts else "N/A"
avg_duration = f"{sum(duration_secs)/len(duration_secs):.0f}s" if duration_secs else "N/A"
avg_tokens = f"{sum(token_counts)/len(token_counts):.0f}" if token_counts else "N/A"

print("╔══════════════════════════════════════════════════════════════╗")
print("║  Pilot Condition A' Progress                                ║")
print("╠══════════════════════════════════════════════════════════════╣")
print(f"║ Total expected:    {total_expected}")
print(f"║ Trials started:    {started}")
print(f"║ Currently running: {running_count}")
print(f"║ Trials complete:   {completed} / {total_expected} ({pct})")
print(f"║ Pass (reward=1):   {pass_count}")
print(f"║ Fail (reward=0):   {fail_count}")
print(f"║ Pass rate:         {pass_rate} (on {valid} valid trials)")
print(f"║ Errors:            {error_count} ({error_rate})")
print(f"╠══════════════════════════════════════════════════════════════╣")
print(f"║ Avg turns/trial:   {avg_turns}")
print(f"║ Avg duration:      {avg_duration}")
print(f"║ Avg tokens:        {avg_tokens}")
print("╚══════════════════════════════════════════════════════════════╝")

if errors:
    print("\nError breakdown:")
    for etype, count in sorted(errors.items(), key=lambda x: -x[1]):
        print(f"  {count:3d}  {etype}")

# Per-task summary
task_results = {}
for td in sorted(trial_dirs):
    task_name = td.name.rsplit("__", 1)[0]
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
        except:
            status = "error"
    else:
        status = "running"
    task_results.setdefault(task_name, []).append(status)

print(f"\nPer-task results ({len(task_results)} tasks):")
for task_name in sorted(task_results.keys()):
    results = task_results[task_name]
    passes = sum(1 for r in results if r == "pass")
    fails = sum(1 for r in results if r == "fail")
    errs = sum(1 for r in results if r == "error")
    runs = sum(1 for r in results if r == "running")
    status_str = f"{passes}P/{fails}F"
    if errs: status_str += f"/{errs}E"
    if runs: status_str += f"/{runs}R"
    print(f"  {task_name:40s}  {status_str}")
PYEOF

# Check process
PID=$(cat terminal-bench/results/pilot-a-prime/run.pid 2>/dev/null)
if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
    ELAPSED=$(ps -p "$PID" -o etime= 2>/dev/null | tr -d ' ')
    echo ""
    echo "Process $PID running (elapsed: $ELAPSED)"
else
    echo ""
    echo "Harbor process not found (may have completed)"
fi
