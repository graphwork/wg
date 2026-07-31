# Roadmap: Rate-Limit Telemetry, Supervisor, Adaptive Parallelism & npm Distribution

> **Historical roadmap.** Its supervisor/adaptive-controller track is
> superseded by `docs/design-deterministic-convergence-reconciler.md`; telemetry
> remains evidence, while one deterministic `wg service` scheduler owns durable
> wakes and exact-route probes. Distribution decisions are unaffected.

**Status:** synthesis / implementation roadmap (fan-in of four sibling study designs).
**Date:** 2026-07-25
**Owner task:** `synthesis-roadmap-from`
**Tags:** synthesis, roadmap, planning
**Inputs (read in full):**
- `docs/studies/supervisor-hard-agent-design.md` (`study-long-lived`)
- `docs/studies/ratelimit-cost-telemetry-design.md` (`study-rate-limit`)
- `docs/studies/adaptive-parallelism-budget-design.md` (`study-adaptive-parallelism`)
- `docs/studies/wg-npm-distribution-design.md` (`study-distribute-wg`)

---

## 0. TL;DR

Four studies converge on **two independent tracks** that share one foundational
substrate:

1. **The daemon-control track** — a rate-limit/cost **telemetry detector** feeds
   two new daemon peers: a long-lived **supervisor** (resets dumb-failed tasks)
   and an **adaptive-parallelism controller** (turns the signal into the
   `max_agents` knob). All three are gated behind a single shared prerequisite:
   fixing the `max_agents` **authority bug** (four sources of truth, flagless
   reload silently clobbers a controller's value).
2. **The release track** — npm distribution is **fully independent** of the
   daemon track: it repackages the existing attested GitHub-Release binaries into
   per-platform npm packages, plus adds OS code-signing. It can run end-to-end in
   parallel with the daemon track.

**The keystone insight:** the telemetry signal is consumed by *both* the
supervisor and the controller (each study §7 / §3.1 says so independently). It
must be built **once**, as a single shared substrate, not twice. Likewise the
authority fix is a shared prerequisite. This roadmap reconciles those overlaps
into single implementation tasks rather than four siloed ones.

The roadmap decomposes into **7 implementation tasks** (under the
`max_child_tasks_per_agent = 10` guardrail — no follow-on planner needed),
wired as a dependency DAG with explicit file-safety serial edges.

---

## 1. The four studies at a glance

| Study | Core contribution | Study's own "what to build first" |
|---|---|---|
| **ratelimit-cost-telemetry** | Normalized `FailureReason`/`FailureSignal`; **the pi-handler gap** (pi error events dropped → `AgentExitNonzero`); rolling telemetry window `.wg/service/provider-telemetry.jsonl`; `ProviderHealth` w/ `cooled_until_ms` | "the two smallest, highest-leverage changes are (1) extend `translate_pi_stream` to forward pi error events and (2) teach `classify_from_raw_stream` the 402 arm + body-envelope parse" (§9) |
| **adaptive-parallelism** | Additive-up/subtractive-down controller for `max_agents`; cost/time budget model; floor/ceiling/cooldown/kill-switch; **the `max_agents` authority bug + fix** (§8) | "Fix the authority bug (§8.2) first, controller-free … ~15 lines + a unit test" then "Controller spark — rate-only" then "Budget model" (§11) |
| **supervisor-hard-agent** | Long-lived slow-tick (180 s) persona; 8 dumb-failure classes (C1–C8); per-task memory + sidecar journal; loop-prevention; dry-run/observer-first rollout | Stage 0 observer (default-on, journal-only) delivers "most of the observability value" (§9); Stage 2 limited-live on the safe classes (C1/C5/C6) |
| **wg-npm-distribution** | Per-platform `optionalDependencies` driver (Shape A) over the **existing** 5-target native-runner matrix; macOS/Windows signing; CI `npm-publish` job | MVP (§10.1): add `npm-publish` job + driver shim + embedded manifest + `--provenance`; hardening (§10.2): signing + glibc-floor |

---

## 2. Cross-cutting themes & shared substrate (build once)

### 2.1 The telemetry signal is the shared substrate

Both daemon peers are, in their own words, *consumers* of the same failure signal:

- The **supervisor** (study §7) "is the natural **detection producer** for the
  rate-limit and parallelism controllers" and writes a bounded health snapshot to
  `.wg/supervisor/state.json` each pass. It also *reads* failure pressure to
  decide per-class reset policy.
- The **controller** (study §3.1) "thresholds on the **`rate-limit`** class" and
  "reads a rolling window of `{timestamp, provider, model, failure_class,
  confidence}` records from the telemetry store the sibling study defines; it
  never re-parses raw streams itself."
- The **telemetry study** itself (§6) defines the rolling window as the single
  store *both* peers read.

**Reconciliation:** there is exactly **one** telemetry task
(`impl-rate-limit-telemetry`) that owns the `FailureReason`/`FailureSignal`
type, the pi-error parse, the `.wg/service/provider-telemetry.jsonl` window, and
the `ProviderHealth` aggregate. The supervisor and controller tasks **read** it;
neither re-implements parsing. This is the single biggest overlap the synthesis
collapses.

> **Direction-of-dependency note (from the studies).** The telemetry study makes
> the signal the *producer*; the controller and supervisor are *consumers*. The
> supervisor additionally *produces* a higher-level **health snapshot**
> (`needs_human` count, eval-pipeline saturation, etc.) that the controller
> *may* read (`supervisor-hard-agent` §7; `adaptive-parallelism` §9 "the
> supervisor's reset log is read by the controller only to discount
> dumb-failure resets"). That supervisor→controller edge is one-way and
>松弛 (polite backoff, never a hard dependency), so it does not create a cycle.

### 2.2 The `max_agents` authority fix is the shared prerequisite

The controller (§8) literally cannot function without it: "a controller that
sets `max_agents` is worthless if a `service reload` silently overrides it."
The fix — a single authoritative `CoordinatorState.runtime_max_agents` that
survives a flagless reload, with `handle_reconfigure` taught to respect it — is a
~15-line correctness fix that is **valuable independently** and is the first
thing in the build sequence.

The supervisor does **not** write `max_agents` (study §9 boundary: "the
supervisor **never** touches `max_agents`"), so it does not *consume* the fix
directly. But it shares the same daemon-state-persistence discipline (its
`state.json` must also survive reloads), so it is sequenced *after* the authority
fix lands — both touch `src/commands/service/mod.rs` (`CoordinatorState` vs the
daemon loop), and serializing the edits prevents merge conflict.

### 2.3 The zero-output breaker is the hard floor both peers compose with

Both studies independently state the same composition rule: the existing
global-outage breaker (`src/commands/service/zero_output.rs`) **overrides** both
peers when it trips (controller §7.1; supervisor does not own "credit-exhaustion
mass recovery"). Neither study proposes to replace it. The roadmap treats the
breaker as an immutable floor and does not task it.

### 2.4 npm distribution shares nothing with the daemon track

The npm study is release engineering: it changes how compiled bytes reach users,
not daemon behavior. Its only in-repo touches are `.github/workflows/release.yml`
(extend the build matrix with signing + add a downstream `npm-publish` job) and
new package-scaffolding files. It has **zero file overlap** with the daemon track
and runs fully in parallel.

---

## 3. Dependency-ordered build sequence

### 3.1 The DAG

```
                        ┌─────────────────────────── RELEASE TRACK (parallel) ─┐
                        │                                                      │
 synthesis-roadmap-from ├──▶ impl-binary-signing-notarization (T6)             │
                        │       └──▶ impl-npm-publish-distribution (T7)        │
                        │                                                      │
                        └─────────────────────────── DAEMON TRACK (serial on mod.rs/config.rs) ─┐
                                                                                              │
 ├──▶ impl-maxagents-authority-fix (T1)  [shared prerequisite]                            │
        └──▶ impl-rate-limit-telemetry (T2)  [shared substrate: signal + window]           │
               └──▶ impl-supervisor-hard-agent (T3)                                        │
                      └──▶ impl-adaptive-parallelism-controller (T4)                       │
                             └──▶ impl-supervisor-controller-composition (T5)  [integrator]┘
```

### 3.2 Ordering rationale (what unblocks what)

| Edge | Why it's required |
|---|---|
| **T1 first** | Pure correctness fix for the 4-source authority bug (controller §8.2). **Shared prerequisite:** the controller's writes must survive a flagless reload or the controller is worthless. Valuable independently; ~15 lines. |
| **T1 → T2** | T2 adds `ProviderHealth` to `SessionCostTracking` and T1 adds `runtime_max_agents` to `CoordinatorState` — both in `src/commands/service/mod.rs`. **Same-file serial edge** (T1 is quick, so the block is short). |
| **T2 before T3 & T4** | **Shared substrate.** T2 defines `FailureReason`/`FailureSignal` + the rolling window that T3 (supervisor reads pressure) and T4 (controller thresholds on `rate-limit`) both consume. Building it once prevents duplicate parsing logic (telemetry study §4.3 explicitly asks for the shared `parse_openrouter_error_envelope`). |
| **T2 → T3 (graph.rs)** | T3 adds per-task memory fields to `Task` (`src/graph.rs:~640`); T2 adds `FailureReason`/`FailureSignal` (`src/graph.rs:~129`). Same file → serial. |
| **T3 → T4** | Both add parallel slow-timer blocks to the daemon loop in `src/commands/service/mod.rs`, new sections to `src/config.rs`, and new command modules registered in `src/commands/mod.rs`. **Three shared files → serial.** Also gives the controller a stable supervisor health-snapshot contract to read. |
| **T4 → T5** | T5 is the integrator (AGENTS guide: "always include an integrator at join points"). It wires + smoke-tests the supervisor↔controller composition contract (controller §9) and a real rate-limit-burst scenario. Needs both peers live. |
| **T6 (signing) independent** | Touches only `.github/workflows/release.yml` (build-matrix jobs) + a new entitlements plist. No daemon-track file overlap. |
| **T6 → T7** | Both edit `.github/workflows/release.yml` (T6 extends the build matrix's macOS/Windows jobs; T7 adds a downstream `npm-publish` job). **Same-file serial edge.** Sequencing also means T7 repackages *signed* binaries (the npm study §6.2 places signing before archiving). |

### 3.3 Why the daemon track is deliberately serial

`src/commands/service/mod.rs` (the daemon loop + `CoordinatorState` +
`SessionCostTracking`) and `src/config.rs` are touched by T1, T2, T3, T4. The
golden graph rule — *same files = sequential edges* — forces T1→T2→T3→T4. This is
a deliberate trade of parallelism for **merge-conflict safety**: these are the two
hottest files in the daemon and concurrent edits would corrupt one another. The
genuine parallelism lives in the release track (T6→T7 runs alongside the entire
daemon track). If future agents want to parallelize the daemon track, the path is
to move `ProviderHealth` out of `SessionCostTracking` into the new telemetry
module (T2) so T2 no longer touches `mod.rs` — noted as an option in §6.

---

## 4. Implementation task decomposition

Seven tasks. Each maps to one or more studies, has a `## Validation` section, and
correct `--after` dependencies. **No task pins a model** — all route through the
active zai profile/dispatcher.

### T1 — `impl-maxagents-authority-fix`  *(from adaptive-parallelism §8)*
Fix the `max_agents` four-source authority bug. Add `runtime_max_agents:
Option<usize>` to `CoordinatorState` (`src/commands/service/mod.rs:~710`); teach
`handle_reconfigure`'s else-branch (`src/commands/service/ipc.rs:~1109`) to keep
a controller-managed runtime override across a flagless reload; re-interpret the
launch `--max-agents` arg as a one-shot initial + session pin when no runtime
value exists. **Shared prerequisite for T4.**
*Files:* `src/commands/service/ipc.rs`, `src/commands/service/mod.rs`,
`src/config.rs`. *Tags:* `coordinator`, `config`, `bugfix`.

### T2 — `impl-rate-limit-telemetry`  *(from ratelimit §4–7; the shared substrate)*
The full detector + persistence. Add `FailureReason` + `FailureSignal` to
`src/graph.rs`; extend `translate_pi_stream` (`src/stream_event.rs:464`) to
forward pi `type:"error"`/`type:"response"`(success:false) as
`StreamEvent::Error`; teach `classify_from_raw_stream`
(`src/commands/spawn/raw_stream_classifier.rs`) the 402 arm + body-envelope parse
+ the new reasons; extract shared `parse_openrouter_error_envelope` (reused by
the native executor and the subprocess classifier); add the rolling
`.wg/service/provider-telemetry.jsonl` window + `ProviderHealth`
(`cooled_until_ms`) on `SessionCostTracking`; wire recording at `fail.rs`, the
spawn wrapper, and the native executor's terminal-retry path. **This is the
shared substrate T3 & T4 read.**
*Files:* `src/graph.rs`, `src/stream_event.rs`,
`src/commands/spawn/raw_stream_classifier.rs`, `src/executor/native/openai_client.rs`,
new `src/telemetry/mod.rs`, `src/commands/service/mod.rs` (`SessionCostTracking`),
`src/commands/fail.rs`, `src/commands/spawn/execution.rs`,
`src/commands/classify_failure.rs`, `src/commands/recover.rs`. *Tags:*
`telemetry`, `failure-classification`, `openrouter`, `pi`.

### T3 — `impl-supervisor-hard-agent`  *(from supervisor study §2–10)*
The long-lived slow-tick supervisor. New `src/supervisor/{mod,memory,policy}.rs`
(persona + per-class policy table as data + sidecar journal/state rollup +
loop-prevention guards + hard pass timeout); per-task memory on `Task`
(`supervisor_reset_count`, `last_supervisor_action`, `needs_human`) in
`src/graph.rs:~640`; `[supervisor]` config (`enabled` default **false**, `dry_run`
default **true**, cadence/interval/cap fields); daemon-loop wiring (parallel
`last_supervisor_pass` timer in `src/commands/service/mod.rs:~3138`, gated on
`enabled` + `paused`); `wg supervisor {run,status,pause,resume,revert}` CLI.
**Ships at Stage 0/1 (observer + dry-run by default)** — detection, journal,
health snapshot, and loop-prevention bounds are the task's validation; live
mutation is implemented but gated/dry-run-default (promotion to Stage 2/3 is a
follow-on operator decision, audited via the journal).
*Files:* new `src/supervisor/`, `src/graph.rs`, `src/config.rs`,
`src/commands/service/mod.rs`, new `src/commands/supervisor_cmd.rs`,
`src/commands/mod.rs`, `src/commands/status.rs`. *Tags:* `supervisor`,
`lifecycle`, `daemon`, `config`.

### T4 — `impl-adaptive-parallelism-controller`  *(from adaptive-parallelism §4–7)*
The controller (rate-only spark **and** budget model in one cohesive unit,
matching the study's single design). New `src/budget/mod.rs`
(additive-up/subtractive-down on the T2 `rate-limit` signal; floor/ceiling/
cooldown hysteresis; writes `CoordinatorState.runtime_max_agents` via the T1
authority; persisted `dir/service/budget_state.json`; composition with the
zero-output breaker as the hard floor); `[budget]` config (`usd_per_hour`,
`usd_per_day`, per-model `rpm`/`usd_per_hour` limits); the
max_agents-from-budget derivation (§6.3); credit-exhausted + daily-wall
kill-switches (§7.4); `wg budget {status,pause,pin,unpin}` CLI; daemon-loop
integration (parallel `control_interval` timer in `src/commands/service/mod.rs`).
*Files:* new `src/budget/mod.rs`, `src/config.rs`,
`src/commands/service/mod.rs`, new `src/commands/budget_cmd.rs`,
`src/commands/mod.rs`. *Tags:* `controller`, `parallelism`, `dispatch`, `budget`.

### T5 — `impl-supervisor-controller-composition`  *(integrator; from adaptive-parallelism §9 + supervisor §7)*
Wire the one-way composition contract: the supervisor **reads**
`budget_state.json`'s `effective_max_agents` to *politely delay* a reset burst
when the controller is at floor and 429s are high (never a hard dependency); the
controller **reads** the supervisor's reset log to discount dumb-failure resets
from its throughput denominator. Add a smoke scenario
(`tests/smoke/scenarios/controller_supervisor_rate_burst.sh`) driving a real
rate-limit burst and asserting the controller sheds and recovers **without**
tripping the global breaker, and that a supervisor reset burst does not
starve the controller's floor. List it in `owners` of
`tests/smoke/manifest.toml` (grow-only).
*Files:* `src/supervisor/mod.rs`, `src/budget/mod.rs` (read-contract methods),
`tests/smoke/scenarios/controller_supervisor_rate_burst.sh`,
`tests/smoke/manifest.toml`. *Tags:* `integration`, `smoke`, `controller`,
`supervisor`.

### T6 — `impl-binary-signing-notarization`  *(from npm §6.2, §6.3, §10.2; independent track)*
Add macOS Developer ID signing + `xcrun notarytool submit --wait` + `stapler
staple` to the two macOS matrix jobs, and Windows Authenticode (Azure Trusted
Signing preferred) to the Windows job — both **before archiving** in
`.github/workflows/release.yml`. Keep the existing Sigstore `actions/attest@v4`
attestations as the supply-chain-provenance layer. This closes the two real gaps
the npm study flags (unsigned macOS binaries → Gatekeeper; unsigned `.exe` →
SmartScreen).
*Files:* `.github/workflows/release.yml` (build-matrix jobs), new
`macos-entitlements.plist`. *Tags:* `release`, `signing`, `security`, `ci`.
*Requires secrets/acquired certs* (Apple Developer ID `.p12`; Azure Trusted
Signing or EV/OV cert) — an org/account action; flag in the task if unavailable.

### T7 — `impl-npm-publish-distribution`  *(from npm §5, §8.3, §10.1; depends on T6)*
Add the downstream `npm-publish` job to `.github/workflows/release.yml`
(`needs: [plan, assemble]`): download the attested GitHub-Release archives,
verify each `gh attestation verify` + SHA256 vs `release-manifest.json`, repackage
each binary into a Shape-A per-platform package (`@wg/cli-<os>-<cpu>` with
`os`/`cpu`/`libc` keys) + the `@wg/cli` driver (the canonical ~30-line
`bin/wg.js`/`bin/nex.js` shim from npm §5.4), embed `release-manifest.json` in
the driver for offline SHA256 verification, `npm publish --provenance --access
public` × 6, and assert the embedded manifest is byte-equal to the released one
(anti-drift gate mirroring `embed-worksgood-pi-check`). Document the glibc floor
+ unsupported arches in the driver README.
*Files:* `.github/workflows/release.yml` (new job), new `npm/` package
scaffolding (`bin/wg.js`, `bin/nex.js`, `package.json` templates), driver README.
*Tags:* `release`, `npm`, `distribution`, `ci`.

---

## 5. Prioritization & leverage

| Priority | Task | Why |
|---|---|---|
| **1 (do first)** | T1 authority fix | Tiny, independent, unblocks the whole daemon track, fixes a live bug (2→8 on reload). Highest leverage-per-line. |
| **2** | T2 telemetry | The shared substrate; its Stage-0-ish value (visible, structured failure reasons + the pi gap closed) lands even before the peers exist. The pi-error-forwarding fix is the single highest-value change across all four studies (ratelimit §4.2/§9). |
| **3a (parallel track)** | T6 signing | Independent of the daemon track; ships user-trust hardening. *Note: needs certs.* |
| **3b** | T3 supervisor (observer) | Delivers "most of the observability value" (supervisor §9) at zero mutation risk once T2 exists. |
| **4** | T4 controller | The headline feature (no more global-pause-on-rate-pressure). Needs T1+T2. |
| **5** | T5 composition | Integrator — proves the two peers compose without fighting. |
| **6** | T7 npm publish | Broadens reach to non-Rust users once binaries are signed. |

If credits/time are tight, **T1 → T2 → (T3 observer OR T6 signing)** delivers the
most value for the least risk; T4/T5/T7 are the headline features that build on
that foundation.

---

## 6. Risk register

| # | Risk | Likelihood | Impact | Mitigation / owner task |
|---|---|---|---|---|
| R1 | **The authority fix (T1) subtly changes `--max-agents` launch semantics** and breaks an existing test/script that relied on the 2→8 clobber. | Med | Med | T1 validation re-derives from launch arg as a session pin only when *no* runtime value exists; adds a `--no-pin` escape for tests wanting old behavior. Add a unit test reproducing the reload-clobber and asserting preservation. |
| R2 | **pi error forwarding (T2) mis-parses** a provider error and emits a wrong `FailureReason`, sending the controller/supervisor the wrong signal. | Med | High | Confidence ladder (1.0 status+error_type → 0.2 exit-only) lets consumers weight low-confidence signals less; the controller's *sustained*-count threshold (≥3 in window) ignores blips; unit-test against the in-repo calibration fixture (`openai_client.rs:5167`) and pi RPC reference impl (`pi_handler.rs:182`). |
| R3 | **The supervisor resets a task the human wanted left failed** (the riskiest class of bug — stopping wanted work). | Med | High | Observer/dry-run default (Stage 0/1); per-task cap (1 for tar pits, 0 for crash loops); per-pass cap (3); every mutation logged with `actor="supervisor"` + `wg supervisor revert`; kill switch `[supervisor] enabled=false` + `WG_SUPERVISOR_DISABLE=1`. Crash/loop classes (C2/C3/C8) stay observer-only past Stage 2. |
| R4 | **The controller oscillates** (add-cut-add flap) under a flaky provider. | Low | Med | Asymmetric timing (slow +1/interval up, fast proportional down); cooldown (90 s cut→grow) + cut_lockout (30 s cut→cut); persisted move log makes oscillation auditable. Proven bounded-rate by construction (study §5.1). |
| R5 | **npm scope `@wg` is unavailable/owned by another party.** | Med | High (blocks T7) | T7 open question (npm §10.3): resolve scope (`@wg` vs `@graphwork`) **before first publish** — baked into package names, hard to change post-fact. T7 should fail-loud if the scope 403s and surface the alternative. |
| R6 | **Signing certs unavailable** (no Apple Developer ID / Azure Trusted Signing account). | Med | High (T6 blocked; T7 ships unsigned) | T6 is structured so signing is an *additive* matrix step; if certs are absent, T6 produces a clear "certs missing" CI skip and T7 can still ship v1 (npm §10.1 MVP does not require signing) — at the cost of Gatekeeper/SmartScreen friction. Flag in T6. |
| R7 | **glibc 2.35 floor** excludes Ubuntu 20.04 / Debian 10 holdouts from the npm Linux binaries. | Low | Low–Med | Documented floor; T7 notes the older-base / `cargo-zigbuild`-musl mitigation (npm §4.4) as a follow-on, not a v1 blocker. |
| R8 | **Daemon-track serial chain (T1→T2→T3→T4→T5) is long** — a single task stalling stalls the tail. | Med | Med | The release track (T6→T7) runs fully parallel, so total throughput is preserved. The chain is serial only because of shared files; if an agent wants to parallelize, the lever is moving `ProviderHealth` out of `SessionCostTracking` (§3.3). |
| R9 | **Supervisor↔controller composition** has a feedback loop (supervisor resets → burst of ready work → controller cuts `max_agents` → supervisor delays resets → …). | Low | Med | Study §9 defines the one-way contract (supervisor reads controller state to *delay*, never to force; controller reads supervisor log to *discount*, never to reset). T5 smoke-tests the closed loop with a real burst. |
| R10 | **Telemetry window grows unbounded** and slows the coordinator's cheap read. | Low | Low | Bounded (last 1000 records OR 24 h, pruned on append, same pattern as the graph log) per ratelimit §6.1. |

---

## 7. Open / unresolved items flagged from the studies

These are deliberately **deferred** (each study's own "open questions" section);
they do not block the 7 tasks but are recorded so a future planner can pick them
up:

- **Supervisor C3 recognition heuristic** (3 identical crash signatures) — is
  there a cheaper signal (a `crash-test` tag / `--max-retries 0` convention)?
  *(supervisor §11.2)*
- **Supervisor journal rotation** (size cap + archive) vs append-only-until-clear.
  *(supervisor §11.4)*
- **Multi-project shared-key RPM coordination** — a cross-project semaphore keyed
  by credential so two WG projects on one OpenRouter key share the 20-RPM cap.
  *(adaptive-parallelism §6.4, §12)*
- **Adaptive `step_down`/`cooldown`** tuning from observed provider behavior.
  *(adaptive-parallelism §12)*
- **Per-task priority under a `max_agents` cut** (budget-aware dispatch policy —
  premium-tier tasks first when spend is tight). *(adaptive-parallelism §12)*
- **Predictive pre-cut from `Retry-After`** (cut for the duration rather than
  waiting for the count threshold). *(adaptive-parallelism §12)*
- **`nex` in npm**: ship alongside `wg` in each platform package (recommended,
  matches `release.yml`) vs a separate `@wg/nex` driver. *(npm §10.3)*
- **Runtime version-skew guard** in the npm driver (assert platform-pkg version
  === driver version before spawn, pi-plugin-compat-style). *(npm §10.3)*
- **Homebrew tap** (same artifacts feed a `brew install wg` formula). *(npm §10.3)*
- **npm scope ownership** (`@wg` vs `@graphwork`) — a naming/ownership call for a
  human, gated before first publish. *(npm §10.3)*
- **Supervisor live-mutation promotion (Stage 2→3)** — the operator-gated
  decision to turn on C1/C5/C6 (Stage 2) then all classes (Stage 3), audited via
  the journal. This roadmap ships Stage 0/1; promotion is a follow-on task.

---

## 8. Mapping: roadmap item → source study

| Roadmap item | Primary study | Study sections |
|---|---|---|
| T1 `impl-maxagents-authority-fix` | adaptive-parallelism | §8 (the bug + fix), §11 phase 1 |
| T2 `impl-rate-limit-telemetry` | ratelimit-cost-telemetry | §4.2 (the gap), §5 (signal), §6 (window), §7 (code map) |
| T3 `impl-supervisor-hard-agent` | supervisor-hard-agent | §2 (cadence), §3 (C1–C8), §4 (memory), §5 (loop-prevention), §9 (rollout), §10 (code map) |
| T4 `impl-adaptive-parallelism-controller` | adaptive-parallelism | §4 (policy), §5 (timing), §6 (budget), §7 (safety), §11 phases 2–3 |
| T5 `impl-supervisor-controller-composition` | adaptive-parallelism + supervisor | adaptive §9 (boundary), supervisor §7 (health snapshot); adaptive §11 phase 4 |
| T6 `impl-binary-signing-notarization` | wg-npm-distribution | §6.2 (macOS), §6.3 (Windows), §10.2 (hardening) |
| T7 `impl-npm-publish-distribution` | wg-npm-distribution | §5 (Shape A + shim), §6.5 (manifest), §8.3 (CI job), §10.1 (MVP) |

---

## 9. Validation of this roadmap

- [x] Covers all four studies (§1 mapping; §8 reverse mapping).
- [x] Sequences them with rationale (§3 DAG + table; explicit "what unblocks what").
- [x] Identifies shared substrate built once (§2.1 telemetry, §2.2 authority fix).
- [x] Includes a risk register (§6) and deferred-items list (§7).
- [x] Explicitly sequences the `max_agents` authority/reload-override fix as the
      shared prerequisite (§2.2, §3, §5).
- [x] Each roadmap item maps to its source study (§8).
- [x] Implementation tasks spawned via `wg add` + `wg publish --only`, each with a
      `## Validation` section and correct `--after` dependencies (see spawned
      sub-graph; logged on `synthesis-roadmap-from`).
- [x] No per-task model pins (all route through the active zai profile).
- [x] No duplicate/overlapping tasks (telemetry + authority overlaps reconciled
      into single tasks); 7 tasks ≤ `max_child_tasks_per_agent = 10` (no
      follow-on planner needed).
