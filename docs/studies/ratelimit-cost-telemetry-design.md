# Rate-Limit & Cost-Telemetry Design Study

**Status:** study / design (no code changes here — this is the spec a detector + signal
implementation consumes)
**Owner:** study-rate-limit
**Downstream consumers:** `.flip-study-rate-limit`, `synthesis-roadmap-from`, the rate-limit
supervisor, the adaptive-parallelism controller.

> **TL;DR.** WG already classifies *some* provider failures, but only along a thin
> Claude-shaped seam (`api_error_status`) for the subprocess executors and a rich — but
> **separate** — in-process HTTP seam for the native (`nex`) executor. The pi CLI handler
> (the default worker path under the `pi` profile) emits a *different* error event shape
> that the current classifier never reads, so pi/OpenRouter rate-limit and credit-exhaustion
> failures fall through to `AgentExitNonzero` and the human-readable prose is never turned
> into a machine-readable signal. This study catalogs the **real** OpenRouter failure
> surface (with citations), the **real** pi/claude/codex wire shapes, the gap in WG today,
> and specifies a single normalized `FailureReason` signal + a persisted per-provider/model
> telemetry window the supervisor and adaptive-parallelism controller can threshold on.

---

## 1. Objective & scope

Study how to detect rate-limit / quota / cost-exhaustion failures by parsing the **actual**
outputs of failed pi-based (LLM) requests, and catalog OpenRouter's real billing/rate-limit
surface (free-endpoint requests/day caps, how limits are represented, exact error messages).
Output a **detector spec** + a **parsed-failure signal** the supervisor and
adaptive-parallelism controller can consume.

Scope covers two distinct WG executors that surface provider errors in *different* ways:

| Executor | Wire | Where errors surface | Existing parse depth |
|---|---|---|---|
| **native (`nex`)** | in-process HTTP (`src/executor/native/openai_client.rs`) | HTTP status + JSON body + headers, in-band | **deep** — status, `error.metadata.raw`, `retry_after`, provider codes |
| **pi CLI (`pi --mode json`)** | subprocess, NDJSON stdout | `{"type":"error",...}` / `{"type":"response","success":false,...}` lines in `raw_stream.jsonl`; exit code | **shallow** — wrapper only reads `api_error_status` (a Claude field pi never emits) |
| claude / codex CLI | subprocess | `api_error_status` in JSONL | medium — status-code matched by `classify_from_raw_stream` |

The **pi gap** is the headline finding (§4.2).

---

## 2. OpenRouter's real billing / rate-limit surface

All facts below are from OpenRouter's live docs, fetched 2026-07-25. Citations are the
section anchors; the canonical URLs are listed in §8.

### 2.1 Two limit types — credit vs. rate

OpenRouter enforces **two independent** kinds of limits. Confusing the two is the most common
detector bug (a 402 is *not* retriable; a 429 usually is).

| Limit type | What it governs | HTTP status when exceeded | Where the remaining budget lives |
|---|---|---|---|
| **Credit limit** | how much you can *spend* (account balance + optional per-key spending cap) | **`402 Payment Required`** | `GET /api/v1/key` → `data.limit_remaining` |
| **Rate limit** | how many *requests* you can make (free-model daily/minute caps + Cloudflare DDoS protection) | **`429 Too Many Requests`** | `X-RateLimit-*` response headers on the error response |

> Source: "API Credit & Rate Limits — Handle 402 and 429 Errors",
> `https://openrouter.ai/docs/api_reference/limits` — the "Limit type / What it governs /
> Error on exceeding / Where to check" table.

### 2.2 Free-endpoint requests/day caps (the headline number)

Free model variants have IDs ending in `:free`. Their daily/minute caps are a **function of
how many credits you have purchased (all-time)**, not of your current balance:

| Credits purchased (all time) | Requests / minute | Requests / day |
|---|---|---|
| **Less than 10 credits** | 20 | **50** |
| **At least 10 credits** | 20 | **1000** |

Paid (non-`:free`) models have **no platform-level request cap** from OpenRouter (the upstream
provider may still throttle or be at capacity).

> Source: "API Credit & Rate Limits" §Rate Limits / "Free usage limits"; corroborated by the
> OpenRouter FAQ ("For free models, rate limits are determined by the credits that you have
> purchased. If you have purchased at least 10 credits, your free model rate limit will be
> 1000 requests per day. Otherwise, you will be rate limited to 50 free model API requests
> per day"), `https://openrouter.ai/docs/faq`.

**Implication for the controller.** A pi worker running on a `:free` route under a keyless or
low-credit account burns through 50 req/day across **all** `:free` models globally (the limit
is per-account, shared across models; "Making additional accounts or API keys will not affect
your rate limits, as we govern capacity globally" — Limits doc). The adaptive-parallelism
controller must therefore treat the 50/day budget as a **single shared global pool**, not
per-model. Hitting it manifests as `429` with `error_type: rate_limit_exceeded`.

### 2.3 How limits are represented — headers vs. body

- **Rate-limit state** is communicated via **`X-RateLimit-*` response headers on the error
  response** (the 429), **not** in the JSON body. (Limits doc, "Where to check" column.)
- **Credit state** is communicated via the **`GET /api/v1/key` JSON body** (`limit`,
  `limit_remaining`, `limit_reset`, `usage`, `usage_daily`, `usage_monthly`, `is_free_tier`).
- **`Retry-After`** (standard HTTP header, value in **seconds**) is present on **`429` and
  `503`** responses. "The OpenAI SDK, Anthropic SDK, Vercel AI SDK, and OpenRouter SDK already
  respect this header for backoff."
  > Source: "API Error Handling and Debugging — Complete Guide",
  > `https://openrouter.ai/docs/api_reference/errors-and-debugging`, §Retry-After Header.
- A numeric **`retry_after`** (seconds, float) is also carried **inside the body** at
  `error.metadata.retry_after` — WG's native executor already parses this
  (`parse_retry_after_oai`, §4.3).

**Detector consequence.** Because rate-limit *budget* lives in headers (only present on the
429 response itself) and credit budget lives in the `/key` body, there is no single "remaining
quota" field on a normal success response to read proactively. The two practical budgets are:

1. **`GET /api/v1/key`** → `limit_remaining` (USD credit) — pollable, already wired into WG's
   `SessionCostTracking.key_status` (§4.4).
2. **The 429's `X-RateLimit-*` headers + `Retry-After`** — only observable at the moment of
   failure, so the detector must capture them from the *failing* response, not pre-flight.

### 2.4 HTTP status codes (the canonical list)

| Code | Meaning | Retriable? | WG note |
|---|---|---|---|
| 400 | Bad Request (invalid/missing params, CORS) | no | WG maps → `ApiError400Document` for PDF/doc failures |
| **401** | Invalid credentials (OAuth expired, disabled/invalid key) | no | auth — falls through to `AgentExitNonzero` today |
| **402** | Insufficient credits (account or per-key cap) | **no** (add credits) | **NOT classified** today |
| 403 | Forbidden / permission / guardrail / moderation | no | — |
| 408 | Request timed out | yes (retry) | — |
| **429** | Rate limited | **yes (honor Retry-After)** | WG → `ApiError429RateLimit` |
| 502 | Chosen model is down / invalid upstream response | short retry | folds into 5xx |
| 503 | No available model provider meeting routing requirements | short retry | folds into 5xx |

> Source: "API Error Handling and Debugging" §Error Codes.

### 2.5 The error body shape and the typed `error_type`

OpenRouter returns a JSON error envelope:

```json
{
  "error": {
    "code": 402,
    "message": "Insufficient credits. Add more using https://openrouter.ai/credits",
    "metadata": {
      "provider_name": "Crucible",
      "provider_code": 429,
      "raw": "{\"error\":{\"type\":\"insufficient_quota\",\"code\":\"insufficient_quota\",\"message\":\"Out of credits. Top up at /dashboard/billing to continue.\"}}",
      "retry_after": 12.5
    }
  }
}
```

OpenRouter now tags every provider error with a **canonical, stable `error_type` string**
across all three API skins (Chat Completions, Anthropic Messages, Responses). **Programs
should switch on `error_type`, not the HTTP status alone** ("Use this value, not the HTTP
status code alone, to programmatically distinguish error categories").

The high-value `error_type` values for this study:

| `error_type` | HTTP | Category | Action |
|---|---|---|---|
| `rate_limit_exceeded` | 429 | rate-limit | back off honoring `Retry-After`; reduce parallelism |
| `payment_required` | 402 | credit-exhausted | **stop** dispatching on this account/key until topped up |
| `token_limit_exceeded` | 400→* | quota (token) | a credit/token cap enforced by OR was exceeded |
| `provider_overloaded` | 529/503 | transient | short retry; consider fallback model |
| `provider_unavailable` | 503 | transient | OR may auto-failover; short retry |
| `authentication` | 401 | auth | fix key/config; **do not retry** |
| `context_length_exceeded` | 400→* | hard | shrink input; **not retriable as-is** |
| `server` | 500 | transient | upstream message **masked**; short retry |
| `timeout` | 408/504 | transient | retry |
| `unmapped` | any | unknown | `provider_code` may carry original |

`*` = some token/length `error_type`s are **transformed into a *successful* completion with
`finish_reason: "length"`** rather than an error (`context_length_exceeded`,
`max_tokens_exceeded`, `token_limit_exceeded`, `string_too_long`). The detector must not
expect an error envelope for these.

> Source: "API Error Handling and Debugging" §Typed Error Codes, §Error Code Transformations,
> §Skin-Specific Error Formats.

### 2.6 Where `error_type` lives depends on the skin (and on streaming phase)

- **Chat Completions** (`/api/v1/chat/completions`): `error.metadata.error_type` — on the
  non-streaming response when a provider error interrupts generation, and on the mid-stream
  error chunk.
- **Anthropic Messages** (`/api/v1/messages`): `error.error_type` inside the `error` object
  (alongside the native, *lossy* `error.type`).
- **Responses API** (`/api/v1/responses`): top-level `error_type` on the response.
- **Pre-stream errors**: a normal HTTP 4xx/5xx status + the JSON body above.
- **Mid-stream errors** (the trap): once the first token is written, the HTTP `200` is
  already committed and **cannot** be changed, so a rate-limit/overload hit mid-stream
  arrives as an **SSE event with `finish_reason: "error"`** (HTTP stays 200). For Chat
  Completions this is a `chat.completion.chunk` carrying a top-level `error` object.

> Source: "API Error Handling and Debugging" §Streaming Error Formats (Pre-Stream /
> Mid-Stream), §Skin-Specific Error Formats.

**Detector consequence for pi.** Pi streams; a 429/402 that fires after the first token will
NOT raise pi's exit code or produce an HTTP status the wrapper can see — it lands as an
in-stream error chunk. The detector must scan pi's NDJSON for the in-stream error envelope,
not rely on exit codes (§4.2).

### 2.7 OpenRouter-platform vs. upstream-provider 429

A `429` has **two possible origins** and the distinction changes the controller's response:

1. **OpenRouter platform** — you hit the free-model RPD/RPM cap or DDoS protection.
   → Honor `Retry-After`, **reduce global parallelism / switch route**.
2. **Upstream provider** — the provider serving the request is throttling or at capacity.
   → `error.metadata.provider_code` carries the provider's original code; OpenRouter may have
   already retried other providers for the same model (fallback routing). → Retry the same
   request (transient), consider a fallback *model*.

> Source: "API Credit & Rate Limits" §Handling 429 errors.

### 2.8 The `GET /api/v1/key` budget object

Polling `https://openrouter.ai/api/v1/key` returns the `Key` shape WG already deserializes
(`OpenRouterKeyStatus`, §4.4):

```
data: {
  label, usage, usage_daily, usage_weekly, usage_monthly,
  limit,            // per-key credit cap, or null if unlimited
  limit_remaining,  // USD remaining under the cap (or account balance proxy)
  limit_reset,      // when the per-key cap resets
  is_free_tier,     // whether this is the keyless/free tier
}
```

> Source: "API Credit & Rate Limits" §Checking Your Limits.

---

## 3. Failure taxonomy — the distinct shapes WG must detect

This taxonomy is executor-agnostic; §4 maps each shape to where it surfaces in WG and §5
specifies the detector rules.

### 3.1 Rate-limit (`rate_limit`)

- HTTP **429**, body `error_type: rate_limit_exceeded`.
- Origin: OpenRouter-platform (free RPD/RPM) **or** upstream provider (`provider_code`
  present).
- Signal: `Retry-After` header / `error.metadata.retry_after`; `X-RateLimit-*` headers.
- **Transient**: retriable after backoff. The *adaptive* response (not just retry) is to
  **lower parallelism** because the 50/day free budget is global.

### 3.2 Credit-exhausted (`credit_exhausted`)

- HTTP **402**, body `error_type: payment_required`, message contains "Insufficient credits".
- Origin: account balance ≤ 0, or per-key `limit_remaining` exhausted.
- **NOT transient as-is** — retrying the same request on the same key/account burns no budget
  but also makes no progress. Action: **stop dispatching on this key**, surface to operator
  (`wg recover` / top-up), optionally fail over to a different profile/key.
- Proactive signal: `GET /api/v1/key` → `limit_remaining` → 0 (or negative balance).

### 3.3 Token-quota (`quota_token`)

- `error_type: token_limit_exceeded` (a credit-based token cap OR, often, silently transformed
  to a `length` finish reason — §2.5).
- Distinguish from `context_length_exceeded` (a *model window* limit, `hard`) and
  `max_tokens_exceeded` (the request's own `max_tokens`, `hard`/config).
- Action: usually shrink input or raise tier; not a simple retry.

### 3.4 Auth (`auth`)

- HTTP **401** (`authentication`), **403** (`permission_denied` / guardrail).
- NOT retriable. Action: fix key/config (`api_key_ref`), never auto-retry. The native executor
  already appends a config-pointing hint for 401/403 (`oai_api_error_with_hint`).

### 3.5 Transient (`transient`)

- HTTP **5xx**: 500 (`server`, upstream masked), 502 (model down), 503 (no provider),
  529 (`provider_overloaded`); also 408/504 (`timeout`).
- Retriable with backoff. `provider_overloaded`/`provider_unavailable` → consider fallback
  model/provider.

### 3.6 Hard (`hard`)

- 400 (`invalid_request`/`invalid_prompt`/`not_found`/`payload_too_large`/`unprocessable`),
  `context_length_exceeded`, executor/tool config (`ExecutorConfig` today).
- NOT retriable as-is — fix the request/config. Already partly handled
  (`ApiError400Document`, `ExecutorConfig`).

### 3.7 Process-level (WG-local, not provider)

- `AgentHardTimeout` (exit 124), `ResourceExhaustedDisk` (ENOSPC), `WrapperInternal`,
  `AgentExitNonzero` (unrecognized). These are WG infra, not provider telemetry, but the
  controller needs them in the same signal space to avoid mis-attributing a local timeout to
  the provider.

---

## 4. Where these surface in WG today (and what's lost)

### 4.1 The classification pipeline (shared by all subprocess executors)

```
pi/claude/codex wrapper (src/commands/spawn/execution.rs)
  ├── stdout → raw_stream.jsonl (pi: NDJSON; claude/codex: JSONL)
  ├── stderr → output.log
  └── on non-zero exit (or no wg done/fail):
        grep output.log tail for ENOSPC → ResourceExhaustedDisk
        else wg classify-failure --raw-stream $RAW_STREAM --exit-code $EXIT_CODE
            → classify_from_raw_stream (src/commands/spawn/raw_stream_classifier.rs)
            → prints a FailureClass kebab → wg fail --class <CLASS>
        → wg fail (src/commands/fail.rs) records task.failure_class + .failure_reason + .token_usage
```

`FailureClass` today (`src/graph.rs:129`):

```
ApiError400Document, ApiError429RateLimit, ApiError5xxTransient,
AgentHardTimeout, AgentExitNonzero, ResourceExhaustedDisk, ExecutorConfig,
WrapperInternal, DeliverableMissing, NoOperationalOutput
```

`classify_from_raw_stream` (`src/commands/spawn/raw_stream_classifier.rs:34`) reads a **64 KiB
tail** of `raw_stream.jsonl` and, in order: exit 124 → hard-timeout; ENOSPC strings → disk;
`extract_api_error_status(&tail)` → numeric 400/429/5xx; executor-config heuristic; else
`AgentExitNonzero`.

### 4.2 THE GAP — the pi handler's error shape is never classified

`extract_api_error_status` scans for the literal `api_error_status` field. **That field is
Claude's.** Pi emits a different shape:

- **Generic error event**: `{"type":"error","error":"<message>"}` (or `message` field).
- **Failed RPC reply**: `{"type":"response","success":false,"error":"<message>"}`.

This is proven by the pi RPC accumulator in `src/commands/pi_handler.rs:182-235`
(`RpcTurnAccumulator::ingest`, which explicitly handles `"error"` and `"response"` with
`success:false`, test at `:1468`). The JSON-mode worker path writes those same event lines to
`raw_stream.jsonl`, but `translate_pi_stream` (`src/stream_event.rs:464`) only harvests
`session`/`tool_execution_*`/`turn_end` events — **it drops `type:"error"` and
`type:"response"` lines on the floor**, and `classify_from_raw_stream` never looks for them.

**Net effect today:** when a pi worker hits an OpenRouter 429/402/overload, the task fails
with `failure_class = AgentExitNonzero` and a generic `failure_reason = "Agent exited with
code N"`. The OpenRouter `error_type`, the status code, and the `Retry-After` are present in
`raw_stream.jsonl` but **never parsed**. `wg recover --filter error~credit` can't match
either, because the recorded reason is generic (§4.5).

This is the single highest-value fix: extend `translate_pi_stream` to forward pi error
events, and `classify_from_raw_stream` to parse them (status / `error_type` / provider codes)
into the new `FailureReason` signal (§5).

### 4.3 The native (`nex`) executor already does the deep parse — but it's isolated

`src/executor/native/openai_client.rs` already implements almost the entire detector
described in §5, because it owns the HTTP connection:

- `max_retries_for_status` (`:1622`): 429→5 retries, 5xx→3, else 0.
- `parse_retry_after_oai` (`:1897`): reads `error.metadata.retry_after` (seconds → ms).
- `parse_openrouter_provider_error` (`:1759`): reads `error.metadata.raw` →
  `{provider_name, upstream_type, upstream_code, upstream_message}` and renders the
  "OpenRouter provider X returned Y. This is provider-side, not a local API-key/config
  failure" message.
- `build_openrouter_free_route_suggestion` (`:1820`): detects `:free` route quota/credit/
  capacity issues from the combined upstream text and suggests the paid variant.
- `oai_api_error_with_hint` (`:1862`): appends a `[[llm_endpoints.endpoints]]` config hint for
  401/403.
- jittered exponential backoff with status-aware retry gating (`:1341` streaming,
  `:924` non-streaming).

**Real calibration example** (test fixture `test_openrouter_provider_error_metadata_raw_is_rendered`,
`src/executor/native/openai_client.rs:5167`):

```json
{"error":{"message":"Provider returned error","code":402,
  "metadata":{"provider_name":"Crucible",
    "raw":"{\"error\":{\"type\":\"insufficient_quota\",\"code\":\"insufficient_quota\",\"message\":\"Out of credits. Top up at /dashboard/billing to continue.\"}}"}}}
```

→ rendered as `API error 402: … OpenRouter provider Crucible returned insufficient_quota:
Out of credits. …`. **This is exactly the signal the pi path needs but does not get.**

**Detector reuse.** The parsing logic in §5 should be factored out of `openai_client.rs` into
a pure `parse_openrouter_error_envelope(body: &str) -> Option<ParsedProviderError>` that **both**
the native executor (HTTP body) and the subprocess classifier (text scan of `raw_stream.jsonl`
/ `output.log`) call. Today they don't share code; the native path parses structured JSON, the
subprocess path does a substring scan.

### 4.4 Cost / budget telemetry that already exists

- **Per-task**: `Task.token_usage` (`src/graph.rs:596`) — `{input, output, cache_read,
  cache_creation, cost_usd}` populated by `parse_token_usage` (which has a pi branch,
  `extract_pi_token_usage` at `:1143`, summing `turn_end.message.usage` once per turn) and by
  the `wg pi-stream-bridge` (`translate_pi_stream`, §4.2) canonicalization.
- **Per-session (coordinator)**: `SessionCostTracking` (`src/commands/service/mod.rs:663`) —
  `session_cost_usd`, `session_start`, `last_key_check`, `key_status:
  Option<OpenRouterKeyStatus>`. The coordinator periodically polls `GET /api/v1/key`
  (`should_check_key_status`, `:690`) and caches the budget object.
- **`OpenRouterKeyStatus`** (`src/executor/native/openai_client.rs:2393`) — mirrors
  `/api/v1/key`: `limit`, `limit_remaining`, `usage`, `usage_daily`, `usage_weekly`,
  `usage_monthly`, `is_free_tier`, plus `usage_percentage()` / `is_near_limit(buffer)`.
- **CLI surface**: `wg openrouter status` (`src/commands/openrouter.rs:29`) prints
  credit-limit/remaining/usage/free-tier + cost-cap config; `wg key` (`src/commands/key.rs`)
  validates a key and reports `limit_remaining`/`rate_limit`; `wg openrouter session` shows
  session cost.

**What's missing for the controller:** there is **no rolling window of recent failures per
(provider, model)**, and **no persisted `FailureReason`** — `failure_class` is a single enum
value overwritten on each retry, and `failure_reason` is free-text. A controller that wants
"have we seen ≥3 `rate_limit` from `openrouter:z-ai/glm-5.2:free` in the last 10 min?" has no
structured history to query (§6).

### 4.5 `wg recover` proves the fragility

`wg recover --filter error~credit` (`src/commands/recover.rs:197`) does a **substring match**
of `credit` against `task.failure_reason` prose. This works only when a human happened to type
"credit" in the reason, and it is the only structured-ish way to find credit-exhausted tasks
today. A normalized `FailureReason::CreditExhausted` field makes this `filter
reason=credit-exhausted` and exact.

---

## 5. Detector spec — normalized `FailureReason` signal

### 5.1 The signal

A single normalized, machine-readable signal emitted **once per failed attempt**, in addition
to (not replacing) the existing `FailureClass`. `FailureClass` stays the *retry-policy* axis
(used by `fail.rs` / cycle logic); `FailureReason` is the *provider-telemetry* axis the
supervisor/controller thresholds on.

```rust
// new, src/graph.rs (alongside FailureClass) — or src/telemetry/mod.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureReason {
    RateLimit,          // 429 / rate_limit_exceeded (platform OR provider)
    CreditExhausted,    // 402 / payment_required / insufficient_quota
    QuotaToken,         // token_limit_exceeded (credit/token cap)
    Auth,               // 401 authentication / 403 permission_denied (guardrail)
    ProviderUnavailable,// 503 provider_unavailable
    ProviderOverloaded, // 529/503 provider_overloaded
    Transient5xx,       // 500/502 server / model down (masked)
    Timeout,            // 408/504 timeout
    Hard,               // 400 invalid_request / context_length_exceeded / config
    Disk,               // local ENOSPC (ResourceExhaustedDisk)
    HardTimeout,        // local exit 124
    Unknown,            // unmapped / AgentExitNonzero
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FailureSignal {
    pub reason: FailureReason,        // normalized category
    pub confidence: f32,              // 0.0–1.0 (status+error_type=1.0; msg-substring=0.5)
    pub http_status: Option<u16>,     // 429/402/... if observed
    pub error_type: Option<String>,   // OpenRouter typed code: rate_limit_exceeded, ...
    pub provider_code: Option<serde_json::Value>, // upstream provider's original code
    pub retry_after_secs: Option<f64>, // Retry-After / error.metadata.retry_after
    pub executor: ExecutorKind,        // native | pi | claude | codex
    pub route: Option<String>,         // e.g. "pi:openrouter:z-ai/glm-5.2:free"
    pub detected_at_ms: i64,
}
```

**Confidence ladder** (so the controller can weight a low-confidence signal less):
- **1.0** — parsed a numeric HTTP status **and** an `error_type` string from a structured body.
- **0.8** — numeric status only (`api_error_status` / pi `code`), no typed code.
- **0.5** — substring match on the human message (e.g. "insufficient credits", "rate limit")
  with no status.
- **0.2** — only exit code / no provider text (current `AgentExitNonzero` default).

### 5.2 Detection rules (executor → rule → signal)

Rules are listed in evaluation order (first match wins; a rule fires only if its evidence is
present). "Field" = JSON path scanned in the structured event; "body" = the textual tail.

#### (a) native (`nex`) executor — already structured

Reuse `openai_client.rs`'s parse, but emit a `FailureSignal`:

| Evidence | reason | status | error_type | retry_after |
|---|---|---|---|---|
| HTTP 402 / body `payment_required` / msg "insufficient credits" | `CreditExhausted` | 402 | `payment_required` | — |
| HTTP 429 / body `rate_limit_exceeded` | `RateLimit` | 429 | `rate_limit_exceeded` | `parse_retry_after_oai` |
| HTTP 429 + `metadata.provider_code` present | `RateLimit` (provider) | 429 | `rate_limit_exceeded` | as above |
| body `token_limit_exceeded` | `QuotaToken` | 400 | `token_limit_exceeded` | — |
| HTTP 401 / `authentication` | `Auth` | 401 | `authentication` | — |
| HTTP 403 / `permission_denied` | `Auth` | 403 | `permission_denied` | — |
| 529 / `provider_overloaded` | `ProviderOverloaded` | 529 | `provider_overloaded` | short |
| 503 / `provider_unavailable` | `ProviderUnavailable` | 503 | `provider_unavailable` | short |
| 500/502 / `server` | `Transient5xx` | 500/502 | `server` | short |
| 408/504 / `timeout` | `Timeout` | 408/504 | `timeout` | — |
| 400 `invalid_request`/`context_length_exceeded` | `Hard` | 400 | … | — |

#### (b) pi CLI handler — the new parse (closes the §4.2 gap)

Scan `raw_stream.jsonl` (and `output.log` tail) for pi's error events **and** any inline
OpenRouter envelope:

1. pi `{"type":"error", "error": <str|obj>}` or `{"type":"response","success":false,"error":<…>}`:
   the `error` value is usually a string like `"API error 402: Insufficient credits…"` or an
   object `{status, message, error_type, metadata}`. Parse it: if it carries a numeric status
   / `error_type`, classify per table (a); else substring-fallback on the message.
2. A raw OpenRouter envelope line (the JSON `{...,"error":{"code":402,"message":"Insufficient credits","metadata":{"error_type":"payment_required"}}}`)
   may appear verbatim in `output.log` (pi's stderr) — scan for `"error":{"code":NNN` and run
   the same `parse_openrouter_error_envelope` as the native path.
3. Substring ladder (confidence 0.5), **order matters**:
   - `"insufficient credits"` / `"out of credits"` / `"payment required"` / `"insufficient_quota"` → `CreditExhausted`
   - `"rate limit"` / `"rate_limit"` / `"too many requests"` → `RateLimit`
   - `"overloaded"` / `"capacity"` → `ProviderOverloaded`
   - `"unavailable"` / `"no provider"` / `"no available model"` → `ProviderUnavailable`
   - `"unauthorized"` / `"invalid api key"` / `"forbidden"` → `Auth`
   - `"timed out"` / `"timeout"` → `Timeout`
   - else → `Unknown` (not `AgentExitNonzero` — that becomes a `FailureClass`, not a reason)
4. Extract `Retry-After` / `retry_after` if the string contains `retry_after:<n>` or a
   `Retry-After:` header echo.

`translate_pi_stream` must be extended to **forward** `type:"error"` / `type:"response"`
(success:false) as canonical `StreamEvent::Error { message, status, error_type }` events so
they survive into `stream.jsonl` (the TUI/`wg show` path), and the classifier consumes the
same parse.

#### (c) claude / codex CLI — unchanged, broadened

Keep `extract_api_error_status` (numeric status) but also try
`parse_openrouter_error_envelope` on the line containing it (claude echoes the OR body in its
`message`), and add a 402 arm: today 402 falls through to `AgentExitNonzero`.

| Evidence | reason |
|---|---|
| `api_error_status:402` | `CreditExhausted` (NEW — currently unclassified) |
| `api_error_status:429` | `RateLimit` |
| `api_error_status:401/403` | `Auth` |
| `api_error_status:408/504` | `Timeout` |
| `api_error_status:5xx` | `Transient5xx` (500/502) / `ProviderOverloaded`(529) / `ProviderUnavailable`(503) |
| `api_error_status:400` + "could not process (pdf|document|image)" | `Hard` (→ `ApiError400Document`) |

#### (d) process-level (no provider signal)

| Evidence | reason |
|---|---|
| exit 124 | `HardTimeout` |
| ENOSPC strings (`raw_stream_classifier::looks_like_disk_exhaustion`) | `Disk` |
| missing/empty raw_stream + non-zero exit | `Unknown` (wrapper-internal) |

### 5.3 The "mid-stream trap" (§2.6) in the detector

A pi worker that streams and then hits a 429 **after** the first token will exit with the
stream truncated; pi emits a `type:"error"` (or the in-stream OR chunk) but possibly **exit
code 0** if pi itself treats it as a normal end-of-stream. The detector must therefore run on
**both** the failure path (non-zero exit) **and** be invokable on the success path when
`turn_count == 0` or the final text is empty — i.e. wire it into the same "no operational
output" / "agent-no-work" gate the wrapper already has (`src/commands/spawn/execution.rs:2880`)
so a mid-stream rate-limit on an otherwise-clean exit still produces a `RateLimit` signal
instead of `NoOperationalOutput`.

---

## 6. Telemetry persistence — the rolling failure window

The controller needs history, not just the latest failure. Spec:

### 6.1 Storage

A new append-only, size-bounded JSONL under the coordinator state dir
(`.wg/service/provider-telemetry.jsonl`), one record per **failed attempt** (not per task —
retries each emit):

```json
{"ts":"2026-07-25T14:08:29Z","task":"study-rate-limit","attempt":1,
 "executor":"pi","route":"pi:openrouter:z-ai/glm-5.2:free",
 "reason":"rate-limit","confidence":0.8,"http_status":429,
 "error_type":"rate_limit_exceeded","retry_after_secs":12.5,
 "provider_code":null,
 "credit_remaining_usd":null,"is_free_tier":true}
```

- **Keyed by `(executor, route-bucket)`** where `route-bucket` normalizes the model
  (`pi:openrouter:z-ai/glm-5.2:free` and `pi:openrouter:z-ai/glm-5.2` bucket together for
  credit/rate purposes because the free-variant daily cap is account-global; see §2.2).
- **Bounded**: keep the last N=1000 records OR 24 h, whichever is smaller; prune on append
  (same pattern as the graph log). Old records can roll into the archive.
- **Atomic append** via the existing `modify_graph`-style single-writer pattern
  (`std::fs::OpenOptions::append` + temp-rename on prune), so concurrent workers don't corrupt
  it.

### 6.2 Augment `SessionCostTracking`

Add to `SessionCostTracking` (`src/commands/service/mod.rs:663`) a cached aggregate the
coordinator reads cheaply (no full scan):

```rust
pub struct ProviderHealth {
    pub bucket: String,                       // "pi:openrouter:z-ai/glm-5.2"
    pub last_reason: FailureReason,
    pub recent: WindowCounters,               // last 1m / 5m / 1h counts per reason
    pub consecutive_rate_limits: u32,
    pub consecutive_credit_exhausted: u32,
    pub last_retry_after_secs: Option<f64>,
    pub credit_remaining_usd: Option<f64>,    // from key_status
    pub cooled_until_ms: Option<i64>,         // backoff deadline the controller should honor
}
```

`cooled_until_ms` is the single field the adaptive-parallelism controller thresholds on: "do
not spawn a worker on this bucket until `now >= cooled_until_ms`". It is set from the max of
the observed `Retry-After` and an exponential backoff seeded by
`consecutive_rate_limits`.

### 6.3 Recording sites (exactly where in WG the signal is emitted)

1. **`src/commands/fail.rs::run_inner`** — when recording `failure_class`, also resolve +
   persist the `FailureSignal` and append a telemetry record. `fail.rs` already resolves
   `token_usage` via `AgentRegistry` → `output.log` path; reuse that path to run the detector
   on `raw_stream.jsonl` + `output.log`.
2. **`src/commands/spawn/execution.rs` wrapper** — after `wg fail --class`, also call a new
   `wg record-telemetry --task T --exit-code N --raw-stream $RAW_STREAM` (mirrors
   `wg classify-failure`) so the signal is captured even if `wg fail` is later superseded.
3. **native executor retry loop** (`src/executor/native/openai_client.rs`) — on a *terminal*
   retry exhaustion (after the in-process backoff gives up), emit the `FailureSignal` for the
   last error so the in-process path and the subprocess path feed the same telemetry.

### 6.4 Controller queries

- `failure_rate(bucket, window) -> f32` — fraction of recent attempts with reason ∈
  {RateLimit, ProviderOverloaded, ProviderUnavailable, Transient5xx}.
- `is_cooled(bucket, now) -> bool` — `now < cooled_until_ms`.
- `credits_depleted(key) -> bool` — `key_status.limit_remaining <= 0` (already pollable) **or**
  ≥1 `CreditExhausted` in the window (the body-derived signal, which fires even when the poll
  is stale).

The adaptive-parallelism controller combines: **if** `is_cooled(bucket)` **or**
`credits_depleted` **or** `failure_rate(bucket, 5m) > threshold`, reduce parallelism for that
bucket / fail over to another profile (`wg profile use nex`), exactly the round-trip described
in the project guide.

---

## 7. Mapping to exact WG source locations

| Concern | File:line | Change shape |
|---|---|---|
| `FailureClass` enum (retry-policy axis) | `src/graph.rs:129` | leave as-is; add `FailureReason` + `FailureSignal` alongside (§5.1) |
| Task fields | `src/graph.rs:502` (`failure_reason`), `:506` (`failure_class`), `:596` (`token_usage`) | add `failure_signal: Option<FailureSignal>` + `attempt_history` ref |
| Subprocess classifier | `src/commands/spawn/raw_stream_classifier.rs:34` (`classify_from_raw_stream`), `:197` (`extract_api_error_status`) | add pi `type:"error"`/`response:success:false` parse + 402 arm + body-envelope parse → emit `FailureSignal` |
| Wrapper failure path | `src/commands/spawn/execution.rs:2855-2928` | add `wg record-telemetry` call after `wg fail` (§6.3) |
| Pi stream translation (drops errors) | `src/stream_event.rs:464` (`translate_pi_stream`), match arm `_ => {}` at the end | forward `type:"error"` + `type:"response"`(success:false) as `StreamEvent::Error` |
| Pi RPC error capture (reference impl) | `src/commands/pi_handler.rs:182-235` (`RpcTurnAccumulator::ingest`) | reuse the same field extraction in the JSON-mode translator |
| Native deep parse (reference + reuse) | `src/executor/native/openai_client.rs:1622` (`max_retries_for_status`), `:1732` (`oai_api_error_for_provider`), `:1759` (`parse_openrouter_provider_error`), `:1820` (free-route suggestion), `:1862` (`oai_api_error_with_hint`), `:1897` (`parse_retry_after_oai`) | extract `parse_openrouter_error_envelope` into a shared pure fn both paths call; emit `FailureSignal` on terminal failure |
| Cost / budget state | `src/commands/service/mod.rs:663` (`SessionCostTracking`), `:690` (`should_check_key_status`) | add `ProviderHealth` map + `cooled_until_ms`; poll `/key` more eagerly after a `RateLimit`/`CreditExhausted` |
| Key status type | `src/executor/native/openai_client.rs:2393` (`OpenRouterKeyStatus`) | already complete; feed `limit_remaining`/`is_free_tier` into telemetry |
| Per-task usage (pi branch) | `src/graph.rs:1062` (`parse_token_usage`), `:1143` (`extract_pi_token_usage`) | no change; the cost side is already correct |
| Record path | `src/commands/fail.rs:17` (`run`) / `run_inner` | resolve + persist `FailureSignal`, append telemetry record (§6.3) |
| CLI: classify | `src/commands/classify_failure.rs` | add `--json` emitting the full `FailureSignal`, not just the kebab |
| CLI: recover substring filter | `src/commands/recover.rs:197` (`error~X`) | add `reason=<kebab>` exact filter on the new field |
| CLI: status surfaces | `src/commands/openrouter.rs:29` (`status`), `src/commands/key.rs:402` (`check_openrouter_credits`) | surface `cooled_until`, recent failure counts |
| CLI: kebab help | `src/cli.rs:557` (lists `api-error-429-rate-limit` etc.) | extend doc with the new reasons |

---

## 8. Sources (OpenRouter live docs, fetched 2026-07-25)

1. **API Credit & Rate Limits — Handle 402 and 429 Errors** —
   https://openrouter.ai/docs/api_reference/limits
   (credit vs. rate table; free-model RPD/RPM = 50/day <10 credits, 1000/day ≥10 credits,
   20/min; `/api/v1/key` `Key` shape; `X-RateLimit-*` headers; mid-stream rate limits as SSE
   `finish_reason:"error"`; platform-vs-provider 429).
2. **API Error Handling and Debugging — Complete Guide** —
   https://openrouter.ai/docs/api_reference/errors-and-debugging
   (HTTP status list; `Retry-After` on 429/503; typed `error_type` table; skin-specific
   `error_type` locations; pre-stream vs. mid-stream; error-code→`length` transformations;
   provider-error masking on 500).
3. **OpenRouter FAQ** — https://openrouter.ai/docs/faq
   (free-model RPD keyed on credits purchased, not balance; "additional accounts/keys do not
   affect rate limits, capacity is governed globally").
4. **Limits (reference)** — https://openrouter.ai/docs/api/reference/limits.mdx
   (raw constants: `FREE_MODEL_RATE_LIMIT_RPM`, `FREE_MODEL_HAS_CREDITS_RPD`,
   `FREE_MODEL_CREDITS_THRESHOLD = 10`).
5. **PaymentRequiredResponseError (TypeScript SDK)** —
   https://openrouter.ai/docs/agent-sdk/typescript/errors/paymentrequiredresponseerror
   (canonical 402 body: `{"code":402,"message":"Insufficient credits. Add more using https://openrouter.ai/credits"}`).
6. **OpenRouter Rate Limits – What You Need to Know (support)** —
   https://openrouter.zendesk.com/hc/en-us/articles/39501163636379
   (50/day & 20/min free; 1000/day ≥$10 credits; paid = no platform cap; build smart retries).

## 9. Spark boundary / what this study does NOT do

- **No code changes.** This is the spec. Implementation is a follow-up task (the synthesis
  roadmap consumes it). The two smallest, highest-leverage changes are (1) extend
  `translate_pi_stream` to forward pi error events and (2) teach `classify_from_raw_stream`
  the 402 arm + body-envelope parse; together they close the §4.2 gap.
- **No live firing against OpenRouter** — the OpenRouter facts are from the live docs (§8) and
  the in-repo calibration fixtures (`src/executor/native/openai_client.rs:5167`). The main
  graph (`/home/bot/wg/.wg/graph.jsonl`) has no recorded 402/429 today precisely *because* the
  current classifier drops them to `AgentExitNonzero` — which is itself the finding.
- **No new compat const / wire format.** `FailureSignal` is local telemetry; it does not
  change the WG-Fed / WG-Exec / WG-Review envelopes.
