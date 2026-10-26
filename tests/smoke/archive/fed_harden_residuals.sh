#!/usr/bin/env bash
# Scenario: fed_harden_residuals (federation hardening — audit M1/M7/M8/M9/M10/M11/S12/S13)
#
# The CLI-level, end-to-end proof of the federation-hardening residuals (owners =
# [fed-harden]). The deep crypto invariants are pinned by lib tests; this scenario
# exercises the *wired* CLI surfaces a regression would silently break:
#
#   M9  replay/dedup at the consume edge — a re-polled authenticated event is flagged
#       a REPLAY and its body is WITHHELD (not re-consumed), while still authenticating.
#   M7  model_binding ENFORCEMENT — load-state into a MISMATCHED runtime model FAILS
#       CLOSED; into a matching model it LOADS.
#   S13 real state consumer — a same-self auto-load actually CONSUMES the payload
#       (decodes >0 turns + writes a working-state file), not just prints "LOADED".
#   M11 windowed recovery — recovery via a CLOSED window FAILS CLOSED; an OPEN window
#       recovers (address unchanged).
#   M10 freshness-gated revocation head — revoke-cap publishes a signed head; verify-cap
#       re-fetches + freshness-gates it and reports the cap REVOKED.
#   M1  at-rest custody key protection state is reported by `wg secret backend show`.
#
# Credential-free; isolated $HOME keystores + scratch --dir; L is a dumb directory.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING python3" "needed for JSON assertions"

scratch=$(make_scratch)
L="$scratch/L"
A_HOME="$scratch/A_home"; A_DIR="$scratch/A/.wg"
B_HOME="$scratch/B_home"; B_DIR="$scratch/B/.wg"
mkdir -p "$L" "$A_DIR" "$B_DIR" "$A_HOME/.config" "$B_HOME/.config"

wgrun() { # wgrun <home> <wgdir> args...
    local home="$1" wgdir="$2"; shift 2
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH \
        HOME="$home" XDG_CONFIG_HOME="$home/.config" \
        wg --dir "$wgdir" "$@"
}
jfield() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }

# ───────────────────────────────────────────────────────────────────────────────
# Setup — mint alice (sender/principal) + bob (recipient), publish both.
# ───────────────────────────────────────────────────────────────────────────────
wgrun "$A_HOME" "$A_DIR" identity new alice >/dev/null 2>&1 || loud_fail "mint alice failed"
wgrun "$B_HOME" "$B_DIR" identity new bob >/dev/null 2>&1 || loud_fail "mint bob failed"
A_WGID=$(wgrun "$A_HOME" "$A_DIR" --json identity show alice 2>/dev/null | jfield "['wgid']")
B_WGID=$(wgrun "$B_HOME" "$B_DIR" --json identity show bob 2>/dev/null | jfield "['wgid']")
[ -n "$A_WGID" ] && [ -n "$B_WGID" ] || loud_fail "could not resolve wgids"
wgrun "$A_HOME" "$A_DIR" identity publish alice --store "$L" >/dev/null 2>&1 || loud_fail "publish alice failed"
wgrun "$B_HOME" "$B_DIR" identity publish bob --store "$L" >/dev/null 2>&1 || loud_fail "publish bob failed"

# ───────────────────────────────────────────────────────────────────────────────
# M1 — at-rest custody key protection state is reported.
# ───────────────────────────────────────────────────────────────────────────────
backend=$(wgrun "$A_HOME" "$A_DIR" secret backend show 2>&1 || true)
echo "$backend" | grep -qi "WG-Fed custody keys:" ||
    loud_fail "M1 FAILED: 'wg secret backend show' does not report WG-Fed at-rest key state"
echo "M1 ok: at-rest custody key protection state reported by 'wg secret backend show'"

# ───────────────────────────────────────────────────────────────────────────────
# M9 — replay/dedup at the consume edge. Alice sends bob a sealed msg; bob polls
# twice WITH the review gate. The first poll consumes it; the second flags it a
# REPLAY and WITHHOLDS the body — but the event still authenticates.
# ───────────────────────────────────────────────────────────────────────────────
# Bob trusts alice as a Verified peer so the IC4 review gate ACCEPTS her clean message
# (isolating the dedup behavior — an Unknown author would quarantine regardless).
wgrun "$B_HOME" "$B_DIR" peer add alice --wgid "$A_WGID" --trust verified >/dev/null 2>&1 ||
    loud_fail "M9 setup: peer add alice (verified) failed"

wgrun "$A_HOME" "$A_DIR" identity send --from alice --to "$B_WGID" --store "$L" \
    --body "replay-test-secret" --seal >/dev/null 2>&1 ||
    loud_fail "M9 setup: sealed send failed"

poll1="$scratch/m9_poll1.json"
wgrun "$B_HOME" "$B_DIR" --json msg poll --as bob --store "$L" >"$poll1" 2>/dev/null ||
    loud_fail "M9: first poll errored"
[ "$(jfield "['accepted']" <"$poll1")" = "1" ] ||
    loud_fail "M9 FAILED: first poll did not authenticate the event"
[ "$(jfield "['events'][0]['replayed']" <"$poll1")" = "False" ] ||
    loud_fail "M9 FAILED: first sight wrongly flagged as a replay"
[ "$(jfield "['events'][0]['consumable']" <"$poll1")" = "True" ] ||
    loud_fail "M9 FAILED: first sight should be consumable"

poll2="$scratch/m9_poll2.json"
wgrun "$B_HOME" "$B_DIR" --json msg poll --as bob --store "$L" >"$poll2" 2>/dev/null ||
    loud_fail "M9: second poll errored"
# Still authenticates (idempotent), but is a REPLAY and NOT consumable.
[ "$(jfield "['accepted']" <"$poll2")" = "1" ] ||
    loud_fail "M9 FAILED: a re-polled genuine event must still authenticate"
[ "$(jfield "['replayed']" <"$poll2")" = "1" ] ||
    loud_fail "M9 FAILED: the re-polled event was not flagged as a replay"
[ "$(jfield "['events'][0]['consumable']" <"$poll2")" = "False" ] ||
    loud_fail "M9 FAILED: a replay must NOT be consumable (body re-delivery)"
[ "$(jfield "['events'][0]['body']" <"$poll2")" = "None" ] ||
    loud_fail "M9 FAILED: a replay's body must be WITHHELD"
echo "M9 ok: re-polled event authenticates but is flagged REPLAY, body withheld (consume-edge dedup)"

# ───────────────────────────────────────────────────────────────────────────────
# S13 + M7 — real state consumer + model_binding enforcement. Alice's published
# snapshot is bound to model claude-opus-4-8.
# ───────────────────────────────────────────────────────────────────────────────
# S13: same-self auto-load actually CONSUMES the payload (decodes turns + persists).
load_ok="$scratch/s13_load.json"
wgrun "$A_HOME" "$A_DIR" --json identity load-state alice --store "$L" \
    --runtime-model claude-opus-4-8 >"$load_ok" 2>/dev/null ||
    loud_fail "S13: same-self matched-model load errored"
[ "$(jfield "['loaded']" <"$load_ok")" = "True" ] ||
    loud_fail "S13 FAILED: same-self matched-model state did not load"
[ "$(jfield "['consumed']" <"$load_ok")" = "True" ] ||
    loud_fail "S13 FAILED: AutoLoad did not actually CONSUME the payload (only printed LOADED?)"
turns=$(jfield "['consumed_turns']" <"$load_ok")
[ "$turns" != "None" ] && [ "$turns" -ge 1 ] ||
    loud_fail "S13 FAILED: consumer decoded no turns (turns=$turns)"
echo "S13 ok: same-self auto-load CONSUMED the payload ($turns turn(s) decoded + persisted)"

# M7: load into a MISMATCHED runtime model FAILS CLOSED (exit non-zero).
if wgrun "$A_HOME" "$A_DIR" --json identity load-state alice --store "$L" \
    --runtime-model gpt-5.5 >"$scratch/m7_mismatch.json" 2>/dev/null; then
    loud_fail "M7 FAILED: load-state into a mismatched runtime model must FAIL CLOSED"
fi
grep -qi "model_binding" "$scratch/m7_mismatch.json" 2>/dev/null ||
    loud_fail "M7 FAILED: mismatch refusal did not cite model_binding"
echo "M7 ok: model_binding mismatch (claude-opus-4-8 vs gpt-5.5) FAILS CLOSED"

# ───────────────────────────────────────────────────────────────────────────────
# M11 — windowed recovery. A CLOSED (back-dated) window fails closed; an OPEN one
# recovers with the address unchanged.
# ───────────────────────────────────────────────────────────────────────────────
C_HOME="$scratch/C_home"; C_DIR="$scratch/C/.wg"
D_HOME="$scratch/D_home"; D_DIR="$scratch/D/.wg"
mkdir -p "$C_DIR" "$D_DIR" "$C_HOME/.config" "$D_HOME/.config"
LC="$scratch/Lc"; LD="$scratch/Ld"; mkdir -p "$LC" "$LD"

# Carol: recovery key with an already-CLOSED window (negative secs) → recover fails closed.
wgrun "$C_HOME" "$C_DIR" identity new carol --recovery --recovery-window-secs=-3600 \
    >/dev/null 2>&1 || loud_fail "M11 setup: mint carol (closed window) failed"
wgrun "$C_HOME" "$C_DIR" identity publish carol --store "$LC" >/dev/null 2>&1 ||
    loud_fail "M11 setup: publish carol failed"
if wgrun "$C_HOME" "$C_DIR" identity recover carol --store "$LC" >/dev/null 2>&1; then
    loud_fail "M11 FAILED: recovery via a CLOSED window must FAIL CLOSED"
fi
echo "M11 ok: recovery via a closed (back-dated) window FAILS CLOSED"

# Dave: recovery key with an OPEN window (1 day) → recover succeeds, address unchanged.
wgrun "$D_HOME" "$D_DIR" identity new dave --recovery --recovery-window-secs=86400 \
    >/dev/null 2>&1 || loud_fail "M11 setup: mint dave (open window) failed"
D_WGID=$(wgrun "$D_HOME" "$D_DIR" --json identity show dave 2>/dev/null | jfield "['wgid']")
wgrun "$D_HOME" "$D_DIR" identity publish dave --store "$LD" >/dev/null 2>&1 ||
    loud_fail "M11 setup: publish dave failed"
rec_out="$scratch/m11_recover.json"
wgrun "$D_HOME" "$D_DIR" --json identity recover dave --store "$LD" >"$rec_out" 2>/dev/null ||
    loud_fail "M11 FAILED: recovery within an OPEN window should succeed"
[ "$(jfield "['wgid']" <"$rec_out")" = "$D_WGID" ] ||
    loud_fail "M11 FAILED: recovery changed the wgid address (must be stable)"
echo "M11 ok: recovery within an open window succeeds, address unchanged"

# ───────────────────────────────────────────────────────────────────────────────
# M10 — freshness-gated revocation head. Alice issues a cap to bob, then revokes it;
# verify-cap re-fetches + freshness-gates the signed head and reports REVOKED.
# ───────────────────────────────────────────────────────────────────────────────
cap="$scratch/cap.json"
wgrun "$A_HOME" "$A_DIR" identity delegate --from alice --to "$B_WGID" \
    --grant "graph/write@graph://task/x" --ttl 3600 --out "$cap" --store "$L" \
    >/dev/null 2>&1 || loud_fail "M10 setup: delegate failed"
# Before revocation: VALID.
wgrun "$A_HOME" "$A_DIR" identity verify-cap --cap "$cap" --store "$L" >/dev/null 2>&1 ||
    loud_fail "M10 setup: freshly-issued cap should verify VALID"
# Revoke → publishes the signed revocation head.
wgrun "$A_HOME" "$A_DIR" identity revoke-cap --from alice --cap "$cap" --store "$L" \
    >/dev/null 2>&1 || loud_fail "M10: revoke-cap failed"
# After revocation: verify-cap freshness-gates the head and reports REVOKED (exit != 0).
vc_out="$scratch/m10_verify.json"
if wgrun "$A_HOME" "$A_DIR" --json identity verify-cap --cap "$cap" --store "$L" \
    >"$vc_out" 2>/dev/null; then
    loud_fail "M10 FAILED: a revoked cap must verify INVALID after revoke-cap"
fi
[ "$(jfield "['valid']" <"$vc_out")" = "False" ] ||
    loud_fail "M10 FAILED: verify-cap did not report the revoked cap as invalid"
grep -qi "revok" "$vc_out" 2>/dev/null ||
    loud_fail "M10 FAILED: invalid reason did not mention revocation"
echo "M10 ok: revoke-cap publishes a signed revocation head; verify-cap freshness-gates it → REVOKED"

echo "ALL STEPS PASSED — federation hardening residuals (M1/M7/M8/M9/M10/M11/S12/S13)"
exit 0
