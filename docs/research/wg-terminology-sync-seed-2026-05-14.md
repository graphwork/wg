# WG terminology synchronization seed review

Date: 2026-05-14
Seed task: `seed-wg-documentation`

## Terminology Policy

The public product name is `WG`.

Use `WG` for the product, project, docs, and user-facing system name. Use
lowercase `wg` for the CLI command, binary, examples, shell invocations, and
code identifiers that are already lowercase.

The scope includes system prompts, bundled agent prompts, role contracts,
quickstart and guide output, prompt templates, setup/onboarding text, generated
help text, and any other agent-facing prompt text. These surfaces are public
interfaces for workers and chat agents, even when they live in `src/**` rather
than `docs/**`.

If the acronym is explained, state it plainly: `WG stands for works good.`
Keep the tone confident, precise, and practical. Do not present the expansion
as a joke, aside, or wink.

Prefer these replacements in public prose:

- `workgraph` as product name -> `WG`
- `Workgraph` as product name -> `WG`
- `work graph` / `work graphs` as older long-form product wording -> `WG` or
  `WG instances`, depending on grammar
- `workgraph project` when addressing users -> `WG project`
- `workgraph docs` / `workgraph setup` -> `WG docs` / `WG setup`
- `workgraph system` -> `WG system`, `WG's graph-based workflow`, or another
  precise phrase, unless the sentence is about generic graph theory rather than
  the product

Use `task graph`, `dependency graph`, `graph`, `a graph of work`, or `the WG
task graph` when the sentence describes the data structure rather than the
product. Do not keep using `workgraph` or `work graph` as the brand term just
because the original sentence blends product identity with graph structure.

## Exceptions

Do not blindly replace every match. Preserve or explicitly justify these cases:

- CLI command and binary names: `wg`, command examples, shell prompts.
- Rust crate and module identifiers: `workgraph` in `Cargo.toml`,
  `workgraph::...`, `crate::graph::WorkGraph`, and the Rust type `WorkGraph`.
- Code identifiers in other languages, including `WorkgraphAgent`,
  `workgraph_state`, package names, function names, and serialized fixture keys.
- Compatibility paths and legacy state directories: `.workgraph`,
  `~/.config/workgraph`, hard-coded historical paths such as
  `/home/erik/workgraph`, and migration notes that intentionally mention them.
- Marker strings and compatibility tokens where changing the literal could break
  behavior, such as `<!-- workgraph-managed -->`.
- Quoted historical output, archived failure reports, benchmark condition names,
  and leaderboard/submission names when the old label is part of the recorded
  experiment.
- Tests and snapshots that intentionally assert legacy output. If a source
  string changes, update the snapshot; if the snapshot is preserving a legacy
  contract, leave it and add a nearby note.
- Generic graph theory wording where `work graph` really means any graph of
  work, not the product. Prefer `task graph` if the intended meaning is the WG
  data model.

## Survey Summary

The repository has already moved several top-level surfaces to `wg` or `WG`,
but there is still naming drift across generated prompts, CLI help, older design
documents, benchmark materials, scripts, and tests.

Root and public overview:

- `README.md:1` is already titled `wg` and mostly uses `wg`/`WG` style.
- `AGENTS.md:1` and `CLAUDE.md:1` are lock-step project guides. They use `wg`
  as the codebase name and keep the `<!-- wg-managed -->` marker. Preserve the
  lock-step requirement if edits are made.
- `docs/README.md:300` and `docs/GUIDE.md:672` still have `Peer Workgraphs` /
  `Peer workgraphs` headings.
- `docs/COMMANDS.md:3479` contains `/home/erik/workgraph/.wg`, which is a path
  example and probably an exception unless the surrounding text is rewritten.

Manual and guide surfaces:

- `docs/manual/01-overview.md` is mostly already `wg`, but still has broader
  role terminology drift such as `coordinator`. That is outside this task unless
  it appears in text touched for WG naming.
- `docs/manual/*.typ` mirrors the manual and must stay consistent with the
  Markdown sources.
- `docs/README.md`, `docs/GUIDE.md`, `docs/AGENT-GUIDE.md`,
  `docs/AGENT-SERVICE.md`, and `docs/COMMANDS.md` are public-facing enough to
  prefer `WG` unless an exception applies.

Generated help, setup text, and prompt sources:

- `src/commands/quickstart.rs:5` prints `WORKGRAPH AGENT QUICKSTART`.
- `src/commands/quickstart.rs:294` says `workgraph is a directed graph`.
- `src/commands/quickstart.rs:645` documents `peer workgraph`.
- `src/commands/setup.rs:20` embeds the project guide block with
  `<!-- workgraph-managed -->` and `# workgraph`; the marker may be a
  compatibility exception, but user-visible prose should be reviewed.
- `src/commands/setup.rs:1320`, `1518`, `1595`, `1975`, and `2159` contain
  first-run/setup prose using `workgraph`.
- `src/commands/spawn/context.rs:700` embeds the worker guide as `# workgraph
  Agent Guide (Essential)`.
- `src/commands/spawn/context.rs:702`, `932`, `955`, and `996` include prompt
  text such as `workgraph project`, `workgraph system`, and `Project:
  workgraph`.
- `src/commands/codex_handler.rs:237` and `250` mention `workgraph task agent`
  and `workgraph chat agent`.
- `src/text/agent_guide.md` is the bundled universal guide source and should be
  reconciled with generated prompt snapshots.
- Role contracts, prompt templates, executor prompt assembly, and system prompt
  fallback files should be included in this pass even when they do not look like
  conventional documentation. These include `src/commands/service/**`,
  `src/service/executor.rs`, `src/commands/spawn/**`, `src/agency/prompt.rs`,
  snapshot fixtures, and any template files that inject agent-facing text.

Tests and snapshots:

- `tests/snapshots/prompt_snapshots__build_prompt_*.snap` contain generated
  prompt text with `workgraph project`, `workgraph is a directed graph`, and
  `workgraph: A lightweight...`.
- `tests/smoke/manifest.toml:374`, `1005`, and `1096` include `workgraph` in
  smoke descriptions. Some are historical bug descriptions or generic fallback
  assertions and may be intentional exceptions.
- `tests/smoke/scenarios/*` includes many path/config comments such as
  `.workgraph`, `~/.config/workgraph`, and `workgraph repo root`; most path
  literals are exceptions, but user-facing failure text should be reviewed.

Current design, research, and report docs:

- `docs/design/**`, `docs/designs/**`, `docs/research/**`, and `docs/reports/**`
  include product-name drift, generic graph terminology, and code references.
- Exact older long-form matches are rare but present:
  `docs/research/cyclic-processes.md:1` and
  `docs/archive/research/beads-gastown-research.md:218`.
- Treat current design docs as editable public/contributor docs unless the text
  is explicitly historical, a code reference, or a quoted trace.

Archive and historical docs:

- `docs/archive/**`, root audit files, old triage files, and rescued artifacts
  contain many older paths and quoted code. These should be handled with
  conservative edits. Prefer an explicit note or no change when the term is part
  of historical evidence.

Website and visual assets:

- `website/assets/favicon.svg:2` has `Workgraph favicon` in a comment. This is
  not user-visible in normal rendering, but it can be synchronized cheaply.
- `website/*.html` should be reviewed for visible copy even if the first sweep
  found little current drift.

Terminal-Bench materials:

- `terminal-bench/README.md:23` includes `/home/erik/workgraph/terminal-bench`
  as a path example.
- `terminal-bench/README.md:28` and many design docs reference
  `WorkgraphAgent`, which is a code identifier and should generally remain.
- `terminal-bench/leaderboard-submission/*/metadata.yaml:2-3` has
  `Workgraph Condition ...` and `agent_org_display_name: "Workgraph"`.
  These may be public display names, but they may also be recorded benchmark
  labels. Decide deliberately and document the choice.
- `terminal-bench/docs/**` and `terminal-bench/analysis/**` mix runbook prose,
  hard-coded paths, code identifiers, and historical experiment notes.

Examples, templates, and scripts:

- `examples/**` and `templates/**` had few direct hits in the scoped survey but
  should still be searched by the edit worker.
- `scripts/**` and legacy `scripts/smoke/**` include comments and user-facing
  output. Preserve `.workgraph` path literals and old fixture names.

## Downstream Graph Shape

Create parallel, non-overlapping edit tasks from this review. Every task should
read this file first and then work only inside its declared file scope. The
integration task after the fan-out may make final cross-scope fixes.

Recommended edit scopes:

1. Root and project guide surfaces: `README.md`, `AGENTS.md`, `CLAUDE.md`,
   with `AGENTS.md` and `CLAUDE.md` kept in lock-step.
2. Manual and public docs: `docs/manual/**`, `docs/README.md`, `docs/GUIDE.md`,
   `docs/COMMANDS.md`, `docs/KEY_DOCS.md`, `docs/DEV.md`, `docs/AGENT-*.md`.
3. Generated help and prompt sources: `src/text/agent_guide.md`,
   `src/commands/quickstart.rs`, `src/commands/setup.rs`,
   `src/commands/spawn/context.rs`, `src/commands/codex_handler.rs`, other
   `src/**` user-facing strings as needed, role-contract text, prompt
   templates, smoke-owned user-facing text, and related prompt snapshots/tests.
4. Current design/research/report docs: `docs/design*.md`, `docs/design/**`,
   `docs/designs/**`, `docs/research/**`, `docs/reports/**`,
   `docs/test-specs/**`, and `docs/codex-gpt55-investigation/**`.
5. Archive and historical docs: `docs/archive/**` plus root historical audit or
   triage Markdown files.
6. Website and screencast assets: `website/**`, `screencast/**`, and
   `docs/assets/**`.
7. Terminal-Bench materials: `terminal-bench/**`, `docs/terminal-bench/**`, and
   `tb-results/**`.
8. Examples, templates, scripts, schemas, and standalone helper surfaces:
   `examples/**`, `templates/**`, `scripts/**`, `schemas/**`, and standalone
   root scripts.

Then fan back into:

- A synchronization task to review all edits for voice, terminology, and
  exceptions.
- A validation/audit task to run targeted `rg` searches, generated help/snapshot
  checks, and relevant docs/tests.
- A final commit/merge/push task if the synchronization task does not own final
  repository hygiene.

## Validation Guidance For Downstream Tasks

Every edit task should include these checks, adapted to its file scope:

- Run targeted searches in the owned files for `workgraph`, `Workgraph`,
  `WORKGRAPH`, `work graph`, and `work graphs`.
- For prompt/help tasks, include system prompts, bundled agent prompts, role
  contracts, quickstart output, prompt templates, and generated guide output in
  the audit.
- For each remaining hit, verify it is an allowed exception and document the
  reason in the task log or artifact.
- Confirm any explanation of `WG` uses `works good` plainly and without joking
  tone.
- If modifying generated prompt/help sources, update affected snapshots and run
  the relevant cargo tests plus `cargo build`.
- If modifying user-visible CLI/TUI behavior or smoke-owned text, run the
  relevant smoke scenario or explain why no scenario is affected.
- Preserve `AGENTS.md` and `CLAUDE.md` lock-step if either file changes.
- Stage only files touched by the task and commit with the task id.
