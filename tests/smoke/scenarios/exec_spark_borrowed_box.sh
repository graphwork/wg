#!/usr/bin/env bash
# Scenario: exec_spark_borrowed_box (WG-Exec Wave B — the EXECUTION SPARK PoC)
#
# The six-step "one task, a borrowed box, a scoped leash" spark
# (docs/execution-federation-study/06-decision-memo-and-roadmap.md §4.2), the
# empirical proof that the WG-Exec choice (docs/ADR-exec-e1..e4-*.md) is buildable
# and correct. Where the WG-Fed spark proves "a downloaded identity cannot
# impersonate", this proves "a borrowed provider cannot exceed its lease". Each step
# is a falsifiable assertion:
#
#   1. Place a task on a SEPARATELY-OWNED provider: the RunGrant delivered to P
#      carries NO root key and NO blanket graph-write — only the two scoped UCANs;
#      the WG_EXEC_COMPAT_VERSION handshake succeeded and is signed.
#   2. The worker runs under the scoped UCAN, reading ONLY its task slice: the opened
#      bundle is exactly the configured ContextScope tier (not the whole graph), and
#      an out-of-slice secret never leaks (cleartext or wire); no credential beyond
#      the two UCANs rides in the slice.
#   3. Write a SIGNED result back: it verifies + attributes to agent G via the
#      delegated signer, usage is not bare; an unsigned/wrong-signed result is
#      REJECTED.
#   4. (a) The provider cannot exceed its lease:
#        (i)   a write aimed at a DIFFERENT task U is REJECTED (graph-write UCAN is
#              task-T-scoped);
#        (ii)  signing as G after the lease/TTL elapses is REJECTED (act-as-agent
#              UCAN expired);
#        (iii) a REPLAY of the result, and a STALE write after reclaim, are REJECTED
#              by the lease-epoch fence (atomic CAS).
#   5. (b) Hostile-provider integrity: a plausible-but-corrupted result (claims tests
#      pass, plants a backdoor, AND rewrites a test) — attribution ALONE does not
#      accept; a disjoint re-run (on Q ≠ P, never the producer) vs the authorizer's
#      PINNED spec catches it; the test-poisoning is flagged; re-running ON the
#      producer is refused.
#   6. Fail-closed confidentiality: a `confidential` task offered only to a
#      non-attested provider is REFUSED — context is NEVER shipped in plaintext; an
#      UNLABELED task does not route to A-on-a-stranger (refuses, fail-closed).
#
# Models WG-A (authorizer, holds agent G's root), Provider-P (separately owned), and
# a disjoint verifier Q as three isolated $HOME keystores (the custody boundary is
# $HOME-relative) + project --dirs. L is a shared, dumb, untrusted store; every
# artifact it serves is self-verifying. Its prerequisite — the WG-Fed spark — is
# implied: this reuses the same identity/UCAN/seal substrate.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

command -v python3 >/dev/null 2>&1 || loud_skip "MISSING python3" "needed for JSON assertions"

scratch=$(make_scratch)
L="$scratch/L" # the dumb, untrusted third location

# Per-actor isolated HOME (=> isolated wg-secret keystore) + project dir (=> --dir).
A_HOME="$scratch/A_home"; A_DIR="$scratch/A/.wg"   # WG-A: authorizer + agent G's custody
P_HOME="$scratch/P_home"; P_DIR="$scratch/P/.wg"   # Provider-P: separately owned
Q_HOME="$scratch/Q_home"; Q_DIR="$scratch/Q/.wg"   # Q: a disjoint trusted verifier
mkdir -p "$L" "$A_DIR" "$P_DIR" "$Q_DIR" \
    "$A_HOME/.config" "$P_HOME/.config" "$Q_HOME/.config"

# Run wg as a given actor: isolated HOME (keystore) + explicit --dir (local state).
wgrun() { # wgrun <home> <wgdir> args...
    local home="$1" wgdir="$2"
    shift 2
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH \
        HOME="$home" XDG_CONFIG_HOME="$home/.config" \
        wg --dir "$wgdir" "$@"
}
# WG-A acts as BOTH the authorizer and the custodian of agent G — so agent G's
# identity is minted/loaded in WG-A's home+dir.
wga() { wgrun "$A_HOME" "$A_DIR" "$@"; }
wgp() { wgrun "$P_HOME" "$P_DIR" "$@"; }
wgq() { wgrun "$Q_HOME" "$Q_DIR" "$@"; }

jfield() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }

# ── Setup: mint + publish the three identities (the WG-Fed substrate, reused) ────
agentG_json="$scratch/agentG.json"
wga --json identity new agentG >"$agentG_json" 2>"$scratch/agentG.err" ||
    loud_fail "wg identity new agentG failed: $(cat "$scratch/agentG.err")"
G_WGID=$(jfield "['wgid']" <"$agentG_json")
wga identity publish agentG --store "$L" >/dev/null 2>&1 || loud_fail "publish agentG failed"

wgp --json identity new providerP >"$scratch/providerP.json" 2>&1 || loud_fail "identity new providerP failed"
P_WGID=$(jfield "['wgid']" <"$scratch/providerP.json")
wgp identity publish providerP --store "$L" >/dev/null 2>&1 || loud_fail "publish providerP failed"

wgq --json identity new verifierQ >"$scratch/verifierQ.json" 2>&1 || loud_fail "identity new verifierQ failed"
Q_WGID=$(jfield "['wgid']" <"$scratch/verifierQ.json")
wgq identity publish verifierQ --store "$L" >/dev/null 2>&1 || loud_fail "publish verifierQ failed"

# WG-A enrolls P as a Verified pool member (the authorizer's local trust assertion).
wga --json provider enroll "$P_WGID" --trust verified --model claude:opus \
    --isolation container >/dev/null 2>&1 || loud_fail "enroll provider P failed"

# The authorizer's pinned acceptance spec for task T (the trusted oracle — X-6).
cat >"$scratch/T.input" <<'EOF'
Implement check(tok) so it returns verify(tok). No backdoors, no network calls.
EOF
cat >"$scratch/spec.json" <<EOF
{ "task_id": "T", "required": ["verify(tok)"], "forbidden": ["__backdoor__", "evil.example", "fetch("] }
EOF

echo "setup ok: G=$G_WGID  P=$P_WGID  Q=$Q_WGID"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 1 — Place a task on a separately-owned provider. The RunGrant carries no root
#          key and no blanket graph-write — only the two scoped UCANs.
# ───────────────────────────────────────────────────────────────────────────────
offer="$scratch/offer.json"
out=$(wga --json provider offer --as-name agentG --task T --model claude:opus \
    --isolation container --sensitivity normal --provider "$P_WGID" \
    --out "$offer") || loud_fail "STEP 1: offer errored: $out"
[ "$(jfield "['placed']" <<<"$out")" = "True" ] || loud_fail "STEP 1: offer not placed: $out"

claim="$scratch/claim.json"
wgp --json provider claim --as-name providerP --offer "$offer" --store "$L" \
    --out "$claim" >/dev/null 2>"$scratch/claim.err" ||
    loud_fail "STEP 1: claim errored: $(cat "$scratch/claim.err")"

grant="$scratch/grant.json"
gout=$(wga --json provider grant --as-name agentG --claim "$claim" \
    --task-input "$scratch/T.input" --store "$L" --out "$grant") ||
    loud_fail "STEP 1: grant errored: $gout"
[ "$(jfield "['signed']" <<<"$gout")" = "True" ] || loud_fail "STEP 1: grant not signed"
[ "$(jfield "['exec_compat']" <<<"$gout")" != "" ] || loud_fail "STEP 1: no exec_compat handshake"
[ "$(jfield "['field_scan']['contains_private_key_material']" <<<"$gout")" = "False" ] ||
    loud_fail "STEP 1 FAILED (CRITICAL): the grant carries private-key material (root leaked)"
[ "$(jfield "['field_scan']['has_blanket_graph_write']" <<<"$gout")" = "False" ] ||
    loud_fail "STEP 1 FAILED (CRITICAL): the grant carries a BLANKET graph-write capability"
gw_res=$(jfield "['field_scan']['graph_write_resource']" <<<"$gout")
[ "$gw_res" = "graph://task/T" ] ||
    loud_fail "STEP 1: graph-write UCAN not task-scoped (got $gw_res)"

# Belt-and-braces raw scan: NONE of P's, G's, or Q's private key hexes may appear in
# the grant bytes delivered to P (mirrors the WG-Fed spark's private-key leak scan).
secret_hexes=()
for ks in "$A_HOME/.wg/keystore" "$P_HOME/.wg/keystore"; do
    for f in "$ks"/wgfed.*; do
        [ -f "$f" ] || continue
        v=$(cat "$f"); secret_hexes+=("${v#*:}")
    done
done
for h in "${secret_hexes[@]}"; do
    [ -n "$h" ] || continue
    grep -qF "$h" "$grant" &&
        loud_fail "STEP 1 FAILED (CRITICAL): a private key leaked into the RunGrant bytes"
done
echo "STEP 1 ok: grant has two scoped UCANs, write=$gw_res, NO root key, NO blanket write"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 2 — Worker runs under the scoped UCAN, reads ONLY its task slice.
# ───────────────────────────────────────────────────────────────────────────────
# Seed an out-of-slice secret that lives in WG-A's broader graph but must NEVER reach
# P's minimal slice (the minimization / X-2 assertion).
OUT_OF_SLICE_SECRET="GRAPHWIDE_SECRET_sk_do_not_leak_42"
result="$scratch/result.json"
rout=$(wgp --json provider run --as-name providerP --grant "$grant" --store "$L" \
    --out "$result" --scope-probe "$OUT_OF_SLICE_SECRET") ||
    loud_fail "STEP 2: run errored: $rout"
[ "$(jfield "['slice_scope_tier']" <<<"$rout")" = "task" ] ||
    loud_fail "STEP 2: slice tier is not the minimal 'task' tier (got $(jfield "['slice_scope_tier']" <<<"$rout"))"
[ "$(jfield "['slice_task_id']" <<<"$rout")" = "T" ] || loud_fail "STEP 2: slice is for the wrong task"
[ "$(jfield "['out_of_slice_secret_found']" <<<"$rout")" = "False" ] ||
    loud_fail "STEP 2 FAILED: an out-of-slice secret leaked into the delivered slice"
[ "$(jfield "['credential_beyond_ucans_found']" <<<"$rout")" = "False" ] ||
    loud_fail "STEP 2 FAILED: a credential beyond the two scoped UCANs rode in the slice"
# The out-of-slice secret must also be absent from the grant bytes on the wire.
grep -qF "$OUT_OF_SLICE_SECRET" "$grant" &&
    loud_fail "STEP 2 FAILED: out-of-slice secret present in the sealed grant bytes"
echo "STEP 2 ok: opened exactly the 'task' slice for T; no out-of-slice secret, no extra credential"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 3 — Write a SIGNED result back; WG-A accepts + attributes to G. An unsigned
#          / wrong-signed result is rejected.
# ───────────────────────────────────────────────────────────────────────────────
aout=$(wga --json provider accept --result "$result" --store "$L") ||
    loud_fail "STEP 3: accept errored: $aout"
[ "$(jfield "['accepted']" <<<"$aout")" = "True" ] || loud_fail "STEP 3: genuine signed result not accepted: $aout"
[ "$(jfield "['attributed_to']" <<<"$aout")" = "$G_WGID" ] ||
    loud_fail "STEP 3: result not attributed to agent G"
[ "$(jfield "['usage']['output_tokens']" <<<"$aout")" -gt 0 ] ||
    loud_fail "STEP 3: usage is bare (FR-V3 requires non-bare usage)"

# A wrong-signed result: flip the signature; accept MUST reject it.
forged="$scratch/result_forged.json"
python3 - "$result" "$forged" <<'PY'
import json, sys
r = json.load(open(sys.argv[1]))
sig = r["sig"]
# Flip a hex nibble so the signature no longer verifies.
r["sig"] = ("f" if sig[0] != "f" else "0") + sig[1:]
json.dump(r, open(sys.argv[2], "w"))
PY
fout=$(wga --json provider accept --result "$forged" --store "$L") ||
    loud_fail "STEP 3: accept(forged) errored: $fout"
[ "$(jfield "['accepted']" <<<"$fout")" = "False" ] ||
    loud_fail "STEP 3 FAILED: a wrong-signed result was ACCEPTED (attribution bypass)"
echo "STEP 3 ok: signed result accepted + attributed to G; wrong-signed result rejected ($(jfield "['reason']" <<<"$fout"))"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 4(a) — The provider cannot exceed its lease.
# ───────────────────────────────────────────────────────────────────────────────
# (i) Write aimed at a DIFFERENT task U → rejected (graph-write UCAN is task-T-scoped).
result_U="$scratch/result_U.json"
wgp --json provider run --as-name providerP --grant "$grant" --store "$L" \
    --out "$result_U" --target-task U >/dev/null 2>&1 || loud_fail "STEP 4i: run(target U) errored"
uout=$(wga --json provider accept --result "$result_U" --store "$L") ||
    loud_fail "STEP 4i: accept(U) errored: $uout"
[ "$(jfield "['accepted']" <<<"$uout")" = "False" ] ||
    loud_fail "STEP 4i FAILED: a write to a DIFFERENT task U was accepted (scope breach)"
[ "$(jfield "['reason']" <<<"$uout")" = "graph-write-scope-violation" ] ||
    loud_fail "STEP 4i: U-write rejected for the wrong reason ($(jfield "['reason']" <<<"$uout"))"
echo "STEP 4(i) ok: write to a different task U rejected (graph-write-scope-violation)"

# (ii) Sign as G AFTER the act-as-agent UCAN expires → rejected. Issue a short-TTL
#      grant for a fresh task T2, produce a result, then accept with --now past expiry.
wga --json provider offer --as-name agentG --task T2 --model claude:opus \
    --isolation container --sensitivity normal --provider "$P_WGID" \
    --out "$scratch/offer_t2.json" >/dev/null 2>&1 || loud_fail "STEP 4ii: offer T2 errored"
wgp --json provider claim --as-name providerP --offer "$scratch/offer_t2.json" \
    --store "$L" --out "$scratch/claim_t2.json" >/dev/null 2>&1 || loud_fail "STEP 4ii: claim T2 errored"
wga --json provider grant --as-name agentG --claim "$scratch/claim_t2.json" \
    --task-input "$scratch/T.input" --ucan-ttl-secs 60 --store "$L" \
    --out "$scratch/grant_t2.json" >/dev/null 2>&1 || loud_fail "STEP 4ii: grant T2 errored"
wgp --json provider run --as-name providerP --grant "$scratch/grant_t2.json" \
    --store "$L" --out "$scratch/result_t2.json" >/dev/null 2>&1 || loud_fail "STEP 4ii: run T2 errored"
FUTURE="2030-01-01T00:00:00Z" # well past the 60s UCAN TTL
eout=$(wga --json provider accept --result "$scratch/result_t2.json" --store "$L" --now "$FUTURE") ||
    loud_fail "STEP 4ii: accept(expired) errored: $eout"
[ "$(jfield "['accepted']" <<<"$eout")" = "False" ] ||
    loud_fail "STEP 4ii FAILED: signing as G after the UCAN expired was accepted"
[ "$(jfield "['reason']" <<<"$eout")" = "attribution-failed" ] ||
    loud_fail "STEP 4ii: post-expiry rejected for the wrong reason ($(jfield "['reason']" <<<"$eout"))"
echo "STEP 4(ii) ok: signing as G after the lease/TTL elapsed rejected (UCAN expired)"

# (iii) Replay the already-committed result, and a stale write after reclaim → fenced.
rout2=$(wga --json provider accept --result "$result" --store "$L") ||
    loud_fail "STEP 4iii: accept(replay) errored: $rout2"
[ "$(jfield "['accepted']" <<<"$rout2")" = "False" ] ||
    loud_fail "STEP 4iii FAILED: a REPLAY of the committed result was accepted"
[ "$(jfield "['reason']" <<<"$rout2")" = "replay-already-committed" ] ||
    loud_fail "STEP 4iii: replay rejected for the wrong reason ($(jfield "['reason']" <<<"$rout2"))"
# Reclaim T2 (bump epoch), then accept the old-epoch T2 result → stale-epoch fence.
wga provider reclaim --task T2 >/dev/null 2>&1 || loud_fail "STEP 4iii: reclaim T2 errored"
sout=$(wga --json provider accept --result "$scratch/result_t2.json" --store "$L") ||
    loud_fail "STEP 4iii: accept(stale) errored: $sout"
[ "$(jfield "['accepted']" <<<"$sout")" = "False" ] ||
    loud_fail "STEP 4iii FAILED: a STALE write after reclaim was accepted"
[ "$(jfield "['reason']" <<<"$sout")" = "stale-epoch" ] ||
    loud_fail "STEP 4iii: stale write rejected for the wrong reason ($(jfield "['reason']" <<<"$sout"))"
echo "STEP 4(iii) ok: replay + stale-after-reclaim writes fenced (replay-already-committed / stale-epoch)"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 5(b) — Hostile-provider integrity: a corrupted result caught by a disjoint
#             re-run vs the PINNED spec; test-poisoning flagged; same-provider refused.
# ───────────────────────────────────────────────────────────────────────────────
# Enroll P as low-trust (provisional) for the overflow tier, place a fresh task T3.
wga --json provider enroll "$P_WGID" --trust provisional --model claude:opus \
    --isolation container >/dev/null 2>&1 || loud_fail "STEP 5: re-enroll P provisional errored"
wga --json provider offer --as-name agentG --task T3 --model claude:opus \
    --isolation container --sensitivity normal --provider "$P_WGID" \
    --out "$scratch/offer_t3.json" >/dev/null 2>&1 || loud_fail "STEP 5: offer T3 errored"
wgp --json provider claim --as-name providerP --offer "$scratch/offer_t3.json" \
    --store "$L" --out "$scratch/claim_t3.json" >/dev/null 2>&1 || loud_fail "STEP 5: claim T3 errored"
wga --json provider grant --as-name agentG --claim "$scratch/claim_t3.json" \
    --task-input "$scratch/T.input" --store "$L" --out "$scratch/grant_t3.json" >/dev/null 2>&1 ||
    loud_fail "STEP 5: grant T3 errored"
# P behaves hostilely: returns the plausible-but-corrupted diff (backdoor + test edit).
bad="$scratch/result_bad.json"
wgp --json provider run --as-name providerP --grant "$scratch/grant_t3.json" \
    --store "$L" --out "$bad" --corrupt >/dev/null 2>&1 || loud_fail "STEP 5: run(corrupt) errored"

# The corrupted result needs a pinned spec for T3 (same oracle shape as T).
cat >"$scratch/spec_t3.json" <<EOF
{ "task_id": "T3", "required": ["verify(tok)"], "forbidden": ["__backdoor__", "evil.example", "fetch("] }
EOF
# (i)+(ii) Disjoint re-run on Q (≠ P) vs the PINNED spec catches the corruption.
vout=$(wga --json provider verify --result "$bad" --verifier "$Q_WGID" \
    --pinned-spec "$scratch/spec_t3.json" --store "$L") ||
    loud_fail "STEP 5: verify errored: $vout"
[ "$(jfield "['attribution_ok']" <<<"$vout")" = "True" ] ||
    loud_fail "STEP 5: attribution should still confirm WHO produced it"
[ "$(jfield "['accepted']" <<<"$vout")" = "False" ] ||
    loud_fail "STEP 5 FAILED (CRITICAL): a corrupted result was ACCEPTED (attribution laundered a forgery)"
[ "$(jfield "['reran']" <<<"$vout")" = "True" ] || loud_fail "STEP 5: the integrity re-run did not run"
[ "$(jfield "['reran_on']" <<<"$vout")" = "$Q_WGID" ] ||
    loud_fail "STEP 5: re-run did not run on the disjoint verifier Q"
[ "$(jfield "['reran_on_is_producer']" <<<"$vout")" = "False" ] ||
    loud_fail "STEP 5: re-run ran on the producing provider (X-5 breach)"
# (iii) The test-poisoning attempt is flagged.
[ "$(jfield "['test_poisoning_flagged']" <<<"$vout")" = "True" ] ||
    loud_fail "STEP 5: the test-file rewrite was NOT flagged (X-6)"
[ "$(jfield "['provenance_producer']" <<<"$vout")" = "$P_WGID" ] ||
    loud_fail "STEP 5: provenance did not record the bad producer"

# Re-running ON the producing provider (X-5) is REFUSED by the engine.
pout=$(wga --json provider verify --result "$bad" --verifier "$P_WGID" \
    --pinned-spec "$scratch/spec_t3.json" --store "$L") ||
    loud_fail "STEP 5: verify(same-provider) errored: $pout"
[ "$(jfield "['refused']" <<<"$pout")" = "True" ] ||
    loud_fail "STEP 5 FAILED: re-running on the PRODUCING provider was not refused (X-5)"
echo "STEP 5 ok: corrupted result caught by disjoint re-run vs pinned spec; test-poisoning flagged; same-provider refused"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 6 — Fail-closed confidentiality routing.
# ───────────────────────────────────────────────────────────────────────────────
# A `confidential` task offered only to P (advertises NO attestation) → REFUSED;
# context is never shipped in plaintext.
conf_offer="$scratch/offer_conf.json"
cout=$(wga --json provider offer --as-name agentG --task Tc --model claude:opus \
    --isolation container --sensitivity confidential --provider "$P_WGID" \
    --out "$conf_offer") || loud_fail "STEP 6: confidential offer errored: $cout"
[ "$(jfield "['refused']" <<<"$cout")" = "True" ] ||
    loud_fail "STEP 6 FAILED (CRITICAL): a confidential task was placed on a NON-attested provider"
[ "$(jfield "['context_shipped']" <<<"$cout")" = "False" ] ||
    loud_fail "STEP 6 FAILED (CRITICAL): confidential context was shipped despite no attestation"
[ "$(jfield "['reason']" <<<"$cout")" = "no-eligible-confidential-provider" ] ||
    loud_fail "STEP 6: confidential refused for the wrong reason ($(jfield "['reason']" <<<"$cout"))"
[ -f "$conf_offer" ] && loud_fail "STEP 6 FAILED: an offer file was written for a refused confidential task"

# An UNLABELED task does not route to A-on-a-stranger — it fails closed (D-i). Enroll
# a stranger (Unknown trust) and offer an unlabeled task; it must refuse.
STRANGER="wgid:zStrangerNoTrust000000000000000000000000000000"
wga --json provider enroll "$STRANGER" --trust unknown --model claude:opus \
    --isolation container >/dev/null 2>&1 || loud_fail "STEP 6: enroll stranger errored"
uout=$(wga --json provider offer --as-name agentG --task Tu --model claude:opus \
    --isolation container --provider "$STRANGER" \
    --out "$scratch/offer_unlabeled.json") || loud_fail "STEP 6: unlabeled offer errored: $uout"
[ "$(jfield "['refused']" <<<"$uout")" = "True" ] ||
    loud_fail "STEP 6 FAILED: an UNLABELED task routed to A-on-a-stranger (must fail closed)"
[ "$(jfield "['reason']" <<<"$uout")" = "unlabeled-fails-closed" ] ||
    loud_fail "STEP 6: unlabeled refused for the wrong reason ($(jfield "['reason']" <<<"$uout"))"
echo "STEP 6 ok: confidential→non-attested REFUSED (context never shipped); unlabeled fails closed"

echo "PASS: WG-Exec spark — all 6 steps hold (scoped UCANs/no-root, slice-only, signed result, leash-bounds, hostile-integrity-caught, fail-closed-confidential)"
exit 0
