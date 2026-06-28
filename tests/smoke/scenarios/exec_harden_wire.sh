#!/usr/bin/env bash
# Scenario: exec_harden_wire (WG-Exec — HARDEN + WIRE remote execution into the product)
#
# Pins the `exec-harden` task (audit B4, M5, M15, M16, M17, S7 from
# docs/prod-audit/audit-exec.md). Where exec_spark proves the library invariants hold
# through a manual 10-verb CLI, this proves the **product wiring** the audit flags as
# missing: the planner places a task remotely, `accept` GATES on the integrity re-run at
# the canonical write boundary, a dead lease auto-reclaims, remote usage reaches `wg
# spend`, and sensitivity is carried + gated at grant. Each step is a falsifiable
# assertion:
#
#   A. M5  — a task ALREADY IN THE GRAPH, tagged `exec-provider:<wgid>` by the planner, is
#            placed on that remote provider via `wg provider place` (the coordinator-side
#            driver sourcing model/sensitivity from the task); the full wire runs; and on
#            accept the result's usage is bridged into the graph task (M15) so it shows in
#            `wg spend` and the task completes.
#   B. B4  — a LOW-TRUST (B/verified-overflow) result is not committed on attribution
#            alone: with no pinned spec `accept` refuses (verification-required); a genuine
#            result + pinned spec is accepted (the eval-gate passes, no over-block); a
#            CORRUPTED result is rejected by the trusted-domain re-run vs the pinned spec
#            (integrity-*), and the producer's trust is lowered (the audit/revoke leg).
#   C. M17 — sensitivity is carried into the wire and the gate is bound AT GRANT: a High
#            task placed on a Verified provider, then the provider lowered to Provisional
#            before grant, is REFUSED at grant (trust-floor-not-met) because grant
#            re-derives the real High label from the authorizer's ledger — NOT a hardcoded
#            Normal (with the old hardcoded Normal this would wrongly succeed).
#   D. M16 — a dead lease auto-reclaims: a live lease is NOT swept at real-now, but the
#            timeout sweep past its term auto-reclaims it (epoch bumped), and the
#            resurrected old-epoch worker's late write is fenced (stale-epoch).
#   E. M16 — the renew heartbeat: a signed LeaseRenewal is accepted (provider live), and a
#            stale renewal after reclaim is fenced (stale-epoch).
#   F. S7  — the verified-overflow (B) tier is checkable-only: a NON-checkable task is
#            REFUSED on a B (Provisional) provider (overflow-requires-checkable) but
#            ALLOWED on the trusted A (Verified) pool.
#
# Models WG-A (authorizer + custodian of agent G + the canonical graph), Provider-P
# (separately owned), and a disjoint verifier Q as three isolated $HOME keystores + project
# --dirs. L is a dumb, untrusted store. Reuses the WG-Fed identity/UCAN/seal substrate (no
# second system); credential-free (the worker is a real subprocess via WG_EXEC_WORKER_CMD).
# Prerequisite: the WG-Exec spark (exec_spark_borrowed_box) — this wires its library bounds
# into the product, it does not re-prove them.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

command -v python3 >/dev/null 2>&1 || loud_skip "MISSING python3" "needed for JSON assertions"

scratch=$(make_scratch)
L="$scratch/L" # the dumb, untrusted third location

# Per-actor isolated HOME (=> isolated wg-secret keystore) + project dir (=> --dir).
A_HOME="$scratch/A_home"; A_DIR="$scratch/A/.wg"   # WG-A: authorizer + agent G + the graph
P_HOME="$scratch/P_home"; P_DIR="$scratch/P/.wg"   # Provider-P: separately owned
Q_HOME="$scratch/Q_home"; Q_DIR="$scratch/Q/.wg"   # Q: a disjoint trusted verifier
# NOTE: do NOT pre-create $A_DIR — `wg init` creates it (a pre-made .wg dir makes init
# think it is already initialized and it never writes graph.jsonl). P/Q are identity-only
# (no graph), so their dirs are created directly.
mkdir -p "$L" "$scratch/A" "$P_DIR" "$Q_DIR" \
    "$A_HOME/.config" "$P_HOME/.config" "$Q_HOME/.config"

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
wgq() { wgrun "$Q_HOME" "$Q_DIR" "$@"; }

jf() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }

# `wg provider run` drives a REAL worker backend (a real subprocess fed the task slice on
# stdin); it emits the implementation diff + a canonical usage marker.
WORKER="$scratch/worker.sh"
cat >"$WORKER" <<'WORKEOF'
#!/usr/bin/env sh
cat <<'DIFF'
--- a/src/auth.rs
+++ b/src/auth.rs
@@
-fn check(tok: &str) -> bool { todo!() }
+fn check(tok: &str) -> bool { verify(tok) }
DIFF
printf '@@WG_EXEC_USAGE@@ {"input_tokens":48,"output_tokens":24,"cost_usd":0.0007}\n'
WORKEOF
chmod +x "$WORKER"
export WG_EXEC_WORKER_CMD="sh $WORKER"

# ── Setup: init A's graph, mint + publish the three identities (the reused substrate) ──
wga init >/dev/null 2>&1 || loud_fail "setup: wg init (WG-A graph) failed"

wga --json identity new agentG >"$scratch/G.json" 2>"$scratch/G.err" ||
    loud_fail "identity new agentG failed: $(cat "$scratch/G.err")"
G=$(jf "['wgid']" <"$scratch/G.json")
wga identity publish agentG --store "$L" >/dev/null 2>&1 || loud_fail "publish agentG failed"

wgp --json identity new providerP >"$scratch/P.json" 2>&1 || loud_fail "identity new providerP failed"
P=$(jf "['wgid']" <"$scratch/P.json")
wgp identity publish providerP --store "$L" >/dev/null 2>&1 || loud_fail "publish providerP failed"

wgq --json identity new verifierQ >"$scratch/Q.json" 2>&1 || loud_fail "identity new verifierQ failed"
Q=$(jf "['wgid']" <"$scratch/Q.json")
wgq identity publish verifierQ --store "$L" >/dev/null 2>&1 || loud_fail "publish verifierQ failed"

echo "implement check(tok) so it returns verify(tok). No backdoors, no network." >"$scratch/T.input"
# The authorizer's PINNED spec (the trusted oracle — X-6). Forbidden markers are what the
# B4 re-run catches on a corrupted diff.
cat >"$scratch/spec.json" <<EOF
{ "task_id": "x", "required": ["verify(tok)"], "forbidden": ["__backdoor__", "evil.example", "fetch("] }
EOF
echo "setup ok: G=$G  P=$P  Q=$Q"

# ───────────────────────────────────────────────────────────────────────────────
# STEP A — M5: a planner-placed graph task runs on a remote provider; M15: usage in spend.
# ───────────────────────────────────────────────────────────────────────────────
wga add "remote dinner plan" --id wed-remote --tag "exec-provider:$P" --model claude:opus \
    >/dev/null 2>&1 || loud_fail "STEP A: wg add (planner task) failed"
wga --json provider enroll "$P" --trust verified --model claude:opus \
    --isolation container >/dev/null 2>&1 || loud_fail "STEP A: enroll P failed"

po=$(wga --json provider place --as-name agentG --task wed-remote --sensitivity normal \
    --out "$scratch/offerA.json") || loud_fail "STEP A: place errored: $po"
[ "$(echo "$po" | jf "['placed']")" = "True" ] || loud_fail "STEP A: place not placed: $po"
[ "$(echo "$po" | jf "['provider']")" = "$P" ] ||
    loud_fail "STEP A: place did not source the provider from the task tag (got $(echo "$po" | jf "['provider']"))"

wgp --json provider claim --as-name providerP --offer "$scratch/offerA.json" --store "$L" \
    --out "$scratch/claimA.json" >/dev/null 2>"$scratch/clA.err" ||
    loud_fail "STEP A: claim errored: $(cat "$scratch/clA.err")"
wga --json provider grant --as-name agentG --claim "$scratch/claimA.json" \
    --task-input "$scratch/T.input" --store "$L" --out "$scratch/grantA.json" >/dev/null 2>"$scratch/grA.err" ||
    loud_fail "STEP A: grant errored: $(cat "$scratch/grA.err")"
wgp --json provider run --as-name providerP --grant "$scratch/grantA.json" --store "$L" \
    --out "$scratch/resultA.json" >/dev/null 2>"$scratch/rnA.err" ||
    loud_fail "STEP A: run errored: $(cat "$scratch/rnA.err")"

ao=$(wga --json provider accept --result "$scratch/resultA.json" --store "$L" --complete-task) ||
    loud_fail "STEP A: accept errored: $ao"
[ "$(echo "$ao" | jf "['accepted']")" = "True" ] || loud_fail "STEP A: result not accepted: $ao"
[ "$(echo "$ao" | jf "['usage_accounted_to_graph']")" = "True" ] ||
    loud_fail "STEP A FAILED: remote usage was NOT bridged into the graph task (M15)"
out_tok=$(wga spend --json 2>/dev/null | jf "['total_output_tokens']")
[ "$out_tok" -gt 0 ] ||
    loud_fail "STEP A FAILED: remote spend is invisible to wg spend (M15) — out_tokens=$out_tok"
[ "$(wga --json show wed-remote 2>/dev/null | jf "['status']")" = "done" ] ||
    loud_fail "STEP A: --complete-task did not finalize the graph task"
echo "STEP A ok: planner-tagged task placed remotely + run + accepted; usage in wg spend (out=$out_tok); task done"

# ───────────────────────────────────────────────────────────────────────────────
# STEP B — B4: accept gates the integrity re-run for a low-trust (B-tier) result.
# ───────────────────────────────────────────────────────────────────────────────
# A genuine low-trust result: no pinned spec ⇒ refused (verification-required); with the
# spec ⇒ accepted (the eval-gate passes — no over-block).
wga --json provider enroll "$P" --trust provisional --model claude:opus \
    --isolation container >/dev/null 2>&1 || loud_fail "STEP B: re-enroll P provisional failed"
wga --json provider offer --as-name agentG --task bgood --model claude:opus \
    --isolation container --sensitivity normal --provider "$P" --out "$scratch/o_bg.json" \
    >/dev/null 2>&1 || loud_fail "STEP B: offer bgood failed"
wgp --json provider claim --as-name providerP --offer "$scratch/o_bg.json" --store "$L" \
    --out "$scratch/c_bg.json" >/dev/null 2>&1 || loud_fail "STEP B: claim bgood failed"
wga --json provider grant --as-name agentG --claim "$scratch/c_bg.json" \
    --task-input "$scratch/T.input" --store "$L" --out "$scratch/g_bg.json" >/dev/null 2>&1 ||
    loud_fail "STEP B: grant bgood failed"
wgp --json provider run --as-name providerP --grant "$scratch/g_bg.json" --store "$L" \
    --out "$scratch/r_bg.json" >/dev/null 2>&1 || loud_fail "STEP B: run bgood failed"

nr=$(wga --json provider accept --result "$scratch/r_bg.json" --store "$L")
[ "$(echo "$nr" | jf "['accepted']")" = "False" ] ||
    loud_fail "STEP B FAILED: a low-trust result with NO pinned spec was accepted (B4 gate absent)"
[ "$(echo "$nr" | jf "['reason']")" = "verification-required" ] ||
    loud_fail "STEP B: no-spec rejected for the wrong reason ($(echo "$nr" | jf "['reason']"))"
gg=$(wga --json provider accept --result "$scratch/r_bg.json" --store "$L" \
    --pinned-spec "$scratch/spec.json" --verifier "$Q")
[ "$(echo "$gg" | jf "['accepted']")" = "True" ] ||
    loud_fail "STEP B FAILED: a GENUINE low-trust result was rejected by the re-run (over-block): $gg"
echo "STEP B (genuine) ok: no-spec ⇒ verification-required; genuine+spec ⇒ accepted (eval-gate passes)"

# A CORRUPTED low-trust result: the trusted-domain re-run vs the pinned spec rejects it.
# --no-review isolates the B4 re-run as the catching mechanism (the default IC2 review is
# an additional layer that also rejects it — asserted first).
wga --json provider enroll "$P" --trust provisional --model claude:opus \
    --isolation container >/dev/null 2>&1 || loud_fail "STEP B: re-enroll P provisional (2) failed"
wga --json provider offer --as-name agentG --task bbad --model claude:opus \
    --isolation container --sensitivity normal --provider "$P" --out "$scratch/o_bb.json" \
    >/dev/null 2>&1 || loud_fail "STEP B: offer bbad failed"
wgp --json provider claim --as-name providerP --offer "$scratch/o_bb.json" --store "$L" \
    --out "$scratch/c_bb.json" >/dev/null 2>&1 || loud_fail "STEP B: claim bbad failed"
wga --json provider grant --as-name agentG --claim "$scratch/c_bb.json" \
    --task-input "$scratch/T.input" --store "$L" --out "$scratch/g_bb.json" >/dev/null 2>&1 ||
    loud_fail "STEP B: grant bbad failed"
wgp --json provider run --as-name providerP --grant "$scratch/g_bb.json" --store "$L" \
    --out "$scratch/r_bb.json" --corrupt >/dev/null 2>&1 || loud_fail "STEP B: run(corrupt) failed"

# Default IC2 review also rejects the backdoor (defense in depth).
crv=$(wga --json provider accept --result "$scratch/r_bb.json" --store "$L" \
    --pinned-spec "$scratch/spec.json" --verifier "$Q")
[ "$(echo "$crv" | jf "['accepted']")" = "False" ] ||
    loud_fail "STEP B FAILED: a corrupted result was accepted (review+re-run both bypassed)"
# Isolate the B4 re-run: --no-review ⇒ the eval-gate re-run is the gate that catches it.
cr=$(wga --json provider accept --result "$scratch/r_bb.json" --store "$L" --no-review \
    --pinned-spec "$scratch/spec.json" --verifier "$Q")
[ "$(echo "$cr" | jf "['accepted']")" = "False" ] ||
    loud_fail "STEP B FAILED (CRITICAL): the B4 re-run ACCEPTED a corrupted result (gate broken)"
case "$(echo "$cr" | jf "['reason']")" in
    integrity-*) : ;;
    *) loud_fail "STEP B: B4 reject reason is not integrity-* ($(echo "$cr" | jf "['reason']"))" ;;
esac
pt=$(wga --json provider providers 2>/dev/null | python3 -c "import json,sys
for r in json.load(sys.stdin):
  if r['wgid']=='$P': print(r['trust'])")
[ "$pt" = "unknown" ] ||
    loud_fail "STEP B FAILED: producer trust was not lowered after the caught defection (got $pt)"
echo "STEP B ok: corrupt low-trust result rejected by the re-run (integrity-*), producer trust lowered → $pt"

# ───────────────────────────────────────────────────────────────────────────────
# STEP C — M17: sensitivity is carried + gated AT GRANT (not hardcoded Normal).
# ───────────────────────────────────────────────────────────────────────────────
wga --json provider enroll "$P" --trust verified --model claude:opus \
    --isolation container >/dev/null 2>&1 || loud_fail "STEP C: enroll P verified failed"
wga --json provider offer --as-name agentG --task hi --model claude:opus \
    --isolation container --sensitivity high --provider "$P" --out "$scratch/o_hi.json" \
    >/dev/null 2>&1 || loud_fail "STEP C: offer high failed"
wgp --json provider claim --as-name providerP --offer "$scratch/o_hi.json" --store "$L" \
    --out "$scratch/c_hi.json" >/dev/null 2>&1 || loud_fail "STEP C: claim high failed"
# Lower P to provisional BEFORE granting. grant must re-derive High from the ledger and
# REFUSE (Provisional < Verified floor for High). With the old hardcoded Normal, Provisional
# would clear the Normal floor and grant would WRONGLY succeed — this is the M17 proof.
wga --json provider enroll "$P" --trust provisional --model claude:opus \
    --isolation container >/dev/null 2>&1 || loud_fail "STEP C: lower P to provisional failed"
if wga --json provider grant --as-name agentG --claim "$scratch/c_hi.json" \
    --task-input "$scratch/T.input" --store "$L" --out "$scratch/g_hi.json" \
    >/dev/null 2>"$scratch/g_hi.err"; then
    loud_fail "STEP C FAILED (CRITICAL): granting a High task to a now-Provisional provider SUCCEEDED — grant used a hardcoded Normal (M17 absent)"
fi
grep -q "trust-floor-not-met" "$scratch/g_hi.err" ||
    loud_fail "STEP C: grant refused for the wrong reason: $(cat "$scratch/g_hi.err")"
echo "STEP C ok: grant re-derived the High label from the ledger and refused (trust-floor-not-met) — sensitivity gated at grant"

# ───────────────────────────────────────────────────────────────────────────────
# STEP D — M16: a dead remote lease auto-reclaims; the resurrected worker is fenced.
# ───────────────────────────────────────────────────────────────────────────────
wga --json provider enroll "$P" --trust verified --model claude:opus \
    --isolation container >/dev/null 2>&1 || loud_fail "STEP D: enroll P verified failed"
wga --json provider offer --as-name agentG --task deadlease --model claude:opus \
    --isolation container --sensitivity normal --provider "$P" --out "$scratch/o_dl.json" \
    >/dev/null 2>&1 || loud_fail "STEP D: offer deadlease failed"
wgp --json provider claim --as-name providerP --offer "$scratch/o_dl.json" --store "$L" \
    --out "$scratch/c_dl.json" >/dev/null 2>&1 || loud_fail "STEP D: claim deadlease failed"
wga --json provider grant --as-name agentG --claim "$scratch/c_dl.json" \
    --task-input "$scratch/T.input" --store "$L" --out "$scratch/g_dl.json" >/dev/null 2>&1 ||
    loud_fail "STEP D: grant deadlease failed"

# A live lease (term ~30min) is NOT swept at real-now.
wga --json provider sweep 2>/dev/null | python3 -c "import json,sys
d=json.load(sys.stdin)
assert all(r['task']!='deadlease' for r in d['reclaimed']), 'deadlease was reclaimed while still live'" ||
    loud_fail "STEP D FAILED: a live lease was auto-reclaimed at real-now"
# Past its term, the timeout sweep auto-reclaims it.
wga --json provider sweep --now 2030-01-01T00:00:00Z 2>/dev/null | python3 -c "import json,sys
d=json.load(sys.stdin)
assert any(r['task']=='deadlease' for r in d['reclaimed']), 'deadlease was NOT auto-reclaimed past its term: %r' % d" ||
    loud_fail "STEP D FAILED: a dead lease did not auto-reclaim on timeout (M16)"
# The resurrected old-epoch worker's late write is fenced by the bumped epoch.
wgp --json provider run --as-name providerP --grant "$scratch/g_dl.json" --store "$L" \
    --out "$scratch/r_dl.json" >/dev/null 2>&1 || loud_fail "STEP D: late run failed"
lw=$(wga --json provider accept --result "$scratch/r_dl.json" --store "$L")
[ "$(echo "$lw" | jf "['accepted']")" = "False" ] ||
    loud_fail "STEP D FAILED: the resurrected worker's late write after auto-reclaim was accepted"
[ "$(echo "$lw" | jf "['reason']")" = "stale-epoch" ] ||
    loud_fail "STEP D: late write rejected for the wrong reason ($(echo "$lw" | jf "['reason']"))"
echo "STEP D ok: live lease not swept; dead lease auto-reclaimed on timeout; resurrected worker fenced (stale-epoch)"

# ───────────────────────────────────────────────────────────────────────────────
# STEP E — M16: the renew heartbeat (signed LeaseRenewal accepted; stale fenced).
# ───────────────────────────────────────────────────────────────────────────────
wga --json provider offer --as-name agentG --task hb --model claude:opus \
    --isolation container --sensitivity normal --provider "$P" --out "$scratch/o_hb.json" \
    >/dev/null 2>&1 || loud_fail "STEP E: offer hb failed"
wgp --json provider claim --as-name providerP --offer "$scratch/o_hb.json" --store "$L" \
    --out "$scratch/c_hb.json" >/dev/null 2>&1 || loud_fail "STEP E: claim hb failed"
wga --json provider grant --as-name agentG --claim "$scratch/c_hb.json" \
    --task-input "$scratch/T.input" --store "$L" --out "$scratch/g_hb.json" >/dev/null 2>&1 ||
    loud_fail "STEP E: grant hb failed"
wgp --json provider renew --as-name providerP --grant "$scratch/g_hb.json" \
    --out "$scratch/renew_hb.json" >/dev/null 2>&1 || loud_fail "STEP E: renew failed"
ar=$(wga --json provider accept-renewal --renewal "$scratch/renew_hb.json" --store "$L")
[ "$(echo "$ar" | jf "['renewal_accepted']")" = "True" ] ||
    loud_fail "STEP E: a signed renewal was not accepted: $ar"
# After reclaim (epoch bump), the old renewal is stale and is fenced.
wga provider reclaim --task hb >/dev/null 2>&1 || loud_fail "STEP E: reclaim hb failed"
sr=$(wga --json provider accept-renewal --renewal "$scratch/renew_hb.json" --store "$L")
[ "$(echo "$sr" | python3 -c "import json,sys; print(json.load(sys.stdin).get('reason'))")" = "stale-epoch" ] ||
    loud_fail "STEP E FAILED: a stale renewal after reclaim was not fenced: $sr"
echo "STEP E ok: signed renewal accepted (provider live); stale renewal after reclaim fenced (stale-epoch)"

# ───────────────────────────────────────────────────────────────────────────────
# STEP F — S7: the verified-overflow (B) tier is checkable-only.
# ───────────────────────────────────────────────────────────────────────────────
wga --json provider enroll "$P" --trust provisional --model claude:opus \
    --isolation container >/dev/null 2>&1 || loud_fail "STEP F: enroll P provisional failed"
nc=$(wga --json provider offer --as-name agentG --task ncjob --model claude:opus \
    --isolation container --sensitivity normal --non-checkable --provider "$P" \
    --out "$scratch/o_nc.json")
[ "$(echo "$nc" | jf "['refused']")" = "True" ] ||
    loud_fail "STEP F FAILED: a NON-checkable task was placed on the B (overflow) pool"
[ "$(echo "$nc" | jf "['reason']")" = "overflow-requires-checkable" ] ||
    loud_fail "STEP F: B-tier refusal had the wrong reason ($(echo "$nc" | jf "['reason']"))"
[ -f "$scratch/o_nc.json" ] && loud_fail "STEP F FAILED: an offer file was written for a refused B-tier task"
# The SAME non-checkable task is allowed on the trusted A (Verified) pool (trust).
wga --json provider enroll "$P" --trust verified --model claude:opus \
    --isolation container >/dev/null 2>&1 || loud_fail "STEP F: enroll P verified failed"
ac=$(wga --json provider offer --as-name agentG --task ncjob2 --model claude:opus \
    --isolation container --sensitivity normal --non-checkable --provider "$P" \
    --out "$scratch/o_nc2.json")
[ "$(echo "$ac" | jf "['placed']")" = "True" ] ||
    loud_fail "STEP F FAILED: a non-checkable task was refused on the trusted A pool"
[ "$(echo "$ac" | jf "['pool_tier']")" = "A" ] || loud_fail "STEP F: A-tier not surfaced on the offer"
echo "STEP F ok: non-checkable refused on B (overflow-requires-checkable), allowed on the trusted A pool"

echo "PASS: WG-Exec harden+wire — all 6 steps hold (planner-placed remote run + accounting, accept-gates-verify, sensitivity-at-grant, auto-reclaim, renew heartbeat, B-tier checkable-only)"
exit 0
