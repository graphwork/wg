#!/usr/bin/env bash
# Installed-binary, human-visible candidate finalization transaction.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
scratch="$(make_scratch)"; project="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"; wrapper_sync="$scratch/wrapper-sync"
mkdir -p "$project/incident" "$home/.config/workgraph" "$fakebin" "$wrapper_sync"; : >"$home/.config/workgraph/config.toml"
cat >"$fakebin/pi" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
cat >/dev/null || true
mkdir -p incident
python3 - <<'PY'
from pathlib import Path
Path('incident/wrapper-payload.txt').write_bytes(b'w'*28672)
Path('wrapper-untracked.txt').write_text('wrapper rescue source\n')
PY
printf '%s\n' "$PWD" >"${WRAPPER_SYNC:?}/worktree"
sleep .4
# This is only terminal intent while this exact Pi process can still write.
wg done "$WG_TASK_ID" >/dev/null
printf '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"complete"}],"usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"total":0}}}}\n'
SH
chmod +x "$fakebin/pi"
(cd "$project" && git init -q -b main && git config user.email finalizer@test.invalid && git config user.name Finalizer && python3 - <<'PY'
from pathlib import Path
Path('incident/payload.txt').write_bytes(b'm'*6144)
PY
git add incident/payload.txt && git commit -qm base && env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH -u WG_BRANCH HOME="$home" XDG_CONFIG_HOME="$home/.config" wg init --no-agency >/dev/null)
wgrun(){ (cd "$project" && env -u WG_AGENT_ID -u WG_TASK_ID WG_DIR="$project/.wg" HOME="$home" XDG_CONFIG_HOME="$home/.config" wg "$@"); }
new_attempt(){
  local id=$1 wt="$scratch/$1-wt" branch="wg/finalizer/$1"
  wgrun add "$id" --id "$id" -d $'Candidate transaction fixture.\n\n## Validation\n- exact immutable candidate binding' >/dev/null
  wgrun claim "$id" >/dev/null
  (cd "$project" && git worktree add -q -b "$branch" "$wt")
  wgrun pi-watchdog fixture-init "$id" --worktree "$wt" --now 0 >/dev/null
  printf '%s|%s' "$wt" "$branch"
}

# Historical 28KB/6KB no-push incident. The old false-stall observation does
# not touch main; same-session terminal intent + exact process-exit receipt then
# hand the immutable bytes to the finalizer.
IFS='|' read -r wt branch <<<"$(new_attempt incident)"
wgrun pi-watchdog fixture-observe incident --event provider-start --now 0 >/dev/null
wgrun pi-watchdog fixture-tick incident --now 300 >/dev/null
[[ $(wc -c <"$project/incident/payload.txt") -eq 6144 ]] || loud_fail "main changed during suspect window"
python3 - "$wt" <<'PY'
from pathlib import Path
import sys
p=Path(sys.argv[1]); (p/'incident/payload.txt').write_bytes(b'c'*28672); (p/'untracked.txt').write_text('rescued untracked\n')
PY
wgrun pi-watchdog fixture-observe incident --event token --now 301 >/dev/null
wgrun pi-watchdog fixture-observe incident --event done --now 302 >/dev/null
wgrun pi-watchdog process-exit incident --exit-code 0 >/dev/null
# Human terminal flow (through installed wg); no worker push exists or is needed.
out=$(cd "$wt" && HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_DIR="$project/.wg" WG_WORKTREE_PATH="$wt" WG_BRANCH="$branch" WG_PROJECT_ROOT="$project" WG_EXECUTOR_TYPE=pi WG_HANDLER_QUIESCENT=1 env -u WG_AGENT_ID -u WG_TASK_ID wg done incident --skip-smoke 2>&1)
grep -q '\[finish\] task-owned Promoted: candidate=wgcid:.* durable=wgcid:' <<<"$out" || loud_fail "task-owned binding receipt not visible: $out"
[[ $(wc -c <"$project/incident/payload.txt") -eq 28672 ]] || loud_fail "6KB main substituted for 28KB candidate"
# The normal wrapper performs this from outside cwd; this fixture models that
# post-process half explicitly.
wgrun finish cleanup incident >/dev/null
status=$(wgrun finalize status incident)
grep -q 'Finalization Cleaned' <<<"$status" || loud_fail "cleaned projection missing: $status"
grep -q 'validation: .*binding=' <<<"$status" || loud_fail "validation binding missing: $status"
cid=$(wgrun finalize status incident --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["candidate"]["candidate_id"])')
material="$scratch/materialized"; wgrun candidate materialize "$cid" --to "$material" >/dev/null
[[ $(wc -c <"$material/incident/payload.txt") -eq 28672 ]] || loud_fail "candidate materialization changed"
receipt1=$(wgrun finalize status incident --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["merge_receipt"]["receipt_id"])')
wgrun finalize reconcile incident >/dev/null
receipt2=$(wgrun finalize status incident --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["merge_receipt"]["receipt_id"])')
[[ $receipt1 == "$receipt2" ]] || loud_fail "duplicate reconcile charged a second merge"

# Explicit useful-WIP fail gets a rescue and no candidate correctness claim.
IFS='|' read -r fwt fbranch <<<"$(new_attempt useful-fail)"
printf 'valuable wip\n' >"$fwt/failure-wip.txt"
wgrun pi-watchdog fixture-observe useful-fail --event fail --now 10 >/dev/null
wgrun pi-watchdog process-exit useful-fail --exit-code 9 >/dev/null
(cd "$fwt" && HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_DIR="$project/.wg" WG_WORKTREE_PATH="$fwt" WG_BRANCH="$fbranch" WG_PROJECT_ROOT="$project" WG_EXECUTOR_TYPE=pi WG_HANDLER_QUIESCENT=1 env -u WG_AGENT_ID -u WG_TASK_ID wg finalize settle useful-fail >/dev/null)
fstatus=$(wgrun finalize status useful-fail)
grep -q 'Finalization FailedPreserved' <<<"$fstatus" || loud_fail "failure rescue missing: $fstatus"
grep -q 'rescue: wgcid:' <<<"$fstatus" || loud_fail "failure rescue CID hidden"

# Main movement after candidate checkpoint is an explicit retained repair,
# never an implicit take-main resolution.
IFS='|' read -r cwt cbranch <<<"$(new_attempt conflict)"
python3 - "$cwt" <<'PY'
from pathlib import Path
import sys
Path(sys.argv[1],'incident/payload.txt').write_bytes(b'x'*28672)
PY
wgrun finalize checkpoint conflict --worktree "$cwt" --quiescence-receipt receipt:conflict >/dev/null
(cd "$project" && python3 - <<'PY'
from pathlib import Path
Path('incident/payload.txt').write_bytes(b'z'*6144)
PY
git add incident/payload.txt && git commit -qm 'main moved')
wgrun finalize reconcile conflict >/dev/null
conflict=$(wgrun finalize status conflict)
grep -q 'Finalization RepairNeeded' <<<"$conflict" || loud_fail "conflict not retained: $conflict"
grep -q 'merge.target_moved' <<<"$conflict" || loud_fail "conflict reason absent"
grep -q 'evaluation: request=.*policy=required.*binding=wgcid:.*read-only=true' <<<"$conflict" || loud_fail "candidate-bound read-only evaluation handoff absent: $conflict"
[[ $(wc -c <"$project/incident/payload.txt") -eq 6144 ]] || loud_fail "conflict overwrote moved main"

# Fixture and real spawned attempts may all use task-local `attempt-0-1`;
# authoritative tuple namespaces keep their watchdog/observer evidence apart.

# Actual daemon + generated wrapper + isolated worktree path. The Pi tool call
# reserves intent while alive; only the wrapper's post-wait process-exit/settle
# path may checkpoint and merge. No push command exists in the fake provider.
wgrun config --local --model pi:openrouter:test/model --auto-assign false --auto-evaluate false --flip-enabled false --no-reload >/dev/null
wgrun add wrapper-flow --id wrapper-flow --model pi:openrouter:test/model -d $'Real daemon/wrapper finalization.\n\n## Validation\n- immutable candidate binding' >/dev/null
wgrun publish wrapper-flow --only >/dev/null
(cd "$project" && env WG_DIR="$project/.wg" HOME="$home" XDG_CONFIG_HOME="$home/.config" PATH="$fakebin:$PATH" WRAPPER_SYNC="$wrapper_sync" OPENROUTER_API_KEY=fake wg service start --max-agents 1 --model pi:openrouter:test/model --no-coordinator-agent --no-supervise >/dev/null)
for _ in $(seq 1 160); do
  s=$(wgrun show wrapper-flow --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["status"])')
  [[ $s == done ]] && break
  sleep .1
done
[[ $s == done ]] || loud_fail "real wrapper did not finalize: $(wgrun show wrapper-flow)"
wrapper_wt=$(cat "$wrapper_sync/worktree")
[[ $wrapper_wt == *'.wg-worktrees/'* ]] || loud_fail "wrapper lacked isolated worktree"
[[ $(wc -c <"$project/incident/wrapper-payload.txt") -eq 28672 ]] || loud_fail "wrapper candidate not integrated"
wstatus=$(wgrun finalize status wrapper-flow)
grep -q 'Finalization Cleaned' <<<"$wstatus" || loud_fail "wrapper finalization cleanup receipt missing: $wstatus"
wgrun service stop >/dev/null 2>&1 || true

# Real daemon restart over durable object/ref/journal boundaries does not alter
# descriptors or receipts.
wgrun service start --no-chat-agent --force >/dev/null 2>&1 || true
sleep .3; wgrun service stop >/dev/null 2>&1 || true
wgrun service start --no-chat-agent --force >/dev/null 2>&1 || true
sleep .3; wgrun service stop >/dev/null 2>&1 || true
[[ $(wgrun finalize status incident --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["merge_receipt"]["receipt_id"])') == "$receipt1" ]] || loud_fail "daemon restart drifted merge receipt"

echo "candidate finalization transaction passed"
