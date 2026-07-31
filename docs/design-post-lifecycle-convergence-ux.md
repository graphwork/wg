# Post-lifecycle convergence and UX synthesis

**Status:** implemented integrated candidate

This document is the current-main decision joining the task-owned finish
transaction, deterministic daemon convergence, typed autonomous graph identity,
and the thin `worksgood` launcher. It supersedes the older supervisor/controller
roadmap wherever that roadmap conflicts.

## One ownership model

| Concern | Sole authority | Visible representation |
|---|---|---|
| task attempt/status/fence | lifecycle kernel | existing task + lifecycle ledger |
| accepted source integration/output | original task under finish lease | finish receipts and terminal disposition |
| dead owner, wait, evaluation, finalization, cleanup | existing domain owner | same goal task; no bookkeeping child |
| when an unchanged goal/route is reconsidered | deterministic scheduler inside `wg service` | `.wg/service/convergence-state.json` |
| genuinely new goal-bearing agent work | normal graph task | `presentation=autonomous`, centered-dot `·` glyph |
| assign/FLIP/eval/placement/verification machinery | agency plumbing | `presentation=plumbing`, collapsed by default |
| existing-repository entry | `worksgood` → authenticated sibling `wg --dir … tui` | TUI immediately |
| new-repository setup | one-time route-free graph bootstrap | then the same TUI |
| route/model/agency setup | explicit `worksgood setup` | concierge only on request |

The daemon is a lifecycle `Reconciler` actor, never a graph persona. It does not
edit source, evaluate semantics, merge, choose a fallback model, or create
`.daemon-*`, probe, merge, repair, or cleanup tasks. An accepted-but-unfinished
candidate wakes its original source owner; a durable disposition with missing
cleanup invokes cleanup only.

## Durable convergence cutover

`src/service/convergence.rs` owns one atomically replaced read model keyed by
`task id + lifecycle generation`. Each record binds its completion contract,
stage, blocker class, authoritative progress digest, persisted jitter seed,
next wake, exponent, and fenced action lease. Startup, graph events, the earliest
persisted deadline, and the safety timer all return through the same service-loop
entry point. Restart derives current evidence without advancing deadlines, so
it cannot reset an exponent or redraw jitter.

Only candidate/evaluation/disposition/cleanup/wait/generation evidence and goal
edits reset falloff. Heartbeats, output growth, logs, reservations, spawns, and
identical failures do not. Unchanged transients use capped exponential waking
and remain live; retry count alone never makes them generic `Failed`.

Provider outages are keyed by the already-resolved non-secret
`HealthRouteKey(handler, wire, endpoint fingerprint)`. A route-local breaker
permits one ordinary credential-bearing task to hold the probe lease. Other
tasks keep their exact route and sleep. Success closes that breaker and applies
a stable per-task release stagger. The former global provider pause and
zero-output global timer are migration-only data, not scheduling authorities;
there is no implicit fallback. Static `runtime_max_agents` remains an admission
cap, not a controller.

Policy is under `[coordinator.convergence]`:

```toml
[coordinator.convergence]
base_seconds = 5
cap_seconds = 21600
route_probe_base_seconds = 30
route_probe_cap_seconds = 3600
action_lease_seconds = 300
jitter_divisor = 4
```

## Graph identity and TUI contract

Task IDs are opaque to every renderer. `primary` and `autonomous` tasks are
visible by default; autonomous goal actors carry `·`. Plumbing is typed, names
its parent, and is collapsed by default. Historical rows are classified once
on deserialization, with explicit metadata always winning.

The one labeled/clickable centered-dot control reports both mode and remaining
count:

```text
· plumbing: hidden · N hidden
```

It cycles `hidden → running only → all`; `.` and historical `<` are aliases.
“Running only” means active plumbing, not queued `Open` satellites. ASCII,
spatial, DOT, Mermaid, list, keyboard, mouse, and inspector paths consume the
same typed presentation.

## Launcher contract

Bare `worksgood` has no concierge side effects in an existing graph. It resolves
the authenticated sibling binary and executes only:

```text
wg --dir <canonical-.wg> tui
```

A fresh repository gets one route-free, no-agency initialization and then that
same TUI. Missing Pi credentials/plugins therefore do not block opening an
existing graph; selecting a Pi chat reports the focused error at use time.
Concierge automation remains available only through explicit `worksgood setup`.

## Historical retirement

Operator publication of the post-lifecycle batch selected clean supersession
(Option A in `design-deterministic-convergence-reconciler.md`). The graph records
`impl-supervisor-hard-agent`, `impl-adaptive-parallelism-controller`, and
`impl-supervisor-controller-composition` plus their extant agency satellites as
abandoned/superseded by this synthesis. Their studies carry historical banners.
No parallel reset, merge, provider-outage, retry, or max-agent controller is
left enabled.

## Integrated proof

The immutable candidate is accepted only when these pass together:

1. convergence unit tests for restart-stable bytes, capped exponential waking,
   progress reset, stage projection, one route probe, and recovery staggering;
2. a credential-free daemon restart/long-falloff/route-breaker smoke;
3. the task-owned finish transaction smoke;
4. the real tmux/SGR autonomous/plumbing visibility smoke;
5. the real PTY thin-launcher/bootstrap/setup smoke;
6. formatting, clippy, build, install, and focused/full feasible test suites.
