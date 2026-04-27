#!/usr/bin/env bash
# Scenario: sweep_archives_orphaned_agent
#
# Regression: when the dispatcher (or `wg sweep` / `wg dead-agents`) unclaims
# an in-progress task whose agent died (stream-hang, heartbeat-timeout, etc.),
# the now-dead agent's `output.log` was orphaned in `.wg/agents/<id>/` and
# never archived to `.wg/log/agents/<task-id>/<timestamp>/`. The TUI's
# iteration switcher only reads the latter, so killed-and-respawned attempts
# were invisible to users. This scenario synthesises that exact situation
# and asserts the archive lands where the TUI expects it.
#
# Reproduces the user-reported bug for tasks `improve-wg-setup`,
# `tui-settings-tab`, `migrate-agency-tasks` (each retried 2-3 times after
# claude.ai stream hangs).
#
# Fast (no daemon, no LLM) and deterministic.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

# Init — any executor, since we never spawn an LLM.
if ! wg init -x shell >init.log 2>&1; then
    loud_fail "wg init failed during smoke setup: $(tail -5 init.log)"
fi

if ! graph_dir=$(graph_dir_in "$scratch"); then
    loud_fail "could not locate .wg/.workgraph dir after init"
fi

# Add a task that we'll mark as in-progress with a fictitious agent.
if ! wg add "smoke task" --id smoke-task >add.log 2>&1; then
    loud_fail "wg add failed: $(cat add.log)"
fi

# Synthesise the orphaned agent: working dir + output.log + prompt.txt.
# This is the directory layout the dispatcher would have created when
# spawning the agent before it died mid-stream.
agent_id="dead-stream-agent"
mkdir -p "$graph_dir/agents/$agent_id"
killed_marker="killed-mid-stream-output-must-survive-as-archive"
echo "$killed_marker" > "$graph_dir/agents/$agent_id/output.log"
echo "smoke prompt content" > "$graph_dir/agents/$agent_id/prompt.txt"

# Mark the task as in-progress and assigned to that agent. Edit the
# canonical graph row directly with python — `wg add` does not expose
# the in-progress + assigned shape, and we want to repro a state the
# dispatcher would create.
python3 - "$graph_dir/graph.jsonl" "$agent_id" <<'PY'
import json, sys
src, agent_id = sys.argv[1], sys.argv[2]
with open(src) as f:
    lines = f.readlines()
out = []
for line in lines:
    if not line.strip():
        continue
    n = json.loads(line)
    if n.get("kind") == "task" and n.get("id") == "smoke-task":
        n["status"] = "in-progress"
        n["assigned"] = agent_id
    out.append(json.dumps(n))
with open(src, "w") as f:
    f.write("\n".join(out) + "\n")
PY

# `wg sweep` looks for in-progress tasks whose assigned agent isn't in the
# registry — exactly our setup, since we never registered $agent_id.
if ! wg sweep >sweep.log 2>&1; then
    loud_fail "wg sweep crashed: $(tail -10 sweep.log)"
fi

# Sweep must report having fixed our task.
if ! grep -q "smoke-task" sweep.log; then
    loud_fail "wg sweep did not report fixing smoke-task:\n$(cat sweep.log)"
fi

# THE REGRESSION CHECK: archive directory must now exist where the TUI
# iteration switcher (find_all_archives) reads from.
archive_base="$graph_dir/log/agents/smoke-task"
if [[ ! -d "$archive_base" ]]; then
    loud_fail "Archive dir was NOT created — TUI iteration switcher will not see this attempt.
Expected: $archive_base
Sweep log:
$(cat sweep.log)
Existing log dir contents:
$(ls -la "$graph_dir/log/" 2>&1 || echo '<missing>')"
fi

# Exactly one timestamped archive should exist (the killed agent's).
archive_count=$(find "$archive_base" -mindepth 1 -maxdepth 1 -type d | wc -l)
if [[ "$archive_count" -ne 1 ]]; then
    loud_fail "Expected exactly 1 archive, found $archive_count under $archive_base:
$(ls -la "$archive_base")"
fi

# Archived output.txt must contain the killed agent's actual content —
# not an empty stub. This is what the user couldn't see in the TUI before
# the fix.
output_file=$(find "$archive_base" -name "output.txt" | head -1)
if [[ -z "$output_file" || ! -f "$output_file" ]]; then
    loud_fail "Archive missing output.txt under $archive_base:
$(find "$archive_base" -type f)"
fi
if ! grep -q "$killed_marker" "$output_file"; then
    loud_fail "Archived output.txt does not contain the killed agent's bytes.
Expected marker '$killed_marker' in: $output_file
Actual content:
$(cat "$output_file")"
fi

# Archived prompt.txt should also be preserved for the iteration view.
prompt_file=$(find "$archive_base" -name "prompt.txt" | head -1)
if [[ -z "$prompt_file" || ! -f "$prompt_file" ]]; then
    loud_fail "Archive missing prompt.txt under $archive_base"
fi

echo "PASS: wg sweep archives orphaned agent's output for TUI iteration switcher"
exit 0
