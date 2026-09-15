#!/usr/bin/env bash
# Controlled Pi fixture for the opt-in immutable opaque assignment experiment.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# Keep the daemon's AF_UNIX socket below sun_path even when the caller's
# TMPDIR is a deep build-cache path. The harness still owns and registers it.
export WG_SMOKE_ROOT=/tmp/wgsmoke
. "$HERE/_helpers.sh"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"
: "${WG_BIN:?smoke harness must provide candidate WG_BIN}"
[[ -x "$WG_BIN" ]] || loud_fail "candidate WG_BIN is not executable: $WG_BIN"

scratch=$(make_scratch)
project="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"; calls="$scratch/pi-calls"
mkdir -p "$project" "$home" "$fakebin" "$calls"
cat >"$fakebin/pi" <<'FAKE_PI'
#!/usr/bin/env bash
set -euo pipefail
n=$(find "${OPAQUE_PI_CALLS:?}" -type f 2>/dev/null | wc -l | tr -d ' ')
n=$((n+1)); printf '%s\0' "$@" >"$OPAQUE_PI_CALLS/$n.argv"
printf '%s' "$PWD" >"$OPAQUE_PI_CALLS/$n.cwd"
printf '%s' "${PI_CODING_AGENT_DIR:-}" >"$OPAQUE_PI_CALLS/$n.config-root"
route=''; previous=''
for arg in "$@"; do [[ "$previous" == --model || "$previous" == --list-models ]] && route="$arg"; previous="$arg"; done
if [[ " $* " == *' --offline '* ]]; then
  case "$route" in
    missing:*) echo 'fixture Pi: configured selection is unavailable' >&2; exit 2 ;;
    transient:*) echo 'fixture Pi: registry temporarily busy' >&2; exit 75 ;;
    empty:*) printf 'Provider Model Context Window Max Output\n'; exit 0 ;;
    *) printf 'Provider Model Context Window Max Output\nfixture exact 1 1\n'; exit 0 ;;
  esac
fi
: >"$OPAQUE_PI_CALLS/worker-started"
# Stay alive so the scenario can exercise daemon restart and explicit cancellation.
trap 'exit 0' TERM INT
sleep 60 & wait
FAKE_PI
chmod +x "$fakebin/pi"

export HOME="$home" WG_GLOBAL_DIR="$home/.wg" PATH="$fakebin:$PATH" OPAQUE_PI_CALLS="$calls"
export WG_EXPERIMENTAL_OPAQUE_ASSIGNMENT=1
unset WG_TASK_ID WG_AGENT_ID WG_TIER WG_EXECUTOR_TYPE WG_MODEL WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH WG_PI_PROCESS_WAKE_EXTENSION
cd "$project"
git init -q -b main; git config user.email opaque@test.invalid; git config user.name Opaque
printf 'base\n' >base.txt; git add base.txt; git commit -qm base
"$WG_BIN" init --no-agency >/dev/null
G="$project/.wg"; wgrun(){ (cd "$project" && "$WG_BIN" --dir "$G" "$@"); }
cleanup(){ wgrun service stop --force --kill-agents >/dev/null 2>&1 || true; }
add_cleanup_hook cleanup
wgrun config --local --model 'pi:openai-codex/gpt-5.6-sol' --reasoning high \
  --auto-assign false --auto-evaluate false --no-reload >/dev/null
wgrun config set dispatcher.worktree_isolation false >/dev/null
wgrun config set dispatcher.settling_delay_ms 0 >/dev/null
wgrun add 'Opaque assignment runtime' --id opaque-run >/dev/null
wgrun publish opaque-run --only >/dev/null
wgrun service start --max-agents 1 --no-coordinator-agent --no-supervise >/dev/null

assignment=''
for _ in $(seq 1 200); do
  assignment=$(find "$G/attempts" -path '*/execution-assignment/assignment.json' -type f 2>/dev/null | head -1 || true)
  [[ -n "$assignment" && -f "$calls/worker-started" ]] && break
  sleep .05
done
[[ -n "$assignment" ]] || loud_fail "daemon did not persist an immutable assignment: $(tail -80 "$G/service/daemon.log" 2>/dev/null || true)"
python3 - "$assignment" "$fakebin/pi" "$project" <<'PY'
import json,sys,os
x=json.load(open(sys.argv[1]))
assert x['task_id']=='opaque-run',x
assert x['execution']['kind']=='pi',x
assert x['execution']['opaque_route']=='openai-codex/gpt-5.6-sol',x
assert x['execution']['reasoning']=='high',x
assert x['execution']['program']==os.path.realpath(sys.argv[2]),x
plan=x['execution']['launch_plan']
assert plan['fixed_argv'][:2]==['--mode','json'],plan
assert plan['config_identity'] and plan['tool_policy'],plan
assert plan['working_directory_policy']=='attempt_workspace',plan
assert plan['capability_cwd']==sys.argv[3] and plan['execution_cwd']==sys.argv[3],plan
assert x['execution_cwd']==sys.argv[3],x
assert x['authored_route']=='pi:openai-codex/gpt-5.6-sol',x
assert x['attempt_id'].startswith('attempt-'),x
assert x['attempt_fence']>0 and x['runtime_agent_id'].startswith('agent-'),x
assert x['config_revision']!='unversioned',x
PY
worker_argv=$(python3 - "$calls" <<'PY'
import glob,sys
for p in glob.glob(sys.argv[1]+'/*.argv'):
 a=open(p,'rb').read().split(b'\0')
 if b'--offline' not in a:
  print(p); break
PY
)
[[ -n "$worker_argv" ]] || loud_fail "fake Pi never received worker launch"
python3 - "$worker_argv" "$assignment" <<'PY'
import json,os,sys
a=[x.decode() for x in open(sys.argv[1],'rb').read().split(b'\0') if x]
plan=json.load(open(sys.argv[2]))['execution']['launch_plan']
assert '--provider' not in a,a
i=a.index('--model'); assert a[i+1]=='openai-codex/gpt-5.6-sol',a
assert a.count('openai-codex/gpt-5.6-sol')==1,a
j=a.index('--thinking'); assert a[j+1]=='high',a
p=a.index('-p'); assert a[p+1]=='Complete the WG task prompt supplied on stdin.',a
assert '--prompt' not in a,a
assert '-ne' in a and '-e' in a,a
e=a.index('-e'); assert a[e+1]==plan['wg_extension']['path'],(a,plan)
stem=os.path.splitext(sys.argv[1])[0]
assert open(stem+'.cwd').read()==plan['execution_cwd'],plan
assert open(stem+'.config-root').read()==plan['config_root'],plan
PY
before=$(sha256sum "$assignment" | cut -d' ' -f1)
# Mutation and daemon restart apply to new assignments only; the running attempt bytes stay pinned.
wgrun config --local --model 'pi:changed:new-route' --reasoning low --no-reload >/dev/null
wgrun service stop --force >/dev/null 2>&1 || true
wgrun service start --max-agents 1 --no-coordinator-agent --no-supervise >/dev/null
sleep .3
after=$(sha256sum "$assignment" | cut -d' ' -f1)
[[ "$before" == "$after" ]] || loud_fail "restart/config mutation rewrote running assignment"
agent=$(python3 - "$assignment" <<'PY'
import json,sys; print(json.load(open(sys.argv[1]))['runtime_agent_id'])
PY
)
wgrun agents kill "$agent" --force >/dev/null 2>&1 || loud_fail "explicit cancellation failed for $agent"
[[ "$(sha256sum "$assignment" | cut -d' ' -f1)" == "$before" ]] \
  || loud_fail "cancellation rewrote immutable assignment evidence"

# Pi itself, not WG name heuristics, rejects deterministic, successful-empty,
# and transient capability results.
for pair in 'missing-case pi:missing:unknown' 'empty-case pi:empty:no-row' 'transient-case pi:transient:busy' 'legacy-case codex:gpt-historic'; do
  set -- $pair; id=$1; route=$2
  wgrun add "$id" --id "$id" --model "$route" --reasoning low >/dev/null
  wgrun publish "$id" --only >/dev/null
done
sleep 2
for id in missing-case empty-case transient-case legacy-case; do
  details=$(wgrun show "$id" --json)
  python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["status"]=="open",x; assert x.get("retry_count",0)==0,x; assert x.get("lifecycle",{}).get("current_attempt") is None,x' <<<"$details"
done
grep -q 'opaque assignment admission blocked: error\[WG-OPAQUE-LEGACY-ACTIVE\]' "$G/service/daemon.log" \
  || loud_fail "legacy active route did not report explicit migration refusal"
grep -q 'opaque Pi preflight missing_required_capability: fixture Pi: configured selection is unavailable' "$G/service/daemon.log" \
  || loud_fail "missing-capability classification was not surfaced"
grep -q 'opaque Pi preflight missing_required_capability: Pi capability query returned 0 rows for opaque route "empty:no-row"' "$G/service/daemon.log" \
  || loud_fail "successful-empty Pi query was not refused"
grep -q 'opaque Pi preflight transient_failure: fixture Pi: registry temporarily busy' "$G/service/daemon.log" \
  || loud_fail "transient preflight classification was not surfaced"
transient_calls(){ python3 - "$calls" <<'PY'
import glob,sys
print(sum(b'transient:busy' in open(p,'rb').read() for p in glob.glob(sys.argv[1]+'/*.argv')))
PY
}
transient_before=$(transient_calls)
[[ "$transient_before" == 1 ]] || loud_fail "transient route probed $transient_before times before backoff"
wgrun service stop --force >/dev/null 2>&1 || true
wgrun service start --max-agents 1 --no-coordinator-agent --no-supervise >/dev/null
sleep 1
[[ "$(transient_calls)" == "$transient_before" ]] \
  || loud_fail "daemon restart bypassed persisted transient preflight backoff"

# Shell remains an explicit assignment, not a covert fallback for rejected Pi or remote work.
wgrun add 'Explicit shell assignment' --id shell-ok --exec "printf shell-ok > '$scratch/shell-ok'" >/dev/null
wgrun publish shell-ok --only >/dev/null
for _ in $(seq 1 100); do [[ -f "$scratch/shell-ok" ]] && break; sleep .05; done
[[ -f "$scratch/shell-ok" ]] || loud_fail "explicit shell assignment did not launch"
shell_assignment=$(find "$G/attempts" -path '*/execution-assignment/assignment.json' -type f -print0 | \
  xargs -0 grep -l '"task_id": "shell-ok"' | head -1 || true)
[[ -n "$shell_assignment" ]] || loud_fail "shell assignment evidence missing"
python3 - "$shell_assignment" "$project" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); e=x['execution']
assert e['kind']=='shell',x
assert e['argv'][0:2]==['bash','-c'] and 'printf shell-ok' in e['argv'][2],x
assert e['environment']=={'TASK_ID':'shell-ok','TASK_TITLE':'Explicit shell assignment'},x
assert e['working_directory']==sys.argv[2],x
PY

echo 'PASS: controlled Pi fixture preserved opaque assignment through preflight, launch, restart, cancellation and fail-closed admission'
