#!/usr/bin/env bash
# Scenario: publish_profile_propagates_wcc
#
# Feature (implement-wg-publish): `wg publish <seed> --profile <name>` pins a
# named profile onto every task in the seed's weakly-connected component —
# both the work tasks AND each work task's agency satellites
# (.assign-*/.evaluate-*) — so they route through that profile at dispatch.
# Tasks attached to the component later inherit the profile (inherit-on-attach).
# No --profile ⇒ behavior unchanged (profile stays unset).
#
# This smoke pins the end-to-end CLI wiring: stamp the WCC, verify every
# member + satellite carries the profile via `wg show --json`, verify a
# later-added child inherits it, and verify the no-profile path leaves it unset.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)

# Isolate from any user-level WG config / global profiles directory.
fake_home="$scratch/home"
mkdir -p "$fake_home/.wg"

wgdir="$scratch/proj/.wg"
mkdir -p "$scratch/proj"

run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$fake_home" \
        wg --dir "$wgdir" "$@"
}

# Read a task's `profile` field out of `wg show --json`.
task_profile() {
    run_wg show "$1" --json 2>/dev/null \
        | python3 -c "import json,sys
try:
    print(json.load(sys.stdin).get('profile') or '')
except Exception:
    print('')"
}

task_field() {
    local id="$1"
    local field="$2"
    run_wg show "$id" --json 2>/dev/null \
        | python3 -c 'import json, sys
field = sys.argv[1]
try:
    value = json.load(sys.stdin).get(field)
except Exception:
    value = None
if value is None and field == "paused":
    value = False
if isinstance(value, bool):
    print("true" if value else "false")
elif value is None:
    print("")
else:
    print(value)
' "$field"
}

set_task_status() {
    local id="$1"
    local status="$2"
    python3 - "$wgdir/graph.jsonl" "$id" "$status" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
task_id = sys.argv[2]
status = sys.argv[3]
rows = []
found = False
for line in path.read_text().splitlines():
    row = json.loads(line)
    if row.get("id") == task_id:
        row["status"] = status
        found = True
    rows.append(row)
if not found:
    raise SystemExit(f"task {task_id!r} not found")
path.write_text("".join(json.dumps(row, separators=(",", ":")) + "\n" for row in rows))
PY
}

set_task_paused() {
    local id="$1"
    local paused="$2"
    python3 - "$wgdir/graph.jsonl" "$id" "$paused" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
task_id = sys.argv[2]
paused = sys.argv[3] == "true"
rows = []
found = False
for line in path.read_text().splitlines():
    row = json.loads(line)
    if row.get("id") == task_id:
        row["paused"] = paused
        found = True
    rows.append(row)
if not found:
    raise SystemExit(f"task {task_id!r} not found")
path.write_text("".join(json.dumps(row, separators=(",", ":")) + "\n" for row in rows))
PY
}

if ! run_wg init >"$scratch/init.log" 2>&1; then
    loud_fail "wg init failed: $(tail -10 "$scratch/init.log")"
fi

# Enable agency scaffolding so .assign-*/.evaluate-* satellites are created
# at publish time (the things that must inherit the profile).
printf '[agency]\nauto_assign = true\nauto_evaluate = true\n' >>"$wgdir/config.toml"

# Build a paused WCC: research -> implement -> test-x
for spec in "research:Research X:" "implement:Implement X:research" "test-x:Test X:implement"; do
    id="${spec%%:*}"; rest="${spec#*:}"; title="${rest%%:*}"; after="${rest#*:}"
    args=(add "$title" --id "$id" --paused --allow-phantom)
    [[ -n "$after" ]] && args+=(--after "$after")
    if ! run_wg "${args[@]}" >"$scratch/add-$id.log" 2>&1; then
        loud_fail "wg add $id failed: $(tail -10 "$scratch/add-$id.log")"
    fi
done

# Publish the seed with a starter profile (`claude` always loads).
if ! run_wg publish research --profile claude >"$scratch/publish.log" 2>&1; then
    loud_fail "wg publish --profile failed: $(tail -10 "$scratch/publish.log")"
fi

# (1) Every work task in the WCC carries the profile.
for id in research implement test-x; do
    got="$(task_profile "$id")"
    if [[ "$got" != "claude" ]]; then
        loud_fail "work task '$id' should have profile 'claude', got '$got'"
    fi
done

# (2) Each work task's agency satellites inherit the profile.
for sat in .assign-research .evaluate-research .assign-implement .evaluate-implement; do
    got="$(task_profile "$sat")"
    if [[ "$got" != "claude" ]]; then
        loud_fail "agency satellite '$sat' should inherit profile 'claude', got '$got'"
    fi
done

# (3) wg show prints the Profile line (human-readable surface).
if ! run_wg show research 2>&1 | grep -q "Profile: claude"; then
    loud_fail "wg show research should display 'Profile: claude'"
fi

# (4) Inherit-on-attach: a task added --after a profiled member inherits it.
if ! run_wg add 'Followup' --id followup --after test-x >"$scratch/add-followup.log" 2>&1; then
    loud_fail "wg add followup failed: $(tail -10 "$scratch/add-followup.log")"
fi
got="$(task_profile followup)"
if [[ "$got" != "claude" ]]; then
    loud_fail "task added --after a profiled member should inherit 'claude', got '$got'"
fi

# (5) No --profile ⇒ behavior unchanged: a fresh unrelated paused task that is
# published without --profile keeps profile unset.
if ! run_wg add 'Solo' --id solo --paused >"$scratch/add-solo.log" 2>&1; then
    loud_fail "wg add solo failed: $(tail -10 "$scratch/add-solo.log")"
fi
if ! run_wg publish solo --only >"$scratch/publish-solo.log" 2>&1; then
    loud_fail "wg publish solo --only failed: $(tail -10 "$scratch/publish-solo.log")"
fi
got="$(task_profile solo)"
if [[ -n "$got" ]]; then
    loud_fail "publish without --profile should leave profile unset, got '$got'"
fi

# (6) Recovery workflow: reload/stamp a named profile over an already-created
# mixed-status WCC without releasing anything. This pins fix-profile-wcc-reload:
# the root is not paused, members include failed/paused/done, and all carry
# stale explicit Claude model pins that must be cleared so the codex profile
# becomes authoritative.
if ! run_wg publish --help 2>&1 | grep -q "wg publish <TASK> --profile codex --no-release --wcc"; then
    loud_fail "wg publish --help should advertise the profile reload recovery workflow"
fi

if ! run_wg add 'Assign Recover Root' --id .assign-recover-root --model claude:haiku >"$scratch/add-recover-assign.log" 2>&1; then
    loud_fail "wg add .assign-recover-root failed: $(tail -10 "$scratch/add-recover-assign.log")"
fi
if ! run_wg add 'Recover Root' --id recover-root --after .assign-recover-root --allow-phantom --model claude:opus >"$scratch/add-recover-root.log" 2>&1; then
    loud_fail "wg add recover-root failed: $(tail -10 "$scratch/add-recover-root.log")"
fi
if ! run_wg add 'Recover Failed' --id recover-failed --after recover-root --model claude:opus >"$scratch/add-recover-failed.log" 2>&1; then
    loud_fail "wg add recover-failed failed: $(tail -10 "$scratch/add-recover-failed.log")"
fi
if ! run_wg add 'Recover Paused' --id recover-paused --after recover-failed --paused --model claude:opus >"$scratch/add-recover-paused.log" 2>&1; then
    loud_fail "wg add recover-paused failed: $(tail -10 "$scratch/add-recover-paused.log")"
fi
if ! run_wg add 'Recover Done' --id recover-done --after recover-paused --model claude:opus >"$scratch/add-recover-done.log" 2>&1; then
    loud_fail "wg add recover-done failed: $(tail -10 "$scratch/add-recover-done.log")"
fi
if ! run_wg add 'FLIP Recover Root' --id .flip-recover-root --after recover-root --model claude:haiku >"$scratch/add-recover-flip.log" 2>&1; then
    loud_fail "wg add .flip-recover-root failed: $(tail -10 "$scratch/add-recover-flip.log")"
fi
if ! run_wg add 'Evaluate Recover Root' --id .evaluate-recover-root --after .flip-recover-root --model claude:haiku >"$scratch/add-recover-eval.log" 2>&1; then
    loud_fail "wg add .evaluate-recover-root failed: $(tail -10 "$scratch/add-recover-eval.log")"
fi
if ! run_wg add 'Verify Recover Root' --id .verify-recover-root --after recover-failed --model claude:haiku >"$scratch/add-recover-verify.log" 2>&1; then
    loud_fail "wg add .verify-recover-root failed: $(tail -10 "$scratch/add-recover-verify.log")"
fi
set_task_status recover-failed failed
set_task_status recover-done done
for id in recover-root recover-failed recover-done .assign-recover-root .flip-recover-root .evaluate-recover-root .verify-recover-root; do
    set_task_paused "$id" false
done
set_task_paused recover-paused true

if ! run_wg publish recover-root --profile codex --no-release --wcc >"$scratch/reload-codex.log" 2>&1; then
    loud_fail "wg publish recover-root --profile codex --no-release --wcc failed: $(tail -20 "$scratch/reload-codex.log")"
fi

for id in recover-root recover-failed recover-paused recover-done .assign-recover-root .flip-recover-root .evaluate-recover-root .verify-recover-root; do
    got="$(task_profile "$id")"
    if [[ "$got" != "codex" ]]; then
        loud_fail "recovery task '$id' should have profile 'codex', got '$got'"
    fi
    model="$(task_field "$id" model)"
    if [[ -n "$model" ]]; then
        loud_fail "recovery task '$id' should have stale model cleared, got '$model'"
    fi
done

for spec in \
    "recover-root:open:false" \
    "recover-failed:failed:false" \
    "recover-paused:open:true" \
    "recover-done:done:false"; do
    id="${spec%%:*}"; rest="${spec#*:}"; want_status="${rest%%:*}"; want_paused="${rest#*:}"
    got_status="$(task_field "$id" status)"
    got_paused="$(task_field "$id" paused)"
    if [[ "$got_status" != "$want_status" || "$got_paused" != "$want_paused" ]]; then
        loud_fail "recovery task '$id' status/paused changed: got status=$got_status paused=$got_paused, want status=$want_status paused=$want_paused"
    fi
done

echo "PASS: publish_profile_propagates_wcc"
