# Executor Arena Final Integration

- Task: `exec-final-integration`
- Date: 2026-06-01
- Scope: final wiring and verification for executor arena user surfaces.

## Enabled Surfaces

- `wg executors --all` now discovers the full executor arena set from a single source of truth in [`src/executor_discovery.rs`](../../src/executor_discovery.rs): core `native`, `claude`, `codex`, `shell`; stable external `opencode`, `aider`, `goose`, `qwen`, `cline`; provider-specific `gemini`; experimental `crush`, `amplifier`.
- `wg config --show` and `wg config --list` now print an `[executor choices]` section in [`src/commands/config_cmd.rs`](../../src/commands/config_cmd.rs), so configuration inspection exposes the same choice groups as discovery.
- `wg config --executor` help text in [`src/cli.rs`](../../src/cli.rs) now names the complete grouped choice set and points custom integrations at `.wg/executors/<name>.toml`.
- `wg init` surfaces are pinned by [`tests/smoke/scenarios/executor_arena_surfaces.sh`](../../tests/smoke/scenarios/executor_arena_surfaces.sh): seeded external executor templates are checked alongside config and discovery output.
- The executor arena guide now calls out the config/list choice surface in [`docs/guides/executor-arena.md`](../guides/executor-arena.md).

## Experimental Boundaries

- `crush` and `amplifier` remain explicitly experimental. Their discovery entries and config choices are visible, but users still need to validate installed CLI flags against their local versions.
- `gemini` is listed separately as provider-specific rather than stable OpenRouter-oriented. It is discoverable and selectable, but it is not part of the recommended OpenRouter default path from [`docs/reports/executor-arena-ranking.md`](executor-arena-ranking.md).
- External CLI workers require their binaries and provider authentication to be installed locally. This final integration verifies WG's surfaces and template seeding, not a paid live run through every third-party CLI.
- The low-cost OpenRouter path remains the Rust-native/Nex route recommended by the ranking report. The existing smoke scenario [`tests/smoke/scenarios/nex_wg_openrouter_endpoint_auth.sh`](../../tests/smoke/scenarios/nex_wg_openrouter_endpoint_auth.sh) continues to cover WG-scoped Nex endpoint credential attachment without storing secrets in tracked files.

## Credential Review

- No real API key was used for the new executor arena surface smoke. The scenario creates an isolated scratch `HOME` and verifies that no credential-looking `sk-...` or `sk-or-...` value appears in `.wg`, `wg init`, `wg executors --all`, `wg config --show`, or `wg config --list` output.
- The implementation adds no tracked config containing `api_key`, `api_key_env`, `api_key_file`, or literal key material.
- `cargo install --path . --locked` was run after the final config-surface change so smoke validation used the updated global `wg` binary.

## Verification

All commands below passed on this branch.

```bash
cargo fmt --check
cargo test --test integration_executor_arena_surfaces
cargo test executor_discovery
cargo test external_cli_model_args
cargo build
cargo install --path . --locked
WG_SMOKE_SCENARIO=executor_arena_surfaces tests/smoke/scenarios/executor_arena_surfaces.sh
WG_SMOKE_SCENARIO=nex_wg_openrouter_endpoint_auth tests/smoke/scenarios/nex_wg_openrouter_endpoint_auth.sh
```

Notes:

- `cargo install --path . --locked` reported the existing yanked `rquest v3.0.0` lockfile warning, then replaced `/home/bot/.cargo/bin/wg` and `/home/bot/.cargo/bin/nex` successfully.
- The build and tests still emit pre-existing compiler warnings unrelated to the executor arena integration.

## Source Links

- Discovery source of truth: [`src/executor_discovery.rs`](../../src/executor_discovery.rs)
- Config/list surface: [`src/commands/config_cmd.rs`](../../src/commands/config_cmd.rs)
- CLI help surface: [`src/cli.rs`](../../src/cli.rs)
- Integration test: [`tests/integration_executor_arena_surfaces.rs`](../../tests/integration_executor_arena_surfaces.rs)
- Smoke scenario: [`tests/smoke/scenarios/executor_arena_surfaces.sh`](../../tests/smoke/scenarios/executor_arena_surfaces.sh)
- Smoke manifest owner: [`tests/smoke/manifest.toml`](../../tests/smoke/manifest.toml)
- User guide update: [`docs/guides/executor-arena.md`](../guides/executor-arena.md)
- Ranking dependency: [`docs/reports/executor-arena-ranking.md`](executor-arena-ranking.md)
- Prior smoke dependency: [`docs/reports/executor-arena-smoke.md`](executor-arena-smoke.md)
