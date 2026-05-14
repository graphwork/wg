# WG Terminology Sync Validation - 2026-05-14

Task: `validate-wg-terminology-sync`

This artifact records the final validation audit for the WG terminology
synchronization. It intentionally quotes legacy audit tokens such as
`workgraph`, `Workgraph`, `WORKGRAPH`, `work graph`, and `work graphs`; this
file is therefore an allowed audit-note exception in future repository-wide
searches.

## Seed

Read first:

- `docs/research/wg-terminology-sync-seed-2026-05-14.md`

The seed establishes `WG` as the product name and allows legacy terminology
only where it is part of compatibility paths, crate/module/type names, code
identifiers, historical benchmark labels, quoted legacy output, or intentional
audit/snapshot assertions.

## Correction Made

Validation found one obvious stale prose miss:

- `.gitignore`: `# Workgraph project state -- never commit`

It was corrected to:

- `.gitignore`: `# WG project state -- never commit`

No broader follow-up task was created because the remaining hits are allowed
exceptions rather than missed user-facing prose.

## Repository-Wide Audit

Command shape:

```bash
rg --hidden --glob '!.git/**' --glob '!.wg/**' --glob '!target/**' \
  -n 'workgraph|Workgraph|WORKGRAPH|work graphs?|work graph' .
```

Post-fix, before adding this validation artifact, the targeted scan found:

- 7,884 matching lines
- 648 files
- `WORKGRAPH`: 16 matches
- `Workgraph`: 111 matches
- `workgraph`: 8,014 matches
- word-boundary `work graph` / `work graphs`: 8 matches

The word-boundary long-form phrase scan matched only the seed and integration
audit notes:

- `docs/research/wg-terminology-sync-seed-2026-05-14.md`
- `docs/research/wg-terminology-sync-integration-2026-05-14.md`

Those are allowed audit-token examples, not live product prose.

## Prompt, Help, and Agent-Facing Surfaces

Audited explicitly:

- `src/text/agent_guide.md`
- `src/commands/quickstart.rs`
- `src/commands/setup.rs`
- `src/commands/spawn/context.rs`
- `src/commands/codex_handler.rs`
- `src/commands/service/**`
- `src/service/executor.rs`
- `tests/snapshots/**`
- role-contract and prompt-template files under `src/agency`, `src/commands/evolve`,
  and `src/commands/service`

Generated checks:

```bash
wg quickstart | rg 'workgraph project|Workgraph project|workgraph docs|workgraph setup|workgraph system|workgraph task agent|workgraph chat agent|WORKGRAPH AGENT QUICKSTART|Peer Workgraphs|Peer workgraphs'
wg agent-guide | rg 'workgraph project|Workgraph project|workgraph docs|workgraph setup|workgraph system|workgraph task agent|workgraph chat agent|WORKGRAPH AGENT QUICKSTART|Peer Workgraphs|Peer workgraphs'
```

Both generated checks were clean.

Remaining hits in prompt/help/agent surfaces were reviewed as allowed
exceptions:

- crate/module/type identifiers: `workgraph`, `WorkGraph`
- code identifiers and helper names: `workgraph_dir`, `setup_workgraph`,
  `has_workgraph_directives`, `WorkgraphAgent`, `WorkgraphInbox`
- compatibility literals and paths: `.workgraph`, `~/.config/workgraph`,
  `~/.workgraph`
- compatibility schema/config keys: `peer_workgraphs`, `workgraph_state`
- intentionally asserted legacy snapshots and smoke checks

## Public Docs and Generated Prose

The stale public phrase scan:

```bash
rg --hidden --glob '!.git/**' --glob '!.wg/**' --glob '!target/**' \
  -n 'workgraph project|Workgraph project|workgraph docs|workgraph setup|workgraph system|workgraph task agent|workgraph chat agent|WORKGRAPH AGENT QUICKSTART|Peer Workgraphs|Peer workgraphs' .
```

returned only seed-file lines before this artifact was added. No live public
documentation or generated user-facing prose retained those old phrases.

Additional public documentation spot checks found only allowed exceptions:

- `terminal-bench/README.md`: historical `/home/erik/workgraph/terminal-bench`
  path and `wg.adapter:WorkgraphAgent` code identifier
- `docs/COMMANDS.md`: quoted historical resolver output under
  `/home/erik/workgraph/.wg`
- Terminal-Bench metadata and submissions: historical `workgraph-condition-*`
  labels and archived display names

## Acronym Expansion

The acronym expansion scan:

```bash
rg --hidden --glob '!.git/**' --glob '!.wg/**' --glob '!target/**' \
  -n 'WG stands for|stands for works good' .
```

found the live expansion only in `README.md`:

```text
WG stands for works good.
```

The other matches were the terminology seed and integration audit note. The
live expansion is exact, plain, and serious.

## Validation Commands

The following checks passed:

- `git diff --check`
- `diff -u AGENTS.md CLAUDE.md`
- `python3 -m compileall terminal-bench/run_pilot_qwen3_local_10_g.py terminal-bench/run_qwen3_hard_20_a.py terminal-bench/run_qwen3_hard_20_g.py`
- `typst compile docs/manual/wg-manual.typ /tmp/wg-manual-terminology-validation.pdf`
- `cargo build`
- `cargo test --test prompt_snapshots`
- `cargo test`

`cargo build` and `cargo test` emitted existing warnings but completed
successfully. Credential-gated OpenRouter tests remained ignored as designed.

## Result

The WG terminology synchronization is validated. Old terminology is gone from
live public prose and generated prompt/help surfaces where it should be gone.
Every remaining repository hit is justified by compatibility, code identity,
historical artifact naming, quoted legacy output, smoke/snapshot assertion, or
audit documentation.
