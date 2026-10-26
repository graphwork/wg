#!/usr/bin/env bash
# Scenario: federation_spark_two_graphs (WG-Fed Wave 3 — the spark PoC)
#
# The seven-step "two graphs, one key, a third location" spark test
# (docs/federation-study/06-decision-memo-and-roadmap.md §4.2), the empirical
# proof that the WG-Fed identity/key/transport choice (ADR-fed-001..004) is
# buildable and correct. Each step is a falsifiable assertion:
#
#   1. Mint a self-certifying identity — root key NEVER leaves the wg-secret
#      keystore (not in the IdentityRecord, not in any worker-reachable file).
#   2. Publish IdentityRecord + a StateSnapshot to a dumb, untrusted location L —
#      the published bytes carry NO private key.
#   3. Fetch + verify OFFLINE by wgid alone, with the origin (WG-A) down; flipping
#      any byte of the fetched record makes verification fail.
#   4. Send a signed + sealed cross-graph SignedEvent to an offline recipient
#      (store-and-forward inbox at L).
#   5. Receive + authenticate by key: the genuine message verifies and decrypts;
#      a forged "from Bob" event (wrong signature) FAILS.
#   6. THE HEADLINE — downloaded-identity != impersonation: a thief who has
#      Alice's published bundle cannot author a new event as Alice, and cannot
#      decrypt a message sealed to Alice.
#   7. Re-fetch from L by a fresh third party and re-verify — portability +
#      self-certifying verification independent of origin, WG-A still offline.
#
# Models WG-A / WG-B / attacker / fourth-party as four isolated $HOME keystores
# (the custody boundary is $HOME-relative) + project dirs. L is a shared, dumb
# directory — explicitly untrusted; every artifact it serves is self-verifying.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

command -v python3 >/dev/null 2>&1 || loud_skip "MISSING python3" "needed for JSON assertions"

scratch=$(make_scratch)
L="$scratch/L" # the dumb, untrusted third location

# Per-actor isolated HOME (=> isolated wg-secret keystore) + project dir (=> --dir).
A_HOME="$scratch/A_home"; A_DIR="$scratch/A/.wg"
B_HOME="$scratch/B_home"; B_DIR="$scratch/B/.wg"
M_HOME="$scratch/M_home"; M_DIR="$scratch/M/.wg" # the attacker / thief
C_HOME="$scratch/C_home"; C_DIR="$scratch/C/.wg" # a fresh fourth party
mkdir -p "$L" "$A_DIR" "$B_DIR" "$M_DIR" "$C_DIR" \
    "$A_HOME/.config" "$B_HOME/.config" "$M_HOME/.config" "$C_HOME/.config"

# Run wg as a given actor: isolated HOME (keystore) + explicit --dir (local state).
wgrun() { # wgrun <home> <wgdir> args...
    local home="$1" wgdir="$2"
    shift 2
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH \
        HOME="$home" XDG_CONFIG_HOME="$home/.config" \
        wg --dir "$wgdir" "$@"
}

# Sanitize a wgid/cid to its on-disk leaf name (must match the Rust `sanitize`).
san() { printf '%s' "$1" | sed 's/[^A-Za-z0-9._-]/_/g'; }

jfield() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }

# ───────────────────────────────────────────────────────────────────────────────
# STEP 1 — Mint a self-certifying identity (root into wg secret).
# ───────────────────────────────────────────────────────────────────────────────
new_out="$scratch/alice_new.json"
wgrun "$A_HOME" "$A_DIR" --json identity new alice >"$new_out" 2>"$scratch/alice_new.err" ||
    loud_fail "wg identity new alice failed: $(cat "$scratch/alice_new.err")"
ALICE_WGID=$(jfield "['wgid']" <"$new_out")
case "$ALICE_WGID" in
    wgid:z6Mk*) ;;
    *) loud_fail "minted wgid not a did:key-family ed25519 address: $ALICE_WGID" ;;
esac

# The keystore must hold the root (and signer/enc) private keys for alice.
ks="$A_HOME/.wg/keystore"
ls "$ks"/wgfed.alice.* >/dev/null 2>&1 ||
    loud_fail "no wgfed.alice.* keys in the keystore — root was not stored in custody"

# Collect the private-key hex values from the keystore for the leak scan below.
secret_hexes=()
for f in "$ks"/wgfed.alice.*; do
    v=$(cat "$f")       # e.g. "ed25519:<64hex>" / "x25519:<64hex>"
    secret_hexes+=("${v#*:}")
done
[ "${#secret_hexes[@]}" -ge 2 ] ||
    loud_fail "expected >=2 private keys (root+signer+enc) in custody, found ${#secret_hexes[@]}"

# The `identity new` output must not leak any private key material.
for h in "${secret_hexes[@]}"; do
    grep -qF "$h" "$new_out" &&
        loud_fail "STEP 1 FAILED: a private key leaked into 'wg identity new' output"
done
grep -qE "ed25519:|x25519:" "$new_out" &&
    loud_fail "STEP 1 FAILED: a custody-tagged private key appears in 'wg identity new' output"

echo "STEP 1 ok: minted $ALICE_WGID; root key held in custody, absent from output"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 2 — Publish to the dumb location L; published bytes carry no private key.
# ───────────────────────────────────────────────────────────────────────────────
wgrun "$A_HOME" "$A_DIR" identity publish alice --store "$L" >"$scratch/publish.log" 2>&1 ||
    loud_fail "wg identity publish failed: $(cat "$scratch/publish.log")"
[ -d "$L/objects" ] && [ -d "$L/heads" ] ||
    loud_fail "publish did not populate L/objects + L/heads"

# Scan EVERY published byte (all of L) AND the local identity record for any
# private key material — neither the bundle nor the local public state may carry
# it (ADR-fed-003 §D1, FR-S1).
local_rec="$A_DIR/identity/alice.json"
scan_targets=$(find "$L" -type f; echo "$local_rec")
for h in "${secret_hexes[@]}"; do
    while IFS= read -r tgt; do
        [ -f "$tgt" ] || continue
        grep -qF "$h" "$tgt" &&
            loud_fail "STEP 2 FAILED: private key leaked into published/local bytes ($tgt)"
    done <<<"$scan_targets"
done
# Spec-check: the published IdentityRecord must not contain private-key tags/fields.
head_file="$L/heads/$(san "$ALICE_WGID")"
[ -f "$head_file" ] || loud_fail "no head pointer for alice at L"
record_cid=$(jfield "['record']" <"$head_file")
record_obj="$L/objects/$(san "$record_cid")"
[ -f "$record_obj" ] || loud_fail "record object missing at L"
grep -qE "ed25519:|x25519:|\"seed\"|private" "$record_obj" &&
    loud_fail "STEP 2 FAILED: published IdentityRecord contains private-key material"

echo "STEP 2 ok: published self-verifying bundle to L; no private key in any byte"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 3 — Fetch + verify OFFLINE by wgid (WG-A is down — no A process); flipping
#          any byte of the fetched record makes verification fail.
# ───────────────────────────────────────────────────────────────────────────────
fetch_out="$scratch/bob_fetch.json"
wgrun "$B_HOME" "$B_DIR" --json identity fetch "$ALICE_WGID" --store "$L" --save alice \
    >"$fetch_out" 2>"$scratch/bob_fetch.err" ||
    loud_fail "WG-B failed to fetch+verify alice offline: $(cat "$scratch/bob_fetch.err")"
[ "$(jfield "['verified']" <"$fetch_out")" = "True" ] ||
    loud_fail "STEP 3 FAILED: offline verification did not pass"

# Flip one byte of the published record object, then re-fetch — must FAIL.
cp "$record_obj" "$scratch/record.bak"
python3 - "$record_obj" <<'PY'
import sys
p = sys.argv[1]
b = bytearray(open(p, "rb").read())
b[len(b) // 2] ^= 0x01
open(p, "wb").write(b)
PY
if wgrun "$C_HOME" "$C_DIR" identity fetch "$ALICE_WGID" --store "$L" >/dev/null 2>&1; then
    loud_fail "STEP 3 FAILED: verification PASSED on a byte-flipped record (must fail)"
fi
cp "$scratch/record.bak" "$record_obj" # restore so later steps work

echo "STEP 3 ok: offline self-verify passes; a flipped byte is rejected"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 4 — Send a signed + sealed cross-graph event to an OFFLINE recipient.
# ───────────────────────────────────────────────────────────────────────────────
wgrun "$B_HOME" "$B_DIR" --json identity new bob >"$scratch/bob_new.json" 2>&1 ||
    loud_fail "wg identity new bob failed"
BOB_WGID=$(jfield "['wgid']" <"$scratch/bob_new.json")
wgrun "$B_HOME" "$B_DIR" identity publish bob --store "$L" >/dev/null 2>&1 ||
    loud_fail "publish bob failed"

SECRET_MSG="hello alice from bob"
send_out="$scratch/send.json"
wgrun "$B_HOME" "$B_DIR" --json identity send --from bob --to "$ALICE_WGID" \
    --store "$L" --body "$SECRET_MSG" --seal >"$send_out" 2>"$scratch/send.err" ||
    loud_fail "WG-B failed to send sealed event to offline alice: $(cat "$scratch/send.err")"
[ "$(jfield "['accepted']" <"$send_out")" = "True" ] ||
    loud_fail "STEP 4 FAILED: sealed event was not accepted for delivery"
[ "$(jfield "['sealed']" <"$send_out")" = "True" ] ||
    loud_fail "STEP 4 FAILED: event was not sealed"

echo "STEP 4 ok: signed+sealed event accepted for store-and-forward to offline alice"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 5 — Receive + authenticate by key; a forged "from Bob" event FAILS.
# ───────────────────────────────────────────────────────────────────────────────
poll1="$scratch/poll1.json"
wgrun "$A_HOME" "$A_DIR" --json identity poll alice --store "$L" >"$poll1" 2>&1 ||
    loud_fail "WG-A poll failed"
[ "$(jfield "['accepted']" <"$poll1")" = "1" ] ||
    loud_fail "STEP 5 FAILED: genuine event did not verify (accepted != 1)"
[ "$(jfield "['rejected']" <"$poll1")" = "0" ] ||
    loud_fail "STEP 5 FAILED: genuine poll had unexpected rejections"
got_body=$(jfield "['events'][0]['body']" <"$poll1")
[ "$got_body" = "$SECRET_MSG" ] ||
    loud_fail "STEP 5 FAILED: sealed body did not decrypt to the expected message (got: $got_body)"
got_from=$(jfield "['events'][0]['from']" <"$poll1")
[ "$got_from" = "$BOB_WGID" ] ||
    loud_fail "STEP 5 FAILED: authenticated sender mismatch (got: $got_from)"

# Forge a "from Bob" event: a DIFFERENT identity (mallory) authors a real event,
# then we rewrite its `from` to Bob. Mallory's signature does not verify against
# Bob's authorized key set, so it must be REJECTED.
wgrun "$M_HOME" "$M_DIR" --json identity new mallory >"$scratch/mallory_new.json" 2>&1 ||
    loud_fail "wg identity new mallory failed"
MALLORY_WGID=$(jfield "['wgid']" <"$scratch/mallory_new.json")
wgrun "$M_HOME" "$M_DIR" identity publish mallory --store "$L" >/dev/null 2>&1 ||
    loud_fail "publish mallory failed"
wgrun "$M_HOME" "$M_DIR" identity send --from mallory --to "$ALICE_WGID" \
    --store "$L" --body "i am totally bob" >/dev/null 2>&1 ||
    loud_fail "mallory failed to send her own (genuine) event"

inbox="$L/inbox/$(san "$ALICE_WGID")"
# Find mallory's event (from == mallory) and forge its `from` to Bob.
forged="$inbox/forged_from_bob.json"
python3 - "$inbox" "$MALLORY_WGID" "$BOB_WGID" "$forged" <<'PY'
import glob, json, os, sys
inbox, mallory, bob, out = sys.argv[1:5]
for p in glob.glob(os.path.join(inbox, "*.json")):
    ev = json.load(open(p))
    if ev.get("from") == mallory:
        ev["from"] = bob  # forge: claim to be Bob, signature unchanged (mallory's)
        json.dump(ev, open(out, "w"))
        sys.exit(0)
sys.exit("could not find mallory's event to forge")
PY
[ -f "$forged" ] || loud_fail "STEP 5 FAILED: could not stage the forged 'from Bob' event"

poll2="$scratch/poll2.json"
wgrun "$A_HOME" "$A_DIR" --json identity poll alice --store "$L" >"$poll2" 2>&1 ||
    loud_fail "WG-A second poll failed"
[ "$(jfield "['rejected']" <"$poll2")" -ge 1 ] ||
    loud_fail "STEP 5 FAILED: forged 'from Bob' event was NOT rejected"

echo "STEP 5 ok: genuine event verified + decrypted; forged 'from Bob' rejected"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 6 — THE HEADLINE: downloaded-identity != impersonation.
# ───────────────────────────────────────────────────────────────────────────────
# Mallory (the thief) downloads Alice's published bundle from L.
wgrun "$M_HOME" "$M_DIR" identity fetch "$ALICE_WGID" --store "$L" --save alice \
    >/dev/null 2>"$scratch/thief_fetch.err" ||
    loud_fail "thief failed to fetch alice's public bundle: $(cat "$scratch/thief_fetch.err")"

# 6a. Thief tries to AUTHOR a new event AS Alice — MUST FAIL (no signer key in
#     the thief's custody; the bundle confers no signing ability).
imp_err="$scratch/impersonate.err"
if wgrun "$M_HOME" "$M_DIR" identity send --from alice --to "$BOB_WGID" \
    --store "$L" --body "this is alice, send funds" >"$scratch/impersonate.out" 2>"$imp_err"; then
    loud_fail "STEP 6 FAILED (CRITICAL): the thief authored an event AS Alice — impersonation succeeded!"
fi
grep -qiE "impersonation|custody|no signer|private key" "$imp_err" ||
    loud_fail "STEP 6: impersonation was blocked but without a custody error. stderr:
$(cat "$imp_err")"

# 6b. Thief holds the ciphertext (it is in L) + Alice's public bundle, but cannot
#     DECRYPT the message sealed to Alice — only Alice's enc key can.
thief_poll="$scratch/thief_poll.json"
wgrun "$M_HOME" "$M_DIR" --json identity poll alice --store "$L" >"$thief_poll" 2>&1 || true
# The sealed-to-Alice event must NOT be readable by the thief (rejected at open).
[ "$(jfield "['rejected']" <"$thief_poll")" -ge 1 ] ||
    loud_fail "STEP 6 FAILED: the thief was able to read a message sealed to Alice"

echo "STEP 6 ok: thief CANNOT author as Alice and CANNOT decrypt Alice's sealed message"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 7 — Re-fetch from L by a FRESH fourth party, WG-A still offline.
# ───────────────────────────────────────────────────────────────────────────────
c_fetch="$scratch/carol_fetch.json"
wgrun "$C_HOME" "$C_DIR" --json identity fetch "$ALICE_WGID" --store "$L" \
    >"$c_fetch" 2>"$scratch/carol_fetch.err" ||
    loud_fail "STEP 7 FAILED: fresh party could not re-verify alice from L: $(cat "$scratch/carol_fetch.err")"
[ "$(jfield "['verified']" <"$c_fetch")" = "True" ] ||
    loud_fail "STEP 7 FAILED: fresh-party re-verification did not pass"
[ "$(jfield "['offline']" <"$c_fetch")" = "True" ] ||
    loud_fail "STEP 7 FAILED: re-verification was not purely offline/self-certifying"

echo "STEP 7 ok: fresh fourth party re-verified alice from L alone, origin offline"

echo "PASS: WG-Fed spark — all 7 steps hold (root in custody, offline self-verify, forged-from rejected, impersonation + sealed-read denied)"
exit 0
