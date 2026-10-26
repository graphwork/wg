#!/usr/bin/env bash
# Scenario: exec_ledger_crash_safe (audit B3 — the lease-ledger integrity backstop)
#
# The WG-Exec epoch fence is only as sound as the persistence of its ledger
# (`<wgdir>/exec/leases.json`). The audit B3 bug: the ledger was loaded with
# `unwrap_or_default()`, so a corrupt/partial parse SILENTLY RESET it to empty — which
# drops every task's epoch and re-opens the double-commit / replay the fence exists to
# close — and it was written with a non-atomic, unlocked `fs::write`. The fix:
#
#   * load REFUSES (errors) on a corrupt/partial parse instead of resetting to empty;
#   * the read-modify-write holds an exclusive advisory lock;
#   * the write is atomic (temp-file + fsync + rename).
#
# This pins the CLI-level behavior the unit tests cannot reach: a real `wg provider`
# invocation against a corrupt on-disk ledger must FAIL CLOSED and must NOT clobber the
# corrupt bytes with a fresh-empty ledger. Credential-free (offer/reclaim are pure
# crypto/ledger ops — no worker model is driven).

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

command -v python3 >/dev/null 2>&1 || loud_skip "MISSING python3" "needed for JSON assertions"

scratch=$(make_scratch)
L="$scratch/L" # the dumb, untrusted store the identities publish to
A_HOME="$scratch/A_home"; A_DIR="$scratch/A/.wg"   # authorizer + agent G custody
P_HOME="$scratch/P_home"; P_DIR="$scratch/P/.wg"   # the separately-owned provider
mkdir -p "$L" "$A_DIR" "$P_DIR" "$A_HOME/.config" "$P_HOME/.config"

wgrun() { # wgrun <home> <wgdir> args...
    local home="$1" wgdir="$2"
    shift 2
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH \
        HOME="$home" XDG_CONFIG_HOME="$home/.config" \
        wg --dir "$wgdir" "$@"
}
wga() { wgrun "$A_HOME" "$A_DIR" "$@"; }
wgp() { wgrun "$P_HOME" "$P_DIR" "$@"; }
jfield() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }

LEDGER="$A_DIR/exec/leases.json"

# ── Setup: mint + publish the two identities; enroll P (the WG-Fed substrate) ────
wga --json identity new agentG >"$scratch/agentG.json" 2>"$scratch/agentG.err" ||
    loud_fail "identity new agentG failed: $(cat "$scratch/agentG.err")"
wga identity publish agentG --store "$L" >/dev/null 2>&1 || loud_fail "publish agentG failed"
wgp --json identity new providerP >"$scratch/providerP.json" 2>&1 || loud_fail "identity new providerP failed"
P_WGID=$(jfield "['wgid']" <"$scratch/providerP.json")
wgp identity publish providerP --store "$L" >/dev/null 2>&1 || loud_fail "publish providerP failed"
wga --json provider enroll "$P_WGID" --trust verified --model claude:opus \
    --isolation container >/dev/null 2>&1 || loud_fail "enroll provider P failed"

# ── STEP 1: an offer places task T → the ledger is created at epoch 1 ────────────
wga --json provider offer --as-name agentG --task T --model claude:opus \
    --isolation container --sensitivity normal --provider "$P_WGID" \
    --out "$scratch/offer.json" >/dev/null 2>"$scratch/offer.err" ||
    loud_fail "STEP 1: offer errored: $(cat "$scratch/offer.err")"
[ -f "$LEDGER" ] || loud_fail "STEP 1: ledger $LEDGER was not created by the offer"
python3 -c "import json,sys; d=json.load(open('$LEDGER')); assert d['tasks']['T']['epoch']==1, d" \
    2>/dev/null || loud_fail "STEP 1: ledger does not record task T at epoch 1"
echo "STEP 1 ok: offer placed task T → ledger at epoch 1"

# ── STEP 2: corrupt the ledger; a mutating command must REFUSE, not reset ────────
# Truncate to a partial JSON (the crash-mid-write / corruption shape).
printf '{ "tasks": { "T": { "epoch":' >"$LEDGER"
corrupt_before=$(cat "$LEDGER")

if wga provider reclaim --task T >"$scratch/reclaim.out" 2>"$scratch/reclaim.err"; then
    loud_fail "STEP 2: reclaim SUCCEEDED on a corrupt ledger — it must refuse (fail closed)"
fi
# The error must be the loud refuse, not a silent reset.
grep -qi "REFUSING\|corrupt" "$scratch/reclaim.err" ||
    loud_fail "STEP 2: reclaim failed but not with the loud corrupt-ledger refusal: $(cat "$scratch/reclaim.err")"

# The corrupt bytes must STILL be on disk — a reset-to-empty would have clobbered them.
corrupt_after=$(cat "$LEDGER")
[ "$corrupt_before" = "$corrupt_after" ] ||
    loud_fail "STEP 2: the corrupt ledger was overwritten (the fence reset to empty — the B3 bug)"
# And it must NOT have become a valid empty ledger.
if python3 -c "import json,sys; json.load(open('$LEDGER'))" >/dev/null 2>&1; then
    loud_fail "STEP 2: the corrupt ledger parses as valid JSON — it was reset, not refused"
fi
echo "STEP 2 ok: corrupt ledger REFUSED (fail closed); bytes preserved, never reset to empty"

# ── STEP 3: a clean ledger round-trips through the locked read-modify-write ───────
# Restore a valid ledger by re-placing the task (offer overwrites atomically), then a
# reclaim succeeds and the epoch monotonically bumps — proving the locked/atomic path
# works end-to-end via the CLI, not only the unit tests.
rm -f "$LEDGER"
wga --json provider offer --as-name agentG --task T --model claude:opus \
    --isolation container --sensitivity normal --provider "$P_WGID" \
    --out "$scratch/offer2.json" >/dev/null 2>"$scratch/offer2.err" ||
    loud_fail "STEP 3: re-offer errored: $(cat "$scratch/offer2.err")"
rout=$(wga --json provider reclaim --task T 2>"$scratch/reclaim2.err") ||
    loud_fail "STEP 3: reclaim on a clean ledger errored: $(cat "$scratch/reclaim2.err")"
[ "$(jfield "['new_epoch']" <<<"$rout")" = "2" ] ||
    loud_fail "STEP 3: reclaim did not bump the epoch to 2 (got: $rout)"
# No temp file left behind by the atomic write.
leftover=$(find "$A_DIR/exec" -name '*.tmp.*' 2>/dev/null | head -1)
[ -z "$leftover" ] || loud_fail "STEP 3: atomic write left a temp file behind: $leftover"
echo "STEP 3 ok: clean ledger reclaim bumped epoch 1→2 (locked + atomic); no temp file left"

echo "exec_ledger_crash_safe: all assertions passed"
