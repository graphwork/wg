#!/usr/bin/env bash
# Real installed-binary Fake-Pi stream follower + canonical session proof.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v python3 >/dev/null 2>&1 || skip "python3 unavailable"

scratch=$(make_scratch)
project="$scratch/project"
home="$scratch/home"
worktree="$scratch/worktree"
mkdir -p "$project" "$home/.config/workgraph" "$worktree"
: >"$home/.config/workgraph/config.toml"
(
  cd "$project"
  git init -q
  git config user.email pi-native@test.invalid
  git config user.name 'Pi Native Smoke'
  printf 'baseline\n' >README
  git add README && git commit -qm baseline
  HOME="$home" XDG_CONFIG_HOME="$home/.config" wg init --no-agency >/dev/null
)
wgrun() { (cd "$project" && env -u WG_AGENT_ID -u WG_TASK_ID -u WG_TIER HOME="$home" XDG_CONFIG_HOME="$home/.config" wg "$@"); }
wgrun add native-proof --id native-proof -d $'Fake Pi native stream.\n\n## Validation\n- bounded live proof' >/dev/null
wgrun claim native-proof >/dev/null
wgrun pi-watchdog fixture-init native-proof --worktree "$worktree" --now 0 >/dev/null
attempt=$(wgrun show native-proof --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["lifecycle"]["current_attempt"]["id"])')
session_dir="$project/.wg/attempts/$attempt/pi/session"
bootstrap="$session_dir/fake-session.jsonl"
substantive="$session_dir/2026-01-01T00-00-00Z_fake-session.jsonl"
printf '%s\n%s\n' \
  '{"type":"session","version":3,"id":"fake-session"}' \
  '{"type":"model_change","provider":"fake","modelId":"slow"}' >"$substantive"
bootstrap_before=$(sha256sum "$bootstrap" | cut -d' ' -f1)

agent_dir="$project/.wg/agents/fake-pi"
mkdir -p "$agent_dir"
printf '{"attempt_id":"%s","executor":"pi","model":"pi:fake:slow"}\n' "$attempt" >"$agent_dir/metadata.json"
: >"$agent_dir/raw_stream.jsonl"

sleep 4 & child=$!
wgrun pi-stream-observe --agent-dir "$agent_dir" --follow-pid "$child" >"$scratch/observe.out" 2>"$scratch/observe.err" & observer=$!
sleep .15
printf '%s\n' '{"type":"turn_start"}' >>"$agent_dir/raw_stream.jsonl"
printf '%s\n' '{"type":"message_start"}' >>"$agent_dir/raw_stream.jsonl"
printf '%s\n' '{"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","delta":"RAW_REASONING_CANARY_native","thinkingTokens":7}}' >>"$agent_dir/raw_stream.jsonl"
printf '%s\n' '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"HOSTILE_OUTPUT_CANARY_native","outputTokens":5}}' >>"$agent_dir/raw_stream.jsonl"
printf '%s\n' '{"type":"tool_execution_start","toolCallId":"build-1","toolName":"bash"}' >>"$agent_dir/raw_stream.jsonl"
printf '%s\n' '{"type":"tool_execution_update","toolCallId":"build-1","toolName":"bash","childState":"running","progress":1}' >>"$agent_dir/raw_stream.jsonl"
sleep .35
live=$(wgrun --json pi-watchdog status native-proof)
python3 - "$live" <<'PY'
import json,sys
v=json.loads(sys.argv[1]); s=v['state']; n=s['native_activity']
assert n['event_seq'] >= 6, n
assert n['thinking_activity_seq'] >= 1 and n['output_activity_seq'] >= 1, n
assert n['current_tool_label'] == 'bash' and n['tool_child_state'] == 'running', n
assert s['classification'] == 'long-tool', s['classification']
assert s['progress_seq'] >= 6, s['progress_seq']
raw=json.dumps(v)
assert 'RAW_REASONING_CANARY' not in raw and 'HOSTILE_OUTPUT_CANARY' not in raw
PY
show=$(wgrun show native-proof)
grep -q 'native: live/unproven seq=' <<<"$show" || loud_fail "wg show omitted bounded native activity: $show"
grep -q 'proof-silence=' <<<"$show" || loud_fail "wg show did not distinguish proof silence: $show"
! grep -Eq 'RAW_REASONING_CANARY|HOSTILE_OUTPUT_CANARY' <<<"$show" || loud_fail 'raw provider content leaked in status'

usage='{"type":"turn_end","turnId":"turn-1","message":{"usage":{"input":10,"output":5,"cacheRead":2,"cacheWrite":1,"totalTokens":18,"cost":{"total":0.25}}}}'
printf '%s\n%s\n' "$usage" "$usage" >>"$agent_dir/raw_stream.jsonl"
printf '%s\n' '{"type":"tool_execution_end","toolCallId":"build-1","toolName":"bash","isError":false}' >>"$agent_dir/raw_stream.jsonl"
kill "$child" 2>/dev/null || true
wait "$child" 2>/dev/null || true
wait "$observer"

state=$(wgrun --json pi-watchdog status native-proof)
python3 - "$state" "$substantive" <<'PY'
import json,sys
s=json.loads(sys.argv[1])['state']; n=s['native_activity']
assert n['usage_receipt_count'] == 1, n
assert n['usage_total'] == 18 and n['usage_cost'] == '0.250000', n
assert s['session']['session_file'] == sys.argv[2], s['session']
assert s['session']['branch_leaf'].startswith('b3:'), s['session']
PY
[[ "$bootstrap_before" == "$(sha256sum "$bootstrap" | cut -d' ' -f1)" ]] || loud_fail 'bootstrap evidence was rewritten'

# Restart/replay from byte zero: the persisted cursor and usage receipt set
# prevent progress/cost duplication.
before=$(python3 - "$state" <<'PY'
import json,sys
s=json.loads(sys.argv[1])['state']; n=s['native_activity']; print(s['progress_seq'],n['event_seq'],n['usage_receipt_count'],n['usage_cost'])
PY
)
sleep .3 & child=$!
wgrun pi-stream-observe --agent-dir "$agent_dir" --follow-pid "$child"
wait "$child" 2>/dev/null || true
after_state=$(wgrun --json pi-watchdog status native-proof)
after=$(python3 - "$after_state" <<'PY'
import json,sys
s=json.loads(sys.argv[1])['state']; n=s['native_activity']; print(s['progress_seq'],n['event_seq'],n['usage_receipt_count'],n['usage_cost'])
PY
)
[[ "$before" == "$after" ]] || loud_fail "replay changed monotonic counters: before=$before after=$after"

# A second substantive match is genuine ambiguity: refuse loudly, retain every
# byte, and put exact-session continuation on hold.
second="$session_dir/2026-01-02T00-00-00Z_fake-session.jsonl"
printf '%s\n%s\n' \
  '{"type":"session","version":3,"id":"fake-session"}' \
  '{"type":"message","id":"other"}' >"$second"
sleep .2 & child=$!
if wgrun pi-stream-observe --agent-dir "$agent_dir" --follow-pid "$child" >"$scratch/ambiguous.out" 2>"$scratch/ambiguous.err"; then
  loud_fail 'two substantive journals were accepted'
fi
wait "$child" 2>/dev/null || true
grep -q 'substantive journals' "$scratch/ambiguous.err" || loud_fail "ambiguity was not loud: $(cat "$scratch/ambiguous.err")"
[[ -s "$bootstrap" && -s "$substantive" && -s "$second" ]] || loud_fail 'session evidence was deleted'
held=$(wgrun --json pi-watchdog status native-proof)
python3 - "$held" <<'PY'
import json,sys
s=json.loads(sys.argv[1])['state']
assert s['classification'] == 'stalled-operator-required', s['classification']
assert s['exact_guards']['session'] is False, s['exact_guards']
PY

echo 'PASS: Pi native activity is live/bounded, replay-idempotent, and exact session proof selects one substantive journal while preserving bootstrap evidence and refusing ambiguity'
