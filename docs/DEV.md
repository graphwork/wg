# Development Notes

Operational notes, recurring patterns, and things to remember.

## Reusable Functions

Functions extracted from completed task graphs. Run `wg func list` for the full catalog.

| Function | Purpose | Usage |
|----------|---------|-------|
| `doc-sync` | Sync all key docs to current code state | `wg func apply doc-sync` |
| `tfp-pattern` | Trace-function protocol implementation pattern | `wg func apply tfp-pattern` |

The `doc-sync` function fans out: spec → 7 parallel doc updates (README, SKILL, quickstart, COMMANDS, AGENCY, AGENT-GUIDE/SERVICE, manual) → integrate → extract. Run it whenever features land and docs drift.

See `docs/KEY_DOCS.md` for the canonical list of documentation files to keep in sync.

## Build & Test

```
cargo install --path .          # rebuild global wg binary
wg service stop                 # stop before rebuilding
cargo test                      # run tests
typst compile docs/manual/workgraph-manual.typ   # rebuild manual PDF
typst compile docs/research/organizational-patterns.typ  # rebuild org patterns PDF
```

## Service Operations

```
wg service start --max-agents 5   # start coordinator
wg service status                 # check health
wg agents                         # who's working
wg list --status open             # what's pending
wg unclaim <task>                 # clear stale assignment
```

## Model Defaults

- **Agents**: configurable via `wg config` or per-task `--model`
- **Evaluator**: haiku (lightweight, cheap — set by `wg agency init`)
- **Assigner**: haiku (same rationale)
- Hierarchy: task `--model` > executor model > coordinator model > `"default"`

## Common Pitfalls

- Forgot `cargo install --path .` after code changes — old binary runs
- `wg evaluate` requires `run` subcommand: `wg evaluate run <task-id>`
- `wg retry` must clear `assigned` field or coordinator skips the task
- `--output-format stream-json` requires `--verbose` with `--print` in Claude CLI
