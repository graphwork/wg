# WG terminology synchronization integration review

Date: 2026-05-14
Integration task: `sync-wg-terminology-integrate`
Seed policy: `docs/research/wg-terminology-sync-seed-2026-05-14.md`

This note intentionally names legacy tokens such as `workgraph`, `Workgraph`,
`WORKGRAPH`, `work graph`, and `work graphs` only to document audit coverage
and exception classes.

## Predecessor review

All fan-out edit tasks were complete before this integration pass, and their
artifacts and validation logs were reviewed with `wg show`:

| Task | Status | Commit/log evidence reviewed |
| --- | --- | --- |
| `sync-wg-terminology-root-guides` | Done | Commit `9e56abbb`; `README.md`, `AGENTS.md`, and `CLAUDE.md` updated, `AGENTS.md`/`CLAUDE.md` lock-step validated. |
| `sync-wg-terminology-public-docs` | Done | Commit `8c2dfdc9`; public docs/manual scope searched for legacy terms and path/code exceptions logged. |
| `sync-wg-terminology-prompt-help` | Done | Commit `b3cf1338`; system prompts, bundled prompts, role contracts, quickstart/guide output, setup/onboarding text, generated help, smoke text, snapshots, and agent-facing prompt text explicitly audited; cargo/smoke validation logged. |
| `sync-wg-terminology-current-design-docs` | Done | Commit `cd70432d`; current design/research/report docs updated, remaining hits logged as identifiers, paths, quoted traces, or seed examples. |
| `sync-wg-terminology-archive-docs` | Done | Commit `7a221147`; archive/root historical docs handled conservatively with historical/path/code exceptions logged. |
| `sync-wg-terminology-website-assets` | Done | Commit `e8641f58`; visible website/screencast copy updated; recorded cast output left unchanged for fidelity. |
| `sync-wg-terminology-terminal-bench` | Done | Commit `d3562fe7`; Terminal-Bench prose/runbooks/scripts updated with exceptions for `WorkgraphAgent`, `.workgraph`, `workgraph_state`, serialized labels, and historical results. |
| `sync-wg-terminology-examples-scripts` | Done | Commit `4e9383df`; script output/comments and helper surfaces updated; shell/cargo validation logged. |

## Scope correction

The user scope correction was honored. The fan-out included not only `docs/**`,
but also generated and agent-facing surfaces:

- System prompts, bundled agent prompts, role contracts, prompt templates, and
  executor prompt assembly under `src/**`.
- `wg quickstart`, `wg agent-guide`, setup/onboarding copy, generated help text,
  and prompt snapshots/tests that assert these outputs.
- Website, screencast, script, Terminal-Bench, and public benchmark metadata
  surfaces where users or agents see product naming.

This integration pass added follow-up cleanup where the fan-out left current
public display text using lowercase or legacy product names:

- Public guide/manual titles and first-use product prose now use `WG` while
  command references still use lowercase `wg`.
- Manual Markdown and Typst sources were updated together.
- `docs/task-id-namespacing.md` now uses `WG task graph` / `shared WG task
  graphs` for structural wording.
- The bundled Claude skill copy in `.claude/skills/wg/SKILL.md` now says
  `Peer WG instances`, matching the hidden agent-facing scope correction.
- Current Terminal-Bench condition summary strings and public metadata now say
  `WG` / `WG-assisted` rather than the old product label.

## Remaining exception classes

Remaining legacy-token hits after the integration search fall into the policy's
allowed exception classes:

- Rust crate/module/type names and other code identifiers, including
  `workgraph::...`, `WorkGraph`, `WorkgraphAgent`, `WorkgraphInbox`, helper
  names such as `setup_workgraph`, and serialized output formats such as
  `workgraph-yaml`.
- Compatibility and migration paths such as `.workgraph`,
  `~/.config/workgraph`, `~/.workgraph`, and historical absolute paths under
  `/home/erik/workgraph`.
- Artifact directory and fixture names such as `workgraph_state`.
- Recorded output, screencast `.cast` files, archived reports, benchmark result
  JSON, and historical experiment labels where rewriting the text would falsify
  the record.
- Seed-policy examples and this integration note's own quoted audit tokens.

The remaining hits in files changed by this integration commit are specifically
path/state exceptions:

- `docs/COMMANDS.md` keeps `/home/erik/workgraph/.wg` as quoted resolver output.
- Terminal-Bench runner scripts keep `.workgraph` and `workgraph_state` as
  compatibility path/artifact literals.
- This note names legacy tokens only for audit and exception documentation.

## Acronym expansion

The only product acronym expansion found in current public prose is
`WG stands for works good.` in `README.md`. It is plain and non-joking.

## AGENTS / CLAUDE lock-step

This integration pass did not edit `AGENTS.md` or `CLAUDE.md`. The predecessor
root-guides task validated that the two files remained byte-for-byte aligned.
