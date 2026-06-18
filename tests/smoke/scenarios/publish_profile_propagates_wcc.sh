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

echo "PASS: publish_profile_propagates_wcc"
