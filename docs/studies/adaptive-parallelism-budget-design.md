# Adaptive Parallelism & Cost/Time Budget Controller — Design

**Status:** design study (sibling of `ratelimit-cost-telemetry-design.md` and
`supervisor-hard-agent-design.md`; feeds `synthesis-roadmap-from`).
**Scope:** the control loop that turns the rate-limit / cost signal into the
`max_agents` knob, and the budget model that expresses limits as cost/time.
**Out of scope:** the failure-detection taxonomy itself (owned by
`ratelimit-cost-telemetry-design.md`); the graph-health reset policy (owned by
`supervisor-hard-agent-design.md`). This controller **consumes** the failure
signal and **owns** the parallelism knob; the supervisor owns reset. The
boundary is defined in §9.

---

## 1. Objective & design thesis

WG dispatches work as a fixed fan-out: the coordinator tick computes
`slots_available = max_agents - alive_count` and spawns that many agents
(`src/commands/service/coordinator.rs:4850`). Today `max_agents` is a static
config value — it does not react to the provider's rate-limit signal or to the
project's measured spend rate. The result is one of two failure modes:

- **Too high:** the project trips OpenRouter's free-tier caps (≈20 RPM / 50
  req/day on free models), agents spend their wall-clock in 429 backoff, and
  the zero-output circuit breaker (`src/commands/service/zero_output.rs`)
  eventually trips a *global* spawn pause (§7.1) — a coarse all-or-nothing
  response to what is really a *graduated* pressure signal.
- **Too low:** the project leaves allocated capacity (credits, RPM headroom)
  unused, finishing slower than it could.

**Thesis.** The controller is a **slow additive-up / subtractive-down** loop
with hysteresis that keeps the project riding at ≈100% of *allocated* resources
— measured as (a) near-zero sustained 429s, (b) spend rate tracking the budget,
and (c) a target busy-fraction of the `max_agents` slots. It replaces the
binary "spawn / global-pause" response to rate pressure with a graduated knob,
while leaving the existing global-outage breaker as a hard last-resort floor.

The controller is **cost/time budgeted**: limits are expressed per provider/model
as USD/interval and requests/interval, and the live `max_agents` is derived as
the largest value that satisfies *both* the rate budget and the spend budget
within the floor/ceiling envelope (§6).

---

## 2. Control objective — what "100% of allocated resources" means operationally

"100% of allocated resources" is **not** a single number; it is a multi-objective
target the controller steers toward. Operationally, the loop is "doing well"
when, over the trailing control window `W` (default **5 min**, see §5.2):

| Metric | Target | Hard guardrail |
|---|---|---|
| **Sustained-429 rate** (rate-limit-class failures per dispatched request) | ≤ 1% | if ≥ 10% sustained → force a cut (§4.2) |
| **Slot busy-fraction** (`alive_count / effective_max_agents`, averaged over `W`) | 0.9 – 1.0 | floor (§7.2) prevents starving below the busy target |
| **Spend rate** (USD / hour, project-wide) | tracks `budget.usd_per_hour` | ceiling (§7.2) caps `max_agents` so projected spend ≤ budget |
| **Request rate** (req / min, per provider/model) | ≤ 0.8 × published RPM | hard cut if exceeded (rate budget, §6.2) |

**Why these and not "spawn as many as possible":** the *429 rate* is the
provider's direct feedback that we are over-subscribed; the *busy-fraction*
proves we are not under-subscribed (slots are actually filled); the *spend
rate* proves we are honoring a human-set budget rather than just filling slots
with cheap work. A loop that only watched 429s would happily run 50 agents on a
free key until the daily cap evaporated; a loop that only watched busy-fraction
would ignore the budget. The controller balances all three, with 429 safety
winning ties (§4.2 — a sustained 429 signal always forces a cut regardless of
the other metrics).

---

## 3. Inputs

The controller runs *inside the daemon process* (it is a tick of the
long-lived daemon loop, not an ephemeral agent — see §8 on where it lives). Its
inputs are all already collected by the daemon or by the agent spawn path:

1. **The failure-reason signal** (from `ratelimit-cost-telemetry-design.md`).
   The sibling study specifies a normalized classification emitted from parsed
   agent output:
   - `rate-limit` (HTTP 429, `X-RateLimit-*` / `Retry-After` headers),
   - `credit-exhausted` (HTTP 402 — *not* retriable; budget is zero),
   - `auth` (401/403 — key problem, never a parallelism issue),
   - `transient` (5xx / timeouts),
   - `hard` (non-retriable, e.g. 400-document).
   The controller thresholds on the **`rate-limit`** class (and treats
   `credit-exhausted` as a budget=0 hard-stop, §7.4). Today the closest
   primitive is `graph::FailureClass::ApiError429RateLimit`
   (`src/graph.rs:129`, detected by
   `src/commands/spawn/raw_stream_classifier.rs:60`) — the telemetry study
   extends this into the richer per-(provider,model) rolling signal the
   controller consumes. **Interface contract:** the controller reads a rolling
   window of `{timestamp, provider, model, failure_class, confidence}` records
   from the telemetry store the sibling study defines; it never re-parses raw
   streams itself.

2. **Measured throughput / cost** — already on the graph:
   `task.token_usage` (`src/graph.rs:596`, struct at `src/graph.rs:1004`) with
   `cost_usd`, `input_tokens`, `output_tokens`. Aggregated by
   `aggregate_usage_stats` (`src/usage.rs:75`) and surfaced by `wg spend`
   (`src/commands/spend.rs`). The controller reads the trailing-window sum of
   `cost_usd` and a count of completed tasks to compute spend rate and
   throughput.

3. **Model pricing** — `ModelRegistryEntry.cost_per_input_mtok` /
   `cost_per_output_mtok` (`src/config.rs:1721`) and the OpenRouter free-tier
   RPM/RPD caps (§6.1). Used to project a candidate `max_agents` into a
   projected spend/req rate (§6.3).

4. **Current dispatch state** — `alive_count` and `effective_max_agents`, both
   already computed in the coordinator tick
   (`src/commands/service/coordinator.rs:4815,4850`).

### 3.1 Reaction speed — sustained vs blip

A single 429 is a **blip**; a cluster of 429s across multiple agents inside `W`
is a **sustained** signal. The controller distinguishes them by requiring a
*count threshold*, not just an occurrence:

- **Blip** (e.g. 1–2 isolated 429s, or a 5xx that self-resolves): no
  `max_agents` change. The existing per-agent retry/backoff already absorbs
  these; the controller must not flap on noise.
- **Sustained** (≥ `sustain_count` rate-limit signals within `W`, default
  **3**): force a subtractive-down cut (§4.2).
- **Quorum across agents**: a rate-limit signal that touches ≥ 2 *distinct*
  agents in the same window is treated as sustained even below the raw count
  threshold (it is a provider-wide signal, not one agent's bad luck).

The asymmetry — *slow to add, faster to cut* — is the core anti-oscillation
property (§4.2, §5.1).

---

## 4. Control policy: additive-up / subtractive-down + hysteresis

### 4.1 The knob

The single output is `effective_max_agents ∈ [floor, ceiling]`. It is the
*only* thing the controller writes. It does **not** touch the coordinator's
spawn logic, the zero-output breaker, or task state — it sets the cap and the
existing `slots_available = max_agents - alive_count` math
(`coordinator.rs:4850`) does the rest.

### 4.2 Up/down policy

Let `E` = current `effective_max_agents`, `B` = trailing busy-fraction, `R` =
trailing sustained-429 rate, `S` = trailing spend rate, `P_budget` =
budget-derived cap (§6.3).

**Subtractive-down (cut) — triggers, in priority order:**

1. **Rate guardrail (highest priority).** If sustained-429 rate `R ≥ cut_429`
   (default 10%) OR a provider-wide quorum (§3.1) fires: `E ← E − step_down`.
   This always wins over an up-move, even if busy-fraction is high.
2. **Spend guardrail.** If projected spend at current `E` would exceed
   `budget.usd_per_hour` over the next window: `E ← min(E, P_budget)`.
3. **Request-rate guardrail.** If projected req/min at `E` would exceed
   `0.8 × rpm_limit(provider, model)`: `E ← min(E, rpm_cap)`.

`step_down` is **subtractive** and slightly aggressive to shed pressure fast:
default `step_down = max(1, ceil(E / 4))` (≈ −25%, minimum 1). One cut per
control interval — never two — so the loop cannot free-fall.

**Additive-up (grow) — triggers only when ALL hold:**

1. Busy-fraction `B ≥ up_busy` (default 0.95 — slots are actually full, so more
   agents would be used, not idle).
2. Sustained-429 rate `R ≤ up_429` (default **1%** — the provider has headroom).
3. Spend rate `S ≤ 0.8 × budget.usd_per_hour` (room left in the budget).
4. **Cooldown elapsed** since the last *down* move (§5.1) — the hysteresis
   guard that prevents add-cut-add oscillation.
5. `E < min(ceiling, P_budget, rpm_cap)`.

If all hold: `E ← E + step_up`, where `step_up` is deliberately **small and
additive**: default `step_up = 1` (grow by exactly one agent per interval). This
is the "slowly balances" in the objective — growth is linear and cautious; cuts
are proportional and fast. The loop provably cannot oscillate faster than the
cooldown (§5.1 proof sketch).

### 4.3 Hysteresis

Two hysteresis bands keep the loop stable:

- **Busy-fraction band.** Up-move requires `B ≥ 0.95`; a *down*-move on
  under-utilization is **not** done (we never cut because slots are empty —
  empty slots are free). This avoids the classic thermostat oscillation.
- **Cooldown band** (the dominant stabilizer, §5.1). After any down-move, the
  controller cannot move *up* for `cooldown` (default 90 s) and cannot move
  *down* again for `cut_lockout` (default 30 s). The two windows differ: cuts
  can repeat (a bad provider keeps biting), but growth must wait — you only
  learn whether a grow was safe after the new agent has run a full task.

---

## 5. Timing & reaction dynamics

### 5.1 Cooldown / lockout — anti-oscillation

The two timers (§4.3) compose into a proof-by-construction that the loop is
bounded-rate:

- *Cut-to-cut:* separated by `cut_lockout` (30 s). Worst case the controller
  sheds from ceiling to floor in `ceil((ceiling−floor)/step_down) × 30 s`.
- *Cut-to-grow:* separated by `cooldown` (90 s). A grow can only happen after
  the loop has observed a full window `W` (5 min) of clean (low-429) operation
  *following* the cooldown, so the provider has demonstrably recovered.

Because growth is +1/interval and cuts are proportional, the controller spends
most of its time either steady or slowly growing, and only sheds fast under
genuine pressure — exactly the "ride at ~100% without tripping" goal.

### 5.2 Control interval vs coordinator tick

The controller evaluates on **its own cadence**, decoupled from the coordinator
spawn tick (which can run every few seconds). Default `control_interval = 60 s`.
This is important: spawning is fast and cheap, but *parallelism decisions* must
be slow to avoid reacting to blips. Concretely, the daemon loop already ticks
on `daemon_cfg.poll_interval` (`src/commands/service/mod.rs:3177`); the
controller piggy-backs a parallelism-evaluation every `control_interval`,
reading the rolling window rather than the instantaneous state.

---

## 6. Cost/time budget model

### 6.1 Grounding — real OpenRouter limits (cited)

From OpenRouter's own docs/help (verified 2025–2026):

- **Free keyless models:** **50 requests/day** total, **20 requests/minute**
  ([OpenRouter FAQ](https://openrouter.ai/docs/faq),
  [Rate Limits help](https://openrouter.zendesk.com/hc/en-us/articles/39501163636379-OpenRouter-Rate-Limits-What-You-Need-to-Know)).
- **HTTP 429** returns `X-RateLimit-Limit` / `X-RateLimit-Remaining` /
  `X-RateLimit-Reset` and a `Retry-After` header
  ([errors & debugging](https://openrouter.ai/docs/api/reference/errors-and-debugging)).
- **HTTP 402** = **insufficient credits** — *not* a rate limit; not retriable.
  Check balance via `GET /api/v1/credits`
  ([credits endpoint](https://openrouter.ai/docs/api/api-reference/credits/get-credits)).
- **BYOK:** first 1M requests/month free, then 5% fee; provider-side RPM/RPD
  still applies ([BYOK](https://openrouter.ai/docs/guides/overview/auth/byok)).
- **Paid/keyed models:** no OpenRouter-enforced RPM cap, but upstream providers
  (Anthropic/OpenAI/Google) enforce their own.

The controller treats these as **per-(provider,model) budgets**, not a single
global number, because a free `:free` model and a paid `claude-opus-4-7` have
wildly different ceilings.

### 6.2 Budget schema (proposed config)

A new optional section, additive to existing config (no change to current
behavior when absent — controller stays at the static `max_agents`):

```toml
[budget]
enabled = true
# Project-wide spend ceiling. The controller caps effective_max_agents so the
# projected USD/hour never exceeds this.
usd_per_hour = 2.0
# Optional hard daily wall (USD). Reaching it triggers the kill-switch (§7.4).
usd_per_day  = 20.0

# Per-provider/model rate budgets. Keyed by handler:model-spec fragment.
# When absent, the controller uses the model-registry pricing + the OpenRouter
# free-tier defaults from §6.1 to *infer* a safe cap.
[[budget.limits]]
model = "openrouter:anthropic/claude-opus-4-7"
rpm   = 50          # requests/min ceiling (0.8× applied as the soft cap)
usd_per_hour = 1.0  # spend ceiling for this model alone

[[budget.limits]]
model = "openrouter:z-ai/glm-4.5:free"
rpm   = 15          # well under the 20 RPM free-tier cap
usd_per_hour = 0.0  # free model — spend is 0, RPM is the only constraint
```

### 6.3 Translating a budget into a `max_agents` cap

Given a budget `B` (USD/hour) and a model with pricing `p_in`/`p_out`
(USD/Mtok) and a measured mean per-task cost `c̄` (from trailing window) and
mean task duration `d̄`:

```
projected_spend_per_hour(E) = E × (c̄ / d̄_hours)
spend_cap   = largest E such that projected_spend_per_hour(E) ≤ B.usd_per_hour
rpm_cap     = largest E such that (E / d̄_minutes) ≤ 0.8 × B.rpm
P_budget    = min(spend_cap, rpm_cap)
```

The controller then enforces `effective_max_agents ≤ P_budget` as one of the
up-move guards (§4.2). When `c̄` or `d̄` are unknown (cold start), it falls back
to a conservative per-model default (from the registry pricing and a default
`d̄ = 120 s`) and relaxes as real measurements arrive. For **free models**
(`usd_per_hour = 0`), only the `rpm_cap` binds — which is exactly the
free-tier-tripping failure mode the controller exists to prevent.

### 6.4 Per-project vs global

- **Spend** is **per-project** (per `--dir` graph): `task.token_usage` and
  `aggregate_usage_stats` are graph-local, and a human's USD budget is almost
  always per-effort. The `usd_per_hour` ceiling is therefore a project-local
  config value.
- **Rate** (RPM/RPD) is **per-(provider,model) credential**: the same
  OpenRouter key has *one* 20-RPM free cap regardless of how many WG projects
  share it. The controller therefore reads the rate budget from a
  **credential-scoped** store (the active profile's endpoint key identity),
  and when multiple projects share a key, each project's controller must
  coordinate on the *shared* RPM budget. The minimal version (this study):
  rate caps are a single project's configured fraction of the published limit;
  multi-project RPM coordination (a shared semaphore keyed by credential) is
  called out as §10 future work.

---

## 7. Safety envelope

The controller is bounded by a hard floor/ceiling, a cooldown, a human
override, and a kill-switch. None of these are advisory — they are enforced in
the single read path that produces `effective_max_agents` (§8.2).

### 7.1 Composition with the existing global-outage breaker

The zero-output sweep already implements a coarse adaptive backoff
(`src/commands/service/zero_output.rs`): when ≥50% of alive agents (≥2 agents)
have zero output, it trips a **global spawn pause** with an exponential backoff
starting at 60 s (`GLOBAL_OUTAGE_RATIO=0.5`, `GLOBAL_OUTAGE_MIN_AGENTS=2`,
`INITIAL_BACKOFF=60s` at `zero_output.rs:32,35,41`). This is the **hard floor's
safety net**: it catches total provider outage faster than the graduated
controller's windowed signal can. **Composition rule:** the global breaker and
the graduated controller never fight — the breaker *overrides* the controller
when it trips (pause-all beats any `effective_max_agents`), and the controller
*treats a breaker trip as a forced cut* (next evaluation starts from a reduced
`E`, with cooldown armed). The breaker is the floor; the controller is the
throttle between floor and ceiling.

### 7.2 Floor / ceiling

- **`floor`** (default 1): the controller never reduces `effective_max_agents`
  below this. Set per-project. Floor = 1 guarantees forward progress even under
  sustained rate pressure (one agent at a time is always legal).
- **`ceiling`** (default = the static `[coordinator].max_agents` from config, or
  a per-project override): the controller never exceeds this. This is the
  human's "how many agents am I willing to run at all" guard, independent of
  budget.

Both are **clamped** after every controller move: `effective_max_agents =
clamp(E, floor, ceiling)`.

### 7.3 Cooldown after a cut

Defined in §5.1 (`cooldown = 90 s` cut-to-grow, `cut_lockout = 30 s`
cut-to-cut). The controller persists its last-move timestamps
(`last_down_at`, `last_up_at`) in the same JSON state file as the zero-output
breaker (`dir/service/...`) so they survive a daemon restart — a restart must
not reset the hysteresis and immediately re-grow into a provider that is still
rate-limiting.

### 7.4 Kill-switch & human override

- **Kill-switch (`budget.enabled = false` or `budget.pause = true`):** the
  controller freezes `effective_max_agents` at its current value and emits no
  moves. Reachable via a single CLI (`wg budget pause`) and a config flag.
- **Credit-exhausted hard-stop:** when the telemetry signal reports
  `credit-exhausted` (HTTP 402) for the active provider, the controller sets
  `effective_max_agents = 0` (or `floor` if a floor > 0 is set and the human
  wants a trickle) and **does not** grow until a non-zero balance is confirmed
  via `/api/v1/credits` (or the human clears the flag). This is the budget
  analog of "money ran out" — no parallelism policy can fix it.
- **Daily-wall kill-switch:** when cumulative spend ≥ `usd_per_day`, same
  hard-stop behavior as credit-exhausted, with a loud log + `wg budget status`
  surface.
- **Human pin:** `wg budget pin <N>` sets a manual `effective_max_agents` that
  the controller will not move away from until `wg budget unpin`. This is the
  "I know better, leave it at 3" escape hatch. A pin is recorded with a
  reason and a TTL (default 1 h) so it cannot be forgotten.

---

## 8. The `max_agents` authority problem — and its concrete fix

This is the central correctness problem found this session, and it must be
solved *before* the controller is useful: **a controller that sets
`max_agents` is worthless if a `service reload` silently overrides it.**

### 8.1 The observed bug (2 → 8 on reload)

There are **four** sources of truth for `max_agents` today, with no defined
precedence:

| # | Source | Lifetime | Where read |
|---|---|---|---|
| 1 | CLI launch arg `--max-agents N` | **transient** — daemon memory only | `run_daemon(...)` builds `DaemonConfig.max_agents = cli_max_agents.unwrap_or(config.coordinator.max_agents)` at `src/commands/service/mod.rs:2415` |
| 2 | `[coordinator].max_agents` in merged config.toml | persistent | `Config::load_merged` → `config.coordinator.max_agents` (`src/config.rs:4089`) |
| 3 | Active profile (overlays `[dispatcher].max_agents` into global config) | persistent (written into config.toml on activation) | `apply_profile_as_global_config` / `overlay_profile_onto_global`, routing keys include `max_agents` (`src/profile/named.rs:473,479`) |
| 4 | Runtime IPC `Reconfigure { max_agents: Some(N) }` | transient — daemon memory | `handle_reconfigure` applies the override (`src/commands/service/ipc.rs:1095`) |

The bug: **the launch arg (source 1) is never persisted back to config.toml.**
So when `wg profile use claude` (or any `wg service reload` *without* a
`--max-agents` flag) fires, it sends
`IpcRequest::Reconfigure { max_agents: None, … }`
(`src/commands/profile_cmd.rs:1205-1210`). `handle_reconfigure` sees
`has_overrides == false` and takes the **else branch** — it re-reads config from
disk and does `daemon_cfg.max_agents = config.coordinator.max_agents`
(`src/commands/service/ipc.rs:1111`). The profile had already written its own
`max_agents = 8` into config.toml (source 3 → source 2), so **the launch arg's
2 is silently replaced by the profile's 8.** This is exactly the observed
behavior, and it is *by construction* — there is no code path that preserves
the launch arg across a flagless reload.

A controller writing `max_agents` would be source #4 (transient IPC), which
suffers the *same* fate: the next flagless reload re-reads config.toml and
clobbers it. So the controller cannot simply IPC-reconfigure the value.

### 8.2 The fix — single authoritative source + runtime overlay

**Principle: `effective_max_agents` is computed in exactly one place, from a
well-defined precedence, and the controller writes to the *highest-precedence
transient* layer so it survives a flagless reload but loses to an explicit
human flag.**

Proposed precedence (highest → lowest), and what must change in code:

1. **Human pin / kill-switch** (§7.4) — absolute; the controller never
   overrides it. Stored in `dir/service/budget_state.json`.
2. **Controller-computed `effective_max_agents`** — written to a new persistent
   field `coordinator.runtime_max_agents` in `CoordinatorState`
   (`src/commands/service/mod.rs:710`). **This is the new authority.** It
   survives daemon restart (it is on disk in the coordinator state file) **and**
   survives a flagless reload, because:
3. **`handle_reconfigure` must learn to respect the runtime override.** The
   fix is a small, surgical change to the else-branch at
   `src/commands/service/ipc.rs:1109-1113`: after re-reading config, if a
   `runtime_max_agents` is present in `CoordinatorState` (i.e. the controller
   is active), *keep it* instead of clobbering with
   `config.coordinator.max_agents`. Concretely:

   ```rust
   // else branch of handle_reconfigure (ipc.rs ~1109)
   daemon_cfg.max_agents = config.coordinator.max_agents;
   // NEW: a controller-managed runtime override wins over the static config,
   //     so a flagless reload (profile swap) does not silently revert the
   //     controller's adaptive value. An explicit --max-agents flag still
   //     wins (it takes the has_overrides branch above and is recorded as a
   //     human pin).
   if let Some(rt) = CoordinatorState::load(dir).runtime_max_agents {
       daemon_cfg.max_agents = rt;
   }
   ```

   This closes the 2→8 bug *for the controller* and, as a side effect, makes
   the launch-arg-vs-reload story coherent: an explicit `--max-agents` flag on
   `reload` is a human action and wins (it's in the `has_overrides` branch and
   should be recorded as a pin); a flagless reload is a config-refresh and
   preserves the controller's adaptive value.

4. **Launch arg `--max-agents`** (source 1) is re-interpreted as a **one-shot
   initial value + an implicit pin for the session** when no controller is
   active. Concretely: at daemon start (`mod.rs:2415`), if `cli_max_agents` is
   `Some(n)` *and* no `runtime_max_agents` is already on disk, write
   `runtime_max_agents = n` into `CoordinatorState` (so it survives reload) and
   arm a session pin. This makes the launch arg's intent durable without
   silently shadowing the controller once the controller is enabled.
5. **`config.coordinator.max_agents`** (source 2) becomes the **ceiling and
   cold-start default only** — it is the human's "max I will ever allow" and
   the value used when the controller is disabled or has no history.

**Net effect:** there is exactly one transient-but-persisted authority
(`CoordinatorState.runtime_max_agents`) that (a) the controller writes, (b) a
flagless reload preserves, (c) an explicit `--max-agents` reload flag overrides
(and pins), and (d) a daemon restart restores from disk. The four sources
collapse into one precedence ladder with no silent-override path. The bug
reproducer ("start with --max-agents 2, `wg profile use`, observe 8") becomes
"observe 2 (preserved), and the controller may then adapt it within budget."

> **Implementation note for the follow-up code task:** the change is ~15 lines
> in `handle_reconfigure` (`ipc.rs`), a new `Option<usize>` field on
> `CoordinatorState` (`mod.rs:710`) with its load/save already present, and a
> `--no-pin` escape on `service start` for tests that want the old behavior.
> No schema migration is needed (the field is `Option`, defaults to `None` =
> today's behavior).

---

## 9. Boundary with the supervisor hard-agent

The sibling study (`supervisor-hard-agent-design.md`) defines a long-lived
**supervisor** hard-agent that wakes on a tick, scans for "dumb failures," and
resets/requeues tasks. The controller and the supervisor are **peers in the
daemon** with a crisp, non-overlapping division of labor. They communicate
through shared, persisted state — never by one calling the other directly.

| Concern | Owner | Mechanism |
|---|---|---|
| **`max_agents` knob** | **controller** (this study) | writes `CoordinatorState.runtime_max_agents` |
| **Task reset / requeue** | **supervisor** | graph writes (`failed` → `ready`) on stuck/dumb-failed tasks |
| **Rate-limit detection** | telemetry study → both consume | shared failure-reason signal store |
| **Global outage (hard pause)** | zero-output breaker (existing) | overrides both when tripped (§7.1) |
| **Credit/budget exhaustion** | **controller** | kill-switch sets `max_agents` to floor/0 (§7.4) |
| **Reset storms / loop prevention** | **supervisor** | per-task attempt caps, backoff, escalation |

**The one rule that prevents them fighting:** the controller **never** resets
tasks, and the supervisor **never** touches `max_agents`. When the supervisor
requeues a batch of stuck tasks (creating a burst of `ready` work), that is
*exactly* the kind of event that could trip a rate limit — but the controller
will see the resulting busy-fraction/429 signal on its *own* cadence and adjust
`max_agents` downward if needed; it does not need a heads-up. Conversely, when
the controller cuts `max_agents`, newly-idle slots just mean the supervisor's
requeued tasks wait longer — they are not reset again. The supervisor's
loop-prevention (max-attempts, backoff) is what keeps a perpetually-rate-limited
task from being reset infinitely; the controller's floor (default 1) is what
guarantees that even a throttled queue still makes progress.

**Shared state contract** (so the two compose without coupling):

- `dir/service/budget_state.json` (controller-owned): `effective_max_agents`,
  `last_down_at`, `last_up_at`, `pin`, `kill_switch`, trailing spend/429
  counters. The supervisor **reads** `effective_max_agents` only to decide
  whether a reset burst would be immediately rate-limited (it may *delay* a
  reset if the controller is at floor and 429s are high — a polite backoff,
  never a hard dependency).
- The supervisor's reset log (its own state) is **read** by the controller only
  to discount "dumb-failure" resets from its throughput denominator (a task the
  supervisor reset for a non-quality reason should not count against the
  provider's apparent success rate).

---

## 10. Metrics that prove it is working

The controller exposes its state via `wg budget status` (JSON + human), the
daemon log, and the existing `wg service status`/`wg spend` surfaces. A loop is
"working" when these hold over a long run:

- **429 rate trending down** after the controller is enabled (the whole point).
- **Busy-fraction in band** (0.9–1.0) most of the time — proving we are not
  leaving capacity idle.
- **Spend ≤ budget** over every hour and day window.
- **Move-rate low** — a healthy controller makes few moves (mostly steady or
  +1/interval). A controller that is cutting every `cut_lockout` is a red flag
  (provider is chronically over-subscribed relative to budget → the human's
  budget/RPM config is wrong, not the loop).
- **No oscillation** — `up` and `down` moves never alternate faster than
  `cooldown`; the persisted move log makes this auditable.

Concrete fields in `budget_state.json`:

```json
{
  "effective_max_agents": 4,
  "floor": 1, "ceiling": 8,
  "last_up_at": "...", "last_down_at": "...",
  "trailing_window": {
    "busy_fraction": 0.97,
    "sustain_429_rate": 0.012,
    "spend_usd_per_hour": 1.7,
    "req_per_min": 38
  },
  "budget": { "usd_per_hour": 2.0, "usd_per_day": 20.0 },
  "kill_switch": null,            // null | "credit_exhausted" | "daily_wall" | "paused"
  "pin": null,                    // null | { "value": 3, "until": "...", "reason": "..." }
  "moves_24h": [ /* {at, dir, from, to, reason} */ ]
}
```

---

## 11. Phased rollout (non-spark → spark → production)

This study is the **design**. The implementation should land in the same
spark-shaped increments the rest of WG uses:

1. **Fix the authority bug (§8.2) first, controller-free.** This is a pure
   correctness fix (the 2→8 reload clobber) that is valuable independently and
   is a prerequisite — without it the controller's writes don't survive. ~15
   lines + a unit test reproducing the reload-clobber and asserting
   preservation.
2. **Controller spark — rate-only, no spend.** Additive-up/subtractive-down on
   the 429 signal only, floor/ceiling/cooldown, persisted state, `wg budget
   status`. No budget model yet. Proves the loop is stable and does not flap
   against a real OpenRouter free key.
3. **Budget model.** Add the `[budget]` config, the spend/rate caps, the
   `max_agents`-from-budget derivation (§6.3), and the credit-exhausted /
   daily-wall kill-switches.
4. **Composition hardening.** Wire the supervisor-reads-controller-state
   contract (§9) and a smoke scenario that drives a real rate-limit burst and
   asserts the controller sheds and recovers without tripping the global
   breaker.

Each phase is independently shippable and independently testable.

---

## 12. Open questions / future work

- **Multi-project shared-key RPM coordination** (§6.4): a cross-project
  semaphore keyed by credential identity, so two WG projects on one OpenRouter
  key share the 20-RPM cap correctly. Out of scope for the spark; the
  per-project configured fraction is the safe default.
- **Adaptive `step_down` / `cooldown`**: the defaults here are conservative
  constants. A future version could tune them from observed provider behavior
  (e.g. tighten cooldown on a provider that recovers fast). Explicitly deferred
  — constant parameters are far easier to reason about and audit.
- **Per-task priority under a cut:** when `max_agents` is cut, which ready
  tasks get the scarce slots? Today it's graph order; a budget-aware priority
  (premium-tier tasks first when spend is tight) is a natural extension but
  belongs to the dispatch policy, not this controller.
- **Predictive pre-cut from `Retry-After`:** the telemetry signal will carry
  `Retry-After`; the controller could pre-emptively cut for that duration
  rather than waiting for the count threshold. Deferred — the count-threshold
  path is simpler and the `Retry-After` window is usually shorter than one
  control interval.

---

## Appendix A — code-path citations

| Concern | Location |
|---|---|
| Dispatch slot math (`slots_available = max_agents - alive`) | `src/commands/service/coordinator.rs:4850` |
| Alive count | `src/commands/service/coordinator.rs:4815` |
| Daemon builds `DaemonConfig.max_agents` from launch arg | `src/commands/service/mod.rs:2415` |
| `run_start` passes `--max-agents` to the forked daemon | `src/commands/service/mod.rs:1314` |
| `handle_reconfigure` else-branch clobbers from config.toml (**the bug**) | `src/commands/service/ipc.rs:1111` |
| `handle_reconfigure` applies explicit `--max-agents` override | `src/commands/service/ipc.rs:1095` |
| `run_reload` sends `Reconfigure { max_agents: None }` on flagless reload | `src/commands/service/mod.rs:3891` |
| `trigger_daemon_reload` (profile activation) sends flagless Reconfigure | `src/commands/profile_cmd.rs:1205` |
| Profile overlay treats `max_agents` as a routing key | `src/profile/named.rs:473,479` |
| `CoordinatorState` struct (the persistence target for the fix) | `src/commands/service/mod.rs:710` |
| `coordinator.max_agents` config field | `src/config.rs:4089` |
| Global-outage breaker (hard floor safety net) | `src/commands/service/zero_output.rs:32,35,41,285` |
| Existing failure taxonomy (`ApiError429RateLimit`, …) | `src/graph.rs:129` |
| 429 detection from raw stream | `src/commands/spawn/raw_stream_classifier.rs:60` |
| Per-task cost/usage (`cost_usd`, tokens) | `src/graph.rs:1004` (struct), `src/graph.rs:596` (on Task) |
| Usage aggregation | `src/usage.rs:75` |
| Spend command | `src/commands/spend.rs` |
| Model pricing | `src/config.rs:1721` (`ModelRegistryEntry`) |
