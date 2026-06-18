# Context-aware nex tool-output channeling

Task: `make-nex-tool` — *Make nex tool-output redirection threshold context-aware.*

## Problem

The native (nex) handler "channels" large tool outputs to a file instead of
returning them inline to the model. Channeled content is **retrievable** (the
model gets a handle with `head`/`tail`/`sed`/`grep` hints), but when the
threshold is too short, outputs that easily fit the model's context (e.g. `wg
quickstart`, ~8 KB / 137 lines) get dumped to disk and the agent thrashes
reading them back in chunks. This is worst for large-context models (130k)
behind OpenAI-compatible endpoints (llama.cpp local, OpenRouter).

## Where the threshold lives

`src/executor/native/channel.rs`:

- `ToolOutputChanneler::maybe_channel_with_input` makes the decision:
  `content.len() <= threshold_bytes` → inline, else write-to-file + handle.
- `threshold_for_context_window(ctx_tokens)` derives the byte budget:
  ~8% of the context window (`ctx_tokens * 4 bytes/token * 8%`), **clamped to
  `[32 KiB, 128 KiB]`** for terminal readability.
- Wired in `agent.rs` (`Agent::new`) via
  `ToolOutputChanneler::for_context_window(dir, client.context_window())`.

### Relationship to `fix-nex-large` (commit `d938e96c`)

`fix-nex-large` already replaced the old fixed ~4 KiB threshold with the
context-aware `threshold_for_context_window` computation, the `[32 KiB, 128 KiB]`
clamp, the first/last edge previews, and the explicit non-`cat` parse guidance.
It did **not** regress the file-fallback path.

The gap it left — and what `make-nex-tool` closes — is the *input* to that
computation. `client.context_window()` for OpenAI-compatible clients
(`OpenAiClient`) came only from:

1. endpoint config `context_window`, or
2. the model registry's `context_window`, or
3. a blind hardcoded `128_000` default.

It was **never probed from the server**, so a llama.cpp server booted with
`-c 8192` was treated as if it had a 128k window (budget over-estimated), and a
130k local model under a generic config could be under-estimated.

## Per-provider answer: is context length queryable?

| Provider                       | Endpoint               | Field                                     | Notes |
|--------------------------------|------------------------|-------------------------------------------|-------|
| llama.cpp server (local)       | `GET /props`           | runtime `n_ctx`                           | The `-c` the server booted with — the real ceiling. Top-level `n_ctx` on newer builds, else `default_generation_settings.n_ctx`. |
| vLLM / generic OpenAI-compat   | `GET /v1/models`       | `max_model_len`                           | Present on vLLM; absent on plain OpenAI. |
| llama.cpp `/v1/models` (newer) | `GET /v1/models`       | `meta.n_ctx` / `meta.n_ctx_train`         | `n_ctx` is runtime; `n_ctx_train` is trained max. |
| OpenRouter                     | `GET /api/v1/models`   | `context_length`                          | Already cached in the WG model registry (`wg models`), so we use the registry rather than a live probe. |
| plain OpenAI                   | (not exposed)          | —                                         | Needs the configurable fallback. |

`/props` `n_ctx` is preferred over any trained-max number because it is the
actual runtime ceiling.

## Implementation

New module `src/executor/native/context_probe.rs`:

- `parse_props_n_ctx(json)` — llama.cpp `/props` runtime `n_ctx` (top-level,
  then nested).
- `parse_models_context_len(json, model)` — `/v1/models` (or `/api/v1/models`),
  checks `max_model_len` → `context_length` → `meta.n_ctx` → `meta.n_ctx_train`
  for the matching model (or the sole entry on a single-model server).
- `probe_context_window_blocking(base_url, api_key, model, hint)` — best-effort
  live probe: tries `/props` then `/v1/models`. Runs on an isolated thread with
  its own current-thread runtime (safe to call from inside an existing async
  runtime), with a **2 s timeout**, and caches the result per
  `(base_url, model)` for the process lifetime. Only probes the local-ish family
  (`local` / `oai-compat` / `openai`); OpenRouter is excluded so we never
  re-download its large `/models` payload on every spawn.
- `resolve_context_window(explicit, probe, registry, fallback)` — pure
  precedence function:
  **explicit config > live probe > model registry > configurable fallback**.
- `DEFAULT_FALLBACK_CONTEXT_WINDOW = 128_000` — generous, not a small constant;
  overridable per deployment.

Wiring in `src/executor/native/provider.rs`:

- The config-based OAI-compat arm resolves the window via
  `context_probe::resolve_context_window(endpoint_cfg, probe, registry, fallback)`
  then `client.with_context_window(...)`.
- The zero-config inline-URL path (`build_inline_url_client`, used by
  `wg nex -e http://localhost:8088`) probes the server and falls back to the
  default. This is the prime llama.cpp case.
- The fallback is configurable via `[native_executor].fallback_context_window`.

The file-fallback path is unchanged — only the threshold's *input* became a
real, probed number instead of a blind default.

## Routing behavior (unchanged shape, dynamic threshold)

- Output (byte-length proxy for tokens) ≤ budget → returned **inline**.
- Output > budget → written to file, model receives a concise notice with the
  path, size, line count, first/last slices, and explicit parse guidance
  (`head -n`, `tail -n`, `sed -n`, `grep -n`, `wc -l`) plus a note **not** to
  `cat` the whole file.

## Validation

- `cargo build` + `cargo test` pass.
- `context_probe` unit tests cover the `/props` and `/v1/models` parsers
  (including runtime-over-trained preference and single-entry fallback), the
  precedence/fallback logic, zero-handling, and the OpenRouter probe-skip.
- `channel` unit tests cover the inline-vs-file budget decision at the exact
  boundary (`test_budget_decision_at_boundary`) and end-to-end across a small-
  vs large-context model (`test_budget_decision_tracks_context_window`).
