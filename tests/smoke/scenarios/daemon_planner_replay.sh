#!/usr/bin/env bash
# Candidate-binary proof that service replay is deterministic and offline.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-$(command -v wg)}"
[[ -x "$WG_BIN" ]] || loud_fail "candidate WG_BIN is not executable: $WG_BIN"
unset WG_AGENT_ID WG_TASK_ID WG_EXECUTOR_TYPE WG_TIER WG_MODEL WG_WORKER_CAPABILITY WG_WORKER_IPC WG_WORKER_CONTROL_PROTOCOL WG_GRAPH_ID

scratch="$(make_scratch)"
project="$scratch/project"
home="$scratch/home"
mkdir -p "$project" "$home/.config/workgraph"
: >"$home/.config/workgraph/config.toml"
(
  cd "$project"
  git init -q -b main
  git config user.email replay@test.invalid
  git config user.name Replay
  printf 'baseline\n' >README
  git add README && git commit -qm baseline
  HOME="$home" XDG_CONFIG_HOME="$home/.config" "$WG_BIN" init --no-agency >/dev/null
)
fixture="$HERE/../../../formal/fixtures/daemon/v1/target_moved_during_finish.json"
trace="$scratch/trace.json"
python3 - "$fixture" "$trace" <<'PY'
import json,sys
fixture=json.load(open(sys.argv[1]))
fixture['trace']['ruleset']='corrected'
json.dump(fixture['trace'],open(sys.argv[2],'w'),indent=2)
PY
before="$(sha256sum "$project/.wg/graph.jsonl")"
(
  cd "$project"
  HOME="$home" XDG_CONFIG_HOME="$home/.config" "$WG_BIN" service replay "$trace" --output "$scratch/one.json" >/dev/null
  HOME="$home" XDG_CONFIG_HOME="$home/.config" "$WG_BIN" service replay "$trace" --output "$scratch/two.json" >/dev/null
)
cmp "$scratch/one.json" "$scratch/two.json" || loud_fail 'same trace produced different bytes'
[[ "$before" == "$(sha256sum "$project/.wg/graph.jsonl")" ]] || loud_fail 'offline replay mutated graph'
python3 - "$scratch/one.json" <<'PY' || loud_fail 'normalized replay report is incorrect'
import json,sys
report=json.load(open(sys.argv[1]))
assert report['trace_schema_version']==1
assert report['planner_schema_version']==1
assert len(report['steps'])==1
assert report['steps'][0]['violations']==[]
assert report['steps'][0]['effects'][0]['action']=='replan_finish'
assert report['final_state']['repaired_incidents']==['target_moved_during_finish']
PY
! grep -Eqi '(secret|token|https?://|/home/|prompt)' "$scratch/one.json" || loud_fail 'replay report leaked non-wire content'
echo 'PASS: daemon planner replay is byte-stable, redacted and side-effect free'
