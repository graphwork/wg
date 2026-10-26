#!/usr/bin/env bash
# Scenario: federation_acl_ucan_delegation (WG-Fed Wave 6 — encryption=ACL + UCAN
# delegation; docs/federation-study/06 §5 Wave 6, ADR-fed-003 §D2/§D3, HQ4).
#
# Wave 6 completes WG-Fed: confidentiality realized as ACLs (R24) + structural,
# attenuating-only capability delegation, with the leash kept slack-by-default
# (Erik's §D2 trust-default amendment). Each step is a falsifiable assertion over the
# real `wg identity` CLI, isolated $HOME keystores (custody is $HOME-relative), and a
# dumb, untrusted third location L. Parties: dave (sender), alice (recipient #1 +
# UCAN principal), bob (recipient #2 + UCAN delegate/agent), carol (third party +
# UCAN sub-delegate).
#
#   A. ENCRYPTION = ACL: dave seals one message to the SET {alice, bob} — both
#      decrypt the same body; a third party (carol) handed the raw ciphertext CANNOT.
#   B. SEALED-SENDER: dave sends sealed-sender to bob — the stored event's outer
#      `from` is anonymized (the relay learns nothing) yet bob authenticates the real
#      author from inside the seal.
#   C. UCAN: issue a broad root capability (leash slack by default) → verify VALID;
#      attenuate to a sub-resource → VALID; a WIDENING sub-delegation is REFUSED;
#      an already-expired capability FAILS CLOSED; revoking a parent kills the
#      delegated subtree; and an environment leash policy TIGHTENS the same issue.
#   D. CUSTODY ≠ AUTHORITY: a downloaded (key-less) bundle cannot issue a capability.
#
# The transport (L) is untrusted: every published byte is signed/CAS and self-verifying.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

command -v python3 >/dev/null 2>&1 || loud_skip "MISSING python3" "needed for JSON assertions"

scratch=$(make_scratch)
L="$scratch/L" # the dumb, untrusted third location

D_HOME="$scratch/D_home"; D_DIR="$scratch/D/.wg" # dave  (sender)
A_HOME="$scratch/A_home"; A_DIR="$scratch/A/.wg" # alice (recipient #1 + UCAN principal)
B_HOME="$scratch/B_home"; B_DIR="$scratch/B/.wg" # bob   (recipient #2 + UCAN delegate)
C_HOME="$scratch/C_home"; C_DIR="$scratch/C/.wg" # carol (third party + UCAN sub-delegate)
mkdir -p "$L" "$D_DIR" "$A_DIR" "$B_DIR" "$C_DIR" \
    "$D_HOME/.config" "$A_HOME/.config" "$B_HOME/.config" "$C_HOME/.config"

wgrun() { # wgrun <home> <wgdir> args...
    local home="$1" wgdir="$2"
    shift 2
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH \
        HOME="$home" XDG_CONFIG_HOME="$home/.config" \
        wg --dir "$wgdir" "$@"
}
san() { printf '%s' "$1" | sed 's/[^A-Za-z0-9._-]/_/g'; }
jfield() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }

mint() { # mint <home> <wgdir> <name> -> echoes wgid
    local home="$1" wgdir="$2" name="$3"
    wgrun "$home" "$wgdir" --json identity new "$name" >"$scratch/${name}_new.json" \
        2>"$scratch/${name}_new.err" || loud_fail "wg identity new $name failed: $(cat "$scratch/${name}_new.err")"
    wgrun "$home" "$wgdir" identity publish "$name" --store "$L" >"$scratch/${name}_pub.log" 2>&1 ||
        loud_fail "publish $name failed: $(cat "$scratch/${name}_pub.log")"
    jfield "['wgid']" <"$scratch/${name}_new.json"
}

DAVE=$(mint "$D_HOME" "$D_DIR" dave)
ALICE=$(mint "$A_HOME" "$A_DIR" alice)
BOB=$(mint "$B_HOME" "$B_DIR" bob)
CAROL=$(mint "$C_HOME" "$C_DIR" carol)
for w in "$DAVE" "$ALICE" "$BOB" "$CAROL"; do
    case "$w" in wgid:z6Mk*) ;; *) loud_fail "minted wgid not ed25519: $w" ;; esac
done
echo "setup ok: minted+published dave/alice/bob/carol to L"

# ───────────────────────────────────────────────────────────────────────────────
# PART A — Encryption = ACL: seal to the SET {alice, bob}; only they decrypt.
# ───────────────────────────────────────────────────────────────────────────────
SECRET="acl-secret-$(san "$BOB" | tail -c 8)"
send_out="$scratch/send_acl.json"
wgrun "$D_HOME" "$D_DIR" --json identity send --from dave \
    --to "$ALICE" --to "$BOB" --store "$L" --body "$SECRET" --seal \
    >"$send_out" 2>"$scratch/send_acl.err" ||
    loud_fail "PART A: multi-recipient sealed send failed: $(cat "$scratch/send_acl.err")"
[ "$(jfield "['sealed']" <"$send_out")" = "True" ] || loud_fail "PART A: event not sealed"
[ "$(jfield "['recipients']" <"$send_out")" = "2" ] || loud_fail "PART A: recipient count != 2"

# alice and bob each decrypt the SAME body.
for who in "A_HOME:$A_DIR:alice" "B_HOME:$B_DIR:bob"; do
    home="${who%%:*}"; rest="${who#*:}"; dir="${rest%%:*}"; name="${rest#*:}"
    eval "home=\$$home"
    out="$scratch/poll_${name}.json"
    wgrun "$home" "$dir" --json identity poll "$name" --store "$L" >"$out" 2>"$scratch/poll_${name}.err" ||
        loud_fail "PART A: $name poll failed: $(cat "$scratch/poll_${name}.err")"
    [ "$(jfield "['accepted']" <"$out")" = "1" ] || loud_fail "PART A: $name did not accept exactly 1 event"
    got=$(jfield "['events'][0]['body']" <"$out")
    [ "$got" = "$SECRET" ] || loud_fail "PART A: $name decrypted wrong body (got: $got)"
done

# A third party with the raw ciphertext CANNOT decrypt. Copy the sealed event from
# alice's inbox into carol's inbox and let carol poll — she holds no recipient key.
alice_inbox="$L/inbox/$(san "$ALICE")"
evt=$(ls "$alice_inbox"/*.json | head -1)
[ -n "$evt" ] || loud_fail "PART A: no sealed event found in alice's inbox"
mkdir -p "$L/inbox/$(san "$CAROL")"
cp "$evt" "$L/inbox/$(san "$CAROL")/"
carol_poll="$scratch/poll_carol.json"
wgrun "$C_HOME" "$C_DIR" --json identity poll carol --store "$L" >"$carol_poll" 2>"$scratch/poll_carol.err" ||
    loud_fail "PART A: carol poll errored unexpectedly: $(cat "$scratch/poll_carol.err")"
[ "$(jfield "['accepted']" <"$carol_poll")" = "0" ] ||
    loud_fail "PART A FAILED (CRITICAL): a third party decrypted a message it was not in the ACL for"
[ "$(jfield "['rejected']" <"$carol_poll")" -ge 1 ] ||
    loud_fail "PART A: carol's poll did not reject the ciphertext she is not addressed in"
echo "PART A ok: {alice,bob} both decrypt; carol (ciphertext, no key) is locked out — the to-set IS the ACL"

# ───────────────────────────────────────────────────────────────────────────────
# PART B — Sealed-sender: hide `from` from the relay; recipient still authenticates.
# ───────────────────────────────────────────────────────────────────────────────
wgrun "$D_HOME" "$D_DIR" --json identity send --from dave --to "$BOB" \
    --store "$L" --body "ss-secret" --sealed-sender \
    >"$scratch/send_ss.json" 2>"$scratch/send_ss.err" ||
    loud_fail "PART B: sealed-sender send failed: $(cat "$scratch/send_ss.err")"
[ "$(jfield "['from']" <"$scratch/send_ss.json")" = "wgid:anon" ] ||
    loud_fail "PART B: sealed-sender outer from is not anonymized"

# The stored event must NOT reveal dave's real wgid (it is sealed inside the payload).
ss_evt=$(ls -t "$L/inbox/$(san "$BOB")"/*.json | head -1)
python3 -c "import json,sys; d=json.load(open('$ss_evt')); sys.exit(0 if d['from']=='wgid:anon' and d.get('enc_multi',{}).get('sealed_sender') else 1)" ||
    loud_fail "PART B: stored event is not a sealed-sender envelope"
grep -qF "$DAVE" "$ss_evt" &&
    loud_fail "PART B FAILED: the real sender wgid leaked into the stored (relay-visible) event"

# bob polls → authenticates the REAL author from inside the seal.
bob_ss="$scratch/poll_bob_ss.json"
wgrun "$B_HOME" "$B_DIR" --json identity poll bob --store "$L" >"$bob_ss" 2>"$scratch/poll_bob_ss.err" ||
    loud_fail "PART B: bob poll failed: $(cat "$scratch/poll_bob_ss.err")"
python3 - "$bob_ss" "$DAVE" <<'PY' || loud_fail "PART B: bob did not authenticate the sealed-sender author"
import json,sys
d=json.load(open(sys.argv[1])); dave=sys.argv[2]
ev=[e for e in d["events"] if e.get("verdict")=="VERIFIED" and e.get("from")==dave and e.get("body")=="ss-secret"]
assert ev, f"no verified sealed-sender event from {dave}: {d['events']}"
PY
echo "PART B ok: relay sees only wgid:anon; bob recovers + authenticates the real author (dave)"

# ───────────────────────────────────────────────────────────────────────────────
# PART C — UCAN: issue / verify / attenuate / expire / revoke + the leash dial.
# ───────────────────────────────────────────────────────────────────────────────
# C1. Issue a BROAD root capability (no --grant) → leash slack by default.
root_cap="$scratch/root.cap.json"
issue_out="$scratch/issue_root.json"
wgrun "$A_HOME" "$A_DIR" --json identity delegate --from alice --to "$BOB" \
    --out "$root_cap" >"$issue_out" 2>"$scratch/issue_root.err" ||
    loud_fail "C1: issue root capability failed: $(cat "$scratch/issue_root.err")"
[ "$(jfield "['leash_slack']" <"$issue_out")" = "True" ] ||
    loud_fail "C1 FAILED: the birth default is not slack (the §D2 amendment is not honored)"
[ "$(jfield "['aud']" <"$issue_out")" = "$BOB" ] || loud_fail "C1: wrong audience"
# Verify it offline.
vc_out="$scratch/verify_root.json"
wgrun "$C_HOME" "$C_DIR" --json identity verify-cap --cap "$root_cap" --store "$L" \
    >"$vc_out" 2>"$scratch/verify_root.err" ||
    loud_fail "C1: verify-cap on a valid root failed: $(cat "$scratch/verify_root.err")"
[ "$(jfield "['valid']" <"$vc_out")" = "True" ] || loud_fail "C1: valid root not VALID"
[ "$(jfield "['principal']" <"$vc_out")" = "$ALICE" ] || loud_fail "C1: wrong principal"
python3 -c "import json,sys; g=json.load(open('$vc_out'))['granted']; sys.exit(0 if any('act-as-agent' in a for a in g) else 1)" ||
    loud_fail "C1: broad default did not grant act-as-agent"
echo "C1 ok: broad root capability issued (leash slack) and verifies offline"

# C2. Attenuating-only: a NARROW root, then a valid sub-resource narrowing.
narrow_root="$scratch/narrow_root.cap.json"
wgrun "$A_HOME" "$A_DIR" --json identity delegate --from alice --to "$BOB" \
    --grant 'graph/write@graph://task/abc' --out "$narrow_root" \
    >"$scratch/issue_narrow.json" 2>"$scratch/issue_narrow.err" ||
    loud_fail "C2: issue narrow root failed: $(cat "$scratch/issue_narrow.err")"
child_cap="$scratch/child.cap.json"
wgrun "$B_HOME" "$B_DIR" --json identity delegate --from bob --to "$CAROL" \
    --parent "$narrow_root" --grant 'graph/write@graph://task/abc/sub' --out "$child_cap" \
    >"$scratch/issue_child.json" 2>"$scratch/issue_child.err" ||
    loud_fail "C2: valid attenuation (sub-resource) was refused: $(cat "$scratch/issue_child.err")"
[ "$(jfield "['chain_len']" <"$scratch/issue_child.json")" = "2" ] || loud_fail "C2: child chain depth != 2"
wgrun "$C_HOME" "$C_DIR" --json identity verify-cap --cap "$child_cap" --store "$L" \
    >"$scratch/verify_child.json" 2>"$scratch/verify_child.err" ||
    loud_fail "C2: verify-cap on a valid attenuated child failed: $(cat "$scratch/verify_child.err")"
[ "$(jfield "['valid']" <"$scratch/verify_child.json")" = "True" ] || loud_fail "C2: attenuated child not VALID"

# A WIDENING sub-delegation is structurally REFUSED (the hydra kill, §D3).
if wgrun "$B_HOME" "$B_DIR" identity delegate --from bob --to "$CAROL" \
    --parent "$narrow_root" --grant 'graph/write@graph://*' --out "$scratch/widen.cap.json" \
    >/dev/null 2>"$scratch/widen.err"; then
    loud_fail "C2 FAILED (CRITICAL): a WIDENING sub-delegation was permitted (attenuation broken)"
fi
grep -qiE "attenuat|subset|narrow|widen" "$scratch/widen.err" ||
    loud_fail "C2: widening was blocked but without an attenuation error. stderr: $(cat "$scratch/widen.err")"
echo "C2 ok: sub-resource narrowing VALID (depth 2); widening REFUSED (attenuating-only)"

# C3. Short-TTL / expiry: an already-expired capability FAILS CLOSED.
expired_cap="$scratch/expired.cap.json"
wgrun "$A_HOME" "$A_DIR" --json identity delegate --from alice --to "$BOB" \
    --ttl=-5 --out "$expired_cap" >/dev/null 2>"$scratch/issue_expired.err" ||
    loud_fail "C3: issuing an expired capability failed: $(cat "$scratch/issue_expired.err")"
if wgrun "$C_HOME" "$C_DIR" --json identity verify-cap --cap "$expired_cap" --store "$L" \
    >"$scratch/verify_expired.json" 2>"$scratch/verify_expired.err"; then
    loud_fail "C3 FAILED: an EXPIRED capability verified (a stolen signer must be worthless after expiry)"
fi
[ "$(jfield "['valid']" <"$scratch/verify_expired.json")" = "False" ] ||
    loud_fail "C3: expired capability not reported invalid"
python3 -c "import json,sys; sys.exit(0 if 'EXPIRED' in json.load(open('$scratch/verify_expired.json'))['reason'] else 1)" ||
    loud_fail "C3: expiry rejection reason did not mention EXPIRED"
echo "C3 ok: an already-expired capability FAILS CLOSED on verify (stolen-signer-after-expiry)"

# C4. Revocation kills the delegated subtree (issuer-subtree, §D3).
rev_root="$scratch/rev_root.cap.json"
wgrun "$A_HOME" "$A_DIR" identity delegate --from alice --to "$BOB" --out "$rev_root" \
    >/dev/null 2>"$scratch/issue_revroot.err" ||
    loud_fail "C4: issue revocable root failed: $(cat "$scratch/issue_revroot.err")"
rev_child="$scratch/rev_child.cap.json"
wgrun "$B_HOME" "$B_DIR" identity delegate --from bob --to "$CAROL" --parent "$rev_root" \
    --out "$rev_child" >/dev/null 2>"$scratch/issue_revchild.err" ||
    loud_fail "C4: sub-delegate revocable child failed: $(cat "$scratch/issue_revchild.err")"
# Valid before revocation.
wgrun "$C_HOME" "$C_DIR" identity verify-cap --cap "$rev_child" --store "$L" >/dev/null 2>&1 ||
    loud_fail "C4: revocable child should verify before revocation"
# Alice revokes the ROOT → the whole subtree (incl. the child) dies.
wgrun "$A_HOME" "$A_DIR" identity revoke-cap --from alice --cap "$rev_root" --store "$L" \
    >"$scratch/revoke.log" 2>&1 || loud_fail "C4: revoke-cap failed: $(cat "$scratch/revoke.log")"
if wgrun "$C_HOME" "$C_DIR" --json identity verify-cap --cap "$rev_child" --store "$L" \
    >"$scratch/verify_revoked.json" 2>"$scratch/verify_revoked.err"; then
    loud_fail "C4 FAILED (CRITICAL): a child of a REVOKED parent still verified (subtree revocation broken)"
fi
python3 -c "import json,sys; sys.exit(0 if 'REVOKED' in json.load(open('$scratch/verify_revoked.json'))['reason'] else 1)" ||
    loud_fail "C4: revocation rejection reason did not mention REVOKED"
echo "C4 ok: revoking a parent kills the delegated subtree (issuer-subtree revocation)"

# C5. The leash is TIGHTENABLE by environment policy (the amendment's dial).
tight_cap="$scratch/tight.cap.json"
# The leash is environment-driven policy (§D2). wgrun's `env` passes exported vars
# through, so export the ceiling, issue, then unset.
export WG_FED_LEASH_MAX_TTL_SECS=900 WG_FED_LEASH_SCOPE='graph/read@graph://task/abc'
wgrun "$A_HOME" "$A_DIR" --json identity delegate --from alice --to "$BOB" \
    --ttl 100000000 --out "$tight_cap" \
    >"$scratch/issue_tight.json" 2>"$scratch/issue_tight.err" ||
    { unset WG_FED_LEASH_MAX_TTL_SECS WG_FED_LEASH_SCOPE; loud_fail "C5: tightened issue failed: $(cat "$scratch/issue_tight.err")"; }
unset WG_FED_LEASH_MAX_TTL_SECS WG_FED_LEASH_SCOPE
[ "$(jfield "['leash_slack']" <"$scratch/issue_tight.json")" = "False" ] ||
    loud_fail "C5 FAILED: an environment-tightened policy still reported slack"
python3 - "$scratch/issue_tight.json" <<'PY' || loud_fail "C5: tightened scope was not narrowed to the ceiling"
import json,sys
g=json.load(open(sys.argv[1]))["granted"]
# The broad request must be clamped to exactly the read-only ceiling.
assert g==["graph/read@graph://task/abc"], f"expected only the ceiling ability, got {g}"
PY
echo "C5 ok: same broad issue, TIGHTENED by env policy → narrow scope + capped TTL (slack-by-default, dialed by config)"

# ───────────────────────────────────────────────────────────────────────────────
# PART D — Custody ≠ authority: a downloaded (key-less) bundle cannot issue a cap.
# ───────────────────────────────────────────────────────────────────────────────
# Carol downloads alice's key-less bundle, then tries to issue a capability AS alice.
wgrun "$C_HOME" "$C_DIR" identity fetch "$ALICE" --store "$L" --save alice_dl \
    >"$scratch/carol_fetch.log" 2>&1 || loud_fail "PART D: carol failed to fetch alice's bundle"
if wgrun "$C_HOME" "$C_DIR" identity delegate --from alice_dl --to "$BOB" \
    --out "$scratch/forged.cap.json" >/dev/null 2>"$scratch/forged_issue.err"; then
    loud_fail "PART D FAILED (CRITICAL): a downloaded key-less bundle issued a capability (impersonation)"
fi
grep -qiE "custody|download|impersonation|signer" "$scratch/forged_issue.err" ||
    loud_fail "PART D: issuance was blocked but without a custody error. stderr: $(cat "$scratch/forged_issue.err")"
echo "PART D ok: a downloaded bundle cannot issue a capability (custody ≠ authority; download ≠ impersonation)"

echo "PASS: WG-Fed Wave 6 — encryption=ACL (multi-recipient + sealed-sender) + UCAN delegation (issue/attenuate/expire/revoke) + leash slack-by-default, env-tightenable"
exit 0
