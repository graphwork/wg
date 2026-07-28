#!/usr/bin/env bash
# Fake-Pi regression for attempt-0-22: an in-process continuation must not
# stale the exact current process's terminal/exit receipts or strand dirty WIP.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

scratch="$(make_scratch)"
project="$scratch/project"
home="$scratch/home"
mkdir -p "$project" "$home/.config/workgraph"
: >"$home/.config/workgraph/config.toml"
(
  cd "$project"
  git init -q -b main
  git config user.email pi-epoch@test.invalid
  git config user.name 'Pi Epoch Fence Smoke'
  printf 'baseline\n' >README
  git add README && git commit -qm baseline
  HOME="$home" XDG_CONFIG_HOME="$home/.config" wg init --no-agency >/dev/null
)
wgrun() { (cd "$project" && env -u WG_AGENT_ID -u WG_TASK_ID -u WG_TIER -u WG_EXECUTOR_TYPE HOME="$home" XDG_CONFIG_HOME="$home/.config" wg "$@"); }
new_fake() {
  local id=$1
  local wt="$scratch/$id-worktree"
  wgrun add "$id" --id "$id" -d $'Pi continuation epoch regression.\n\n## Validation\n- exact current receipt survives continuation' >/dev/null
  wgrun claim "$id" >/dev/null
  mkdir -p "$wt/src"
  wgrun pi-watchdog fixture-init "$id" --worktree "$wt" --now 0 >/dev/null
  printf 'retained dirty source for %s\n' "$id" >"$wt/src/retained.rs"
}
state_path() {
  local id=$1
  local attempt
  attempt=$(wgrun show "$id" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["lifecycle"]["current_attempt"]["id"])')
  printf '%s/pi/state.json' "$(attempt_runtime_dir "$project/.wg" "$id" "$attempt")"
}
assert_authority() {
  local id=$1 expected_terminal=$2
  wgrun show "$id" --json | python3 -c '
import json,sys
j=json.load(sys.stdin); l=j["lifecycle"]
assert j["status"] == "in-progress", j["status"]
assert l["pi_process_epoch"] == 1, l
assert l["pi_continuation_epoch"] == 1, l
assert l["pi_continuation"]["epochs_used"] == 1, l
assert (l.get("pi_terminal_reservation") is not None) == (sys.argv[1] == "yes"), l
if l.get("pi_terminal_reservation"):
    assert l["pi_terminal_reservation"]["process_epoch"] == 1, l
assert sum(e["event_kind"] == "pi-process-epoch-exited" for e in l["audit"]) == 1, l
assert all(e["new_state"] != "failed" for e in l["audit"]), l
' "$expected_terminal"
  python3 - "$(state_path "$id")" <<'PY'
import json,sys
s=json.load(open(sys.argv[1]))["state"]
assert s["schema_version"] == 2, s
assert s["process_epoch"] == 1, s
assert s["continuation_epoch"] == 1, s
assert s["epochs_used"] == 1, s
PY
  grep -q "retained dirty source for $id" "$scratch/$id-worktree/src/retained.rs" || loud_fail "$id dirty WIP was lost"
}

# Same exact fixture process authority receives an in-session continuation,
# restarts the reconciler, reserves wg_done, then exits. Duplicate exit replay
# is exact-once and never stale_process_epoch.
new_fake with-done
out=$(wgrun pi-watchdog fixture-observe with-done --event settled --now 1)
grep -q 'process_epoch=1 continuation_epoch=1' <<<"$out" || loud_fail "continuation impersonated process replacement: $out"
wgrun pi-watchdog status with-done >/dev/null
wgrun pi-watchdog fixture-observe with-done --event done --now 2 >/dev/null
current_pid=$(python3 - "$(state_path with-done)" <<'PY'
import json,sys
print(json.load(open(sys.argv[1]))["state"]["process"]["pid"])
PY
)
if (cd "$project" && env -u WG_AGENT_ID -u WG_TASK_ID -u WG_TIER HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_EXECUTOR_TYPE=pi wg pi-watchdog process-exit with-done --exit-code 0 --pid "$((current_pid + 1))" >/dev/null 2>&1); then
  loud_fail 'old/replacement PID exit impersonated current process epoch'
fi
(cd "$project" && env -u WG_AGENT_ID -u WG_TASK_ID -u WG_TIER HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_EXECUTOR_TYPE=pi wg pi-watchdog process-exit with-done --exit-code 0 --pid "$current_pid" >/dev/null)
(cd "$project" && env -u WG_AGENT_ID -u WG_TASK_ID -u WG_TIER HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_EXECUTOR_TYPE=pi wg pi-watchdog process-exit with-done --exit-code 0 --pid "$current_pid" >/dev/null)
assert_authority with-done yes

# Exit without wg_done remains exact-session completion evidence. It must not
# become Lost/Failed and retained source/session bytes remain untouched.
new_fake without-done
wgrun pi-watchdog fixture-observe without-done --event settled --now 1 >/dev/null
wgrun pi-watchdog status without-done >/dev/null
wgrun pi-watchdog process-exit without-done --exit-code 0 >/dev/null
wgrun pi-watchdog process-exit without-done --exit-code 0 >/dev/null
assert_authority without-done no

# Replay the persisted attempt-0-22 split produced by the old schema: lifecycle
# is still process 1 while watchdog/native projection falsely advanced to 2 for
# continuation 1. Reopen repairs that exact legacy shape once, then current exit
# succeeds and dirty WIP plus substantive session bytes are byte-identical.
new_fake legacy-split
wgrun pi-watchdog fixture-observe legacy-split --event settled --now 1 >/dev/null
legacy_state=$(state_path legacy-split)
legacy_session=$(python3 - "$legacy_state" <<'PY'
import json,sys
p=json.load(open(sys.argv[1]))["state"]["session"]["session_file"]
print(p)
PY
)
printf '{"type":"message","id":"substantive-retained"}\n' >>"$legacy_session"
before_wip=$(sha256sum "$scratch/legacy-split-worktree/src/retained.rs" | cut -d' ' -f1)
before_session=$(sha256sum "$legacy_session" | cut -d' ' -f1)
python3 - "$legacy_state" <<'PY'
import json,sys
p=sys.argv[1]; j=json.load(open(p)); s=j["state"]
s["schema_version"]=1
s["process_epoch"]=2
s["native_activity"]["process_epoch"]=2
open(p,"w").write(json.dumps(j,indent=2)+"\n")
PY
repair=$(wgrun pi-watchdog status legacy-split)
grep -q 'continuation-epoch=1 epoch=1' <<<"$repair" || loud_fail "legacy split did not converge: $repair"
wgrun pi-watchdog process-exit legacy-split --exit-code 0 >/dev/null
assert_authority legacy-split no
[[ $before_wip == "$(sha256sum "$scratch/legacy-split-worktree/src/retained.rs" | cut -d' ' -f1)" ]] || loud_fail 'legacy WIP bytes changed'
[[ $before_session == "$(sha256sum "$legacy_session" | cut -d' ' -f1)" ]] || loud_fail 'legacy substantive session bytes changed'

# Dead-agent/manual orphan reconciliation shares the same Pi authority and may
# not overwrite active/held/consumed finalization state with AttemptLost.
wgrun sweep >/dev/null
for id in with-done without-done legacy-split; do
  [[ $(wgrun show "$id" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])') == in-progress ]] || loud_fail "$id became Lost during dead-agent reconciliation"
done

if grep -Rqs 'stale_process_epoch' "$project/.wg"; then
  loud_fail 'current same-process terminal/exit was classified stale_process_epoch'
fi

echo 'pi continuation/exit process epoch fence passed'
