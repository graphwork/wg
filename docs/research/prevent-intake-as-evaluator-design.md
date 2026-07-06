# Design note: prevent intake tasks from being handled as evaluator/review work

**Task:** `study-prevent-intake` · **Date:** 2026-07-06
**Scope:** study/design only — no seed download/registration performed.

## TL;DR

`register-refreshed-e97-seed-latest` failed three times the same way because four
independent mechanisms all default toward *"an agent that wrote plausible-sounding
meta text is fine"*, and none of them require the concrete deliverables an
intake/registration task is actually for:

1. **Worker prompt primes "review / grow the graph" framing.** The assembled
   worker prompt (`src/service/executor.rs::build_prompt`) leads with
   `ETHOS_SECTION` ("leave the system better… grow the graph… you may be any
   node") and `AUTOPOIETIC_GUIDANCE` ("decompose / review / improve"). It never
   tells the model *"this task is operational: you MUST produce these files."*
   A weak model (the seed task ran on a `pi:`/OpenRouter route) reading an
   intake task whose title starts with `register-…` and whose body mentions
   "checkpoint" / "latest" / "seed" lands comfortably in observe-and-report mode.
2. **No deliverable preflight at `wg done`.** `wg done` checks the smoke gate
   and integrated-validation log, but it does **not** check that the
   deliverables named in `## Validation` / `## Deliverables` actually exist on
   disk. So "logged checkpoint metadata but produced no `latest.pt`, no
   manifest, no env paths" sails through `wg done` → `PendingEval`.
3. **The eval gate is off by default and, when off, is score-agnostic.**
   `check_eval_gate` (`src/commands/evaluate.rs:1742`) early-returns
   `Ok(false)` unless `eval_gate_all` is set or the task carries the `eval-gate`
   tag. Ungated `PendingEval → Done` (`resolve_pending_eval_tasks`,
   `coordinator.rs:880`) flips on *terminal eval status*, not score
   (see `docs/eval-gate-threshold-investigation.md`). So a weak-model
   evaluator that gives the meta write-up a soft pass promotes the
   no-deliverable task to `Done`, and the failure only surfaces later when a
   downstream consumer can't find `latest.pt`.
4. **Retry is non-mutating.** `wg retry` (`src/commands/retry.rs`) resets status
   to `Open`, bumps the tier, and the spawn layer injects
   `previous_attempt_context` (`spawn/context.rs:1143`) — the prior agent's
   `output.log` tail / checkpoint summary / eval rationale, framed neutrally as
   "previous attempt." A model that already leaned meta reads that as a
   *summary to refine*, not as *evidence that it must pivot to operational
   work*. Same prompt shape → same behavior → three identical failures.

The seed example is the canonical shape of this class: an **operational intake
task** (verb = register/download/submit/write/verify, deliverables = file paths
+ registered ids) routed through defaults built for **open-ended implementation
tasks** (verb = implement/refactor/research, deliverables = "code change").

## Root cause vs symptoms

| Layer | Symptom (what was observed) | Root cause |
|---|---|---|
| Agent | "Logged checkpoint metadata, did not submit/download/register." | No prompt signal told the model this was operational work with mandatory artifacts; the prompt's framing rewards graph-meta behavior. |
| `wg done` | Accepted a run that produced no `latest.pt` / manifest / env paths. | No deliverable preflight — `wg done` only checks smoke + integrated-validation log. |
| Eval/eval-gate | Meta write-up was not rejected; task reached `Done` (or was rescued) and the failure only surfaced downstream. | Eval gate defaults off and, off, is score-agnostic (`resolve_pending_eval_tasks`). |
| Retry | Three attempts, same behavior. | Retry injects prior context neutrally and mutates neither the task description nor the failure class; tier escalation alone doesn't change *what* the model thinks the task is. |
| Task authoring | Title/body used review-adjacent nouns ("checkpoint", "latest", "seed") with no explicit `## Deliverables` list. | No intake/operational task template or `kind:` hint that would (a) force a deliverable list and (b) opt into the gate. |

The root cause is **"nothing in the pipeline distinguishes operational intake
tasks from review/research tasks, and every default rewards the latter."** The
three retries are a *symptom* of (4); the missing `latest.pt` is a *symptom* of
(1)+(2)+(3).

## Why an intake task gets interpreted as evaluator/review work

- The worker system prompt is the same for every task kind; its loudest
  sections (`ETHOS_SECTION`, `AUTOPOIETIC_GUIDANCE`, `RESEARCH_HINTS_SECTION`)
  describe a *reflective agent that reviews, decomposes, and grows the graph* —
  not an agent that must produce a specific file at a specific path.
- The task body's `## Validation` is prose ("produce a usable `latest.pt`
  path", "verify Frontier readability"). Nothing machine-reads it as a
  deliverable contract.
- The auto-evaluator (`.evaluate-X`) and FLIP inference score *fidelity to
  inferred intent*. A coherent meta write-up about the checkpoint scores
  reasonably on "intent fidelity" because the *described* intent (refresh a
  seed) is reflected in the summary — even though no operational action was
  taken. The evaluator has no signal for "did the agent touch the filesystem
  / run the register command."
- When the gate is off (default), a soft evaluator pass is sufficient to
  promote `PendingEval → Done`. The failure is deferred to the downstream
  consumer, which looks like a *different* task failing.

## Signals that distinguish operational intake from review/eval

A task is **operational intake** when any of:

- **Verb set**: title/description leads with `register`, `download`, `submit`,
  `create`, `write`, `install`, `provision`, `verify <readability|exists>`,
  `refresh`, `publish <artifact>`, `migrate`.
- **Deliverable contract**: a `## Deliverables` (or `## Validation`) section
  names **concrete paths / registered ids** (e.g. `latest.pt`,
  `seed/manifest.json`, `OPENAI_API_KEY` in `env.config`, a row in
  `registry.json`), not a score/verdict/report.
- **External side effects**: the task requires a network call, a CLI
  submission, or a mutation of state outside the repo (download, register,
  enroll).
- **Downstream consumers expect a path/id**: dependents reference an artifact
  path the task is supposed to produce (a strong structural signal already
  visible in the graph via `task.artifacts` / `after` edges).

A task is **review/eval** when its deliverable is a *verdict* (`accept` /
`reject` / a score / a report) and its `## Validation` is a rubric, not a file
list. (These already get `eval-scheduled` / `evaluation` / `assignment` /
`evolution` tags and are excluded from auto-eval — `coordinator.rs:1946`.)

## Recommended guardrails (least invasive, in priority order)

The goal is **not** to ban evaluator/reviewer agents. It is to make operational
intake tasks (a) *say what they must produce*, (b) *refuse `wg done` without
it*, and (c) *break the retry loop* when the agent does meta work instead. The
first two changes also help every other "must produce file X" task, not just
intake.

### G1 — Deliverable preflight at `wg done` (highest leverage)

Parse a `## Deliverables` block (and, as a fallback, path-like lines in
`## Validation`) from the task description into a list of expected paths and/or
`registry:id` tokens. In `wg done`, before the smoke gate:

- For each **filesystem path** deliverable: refuse `wg done` (exit non-zero,
  print the missing paths) if the path does not exist or is empty.
- For each **registry:id** deliverable: refuse if the id is absent from the
  named registry file (cheap `grep`).

On refusal, record a **new failure class** `DeliverableMissing` (peer of
`FailureClass::AgentExitNonzero` in `src/graph.rs:129`) so the dispatcher and
retry path can recognize "agent exited cleanly but produced no deliverables"
distinctly from a crash or a 5xx. This single change converts the seed task's
three silent-meta rescues into one loud, classable failure with a concrete
message ("missing latest.pt; missing seed/manifest.json").

**Why this is the linchpin:** it moves the failure from *downstream-consumer,
much later* to *the same task, immediately, with a machine-readable reason*.
That reason is what enables G3.

### G2 — `kind:` / `intake` tag that opts operational tasks into the gate

Introduce a lightweight task tag `intake` (no new schema — `tags` already
exists). A task tagged `intake` (or whose parsed deliverable list is
non-empty) is treated by `check_eval_gate` **as if it carried `eval-gate`** —
i.e. the configured `eval_gate_threshold` (default 0.7) actually gates, so a
soft evaluator pass can no longer promote a no-deliverable run to `Done`. This
is a one-line addition to the `is_gated` predicate at
`evaluate.rs:1762` (`|| task_tags.contains("intake") || has_deliverables`).

This preserves the existing default (research/review tasks stay ungated and
fast) while making the *operational* path fail-closed.

### G3 — Retry mutation for the no-output / deliverable-missing class

In `build_previous_attempt_context` (`spawn/context.rs:1143`), when the prior
attempt's failure class is `DeliverableMissing` (or a new
`NoOperationalOutput` class — see detection below), **replace** the neutral
"previous attempt" framing with an explicit directive block at the *top* of
the worker prompt:

```
## PREVIOUS ATTEMPT FAILED — DO NOT REPEAT

Attempt #N performed observation/summary work only and produced NONE of the
required deliverables. Do not write a review, summary, or analysis. You MUST
perform the concrete operational actions and produce:
  - <path 1>
  - <path 2>
  - <registry:id>
`wg done` will refuse unless these exist.
```

This breaks the "refine the summary" loop. Combined with the existing
tier-escalation-on-retry, the second attempt runs on a stronger model *with*
an explicit "do the work, not the meta" directive. (Optionally, on a second
`DeliverableMissing`, also force a model bump via the existing
`escalate_on_retry` path — already implemented.)

### G4 — Detect meta/no-op behavior post-attempt (failure classifier)

Extend `raw_stream_classifier.rs` / the wrapper to classify a run as
`NoOperationalOutput` when **all** of:

- the agent exited cleanly (exit 0) or called `wg done`, **and**
- `task.artifacts` is empty (no `wg artifact` calls) **and** no files were
  written outside `log/` (cheap: `git status --porcelain` empty in the
  worktree, or `output.log` shows no `write_file`/`edit_file`/`wg add`/shell
  mutation commands), **and**
- `output.log` is non-empty (i.e. the agent *did* something — wrote a
  summary/log — so it's not a crash).

This is the "agent talked but didn't act" signature. It pairs with G1:
`DeliverableMissing` is the *strong* signal (preflight refused); 
`NoOperationalOutput` is the *weak* fallback for tasks without a parsed
deliverable list (so G3's retry mutation still fires even when the task author
didn't write a `## Deliverables` block).

### G5 — Optional: intake task template / authoring nudge

A `wg add` flag `--kind intake` (or a `wg func` template) that emits a
skeleton with a `## Deliverables` section and an `intake` tag, and warns if
an `intake` task is created without deliverables. This is the lightest
authoring-side fix and makes G1–G3 fire automatically. Not required for the
seed case (which was hand-authored) but prevents the next one.

## What I am NOT recommending

- **No broad ban on evaluator/reviewer agents** — they remain the right
  default for research/review tasks. The gate opts *in* per intake task, not
  out for everyone.
- **No new task-type schema** — `kind:` is just a tag; deliverables are parsed
  from existing `## Deliverables` / `## Validation` markdown. No migration.
- **No mandatory stronger model for intake** — tier escalation on retry (G3)
  already exists; G3 only adds prompt mutation, which is the cheaper lever.
- **No change to the eval-gate default for non-intake tasks** — research/review
  tasks keep the fast, score-agnostic path.

## Least-invasive design that would have prevented the 3× failure

**G1 + G3 alone** would have prevented it:

1. **First attempt** logs checkpoint metadata and calls `wg done`. **G1
   preflight** refuses: *"missing latest.pt; missing seed/manifest.json; missing
   env.config entry."* Task is recorded `FailedPendingEval` with class
   `DeliverableMissing` — not promoted to `Done`, not deferred to a downstream
   consumer.
2. **Retry** injects the G3 directive block ("previous attempt produced NONE of
   the deliverables; do the operational work") at the top of the prompt and
   bumps the tier. The (now stronger) model gets an unambiguous "produce these
   files" instruction instead of a neutral summary to refine.
3. The downstream consumer never sees a phantom `Done` seed, so its validation
   failure (the thing that eventually surfaced) is never triggered.

G2 and G4 harden the path further (gate actually scores; weak-signal fallback
for tasks without a deliverable list) but are not strictly required to break
this specific loop.

## Acceptance criteria for follow-up implementation tasks

Two implementation tasks are warranted (see `wg add` below). Acceptance:

### Impl 1 — Deliverable preflight at `wg done` + `DeliverableMissing` class

- [ ] A `## Deliverables` section is parsed into a list of `{path}` and
      `{registry, id}` deliverables; a `## Validation` section with path-like
      lines is used as a fallback when no `## Deliverables` block exists.
- [ ] `wg done` refuses (non-zero exit, clear message listing missing
      deliverables) when a filesystem deliverable is absent/empty or a
      registry deliverable is missing; the refusal is recorded with failure
      class `deliverable-missing`.
- [ ] `FailureClass::DeliverableMissing` is added to `src/graph.rs` with a
      kebab `Display` and does *not* suppress cycle-failure-restart (it's a
      real, retryable failure).
- [ ] Tasks with no parsed deliverables are unchanged (no regression for
      research/review tasks).
- [ ] Unit tests: `done_refuses_missing_deliverable`,
      `done_passes_when_deliverables_present`, `done_ignores_tasks_without_deliverables`.
- [ ] `cargo fmt --check` + `cargo clippy` + `cargo test` clean.

### Impl 2 — Retry mutation + `NoOperationalOutput` detection + intake gate opt-in

- [ ] `build_previous_attempt_context` emits the G3 directive block (with the
      concrete deliverable list) when the prior failure class is
      `DeliverableMissing` or `NoOperationalOutput`.
- [ ] The raw-stream classifier (or wrapper) classifies
      `NoOperationalOutput` per the G4 rule (clean exit, no artifacts, no
      file writes, non-empty output.log).
- [ ] `check_eval_gate`'s `is_gated` predicate treats `intake` tag (or a
      non-empty parsed-deliverable list) as gated, so the threshold actually
      gates.
- [ ] Unit tests: `retry_mutates_prompt_on_deliverable_missing`,
      `classifier_detects_no_operational_output`,
      `intake_task_is_eval_gated`.
- [ ] A smoke scenario `tests/smoke/scenarios/intake_deliverable_preflight.sh`
      (owned by this task in `tests/smoke/manifest.toml`) exercising the full
      loop: an intake task with a `## Deliverables` block, an agent that does
      meta work only → `wg done` refuses with `deliverable-missing` → retry
      injects the directive → second attempt produces the file → `wg done`
      succeeds.
- [ ] `cargo fmt --check` + `cargo clippy` + `cargo test` clean.

## Tradeoffs

- **G1 false positives**: a task that legitimately can't produce a deliverable
  on the first pass (e.g. blocked on an upstream) would be refused. Mitigation:
  preflight only fires for tasks with a parsed deliverable list; `## Deliverables`
  is opt-in authoring. Tasks without it are unaffected.
- **G2 gate latency**: gating intake tasks means waiting for `.evaluate-X`
  before `Done`. Acceptable — intake tasks are exactly the ones where a wrong
  `Done` is expensive (downstream consumers build on a phantom artifact).
- **G3 prompt growth**: the directive block adds ~200 tokens on retry only.
  Bounded by `retry_context_tokens`.
- **G4 classifier heuristics**: "no file writes" via `git status` could
  misfire if the agent legitimately only edited `log/`. Mitigation: G4 is the
  *weak* fallback; G1's preflight is the strong signal and is deterministic.
- **Parsing markdown deliverables** is fuzzy. Mitigation: keep the grammar
  strict (bullet list of paths / `registry:id` under a `## Deliverables`
  header); fall back to "no deliverables → no preflight" rather than guessing.

## Open questions (for the FLIP / implementer)

- Should `intake` be a tag or a first-class `kind:` field? Tag is cheaper and
  consistent with `eval-gate` / `eval-scheduled`; a `kind:` field is cleaner
  but a schema change. Recommendation: **tag**, revisit if more kinds appear.
- Should `DeliverableMissing` auto-rescue (like `FailedPendingEval` score≥thr)
  or always retry? Recommendation: **always retry with G3 mutation**; do not
  rescue a no-deliverable run — rescue is the failure mode we're fixing.
