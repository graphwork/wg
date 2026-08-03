#!/usr/bin/env bash
# Crash the candidate terminal adapter after every durable v2 boundary and replay.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v cargo >/dev/null || loud_skip "MISSING CARGO" "candidate build requires cargo"
ROOT=$(git -C "$HERE" rev-parse --show-toplevel) || loud_fail "cannot find repository root"
(cd "$ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg) || loud_fail "candidate build failed"
WG_BIN="$ROOT/target/debug/wg"; export PATH="$(dirname "$WG_BIN"):$PATH"
unset WG_AGENT_ID WG_TASK_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_PROJECT_ROOT WG_WORKTREE_PATH WG_DIR WG_BRANCH
scratch=$(make_scratch); project="$scratch/project"; home="$scratch/home"; mkdir -p "$project" "$home"
cd "$project"; git init -q -b main; git config user.email crash-replay@test.invalid; git config user.name CrashReplay
printf 'immutable base\n' >payload; git add payload; git commit -qm base
HOME="$home" XDG_CONFIG_HOME="$home/.config" "$WG_BIN" init --no-agency >/dev/null
wgrun(){ env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_DIR="$project/.wg" "$WG_BIN" "$@"; }
phases=(Prepared Quiescing WorkSaved CandidateSealed Validated Accepted DispositionRecorded EffectPrepared EffectCommitted CleanupPrepared CleanupCommitted GraphSaved)
for phase in "${phases[@]}"; do
  id="cut-$(tr '[:upper:]' '[:lower:]' <<<"$phase")"
  wgrun add "$id" --id "$id" -d $'fault injection fixture\n\n## Validation\n- exact replay' >/dev/null
  wgrun claim "$id" >/dev/null
  set +e
  WG_TEST_SAVE_CRASH_AFTER="$phase" wgrun done "$id" --skip-smoke >"$scratch/$id.out" 2>"$scratch/$id.err"
  rc=$?
  set -e
  [[ $rc -ne 0 ]] || loud_fail "$phase injection did not cut the candidate"
  grep -q "injected terminal SaveTransaction crash after $phase" "$scratch/$id.err" \
    || loud_fail "$phase cut was not the requested boundary: $(cat "$scratch/$id.err")"
  [[ $(wgrun show "$id" --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["status"])') != done ]] \
    || loud_fail "$phase cut falsely satisfied the task"
  head=$(find "$project/.wg/completion/v2/transactions" -name head.json -type f | xargs grep -l "\"task_id\": \"$id\"" | head -1)
  [[ -n $head ]] || loud_fail "$phase cut lost its durable transaction"
  python3 - "$head" "$phase" <<'PY'
import json,sys
j=json.load(open(sys.argv[1])); expected=sys.argv[2]
wire={"WorkSaved":"work-saved","CandidateSealed":"candidate-sealed","DispositionRecorded":"disposition-recorded","EffectPrepared":"effect-prepared","EffectCommitted":"effect-committed","CleanupPrepared":"cleanup-prepared","CleanupCommitted":"cleanup-committed","GraphSaved":"graph-saved"}.get(expected,expected.lower())
assert j["phase"]==wire,(j["phase"],wire)
PY
  before=$(git rev-parse HEAD)
  wgrun done "$id" --skip-smoke >/dev/null
  [[ $(git rev-parse HEAD) == "$before" ]] || loud_fail "$phase replay moved Git unexpectedly"
  wgrun show "$id" --json | python3 -c 'import json,sys;j=json.load(sys.stdin);assert j["status"]=="done"; assert sum(e.get("event_kind")=="graph-save-committed" for e in j["lifecycle"]["audit"])==1' \
    || loud_fail "$phase replay did not commit exactly one GraphSave"
done
(cd "$ROOT" && cargo test --quiet --test atomic_save_faults) \
  || loud_fail "table-driven crash/target/skew/cleanup faults failed"
echo "PASS: every candidate durable boundary crashed before Done and replayed to one GraphSave with stable Git; target movement/skew/cleanup cuts held safely"
