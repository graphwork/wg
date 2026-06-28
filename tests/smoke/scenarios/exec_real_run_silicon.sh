#!/usr/bin/env bash
# Scenario: exec_real_run_silicon (exec-real-run — the REAL execution silicon)
#
# Replaces the two stubs the exec spark shipped (audit-exec F10 + F6):
#
#   * `wg provider run` is a REAL worker — it drives a real subprocess over the opened
#     task slice and reports the REAL token/cost usage measured from that run, NOT the
#     constant `LEGIT_DIFF`/`CORRUPT_DIFF` diff with canned `1200/340/$0.012` usage.
#   * `wg provider verify`'s integrity re-run is a REAL executable acceptance test: the
#     producer's implementation is materialized into a TRUSTED workspace (the disjoint
#     verifier Q, never the producer P) and the authorizer's PINNED test command is
#     actually RUN — the verdict is test-pass EQUIVALENCE, not a substring match.
#
# Each step is a falsifiable assertion:
#   1. The grant names the silicon the worker must drive (the authorizer's `model`).
#   2. A real run produces a real work product + real usage (exact, via a usage marker —
#      NOT the old 340-token constant), backend = "command".
#   3. The genuine result is accepted; its usage flows through to accept.
#   4. The disjoint re-run on Q (≠ P) ACTUALLY EXECUTES the pinned acceptance test and
#      ACCEPTS the genuine result (mode = executable-test).
#   5. A hostile provider returns a corrupted result whose usage is ESTIMATED from the
#      real run (still non-bare) — and whose backdoor is hidden from any substring oracle
#      (the literal "__backdoor__" never appears in the bytes; it is built at runtime).
#   6. The same executable re-run on Q CATCHES the corrupted result by EXECUTION (the
#      backdoored check disagrees with the spec) — proving the oracle is behavior, not
#      substring. reran_on = Q, never the producer.
#   7. Re-running ON the producing provider P is REFUSED (X-5).

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

command -v python3 >/dev/null 2>&1 || loud_skip "MISSING python3" "needed for the executable re-run + JSON assertions"

scratch=$(make_scratch)
L="$scratch/L" # the dumb, untrusted third location

A_HOME="$scratch/A_home"; A_DIR="$scratch/A/.wg"   # WG-A: authorizer + agent G's custody
P_HOME="$scratch/P_home"; P_DIR="$scratch/P/.wg"   # Provider-P: separately owned
Q_HOME="$scratch/Q_home"; Q_DIR="$scratch/Q/.wg"   # Q: a disjoint trusted verifier
mkdir -p "$L" "$A_DIR" "$P_DIR" "$Q_DIR" \
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
jfield() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }

# ── The two REAL worker backends (real subprocesses fed the task input) ──────────
# Legit: implement check(tok)=verify(tok) as a runnable python module, then report
# CANONICAL usage via the marker (exact, not estimated, not the 340 constant).
WORKER_LEGIT="$scratch/worker_legit.sh"
cat >"$WORKER_LEGIT" <<'WLEOF'
#!/usr/bin/env sh
cat <<'PY'
def verify(tok):
    return tok == "GOODTOKEN"


def check(tok):
    return verify(tok)
PY
printf '@@WG_EXEC_USAGE@@ {"input_tokens":50,"output_tokens":12,"cost_usd":0.0009}\n'
WLEOF
chmod +x "$WORKER_LEGIT"

# Hostile: a backdoored check that returns true for a magic token — but the literal
# token is BUILT AT RUNTIME ("__back"+"door__") so it NEVER appears in the bytes a
# substring oracle could scan. No usage marker ⇒ exercises the I/O-size estimate path.
WORKER_HOSTILE="$scratch/worker_hostile.sh"
cat >"$WORKER_HOSTILE" <<'WHEOF'
#!/usr/bin/env sh
cat <<'PY'
_BD = "__back" + "door__"


def verify(tok):
    return tok == "GOODTOKEN"


def check(tok):
    if tok == _BD:
        return True
    return verify(tok)
PY
WHEOF
chmod +x "$WORKER_HOSTILE"

# ── The authorizer's PINNED, EXECUTABLE acceptance spec (the trusted oracle) ─────
# harness.py imports the produced solution and asserts check() BEHAVES per the spec.
HARNESS="$scratch/harness.py"
cat >"$HARNESS" <<'HEOF'
import importlib.util, sys

spec = importlib.util.spec_from_file_location("solution", "solution.py")
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)

# The backdoor token is probed by VALUE — a backdoored check disagrees here even when
# the literal token never appears in the source bytes.
EXPECTED = {"GOODTOKEN": True, "BADTOKEN": False, "__back" + "door__": False}
for tok, want in EXPECTED.items():
    got = bool(m.check(tok))
    if got != want:
        print("FAIL: check(%r)=%s want %s" % (tok, got, want), file=sys.stderr)
        sys.exit(1)
print("ACCEPT")
sys.exit(0)
HEOF

# Build spec.json embedding harness.py as a pinned fixture. NOTE: required/forbidden are
# EMPTY — the ONLY oracle is the executable acceptance test, so a pass proves execution,
# not substring matching.
SPEC="$scratch/spec.json"
python3 - "$HARNESS" "$SPEC" <<'PY'
import json, sys
harness = open(sys.argv[1]).read()
spec = {
    "task_id": "RT",
    "required": [],
    "forbidden": [],
    "acceptance": {
        "artifact_path": "solution.py",
        "test_cmd": "python3 harness.py",
        "timeout_secs": 30,
        "fixtures": [{"path": "harness.py", "content": harness}],
    },
}
json.dump(spec, open(sys.argv[2], "w"), indent=2)
PY

# ── Setup: mint + publish G, P, Q; enroll P at provisional (the low-trust row) ───
wga --json identity new agentG >"$scratch/g.json" 2>"$scratch/g.err" ||
    loud_fail "identity new agentG failed: $(cat "$scratch/g.err")"
G_WGID=$(jfield "['wgid']" <"$scratch/g.json")
wga identity publish agentG --store "$L" >/dev/null 2>&1 || loud_fail "publish agentG failed"

wgp --json identity new providerP >"$scratch/p.json" 2>&1 || loud_fail "identity new providerP failed"
P_WGID=$(jfield "['wgid']" <"$scratch/p.json")
wgp identity publish providerP --store "$L" >/dev/null 2>&1 || loud_fail "publish providerP failed"

wgq --json identity new verifierQ >"$scratch/q.json" 2>&1 || loud_fail "identity new verifierQ failed"
Q_WGID=$(jfield "['wgid']" <"$scratch/q.json")
wgq identity publish verifierQ --store "$L" >/dev/null 2>&1 || loud_fail "publish verifierQ failed"

wga --json provider enroll "$P_WGID" --trust provisional --model claude:opus \
    --isolation container >/dev/null 2>&1 || loud_fail "enroll provider P failed"

cat >"$scratch/T.input" <<'EOF'
Implement check(tok) so it returns verify(tok), where verify(tok) is true only for the
real token. No backdoors, no magic tokens, no network calls.
EOF
echo "setup ok: G=$G_WGID  P=$P_WGID  Q=$Q_WGID"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 1 — offer → claim → grant; the grant names the silicon the worker must drive.
# ───────────────────────────────────────────────────────────────────────────────
offer="$scratch/offer.json"; claim="$scratch/claim.json"; grant="$scratch/grant.json"
wga --json provider offer --as-name agentG --task RT --model claude:opus \
    --isolation container --sensitivity normal --provider "$P_WGID" --out "$offer" \
    >/dev/null 2>"$scratch/offer.err" || loud_fail "STEP 1: offer errored: $(cat "$scratch/offer.err")"
wgp --json provider claim --as-name providerP --offer "$offer" --store "$L" --out "$claim" \
    >/dev/null 2>"$scratch/claim.err" || loud_fail "STEP 1: claim errored: $(cat "$scratch/claim.err")"
gout=$(wga --json provider grant --as-name agentG --claim "$claim" \
    --task-input "$scratch/T.input" --store "$L" --out "$grant") || loud_fail "STEP 1: grant errored: $gout"
GRANT_MODEL=$(jfield "['model']" <"$grant")
[ "$GRANT_MODEL" = "claude:opus" ] ||
    loud_fail "STEP 1 FAILED: grant does not name the silicon to run (model='$GRANT_MODEL')"
echo "STEP 1 ok: grant names model=$GRANT_MODEL"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 2 — REAL run: a real subprocess produces a real result + real (exact) usage.
# ───────────────────────────────────────────────────────────────────────────────
good="$scratch/result_good.json"
rout=$(WG_EXEC_WORKER_CMD="sh $WORKER_LEGIT" wgp --json provider run --as-name providerP \
    --grant "$grant" --store "$L" --out "$good") || loud_fail "STEP 2: run(legit) errored: $rout"
[ "$(jfield "['backend']" <<<"$rout")" = "command" ] ||
    loud_fail "STEP 2 FAILED: run did not drive a real command backend (got $(jfield "['backend']" <<<"$rout"))"
GOOD_OUT_TOK=$(jfield "['usage']['output_tokens']" <<<"$rout")
[ "$GOOD_OUT_TOK" = "12" ] ||
    loud_fail "STEP 2 FAILED: usage not taken from the real run's marker (output_tokens=$GOOD_OUT_TOK, want 12)"
[ "$GOOD_OUT_TOK" != "340" ] ||
    loud_fail "STEP 2 FAILED: usage is the canned 340-token stub, not real"
[ "$(jfield "['model']" <<<"$rout")" = "claude:opus" ] || loud_fail "STEP 2: run did not record the silicon"
# The result is a genuine function of the run (contains the produced module).
python3 -c "import json,sys; wp=json.load(open('$good'))['work_product']; sys.exit(0 if 'def check(tok)' in wp else 1)" ||
    loud_fail "STEP 2 FAILED: the result work product is not the real produced artifact"
echo "STEP 2 ok: real command-backend run; exact usage out=$GOOD_OUT_TOK (not the 340 constant)"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 3 — The genuine result is accepted; its real usage flows through.
# ───────────────────────────────────────────────────────────────────────────────
aout=$(wga --json provider accept --result "$good" --store "$L") || loud_fail "STEP 3: accept errored: $aout"
[ "$(jfield "['accepted']" <<<"$aout")" = "True" ] || loud_fail "STEP 3: genuine result not accepted: $aout"
[ "$(jfield "['usage']['output_tokens']" <<<"$aout")" = "12" ] ||
    loud_fail "STEP 3 FAILED: the real usage did not flow through accept"
echo "STEP 3 ok: genuine result accepted, real usage carried"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 4 — The disjoint re-run on Q ACTUALLY EXECUTES the pinned test and ACCEPTS.
# ───────────────────────────────────────────────────────────────────────────────
vgood=$(wga --json provider verify --result "$good" --verifier "$Q_WGID" \
    --pinned-spec "$SPEC" --store "$L") || loud_fail "STEP 4: verify(genuine) errored: $vgood"
[ "$(jfield "['accepted']" <<<"$vgood")" = "True" ] ||
    loud_fail "STEP 4 FAILED: a GENUINE result failed the real re-run: $vgood"
[ "$(jfield "['reran']" <<<"$vgood")" = "True" ] || loud_fail "STEP 4: the re-run did not run"
[ "$(jfield "['rerun_mode']" <<<"$vgood")" = "executable-test" ] ||
    loud_fail "STEP 4 FAILED: re-run was not the REAL executable test (mode=$(jfield "['rerun_mode']" <<<"$vgood"))"
[ "$(jfield "['reran_on']" <<<"$vgood")" = "$Q_WGID" ] ||
    loud_fail "STEP 4: re-run did not run on the disjoint verifier Q"
echo "STEP 4 ok: genuine result PASSES the real executable re-run on Q (mode=executable-test)"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 5 — A hostile provider returns a corrupted result; usage is ESTIMATED (non-bare),
#          and the backdoor literal never appears in the bytes (substring-blind).
# ───────────────────────────────────────────────────────────────────────────────
bad="$scratch/result_bad.json"
hout=$(WG_EXEC_WORKER_CMD="sh $WORKER_HOSTILE" wgp --json provider run --as-name providerP \
    --grant "$grant" --store "$L" --out "$bad") || loud_fail "STEP 5: run(hostile) errored: $hout"
BAD_OUT_TOK=$(jfield "['usage']['output_tokens']" <<<"$hout")
[ "$BAD_OUT_TOK" -gt 0 ] ||
    loud_fail "STEP 5 FAILED: estimated usage is bare (FR-V3 requires non-bare usage)"
# The poison is invisible to a substring oracle: the literal token is built at runtime.
python3 -c "import json,sys; wp=json.load(open('$bad'))['work_product']; sys.exit(0 if '__backdoor__' not in wp else 1)" ||
    loud_fail "STEP 5 FAILED: the test relies on the backdoor literal being absent from the bytes"
echo "STEP 5 ok: hostile run; estimated usage out=$BAD_OUT_TOK (non-bare); backdoor literal absent from bytes"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 6 — The same executable re-run on Q CATCHES the corruption by EXECUTION (not
#          substring): the backdoored check disagrees with the spec at runtime.
# ───────────────────────────────────────────────────────────────────────────────
vbad=$(wga --json provider verify --result "$bad" --verifier "$Q_WGID" \
    --pinned-spec "$SPEC" --store "$L") || loud_fail "STEP 6: verify(hostile) errored: $vbad"
[ "$(jfield "['attribution_ok']" <<<"$vbad")" = "True" ] ||
    loud_fail "STEP 6: attribution should still confirm WHO produced it"
[ "$(jfield "['accepted']" <<<"$vbad")" = "False" ] ||
    loud_fail "STEP 6 FAILED (CRITICAL): a corrupted result was ACCEPTED by the real re-run"
[ "$(jfield "['reran']" <<<"$vbad")" = "True" ] || loud_fail "STEP 6: the re-run did not run"
[ "$(jfield "['rerun_mode']" <<<"$vbad")" = "executable-test" ] ||
    loud_fail "STEP 6 FAILED: rejection was not by the executable test (mode=$(jfield "['rerun_mode']" <<<"$vbad"))"
[ "$(jfield "['reason']" <<<"$vbad")" = "rerun-failed" ] ||
    loud_fail "STEP 6: hostile rejected for the wrong reason ($(jfield "['reason']" <<<"$vbad"))"
[ "$(jfield "['reran_on']" <<<"$vbad")" = "$Q_WGID" ] ||
    loud_fail "STEP 6: re-run did not run on the disjoint verifier Q"
[ "$(jfield "['reran_on_is_producer']" <<<"$vbad")" = "False" ] ||
    loud_fail "STEP 6: re-run ran on the producing provider (X-5 breach)"
[ "$(jfield "['provenance_producer']" <<<"$vbad")" = "$P_WGID" ] ||
    loud_fail "STEP 6: provenance did not record the bad producer"
echo "STEP 6 ok: corrupted result CAUGHT by the real executable re-run on Q (behavior, not substring)"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 7 — Re-running ON the producing provider P is REFUSED (X-5).
# ───────────────────────────────────────────────────────────────────────────────
pout=$(wga --json provider verify --result "$bad" --verifier "$P_WGID" \
    --pinned-spec "$SPEC" --store "$L") || loud_fail "STEP 7: verify(same-provider) errored: $pout"
[ "$(jfield "['refused']" <<<"$pout")" = "True" ] ||
    loud_fail "STEP 7 FAILED: re-running on the PRODUCING provider was not refused (X-5)"
echo "STEP 7 ok: re-run on the producing provider refused (X-5)"

echo "PASS: WG-Exec real silicon — real worker run + real executable integrity re-run (genuine accepted, corrupted caught by EXECUTION on a disjoint domain, X-5 refused)"
exit 0
