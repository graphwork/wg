#!/usr/bin/env bash
# Scenario: nex_streaming_resilience
#
# Pins diagnose-fix-nex: the nex (OpenAI-compatible) streaming client must
# ride out a long, slow-but-healthy generation without the cryptic
# "[openai-client] Stream interrupted after ~N chunks: error decoding
# response body" the user hit against a local llama.cpp endpoint.
#
# Root cause (see docs/nex-streaming-resilience.md): reqwest 0.12's
# `bytes_stream()` collapses EVERY body error to Kind::Decode = "error
# decoding response body", so the old 300s TOTAL request timeout cut a
# healthy long stream around the cap (~6300 tokens at a steady local rate)
# and surfaced as that generic message. The fix swaps the total timeout for
# a per-read (idle) timeout that resets on each frame, plus a raw-byte SSE
# buffer so multi-byte UTF-8 split across chunks is not corrupted.
#
# These tests drive the REAL `OpenAiClient::send_streaming` path over a real
# TCP socket with chunked SSE (an in-process mock llama.cpp), exercising:
#   - total_timeout_reproduces_the_symptom    (proves the symptom + cause)
#   - fixed_client_completes_a_long_slow_stream (proves the fix)
#   - read_timeout_aborts_only_a_stalled_stream (idle protection retained)
#   - connection_drop_then_retry_recovers       (seamless retry)
#   - split_multibyte_utf8_is_reassembled       (decode resilience)
#
# Backed by tests/integration_nex_streaming_resilience.rs. A regression here
# (e.g. reintroducing a total timeout on the streaming path, or per-chunk
# lossy UTF-8 decode) blocks `wg done` on tasks that own this scenario.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

repo_root="$(cd "$HERE/../../.." && pwd)"

if ! command -v cargo >/dev/null 2>&1; then
    loud_skip "MISSING CARGO" "cargo not on PATH"
fi

if [[ ! -f "$repo_root/Cargo.toml" ]]; then
    loud_skip "NOT IN WG REPO" "Cargo.toml not at $repo_root"
fi

cd "$repo_root"

log=$(mktemp -t nex_streaming_resilience.XXXXXX.log)
add_cleanup_hook "rm -f $log"

if ! cargo test --test integration_nex_streaming_resilience -- --nocapture >"$log" 2>&1; then
    loud_fail "integration_nex_streaming_resilience failed:
$(tail -60 "$log")"
fi

if ! grep -q "test result: ok\. 5 passed" "$log"; then
    loud_fail "expected '5 passed' from integration_nex_streaming_resilience. Output:
$(tail -60 "$log")"
fi

echo "PASS: nex streaming rides out long/slow gens; resilient to drops + split UTF-8"
exit 0
