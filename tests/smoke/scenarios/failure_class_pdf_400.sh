#!/usr/bin/env bash
# Scenario: failure_class_pdf_400
#
# Pins Option A of the PDF/binary attachment failure handling design:
# agents that fail with api_error_status:400 ("Could not process PDF") must
# produce a distinct failure_class="api-error-400-document" rather than the
# generic "Agent exited with code 1" undifferentiated failure.
#
# This scenario runs OFFLINE — it does NOT call the Anthropic API.
# Instead it injects a synthetic raw_stream.jsonl containing the exact
# api_error_status:400 event that the real API returns, then invokes
# `wg classify-failure` and `wg fail --class` as the wrapper would.
#
# The tests/smoke/fixtures/broken.pdf fixture is a real malformed PDF
# (magic bytes %PDF-1.4, garbage body, %%EOF). It has been manually
# verified to trigger Anthropic HTTP 400 "Could not process PDF" when
# passed to the Read tool in a Claude agent session. The fixture is
# committed so future maintainers have a stable repro without needing
# API credits.
#
# Owners: design-pdf-binary, fix-pdf-binary, verify-end-to

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

# ── Verify fixture is present ──────────────────────────────────────────
FIXTURE="$HERE/../fixtures/broken.pdf"
if [[ ! -f "$FIXTURE" ]]; then
    loud_fail "Missing fixture: $FIXTURE — commit tests/smoke/fixtures/broken.pdf to the repo"
fi

# Sanity check: the fixture starts with %PDF- (magic bytes)
if ! head -c 5 "$FIXTURE" | grep -qF '%PDF-'; then
    loud_fail "Fixture $FIXTURE does not start with %PDF- magic bytes"
fi

# ── Set up scratch workgraph ───────────────────────────────────────────
scratch=$(make_scratch)
cd "$scratch"
wg init --executor shell >init.log 2>&1 || loud_fail "wg init failed: $(cat init.log)"

wg add "smoke-bad-pdf" \
    -d "Read ./broken.pdf and summarise" >add.log 2>&1 || loud_fail "wg add failed: $(cat add.log)"

# Transition to in-progress so `wg fail` accepts it
wg claim smoke-bad-pdf >claim.log 2>&1 || true  # best-effort (claim may fail in test env)

# ── Inject synthetic raw_stream.jsonl ─────────────────────────────────
# This is the exact JSON event emitted by the Claude CLI when the
# Anthropic API returns HTTP 400 on a malformed PDF attachment.
RAW_STREAM="$scratch/raw_stream.jsonl"
cat > "$RAW_STREAM" <<'JSONL'
{"type":"result","subtype":"error_during_execution","is_error":true,"api_error_status":400,"message":"Could not process PDF"}
JSONL

# ── Test 1: classify-failure subcommand outputs the correct class ──────
FAIL_CLASS=$(wg classify-failure --raw-stream "$RAW_STREAM" --exit-code 1 2>/dev/null)
if [[ "$FAIL_CLASS" != "api-error-400-document" ]]; then
    loud_fail "classify-failure returned '$FAIL_CLASS', expected 'api-error-400-document'"
fi
echo "PASS classify-failure: got $FAIL_CLASS"

# ── Test 2: wg fail --class persists in graph ──────────────────────────
wg fail smoke-bad-pdf \
    --class "$FAIL_CLASS" \
    --reason "Agent exited with code 1" \
    >fail.log 2>&1 || loud_fail "wg fail failed: $(cat fail.log)"

# ── Test 3: wg show renders failure_class ─────────────────────────────
SHOW_OUTPUT=$(wg show smoke-bad-pdf 2>&1)
if ! echo "$SHOW_OUTPUT" | grep -q "api-error-400-document"; then
    loud_fail "wg show does not contain 'api-error-400-document'. Output:\n$SHOW_OUTPUT"
fi
echo "PASS wg show: contains api-error-400-document"

# ── Test 4: operator hint is present ──────────────────────────────────
if ! echo "$SHOW_OUTPUT" | grep -qi "fix the input"; then
    loud_fail "wg show does not contain operator hint 'fix the input'. Output:\n$SHOW_OUTPUT"
fi
echo "PASS wg show: contains operator hint"

# ── Test 5: classify-failure for hard timeout ──────────────────────────
TIMEOUT_CLASS=$(wg classify-failure --exit-code 124 2>/dev/null)
if [[ "$TIMEOUT_CLASS" != "agent-hard-timeout" ]]; then
    loud_fail "classify-failure exit 124 returned '$TIMEOUT_CLASS', expected 'agent-hard-timeout'"
fi
echo "PASS classify-failure hard timeout: got $TIMEOUT_CLASS"

# ── Test 6: classify-failure for generic exit (no api_error) ──────────
GENERIC_STREAM="$scratch/generic_stream.jsonl"
echo '{"type":"result","subtype":"success","result":"partial output"}' > "$GENERIC_STREAM"
GENERIC_CLASS=$(wg classify-failure --raw-stream "$GENERIC_STREAM" --exit-code 1 2>/dev/null)
if [[ "$GENERIC_CLASS" != "agent-exit-nonzero" ]]; then
    loud_fail "classify-failure generic exit returned '$GENERIC_CLASS', expected 'agent-exit-nonzero'"
fi
echo "PASS classify-failure generic exit: got $GENERIC_CLASS"

echo "PASS: failure_class_pdf_400 scenario complete"
exit 0
