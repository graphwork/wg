# nex streaming resilience (the "error decoding response body" interruption)

This documents the diagnosis and fix for a streaming interruption the nex
(OpenAI-compatible) client hit against a local llama.cpp endpoint, and the
guarantees the client now provides.

## Symptom

During a long streaming turn against a local endpoint (e.g. puppost
`http://127.0.0.1:8091/v1`, `qwen3.6-35b-a3b-mtp`), the client logged:

```
[openai-client] Stream interrupted after ~6260–6317 chunks: error decoding response body
[openai-client] Streaming error (attempt 1/3): … Retrying in ~800–1150ms
```

It then retried (up to 3×) and usually recovered. The interruption landed at
a **consistent ~6300 chunk count**, on **localhost** (no network jitter), and
the user noticed it seemed to correlate with **hitting Enter repeatedly**
during the turn.

## Root cause

Two independent facts combine:

1. **reqwest 0.12 hides the real cause.** `Response::bytes_stream()`
   (reqwest `src/async_impl/response.rs`) wraps the response body in
   `.map_err(crate::error::decode)`, so **every** body error — a total
   timeout firing, a per-read timeout, a dropped connection, a chunked
   framing error — is mapped to `Kind::Decode`, whose `Display` is the
   single generic string **"error decoding response body"**. The message
   alone cannot tell these apart; you must inspect `is_timeout()` /
   `is_connect()` and walk the source chain.

2. **The client put a *total* request timeout on the streaming body.** The
   shared HTTP client was built with `.timeout(Duration::from_secs(300))`.
   reqwest applies a total `.timeout()` to the *entire* request including
   reading the streaming body. A long-but-healthy generation kept producing
   tokens past the 300s mark; reqwest then fired the total timeout
   mid-stream, which surfaced — per fact (1) — as the cryptic
   "error decoding response body".

At a steady local generation rate of roughly ~21 tok/s, 300s ≈ **~6300
tokens**, which is exactly the consistent chunk count observed. "Usually
recovers" follows too: when the answer length sits right at the 300s
boundary, a retry sometimes finishes just under the cap and sometimes does
not.

### The "hitting Enter" clue: correlation, not causation

The input-race hypothesis was **ruled out**. nex reads the keyboard on a
**dedicated OS thread** running rustyline (`LiveTerminalInput::start` in
`src/executor/native/agent.rs`); lines are handed to the agent loop over an
mpsc channel and *queued* for the next turn. Only a **double Ctrl-C** flips
the cooperative/hard `CancelToken`; the interactive streaming `tokio::select!`
aborts the in-flight request **only** on `cancel.cancelled()`. Plain Enter
never cancels or touches the HTTP body read — at worst it causes some
terminal repaint churn. The correlation is incidental: long generations are
the ones that (a) make a user impatient enough to mash Enter and (b) exceed
the 300s total timeout. The common driver is "this generation is taking a
long time", not the keystroke.

### Secondary bug: per-chunk lossy UTF-8 decode

The SSE read loop accumulated text with
`buffer.push_str(&String::from_utf8_lossy(&chunk))` **per network chunk**. A
multi-byte UTF-8 sequence split across two chunks was decoded before
reassembly, turning the split bytes into a U+FFFD replacement char and
silently corrupting any non-ASCII output that straddled a chunk boundary.
This did not cause the interruption, but it is a real robustness bug in the
same loop.

## Fix

In `src/executor/native/openai_client.rs`:

1. **Streaming timeout semantics.** The shared client is now built with
   `connect_timeout(30s)` + `read_timeout(600s)` and **no total timeout**
   (`build_oai_http_client`). reqwest's `read_timeout` is a per-read /
   idle timeout that **resets on every received frame**, so a healthy stream
   that keeps emitting tokens never trips it no matter how long the
   generation runs — only a genuinely stalled / silently-dropped connection
   does. The non-streaming path keeps a per-request total
   `.timeout(300s)` in `send_with_retry` (a single body, so an overall cap is
   correct there). The idle timeout is overridable via
   `WG_STREAM_READ_TIMEOUT_SECS`.

2. **Raw-byte SSE buffer.** The streaming loops accumulate `Vec<u8>` and
   parse complete SSE lines with `parse_next_oai_sse_data_bytes`. Splitting
   on `\n` (0x0A) is multi-byte-safe — a UTF-8 lead/continuation byte is
   never 0x0A — so only *whole* lines are ever decoded, and a sequence split
   across chunks is reassembled intact.

3. **Honest diagnostics.** A mid-stream error is logged via
   `describe_reqwest_stream_error`, which classifies the error
   (timeout/connect/request/body/decode) and walks its source chain, so logs
   reveal the *actual* cause instead of the generic decode string. The
   returned `anyhow::Error` now **preserves** the underlying
   `reqwest::Error` (via `anyhow::Error::new(e).context(...)`) so upstream
   timeout/retry classification keeps working.

## Tests / evidence

- `tests/integration_nex_streaming_resilience.rs` drives the **real**
  `OpenAiClient::send_streaming` path over a real TCP socket against an
  in-process mock llama.cpp (chunked SSE):
  - `total_timeout_reproduces_the_symptom` — a total timeout firing
    mid-stream yields `is_timeout() == true` **and** Display
    "error decoding response body" (reproduces the symptom; proves the
    message hides the cause).
  - `fixed_client_completes_a_long_slow_stream` — the shipped client rides
    out a slow ~3s stream that a total timeout would cut, returning the full
    content.
  - `read_timeout_aborts_only_a_stalled_stream` — the idle timeout still
    aborts a genuinely stalled stream while sparing a slow-but-alive one.
  - `connection_drop_then_retry_recovers` — a mid-stream connection drop is
    recovered by the retry path.
  - `split_multibyte_utf8_is_reassembled` — multi-byte UTF-8 split across
    chunks decodes intact (no U+FFFD).
- Unit tests in `openai_client.rs` cover `parse_next_oai_sse_data_bytes`
  including the split-multibyte regression.
- Smoke scenario `tests/smoke/scenarios/nex_streaming_resilience.sh`
  (owners: `diagnose-fix-nex`) pins all of the above so a regression — e.g.
  reintroducing a total timeout on the streaming path — blocks `wg done`.

## Operational note

If a *genuinely* stalled upstream needs to be abandoned sooner or later,
tune `WG_STREAM_READ_TIMEOUT_SECS` (per-read idle timeout, default 600s). The
interactive loop additionally has its own application-level idle watchdog
(`WG_STREAM_IDLE_TIMEOUT_SECS`, default 600s) as a second line of defense.
Do **not** reintroduce a total request timeout on the streaming client — it
caps healthy long generations and resurfaces as the cryptic decode error.
