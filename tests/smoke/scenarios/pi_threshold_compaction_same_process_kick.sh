#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

unset WG_WORKER_CAPABILITY WG_WORKER_IPC WG_WORKER_CONTROL_PROTOCOL WG_GRAPH_ID WG_AGENT_ID
require_wg
pi_bin="$(command -v pi 2>/dev/null || true)"
repo_pi="$HERE/../../../worksgood-pi/node_modules/.bin/pi"
if [[ -z "$pi_bin" || "$("$pi_bin" --version 2>/dev/null || true)" != "0.83.0" ]]; then
  # The plugin's lockfile installs the exact real host used by its API and
  # contract tests. Prefer it when an unrelated global Pi has moved ahead;
  # this remains the native Pi binary/package, not a host simulator.
  [[ -x "$repo_pi" && "$("$repo_pi" --version 2>/dev/null || true)" == "0.83.0" ]] \
    || loud_skip "UNSUPPORTED PI HOST" "this regression requires Pi 0.83.0 session_compact queue serialization"
  pi_bin="$repo_pi"
fi
export PATH="$(dirname "$pi_bin"):$PATH"

# Keep the Unix-domain control socket below sun_path's 108-byte limit even
# when the caller's TMPDIR is a long cargo-agent path.
export WG_SMOKE_ROOT="/tmp/wgpk-smoke"
scratch="$(make_scratch)"
project="$scratch/project"
home="$scratch/home"
fixture="$(cd "$HERE/../../fixtures/fake-pi-compaction-stall" && pwd)/fixture-extension.ts"
mkdir -p "$project" "$home/.config/workgraph" "$home/.pi/agent" "$home/.cache"
: >"$home/.config/workgraph/config.toml"
cat >"$home/.pi/agent/settings.json" <<'JSON'
{
  "compaction": { "enabled": true, "reserveTokens": 500, "keepRecentTokens": 1 },
  "retry": { "enabled": false },
  "quietStartup": true
}
JSON

cd "$project"
git init -q
git config user.name 'WG Pi kick smoke'
git config user.email 'wg-pi-kick@test.invalid'
printf 'fixture project\n' >README.md
git add README.md
git commit -qm init

export HOME="$home"
export XDG_CONFIG_HOME="$home/.config"
export XDG_CACHE_HOME="$home/.cache"
export WG_PI_PLUGIN_FORCE_CACHE=1
export FAKE_PI_SCENARIO=threshold

wg init -m pi:fake-pi-compaction-stall:fake-long-agentic-turn --no-agency >"$scratch/init.log" 2>&1 || loud_fail "wg init failed: $(cat "$scratch/init.log")"
G="$(graph_dir_in "$project")"
wg --dir "$G" config --auto-assign false --no-reload >/dev/null 2>&1 || loud_fail "wg config failed"
mkdir -p "$G/executors"
cat >"$G/executors/pi.toml" <<TOML
[executor]
type = "pi"
command = "pi"
args = [
  "--mode", "json",
  "-p", "Complete the WG task prompt supplied on stdin.",
  "--offline", "--approve",
  "--no-skills", "--no-prompt-templates", "--no-context-files", "--no-builtin-tools",
  "--extension", "$fixture"
]

[executor.env]
FAKE_PI_SCENARIO = "threshold"
TOML

wg --dir "$G" add "Pi threshold compaction continuation" --id pi-kick-smoke \
  --model pi:fake-pi-compaction-stall:fake-long-agentic-turn --reasoning high \
  -d $'Drive one successful threshold compaction, then execute the concrete recovery turn.\n\n## Validation\n- exactly one same-process WG kick is acknowledged' >/dev/null
wg --dir "$G" publish pi-kick-smoke --only >/dev/null

before="$scratch/before.json"
wg --dir "$G" show pi-kick-smoke --json >"$before"
start_wg_daemon "$project" --max-agents 1 --no-chat-agent --force
spawn_log="$G/service/daemon.log"

agent_dir=""
for _ in $(seq 1 120); do
  for candidate in "$G/agents"/agent-*; do
    [[ -f "$candidate/metadata.json" ]] || continue
    if grep -q '"task_id": "pi-kick-smoke"' "$candidate/metadata.json"; then
      agent_dir="$candidate"
      break
    fi
  done
  [[ -n "$agent_dir" ]] && break
  sleep 0.25
done
[[ -n "$agent_dir" ]] || loud_fail "generated task wrapper/agent directory not found: $(cat "$spawn_log")"
raw="$agent_dir/raw_stream.jsonl"
for _ in $(seq 1 160); do
  if [[ -s "$raw" ]] \
    && grep -q 'FIXTURE_RECOVERY_TURN_EXECUTED' "$raw" \
    && grep -q '"type":"tool_execution_end","toolCallId":"fixture-terminal-FIXTURE_RECOVERY_TURN_EXECUTED"' "$raw"; then
    break
  fi
  sleep 0.25
done
grep -q 'FIXTURE_RECOVERY_TURN_EXECUTED' "$raw" 2>/dev/null \
  || loud_fail "RED: threshold compaction with explicit unfinished work must schedule one concrete post-compaction recovery turn (expected assistant marker FIXTURE_RECOVERY_TURN_EXECUTED after successful compaction_end(willRetry=false)); output=$(tail -120 "$agent_dir/output.log" 2>/dev/null) raw=$(tail -80 "$raw" 2>/dev/null)"

# Let acknowledgement/settlement and wrapper exit reconciliation reach disk.
for _ in $(seq 1 80); do
  state_file="$(find "$G" -path '*/pi/state.json' -type f -print -quit 2>/dev/null || true)"
  [[ -n "$state_file" ]] || { sleep 0.25; continue; }
  python3 - "$state_file" <<'PY' >/dev/null 2>&1 && break || true
import json,sys
s=json.load(open(sys.argv[1]))['state']
k=s.get('compaction_kicks',[])
assert len(k)==1 and k[0]['state'] in ('acknowledged','running','settled_after_kick','terminal_observed','terminal_abort_acknowledged')
PY
  sleep 0.25
done
state_file="$(find "$G" -path '*/pi/state.json' -type f -print -quit 2>/dev/null || true)"
[[ -n "$state_file" ]] || loud_fail "watchdog state missing"

sessions_dir="$agent_dir/pi-session"
python3 - "$raw" "$state_file" "$before" "$agent_dir/metadata.json" "$sessions_dir" <<'PY'
import json,sys,pathlib
raw_path,state_path,before_path,metadata_path,sessions_dir=sys.argv[1:]
events=[json.loads(x) for x in open(raw_path) if x.strip()]
state=json.load(open(state_path))['state']
before=json.load(open(before_path))
metadata=json.load(open(metadata_path))

def idx(pred,start=-1):
    for i,e in enumerate(events):
        if i>start and pred(e): return i
    raise AssertionError((start, events[max(0,start-3):start+8]))

def text(e):
    m=e.get('message',{})
    content=m.get('content',[])
    if isinstance(content,str): return content
    return ''.join(x.get('text','') for x in content if isinstance(x,dict) and x.get('type')=='text')

c=idx(lambda e:e.get('type')=='compaction_end' and e.get('reason')=='threshold' and not e.get('aborted') and not e.get('willRetry') and e.get('result'))
a=idx(lambda e:e.get('type')=='agent_start',c)
t=idx(lambda e:e.get('type')=='turn_start',a)
custom=idx(lambda e:e.get('type')=='message_start' and e.get('message',{}).get('role')=='custom' and e.get('message',{}).get('customType')=='wg-pi-compaction-kick',t)
show_start=idx(lambda e:e.get('type')=='tool_execution_start' and e.get('toolName')=='wg_show',custom)
show_end=idx(lambda e:e.get('type')=='tool_execution_end' and e.get('toolName')=='wg_show',show_start)
show_receipt=json.dumps(events[show_end],sort_keys=True)
assert 'pi-kick-smoke' in show_receipt and 'Validation' in show_receipt and 'exactly one same-process WG kick' in show_receipt,show_receipt
marker=idx(lambda e:e.get('type')=='message_end' and 'FIXTURE_RECOVERY_TURN_EXECUTED' in text(e) and 'FIXTURE_COMPACTED_SUMMARY_VISIBLE' in text(e) and 'FIXTURE_DURABLE_TASK_CONTRACT_VISIBLE' in text(e),show_end)
terminal_start=idx(lambda e:e.get('type')=='tool_execution_start' and e.get('toolName')=='wg_fail',marker)
terminal_end=idx(lambda e:e.get('type')=='tool_execution_end' and e.get('toolName')=='wg_fail',terminal_start)
assert not any(e.get('type')=='agent_settled' for e in events[c:terminal_end])

kicks=state.get('compaction_kicks',[])
assert len(kicks)==1,kicks
kick=kicks[0]
assert kick['state'] in ('acknowledged','running','settled_after_kick','terminal_observed','terminal_abort_acknowledged'),kick
assert state['continuation_epoch']==1,state
assert state['epochs_used']==1,state
assert state['process_epoch']==1,state
assert state['source']['generation']==before['lifecycle']['generation']==0
assert state['source']['attempt_id']==metadata['attempt_id']
assert state['source']['attempt_fence']==metadata['attempt_fence']
assert state['source']['worktree_lease_epoch']==metadata['attempt_fence']
assert state['source']['worktree_path']==metadata['worktree_path']
assert state['route']['provider']=='fake-pi-compaction-stall'
assert state['route']['model']=='fake-long-agentic-turn'
assert state['route']['reasoning']=='high'
assert state['domain_counters']=={'admission':0,'source_retry':0,'spawn_breaker':0,'provider_breaker':0,'evaluation_jobs':0,'accounting_attempts':0}
assert sum(1 for e in events if e.get('type')=='message_start' and e.get('message',{}).get('customType')=='wg-pi-compaction-kick')==1
assert sum(1 for e in events if e.get('type')=='message_end' and 'FIXTURE_RECOVERY_TURN_EXECUTED' in text(e))==1
assert sum(1 for e in events if e.get('type')=='tool_execution_start' and e.get('toolName')=='wg_show')==1
assert events[custom]['message']['details']['actionId']==kick['action_id']

entries=[]
for path in pathlib.Path(sessions_dir).glob('*.jsonl'):
    for line in path.read_text().splitlines():
        if line.strip(): entries.append(json.loads(line))
custom_entries=[e for e in entries if e.get('type')=='custom_message' and e.get('customType')=='wg-pi-compaction-kick']
assert len(custom_entries)==1,custom_entries
assert custom_entries[0]['details']['actionId']==kick['action_id']
compactions=[e for e in entries if e.get('type')=='compaction']
assert len(compactions)==1,compactions
assert custom_entries[0]['parentId']==compactions[0]['id'],(custom_entries,compactions)
print(json.dumps({'action':kick['action_id'],'occurrence':kick['occurrence_id'],'pid':state['process']['pid'],'session':state['session']['session_id']}))
PY

# Production argv proof: real wrapper, credential-free fixture first, exact
# embedded cache plugin last, and ambient discovery disabled.
grep -qF -- "'--extension' '$fixture'" "$agent_dir/run.sh" || loud_fail "fixture extension missing from generated wrapper"
grep -q -- "worksgood-pi/0.3.0/pi-worksgood/index.js" "$agent_dir/run.sh" || loud_fail "embedded compatible plugin missing from generated wrapper"
grep -q -- " -ne" "$agent_dir/run.sh" || loud_fail "ambient Pi extension discovery was not disabled"
[[ "$(find "$G/agents" -mindepth 1 -maxdepth 1 -type d | wc -l)" -eq 1 ]] || loud_fail "kick spawned a second process/agent owner"

# Existing diagnostics expose bounded IDs/states but no prompt/summary bytes,
# and the recovery turn must have produced an accepted protocol terminal.
status_json="$scratch/status.json"
show_json="$scratch/show.json"
wg --dir "$G" --json pi-watchdog status pi-kick-smoke >"$status_json" \
  || loud_fail "pi-watchdog status failed after accepted recovery terminal"
wg --dir "$G" show pi-kick-smoke --json >"$show_json" \
  || loud_fail "wg show failed after accepted recovery terminal"
python3 - "$status_json" "$show_json" <<'PY'
import json,sys
status=json.load(open(sys.argv[1]))
show=json.load(open(sys.argv[2]))
state=status.get('state',status)
kicks=state.get('compaction_kicks',[])
assert len(kicks)==1,kicks
assert kicks[0]['state'] in ('acknowledged_terminal_race','terminal_observed','terminal_abort_acknowledged'),kicks[0]
assert show['status']=='failed',show['status']
attempt=show['lifecycle'].get('current_attempt')
assert attempt and attempt.get('disposition')=='failed',attempt
assert any(e.get('event_kind')=='attempt-failed' for e in show['lifecycle'].get('audit',[]))
watchdog=show.get('pi_watchdog')
assert watchdog and len(watchdog.get('compaction_kicks',[]))==1,watchdog
auth=show['lifecycle'].get('pi_continuation')
assert auth and kicks[0]['route_snapshot_digest']==auth['route_snapshot_digest'],(kicks[0],auth)
PY
! grep -q 'UNFINISHED_WORK_STATE' "$status_json" || loud_fail "watchdog diagnostics leaked compaction summary"
! grep -q 'UNFINISHED_WORK_STATE' "$show_json" || loud_fail "wg show diagnostics leaked compaction summary"

# Hardening phase: a recovery run itself compacts before settlement. Each
# distinct persisted entry receives one independent action/epoch in the same
# process/session; neither the first action nor replay creates a third kick.
wg --dir "$G" service stop >/dev/null 2>&1 || true
project2="$scratch/two-project"
home2="$scratch/two-home"
mkdir -p "$project2" "$home2/.config/workgraph" "$home2/.pi/agent" "$home2/.cache"
: >"$home2/.config/workgraph/config.toml"
cp "$home/.pi/agent/settings.json" "$home2/.pi/agent/settings.json"
cd "$project2"
git init -q
git config user.name 'WG Pi double-kick smoke'
git config user.email 'wg-pi-double-kick@test.invalid'
printf 'double fixture project\n' >README.md
git add README.md
git commit -qm init
export HOME="$home2"
export XDG_CONFIG_HOME="$home2/.config"
export XDG_CACHE_HOME="$home2/.cache"
export FAKE_PI_SCENARIO=threshold-twice
wg init -m pi:fake-pi-compaction-stall:fake-long-agentic-turn --no-agency >"$scratch/init-two.log" 2>&1 \
  || loud_fail "second-phase wg init failed: $(cat "$scratch/init-two.log")"
G2="$(graph_dir_in "$project2")"
wg --dir "$G2" config --auto-assign false --no-reload >/dev/null 2>&1 || loud_fail "second-phase wg config failed"
mkdir -p "$G2/executors"
cat >"$G2/executors/pi.toml" <<TOML
[executor]
type = "pi"
command = "pi"
args = [
  "--mode", "json",
  "-p", "Complete the WG task prompt supplied on stdin.",
  "--offline", "--approve",
  "--no-skills", "--no-prompt-templates", "--no-context-files", "--no-builtin-tools",
  "--extension", "$fixture"
]

[executor.env]
FAKE_PI_SCENARIO = "threshold-twice"
TOML
wg --dir "$G2" add "Pi two successive threshold compactions" --id pi-kick-smoke-two \
  --model pi:fake-pi-compaction-stall:fake-long-agentic-turn --reasoning high \
  -d $'Drive two distinct successful threshold compactions in one exact process/session.\n\n## Validation\n- exactly two distinct WG kicks are acknowledged' >/dev/null
wg --dir "$G2" publish pi-kick-smoke-two --only >/dev/null
start_wg_daemon "$project2" --max-agents 1 --no-chat-agent --force
agent_dir2=""
for _ in $(seq 1 120); do
  for candidate in "$G2/agents"/agent-*; do
    [[ -f "$candidate/metadata.json" ]] || continue
    if grep -q '"task_id": "pi-kick-smoke-two"' "$candidate/metadata.json"; then
      agent_dir2="$candidate"
      break
    fi
  done
  [[ -n "$agent_dir2" ]] && break
  sleep 0.25
done
[[ -n "$agent_dir2" ]] || loud_fail "second-phase generated wrapper missing"
raw2="$agent_dir2/raw_stream.jsonl"
for _ in $(seq 1 200); do
  if [[ -s "$raw2" ]] \
    && grep -q 'FIXTURE_RECOVERY_TURN_2_EXECUTED' "$raw2" \
    && grep -q '"type":"tool_execution_end","toolCallId":"fixture-terminal-FIXTURE_RECOVERY_TURN_2_EXECUTED"' "$raw2"; then
    break
  fi
  sleep 0.25
done
grep -q 'FIXTURE_RECOVERY_TURN_2_EXECUTED' "$raw2" 2>/dev/null \
  || loud_fail "two-occurrence recovery did not reach its second concrete turn: $(tail -100 "$agent_dir2/output.log" 2>/dev/null)"
for _ in $(seq 1 80); do
  state2="$(find "$G2" -path '*/pi/state.json' -type f -print -quit 2>/dev/null || true)"
  [[ -n "$state2" ]] || { sleep 0.25; continue; }
  python3 - "$state2" <<'PY' >/dev/null 2>&1 && break || true
import json,sys
s=json.load(open(sys.argv[1]))['state']
assert len(s.get('compaction_kicks',[]))==2 and s['compaction_kicks'][-1]['state'] in ('acknowledged_terminal_race','terminal_observed','terminal_abort_acknowledged')
PY
  sleep 0.25
done
state2="$(find "$G2" -path '*/pi/state.json' -type f -print -quit 2>/dev/null || true)"
[[ -n "$state2" ]] || loud_fail "second-phase watchdog state missing"
show2="$scratch/show-two.json"
wg --dir "$G2" show pi-kick-smoke-two --json >"$show2" \
  || loud_fail "second-phase wg show failed after accepted recovery terminal"
python3 - "$raw2" "$state2" "$agent_dir2/pi-session" "$show2" <<'PY'
import json,sys,pathlib
raw_path,state_path,sessions_dir,show_path=sys.argv[1:]
events=[json.loads(x) for x in open(raw_path) if x.strip()]
state=json.load(open(state_path))['state']
show=json.load(open(show_path))

def text(e):
    content=e.get('message',{}).get('content',[])
    if isinstance(content,str): return content
    return ''.join(x.get('text','') for x in content if isinstance(x,dict) and x.get('type')=='text')

def indices(pred): return [i for i,e in enumerate(events) if pred(e)]
comp=indices(lambda e:e.get('type')=='compaction_end' and e.get('reason')=='threshold' and not e.get('aborted') and not e.get('willRetry') and e.get('result'))
custom=indices(lambda e:e.get('type')=='message_start' and e.get('message',{}).get('role')=='custom' and e.get('message',{}).get('customType')=='wg-pi-compaction-kick')
mark1=indices(lambda e:e.get('type')=='message_end' and 'FIXTURE_RECOVERY_TURN_1_EXECUTED' in text(e) and 'FIXTURE_COMPACTED_SUMMARY_VISIBLE' in text(e) and 'FIXTURE_DURABLE_TASK_CONTRACT_VISIBLE' in text(e))
mark2=indices(lambda e:e.get('type')=='message_end' and 'FIXTURE_RECOVERY_TURN_2_EXECUTED' in text(e) and 'FIXTURE_COMPACTED_SUMMARY_VISIBLE' in text(e) and 'FIXTURE_DURABLE_TASK_CONTRACT_VISIBLE' in text(e))
shows=indices(lambda e:e.get('type')=='tool_execution_end' and e.get('toolName')=='wg_show')
terminals=indices(lambda e:e.get('type')=='tool_execution_start' and e.get('toolName')=='wg_fail')
terminal_ends=indices(lambda e:e.get('type')=='tool_execution_end' and e.get('toolName')=='wg_fail')
assert len(comp)==2 and len(custom)==2 and len(mark1)==1 and len(mark2)==1 and len(shows)==2 and len(terminals)==len(terminal_ends)==1,(comp,custom,mark1,mark2,shows,terminals,terminal_ends)
assert comp[0] < custom[0] < shows[0] < mark1[0] < comp[1] < custom[1] < shows[1] < mark2[0] < terminals[0] < terminal_ends[0]
assert not any(e.get('type')=='agent_settled' for e in events[comp[0]:terminal_ends[0]])
for show_index in shows:
    receipt=json.dumps(events[show_index],sort_keys=True)
    assert 'pi-kick-smoke-two' in receipt and 'Validation' in receipt and 'exactly two distinct WG kicks' in receipt,receipt
kicks=state.get('compaction_kicks',[])
assert len(kicks)==2,kicks
assert len({k['occurrence_id'] for k in kicks})==2
assert len({k['action_id'] for k in kicks})==2
assert state['continuation_epoch']==2,state
assert state['epochs_used']==2,state
assert state['process_epoch']==1,state
assert state['domain_counters']=={'admission':0,'source_retry':0,'spawn_breaker':0,'provider_breaker':0,'evaluation_jobs':0,'accounting_attempts':0}
assert kicks[-1]['state'] in ('acknowledged_terminal_race','terminal_observed','terminal_abort_acknowledged'),kicks[-1]
assert show['status']=='failed',show['status']
attempt=show['lifecycle'].get('current_attempt')
assert attempt and attempt.get('disposition')=='failed',attempt
assert any(e.get('event_kind')=='attempt-failed' for e in show['lifecycle'].get('audit',[]))
auth=show['lifecycle'].get('pi_continuation')
assert auth and all(k['route_snapshot_digest']==auth['route_snapshot_digest'] for k in kicks),(kicks,auth)
assert [events[i]['message']['details']['actionId'] for i in custom]==[k['action_id'] for k in kicks]
entries=[]
for path in pathlib.Path(sessions_dir).glob('*.jsonl'):
    entries += [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
comp_entries=[e for e in entries if e.get('type')=='compaction']
custom_entries=[e for e in entries if e.get('type')=='custom_message' and e.get('customType')=='wg-pi-compaction-kick']
assert len(comp_entries)==len(custom_entries)==2,(comp_entries,custom_entries)
assert [e['id'] for e in comp_entries]==[k['compaction_entry_id'] for k in kicks]
assert [e['details']['actionId'] for e in custom_entries]==[k['action_id'] for k in kicks]
assert custom_entries[0]['parentId']==comp_entries[0]['id']
assert custom_entries[1]['parentId']==comp_entries[1]['id']
print(json.dumps({'actions':[k['action_id'] for k in kicks],'pid':state['process']['pid'],'session':state['session']['session_id']}))
PY
[[ "$(find "$G2/agents" -mindepth 1 -maxdepth 1 -type d | wc -l)" -eq 1 ]] \
  || loud_fail "two qualifying occurrences spawned a second process/agent owner"

echo "PASS: installed wg wrapper + embedded plugin delivered one kick for one occurrence and two distinct kicks for two same-process occurrences, each ending in one accepted WG terminal receipt"
