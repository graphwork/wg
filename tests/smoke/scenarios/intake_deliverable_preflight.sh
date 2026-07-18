#!/usr/bin/env bash
# Scenario: intake_deliverable_preflight
#
# Regression for the "intake task handled as evaluator/review work" class
# (docs/research/prevent-intake-as-evaluator-design.md guardrails G1/G3):
# an operational intake task with a `## Deliverables` block must be refused
# at `wg done` when the named deliverables are absent (failure_class
# `deliverable-missing`), the retry path must inject the G3 "do the
# operational work, not the meta" directive block at the TOP of the next
# worker's prompt — naming the concrete deliverables — instead of the
# neutral "continue from where they left off" framing, and after the
# deliverable is produced `wg done` must succeed and clear the marker.
#
# Asserts:
#   (a) `wg done` refuses (non-zero) and names the missing deliverable.
#   (b) The task records failure_class `deliverable-missing` and stays
#       in-progress (not promoted to Done).
#   (c) On retry (retry_count=1, failure_class=deliverable-missing), the
#       spawned worker prompt.txt leads with the G3 directive block, names
#       the deliverable, and does NOT use the neutral framing.
#   (d) After the deliverable file is produced, `wg done` succeeds (exit 0),
#       the task reaches `done`, and the failure_class marker is cleared.
#
# The G3 directive-injection pure function is additionally pinned by the
# `retry_mutates_prompt_on_deliverable_missing` unit test. This scenario
# drives the real `wg done` + `wg spawn` CLI paths end-to-end.
#
# Requires: python3 (to set retry_count/failure_class) and wg.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON3" "python3 needed to set retry_count/failure_class on the graph row"
fi

scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME"
project_root="$scratch/project"
mkdir -p "$project_root"
wg_dir="$project_root/.wg"

# ── Init a git repo with `main` (wg done / spawn touch git state) ─────
git init -b main "$project_root" >/dev/null 2>&1 \
    || loud_fail "git init failed in $project_root"
(
    cd "$project_root"
    git config user.email "smoke@test" >/dev/null
    git config user.name "Smoke" >/dev/null
    echo "initial" > README.md
    git add README.md >/dev/null
    git commit -m "initial" >/dev/null
) || loud_fail "git initial commit setup failed"

cd "$project_root"
wg --dir "$wg_dir" init >/dev/null 2>&1 \
    || loud_fail "wg init failed in scratch dir"
# Fresh graphs are intentionally graph-only. Select the Claude CLI route
# explicitly; setup itself is credential-free, and this scenario only needs
# spawn to render prompt.txt before any model interaction.
wg --dir "$wg_dir" setup --route claude-cli --scope local --yes >/dev/null 2>&1 \
    || loud_fail "credential-free Claude CLI route setup failed in scratch dir"

DESC='## Description
Refresh the e97 seed checkpoint.

## Deliverables
- latest.pt
'

# An intake task with a ## Deliverables block. No `--exec`: the task stays
# exec_mode=full so `wg spawn --executor claude` takes the full-mode arm
# that writes prompt.txt (the shell arm does not render the LLM prompt).
wg --dir "$wg_dir" add "register-refreshed-e97-seed" \
    --id intake-preflight \
    -d "$DESC" \
    -t intake >/dev/null 2>&1 \
    || loud_fail "wg add of intake-preflight failed"

# Helper: rewrite the task row with arbitrary field overrides.
set_task_field() {
    python3 - "$wg_dir/graph.jsonl" "$@" <<'PY'
import json, sys
path = sys.argv[1]
overrides = {}
for kv in sys.argv[2:]:
    k, _, v = kv.partition('=')
    overrides[k] = v
out = []
for line in open(path):
    if not line.strip():
        continue
    obj = json.loads(line)
    if obj.get("kind") == "task" and obj.get("id") == "intake-preflight":
        for k, v in overrides.items():
            if k == "retry_count":
                obj[k] = int(v)
            elif v == "":
                obj.pop(k, None)
            else:
                obj[k] = v
    out.append(json.dumps(obj))
open(path, "w").write("\n".join(out) + "\n")
PY
}

# Helper: read a single field from the intake-preflight task row.
get_task_field() {
    python3 - "$wg_dir/graph.jsonl" "$1" <<'PY'
import json, sys
path, key = sys.argv[1], sys.argv[2]
for line in open(path):
    if not line.strip():
        continue
    obj = json.loads(line)
    if obj.get("kind") == "task" and obj.get("id") == "intake-preflight":
        v = obj.get(key, "")
        print(v if v is not None else "")
        break
PY
}

# ── (a)+(b): wg done refuses when the deliverable is absent ───────────
set_task_field status=in-progress

done_log="$scratch/done-refuse.log"
unset WG_AGENT_ID
unset WG_SMOKE_AGENT_OVERRIDE
set +e
WG_WORKTREE_PATH="$project_root" \
WG_BRANCH="wg/smoke/intake-preflight" \
WG_PROJECT_ROOT="$project_root" \
    wg --dir "$wg_dir" done intake-preflight --skip-smoke \
    >"$done_log" 2>&1
done_exit=$?
set -e

if [[ $done_exit -eq 0 ]]; then
    loud_fail "wg done exited 0 despite missing deliverable 'latest.pt' — G1 preflight regressed.
done.log:
$(cat "$done_log")"
fi
if ! grep -q "deliverable preflight refused" "$done_log"; then
    loud_fail "wg done refusal did not mention 'deliverable preflight refused'.
done.log:
$(cat "$done_log")"
fi
if ! grep -q "latest.pt" "$done_log"; then
    loud_fail "wg done refusal did not name the missing deliverable 'latest.pt'.
done.log:
$(cat "$done_log")"
fi

# failure_class recorded; status still in-progress (not promoted to Done).
fc=$(get_task_field failure_class)
st=$(get_task_field status)
if [[ "$fc" != "deliverable-missing" ]]; then
    loud_fail "failure_class expected 'deliverable-missing', got '$fc'"
fi
if [[ "$st" != "in-progress" ]]; then
    loud_fail "status expected 'in-progress' (not promoted to done), got '$st'"
fi

# ── (c): on retry, the spawned worker prompt injects the G3 directive ─
# Simulate the retry state: retry_count=1, failure_class=deliverable-missing,
# and re-open the task so `wg spawn` will pick it up.
set_task_field status=open retry_count=1

spawn_log="$scratch/spawn.log"
set +e
wg --dir "$wg_dir" spawn intake-preflight --executor claude >"$spawn_log" 2>&1
spawn_exit=$?
set -e
if [[ $spawn_exit -ne 0 ]]; then
    loud_fail "wg spawn of intake-preflight failed: $(cat "$spawn_log")"
fi
if ! grep -qi "spawned" "$spawn_log"; then
    loud_fail "wg spawn did not report a spawned agent: $(cat "$spawn_log")"
fi

# prompt.txt lands under .wg/agents/agent-*/prompt.txt
prompt_file=$(ls "$wg_dir"/agents/agent-*/prompt.txt 2>/dev/null | head -1 || true)
if [[ -z "$prompt_file" || ! -f "$prompt_file" ]]; then
    loud_fail "no prompt.txt found under $wg_dir/agents/ (looked for agent-*/prompt.txt)"
fi

prompt_content=$(cat "$prompt_file")
if ! echo "$prompt_content" | grep -q "PREVIOUS ATTEMPT FAILED — DO NOT REPEAT"; then
    loud_fail "G3 directive header missing from spawned prompt.txt.
prompt.txt:
$prompt_content"
fi
if ! echo "$prompt_content" | grep -q "latest.pt"; then
    loud_fail "G3 directive did not name the deliverable 'latest.pt'.
prompt.txt:
$prompt_content"
fi
if echo "$prompt_content" | grep -q "Continue from where they left off"; then
    loud_fail "G3 directive did not replace the neutral framing — 'Continue from where they left off' still present.
prompt.txt:
$prompt_content"
fi

# ── (d): after the deliverable is produced, wg done succeeds ──────────
# Reset the task to in-progress (spawn flipped it) and clear the agent so
# the human-harness `wg done` path runs cleanly.
set_task_field status=in-progress assigned= retry_count=1
echo "checkpoint bytes" > "$project_root/latest.pt"

done_log2="$scratch/done-succeed.log"
unset WG_AGENT_ID
unset WG_SMOKE_AGENT_OVERRIDE
set +e
WG_WORKTREE_PATH="$project_root" \
WG_BRANCH="wg/smoke/intake-preflight" \
WG_PROJECT_ROOT="$project_root" \
    wg --dir "$wg_dir" done intake-preflight --skip-smoke \
    >"$done_log2" 2>&1
done_exit2=$?
set -e

if [[ $done_exit2 -ne 0 ]]; then
    loud_fail "wg done failed after the deliverable was produced — expected success.
done.log:
$(cat "$done_log2")"
fi

st2=$(get_task_field status)
fc2=$(get_task_field failure_class)
if [[ "$st2" != "done" ]]; then
    loud_fail "status expected 'done' after producing the deliverable, got '$st2'"
fi
if [[ -n "$fc2" ]]; then
    loud_fail "failure_class should be cleared on success, got '$fc2'"
fi

echo "PASS: intake_deliverable_preflight"
exit 0
