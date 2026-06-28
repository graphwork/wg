#!/usr/bin/env bash
# Scenario: cross_task_poison (WG-Exec + WG-Review — defend cross-task poison, audit B7 / TC8)
#
# Pins the `cross-task-poison` task (audit B7 / D-iii — the WORST-RANKED threat in the
# adversarial study, jointly owned by the exec + review planes). Before this, only
# provenance + trust-lowering existed (and a code comment overstated that descendants were
# surfaced). This proves the three structural defenses the study names, each a falsifiable
# assertion:
#
#   1. TIER-BY-GRAPH-POSITION — a FOUNDATIONAL task (one with downstream descendants in the
#      authorizer's graph) will NOT place on a low-trust provider: `wg provider offer` of a
#      foundational task to a Provisional (B-tier) provider is REFUSED (trust-floor-not-met),
#      while a LEAF task (no descendants) on the SAME Provisional provider is PLACED
#      (graph_position=leaf, pool_tier=B). A foundational task is admitted only on the
#      trusted A (Verified) pool (graph_position=foundational).
#
#   2. DESCENDANT RE-RUN — when an upstream artifact is later found bad, every transitive
#      descendant that consumed it is ENUMERATED and RE-RUN. A corrupted result for an
#      upstream graph task, rejected by `wg provider verify`, enumerates its descendants
#      (poison_descendants) and re-queues them: descendants that had FINISHED (status=done)
#      flip back to `open` so the coordinator rebuilds them on the corrected input.
#
#   3. INPUT RE-VERIFICATION ACROSS TRUST BOUNDARIES — a higher-trust task re-verifies inputs
#      a strictly-lower-trust task produced before consuming them. `wg provider grant` of a
#      consumer on a Verified (A) box, with an `--after` input produced by a Provisional (B)
#      box, is REFUSED (cross-trust-input-unverified) until that input is re-verified in a
#      trusted domain (`wg provider verify`); after the re-verify, the same grant SUCCEEDS.
#
# Models WG-A (authorizer + custodian of agent G + the canonical graph), two separately-owned
# providers P_low (B-tier) + P_high (A-tier), and a disjoint verifier Q, as isolated $HOME
# keystores + project --dirs. L is a dumb, untrusted store. Reuses the WG-Fed identity/UCAN/
# seal substrate (no second system); credential-free (the worker is a real subprocess via
# WG_EXEC_WORKER_CMD). Prerequisite: the WG-Exec harden+wire (exec_harden_wire) — this adds
# the cross-task-poison defenses on top of the wired exec plane, it does not re-prove it.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

command -v python3 >/dev/null 2>&1 || loud_skip "MISSING python3" "needed for JSON assertions"

scratch=$(make_scratch)
L="$scratch/L" # the dumb, untrusted third location

# Per-actor isolated HOME (=> isolated wg-secret keystore) + project dir (=> --dir).
A_HOME="$scratch/A_home"; A_DIR="$scratch/A/.wg"     # WG-A: authorizer + agent G + the graph
PL_HOME="$scratch/PL_home"; PL_DIR="$scratch/PL/.wg" # P_low: separately-owned B-tier provider
PH_HOME="$scratch/PH_home"; PH_DIR="$scratch/PH/.wg" # P_high: separately-owned A-tier provider
Q_HOME="$scratch/Q_home"; Q_DIR="$scratch/Q/.wg"     # Q: a disjoint trusted verifier
mkdir -p "$L" "$scratch/A" "$PL_DIR" "$PH_DIR" "$Q_DIR" \
    "$A_HOME/.config" "$PL_HOME/.config" "$PH_HOME/.config" "$Q_HOME/.config"

wgrun() { # wgrun <home> <wgdir> args...
    local home="$1" wgdir="$2"
    shift 2
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH \
        HOME="$home" XDG_CONFIG_HOME="$home/.config" \
        wg --dir "$wgdir" "$@"
}
wga() { wgrun "$A_HOME" "$A_DIR" "$@"; }
wgpl() { wgrun "$PL_HOME" "$PL_DIR" "$@"; }
wgph() { wgrun "$PH_HOME" "$PH_DIR" "$@"; }
wgq() { wgrun "$Q_HOME" "$Q_DIR" "$@"; }

jf() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }

# Set a task's status directly in the graph (simulating a FINISHED task), so the
# descendant-re-run reset (done → open) is observable.
set_status() { # set_status <wgdir> <task_id> <status>
    python3 - "$1/graph.jsonl" "$2" "$3" <<'PY'
import json, sys
path, tid, st = sys.argv[1], sys.argv[2], sys.argv[3]
out = []
for line in open(path):
    s = line.rstrip("\n")
    if not s.strip():
        out.append(s); continue
    d = json.loads(s)
    if d.get("kind") == "task" and d.get("id") == tid:
        d["status"] = st
    out.append(json.dumps(d))
open(path, "w").write("\n".join(out) + "\n")
PY
}
status_of() { wga --json show "$2" 2>/dev/null | jf "['status']"; }

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

# ── Setup: init A's graph, mint + publish the identities (the reused substrate) ──
wga init >/dev/null 2>&1 || loud_fail "setup: wg init (WG-A graph) failed"

wga --json identity new agentG >"$scratch/G.json" 2>"$scratch/G.err" ||
    loud_fail "identity new agentG failed: $(cat "$scratch/G.err")"
G=$(jf "['wgid']" <"$scratch/G.json")
wga identity publish agentG --store "$L" >/dev/null 2>&1 || loud_fail "publish agentG failed"

wgpl --json identity new provLow >"$scratch/PL.json" 2>&1 || loud_fail "identity new provLow failed"
PL=$(jf "['wgid']" <"$scratch/PL.json")
wgpl identity publish provLow --store "$L" >/dev/null 2>&1 || loud_fail "publish provLow failed"

wgph --json identity new provHigh >"$scratch/PH.json" 2>&1 || loud_fail "identity new provHigh failed"
PH=$(jf "['wgid']" <"$scratch/PH.json")
wgph identity publish provHigh --store "$L" >/dev/null 2>&1 || loud_fail "publish provHigh failed"

wgq --json identity new verifierQ >"$scratch/Q.json" 2>&1 || loud_fail "identity new verifierQ failed"
Q=$(jf "['wgid']" <"$scratch/Q.json")
wgq identity publish verifierQ --store "$L" >/dev/null 2>&1 || loud_fail "publish verifierQ failed"

echo "implement check(tok) so it returns verify(tok). No backdoors, no network." >"$scratch/T.input"
cat >"$scratch/spec.json" <<EOF
{ "task_id": "x", "required": ["verify(tok)"], "forbidden": ["__backdoor__", "evil.example", "fetch("] }
EOF

# The graph topology for tier-by-position + descendant re-run:
#   root-found ──▶ mid ──▶ leaf-tail
# root-found & mid are FOUNDATIONAL (they have descendants); leaf-tail is a LEAF.
wga add "root foundational task" --id root-found >/dev/null 2>&1 || loud_fail "setup: add root-found failed"
wga add "mid task" --id mid --after root-found >/dev/null 2>&1 || loud_fail "setup: add mid failed"
wga add "leaf tail task" --id leaf-tail --after mid >/dev/null 2>&1 || loud_fail "setup: add leaf-tail failed"
echo "setup ok: G=$G  P_low=$PL  P_high=$PH  Q=$Q  graph: root-found→mid→leaf-tail"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 1 — TIER-BY-GRAPH-POSITION: a foundational task will NOT place on a low-trust box.
# ───────────────────────────────────────────────────────────────────────────────
wga --json provider enroll "$PL" --trust provisional --model claude:opus \
    --isolation container >/dev/null 2>&1 || loud_fail "STEP 1: enroll P_low provisional failed"

# A FOUNDATIONAL task (root-found has descendants) offered to a Provisional (B) provider is
# REFUSED — the positional floor demands Verified regardless of the Normal sensitivity.
fr=$(wga --json provider offer --as-name agentG --task root-found --model claude:opus \
    --isolation container --sensitivity normal --provider "$PL" --out "$scratch/o_rf_lo.json")
[ "$(echo "$fr" | jf "['refused']")" = "True" ] ||
    loud_fail "STEP 1 FAILED (CRITICAL): a foundational task was PLACED on a low-trust provider: $fr"
[ "$(echo "$fr" | jf "['reason']")" = "trust-floor-not-met" ] ||
    loud_fail "STEP 1: foundational refusal had the wrong reason ($(echo "$fr" | jf "['reason']"))"
[ -f "$scratch/o_rf_lo.json" ] && loud_fail "STEP 1 FAILED: an offer file was written for a refused foundational task"

# A LEAF task (leaf-tail has no descendants) on the SAME Provisional provider is PLACED.
lp=$(wga --json provider offer --as-name agentG --task leaf-tail --model claude:opus \
    --isolation container --sensitivity normal --provider "$PL" --out "$scratch/o_lt_lo.json")
[ "$(echo "$lp" | jf "['placed']")" = "True" ] ||
    loud_fail "STEP 1 FAILED: a leaf task was refused on the Provisional pool: $lp"
[ "$(echo "$lp" | jf "['graph_position']")" = "leaf" ] ||
    loud_fail "STEP 1: leaf task not surfaced as graph_position=leaf ($(echo "$lp" | jf "['graph_position']"))"
[ "$(echo "$lp" | jf "['pool_tier']")" = "B" ] || loud_fail "STEP 1: leaf task not on the B (overflow) pool"

# The SAME foundational task IS admitted on the trusted A (Verified) pool.
wga --json provider enroll "$PH" --trust verified --model claude:opus \
    --isolation container >/dev/null 2>&1 || loud_fail "STEP 1: enroll P_high verified failed"
fa=$(wga --json provider offer --as-name agentG --task root-found --model claude:opus \
    --isolation container --sensitivity normal --provider "$PH" --out "$scratch/o_rf_hi.json")
[ "$(echo "$fa" | jf "['placed']")" = "True" ] ||
    loud_fail "STEP 1 FAILED: a foundational task was refused on the trusted A pool: $fa"
[ "$(echo "$fa" | jf "['graph_position']")" = "foundational" ] ||
    loud_fail "STEP 1: foundational task not surfaced as graph_position=foundational"
[ "$(echo "$fa" | jf "['pool_tier']")" = "A" ] || loud_fail "STEP 1: foundational task not on the trusted A pool"
echo "STEP 1 ok: foundational refused on low-trust (trust-floor-not-met), leaf placed on B, foundational placed on A"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 2 — DESCENDANT RE-RUN: a poisoned upstream artifact re-queues its descendants.
# ───────────────────────────────────────────────────────────────────────────────
# Place root-found on the trusted A (Verified) box (foundational ⇒ A), then have the box
# produce a CORRUPTED result. The disjoint re-run vs the pinned spec catches the poison and
# enumerates + re-runs the descendants that consumed it.
wgph --json provider claim --as-name provHigh --offer "$scratch/o_rf_hi.json" --store "$L" \
    --out "$scratch/c_rf.json" >/dev/null 2>"$scratch/c_rf.err" ||
    loud_fail "STEP 2: claim root-found failed: $(cat "$scratch/c_rf.err")"
wga --json provider grant --as-name agentG --claim "$scratch/c_rf.json" \
    --task-input "$scratch/T.input" --store "$L" --out "$scratch/g_rf.json" >/dev/null 2>"$scratch/g_rf.err" ||
    loud_fail "STEP 2: grant root-found failed: $(cat "$scratch/g_rf.err")"
wgph --json provider run --as-name provHigh --grant "$scratch/g_rf.json" --store "$L" \
    --out "$scratch/r_rf.json" --corrupt >/dev/null 2>"$scratch/r_rf.err" ||
    loud_fail "STEP 2: run(corrupt) root-found failed: $(cat "$scratch/r_rf.err")"

# The descendants have FINISHED (built on the poison): mark them done so the re-run reset is
# observable as a done → open flip.
set_status "$A_DIR" mid done
set_status "$A_DIR" leaf-tail done
[ "$(status_of "$A_DIR" mid)" = "done" ] || loud_fail "STEP 2 setup: mid was not marked done"

# `wg provider verify` rejects the corrupted result and re-runs the descendants.
vr=$(wga --json provider verify --result "$scratch/r_rf.json" --verifier "$Q" \
    --pinned-spec "$scratch/spec.json" --store "$L")
[ "$(echo "$vr" | jf "['accepted']")" = "False" ] ||
    loud_fail "STEP 2 FAILED (CRITICAL): a corrupted upstream result was ACCEPTED by verify"
[ "$(echo "$vr" | jf "['descendants_requeued']")" = "True" ] ||
    loud_fail "STEP 2 FAILED: a poisoned upstream did not trigger the descendant re-run"
# Both transitive descendants are enumerated (the blast radius).
echo "$vr" | python3 -c "import json,sys
d=json.load(sys.stdin)['poison_descendants']
assert 'mid' in d and 'leaf-tail' in d, 'descendants not enumerated: %r' % d" ||
    loud_fail "STEP 2 FAILED: the poisoned upstream's descendants were not enumerated ($(echo "$vr" | jf "['poison_descendants']"))"
# The finished descendants are re-queued back to `open`.
[ "$(status_of "$A_DIR" mid)" = "open" ] ||
    loud_fail "STEP 2 FAILED: descendant 'mid' was not re-queued (still $(status_of "$A_DIR" mid))"
[ "$(status_of "$A_DIR" leaf-tail)" = "open" ] ||
    loud_fail "STEP 2 FAILED: descendant 'leaf-tail' was not re-queued (still $(status_of "$A_DIR" leaf-tail))"
echo "STEP 2 ok: corrupt upstream rejected; descendants [mid, leaf-tail] enumerated + re-queued (done → open)"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 3 — INPUT RE-VERIFICATION ACROSS TRUST BOUNDARIES: a higher-trust task re-verifies
# a strictly-lower-trust input before consuming it.
# ───────────────────────────────────────────────────────────────────────────────
# Re-assert the pool: P_low Provisional (B), P_high Verified (A). STEP 2's verify-reject
# lowered P_high (the root-found producer), so restore it to the trusted A tier here.
wga --json provider enroll "$PL" --trust provisional --model claude:opus \
    --isolation container >/dev/null 2>&1 || loud_fail "STEP 3: re-enroll P_low provisional failed"
wga --json provider enroll "$PH" --trust verified --model claude:opus \
    --isolation container >/dev/null 2>&1 || loud_fail "STEP 3: re-enroll P_high verified failed"

# An upstream input `low-up` is PRODUCED by the Provisional (B) box P_low (a pure-exec leaf,
# so placement itself is allowed). It is NOT yet re-verified.
wga --json provider offer --as-name agentG --task low-up --model claude:opus \
    --isolation container --sensitivity normal --provider "$PL" --out "$scratch/o_lu.json" \
    >/dev/null 2>&1 || loud_fail "STEP 3: offer low-up failed"
wgpl --json provider claim --as-name provLow --offer "$scratch/o_lu.json" --store "$L" \
    --out "$scratch/c_lu.json" >/dev/null 2>&1 || loud_fail "STEP 3: claim low-up failed"
wga --json provider grant --as-name agentG --claim "$scratch/c_lu.json" \
    --task-input "$scratch/T.input" --store "$L" --out "$scratch/g_lu.json" >/dev/null 2>&1 ||
    loud_fail "STEP 3: grant low-up failed"
wgpl --json provider run --as-name provLow --grant "$scratch/g_lu.json" --store "$L" \
    --out "$scratch/r_lu.json" >/dev/null 2>&1 || loud_fail "STEP 3: run low-up failed"
echo "low-up artifact (consumed downstream)" >"$scratch/low-up.art"

# A higher-trust consumer `hi-cons` is placed on the Verified (A) box P_high and granted with
# low-up as an `--after` input. The grant is REFUSED — low-up was produced by a strictly
# lower-trust box and has not been re-verified across the boundary.
wga --json provider offer --as-name agentG --task hi-cons --model claude:opus \
    --isolation container --sensitivity normal --provider "$PH" --out "$scratch/o_hc.json" \
    >/dev/null 2>&1 || loud_fail "STEP 3: offer hi-cons failed"
wgph --json provider claim --as-name provHigh --offer "$scratch/o_hc.json" --store "$L" \
    --out "$scratch/c_hc.json" >/dev/null 2>&1 || loud_fail "STEP 3: claim hi-cons failed"

if wga --json provider grant --as-name agentG --claim "$scratch/c_hc.json" \
    --task-input "$scratch/T.input" --after "low-up=$scratch/low-up.art" \
    --store "$L" --out "$scratch/g_hc.json" >/dev/null 2>"$scratch/g_hc.err"; then
    loud_fail "STEP 3 FAILED (CRITICAL): a higher-trust task consumed an UNVERIFIED lower-trust input (grant succeeded)"
fi
grep -q "cross-trust-input-unverified" "$scratch/g_hc.err" ||
    loud_fail "STEP 3: grant refused for the wrong reason: $(cat "$scratch/g_hc.err")"
[ -f "$scratch/g_hc.json" ] && loud_fail "STEP 3 FAILED: a grant file was written for a refused cross-trust consume"

# Re-verify low-up in a trusted domain (disjoint verifier Q) — the genuine result passes the
# eval-gate and is recorded as integrity-verified.
lv=$(wga --json provider verify --result "$scratch/r_lu.json" --verifier "$Q" \
    --pinned-spec "$scratch/spec.json" --store "$L")
[ "$(echo "$lv" | jf "['accepted']")" = "True" ] ||
    loud_fail "STEP 3 FAILED: the genuine lower-trust input was rejected by the re-verify (over-block): $lv"
[ "$(wga --json provider show --task low-up 2>/dev/null | jf "['integrity_verified']")" = "True" ] ||
    loud_fail "STEP 3 FAILED: the re-verify did not record low-up as integrity-verified"

# The SAME grant now SUCCEEDS — the cross-boundary input has been re-verified.
wga --json provider grant --as-name agentG --claim "$scratch/c_hc.json" \
    --task-input "$scratch/T.input" --after "low-up=$scratch/low-up.art" \
    --store "$L" --out "$scratch/g_hc.json" >/dev/null 2>"$scratch/g_hc2.err" ||
    loud_fail "STEP 3 FAILED: grant still refused AFTER the cross-trust input was re-verified: $(cat "$scratch/g_hc2.err")"
[ -f "$scratch/g_hc.json" ] ||
    loud_fail "STEP 3 FAILED: grant did not emit after the input was re-verified"
echo "STEP 3 ok: cross-trust input refused until re-verified (cross-trust-input-unverified), then granted after re-verify"

echo "PASS: cross-task poison (B7/TC8) — all 3 defenses hold (tier-by-position, descendant re-run, input re-verification across trust boundaries)"
exit 0
