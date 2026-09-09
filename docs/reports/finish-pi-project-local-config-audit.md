# Pi-only project-local configuration completion audit

Task: `finish-pi-project-local-config`

## Result

The supported configuration contract has one checked-in authority:
`<project-root>/worksgood.toml`. The hidden `.wg/config.toml` and
`.wg/profile-selection.json` files are one-release, exclusive compatibility
inputs only when `worksgood.toml` is absent. They are never merged with it.
Machine-global `~/.wg/config.toml` and `~/.wg/active-profile` are not project
configuration layers.

## Requirement matrix

| Requirement | Winning implementation / evidence |
|---|---|
| Pi owns authentication, providers, endpoints, and model discovery | `src/project_config.rs:406-578` rejects `auth`, `secrets`, `llm_endpoints`, native-executor and OpenRouter machine namespaces; `src/config.rs:5889-5946` never merges global configuration. |
| Project behavior is checked in | `src/project_config.rs:15` fixes `worksgood.toml`; `src/config.rs:6139-6155` resolves the exclusive project authority. Resource and archive policy are exercised in `project_local_pi_e2e.sh`. |
| Setup/config/profile are local by default | Setup and profile selection use `src/project_config.rs:54-183`; `config set` targets `worksgood.toml` at `src/commands/config_cmd.rs:3020-3179`. `profile use` is a warned project-local alias. |
| Explicit global operation is unmistakable and isolated | `src/commands/config_cmd.rs:3033-3043,3127-3156` warns before a non-routing global mutation with the exact path and `legacy-global-inactive` scope, reloads no daemon, and changes no project. Global routing/setup writes remain refused. |
| Profile selection preserves guardrails | `src/project_config.rs:54-183` replaces only the closed model/reasoning projection. `src/commands/profile_cmd.rs:353-467,881-921` reads `worksgood.toml.profile_origin`, not the inactive global pointer/legacy association. |
| Effective values expose provenance | `Config::load_with_sources`, `config get`, `config --list`, and `config --models` report project-file/project-profile-import/builtin-default sources. Profile inspection (`src/commands/profile_cmd.rs:228-530`) names the origin and authoritative path. |
| Missing route fails loudly under stale machine state | `src/execution_selection.rs:119-235,237-310` ignores `Default` and `Global` sources and emits `WG-EXEC-UNSELECTED`, including explicitly inactive legacy paths. |
| Narrow, preserving migration | `src/migrate_project_local_pi.rs:292-376,511-699` removes only enumerated global routing selectors and the active pointer; receipts/backups support CAS rollback. The apply phase compares against exact planned preimages; regression at `src/migrate_project_local_pi.rs:1313-1345`. |
| One deterministic filename/precedence | `worksgood.toml` is the sole current authority. The documented compatibility rule never allows it to compete with `.wg/config.toml`; attended `setup`/`profile select` materializes the current file. |
| Two-repository isolation | `tests/smoke/scenarios/project_local_pi_e2e.sh` uses two repositories sharing one HOME/global directory and checks setup, local config changes, an explicit inactive global write, missing-route refusal, clone reproduction, and cleanup preservation. |

## Gaps closed by this completion pass

1. `wg config set agent.model …` previously changed only a display leaf while
   `dispatcher.model`/`models.task_agent.model` could keep actual dispatch on
   the old route. It now updates the complete closed model projection
   atomically while retaining per-role reasoning.
2. `wg profile show` and `wg profile list` previously displayed legacy
   `profile-selection.json`/global-active state as though it could be current
   authority after `profile select`. They now report materialized
   `profile_origin` and label the machine-global pointer inactive.
3. The assignment requires an explicit global write to warn and affect only
   the named global layer. Non-routing `config set --global` now does exactly
   that; global routing remains prohibited because it cannot select a project.
4. Global cleanup previously captured its comparison bytes at apply time rather
   than carrying the planned preimage. Apply now fails before receipts or
   mutation if either planned file changed.
5. Profile/setup materialization now rechecks the project document preimage
   immediately before atomic replacement, refusing a concurrent guardrail edit.
6. The routing-write refusal explicitly covers both canonical `dispatcher.*`
   and compatibility `coordinator.*` selectors plus model/tier/endpoint
   namespaces, before any global file mutation.
7. CLI help and configuration documentation now consistently name
   `worksgood.toml`, the closed schema, the inactive global layer, and the
   attended compatibility/materialization rule.

## Deliberate boundaries

- Reusable profile definitions remain machine-global inputs under
  `~/.wg/profiles/`, but runtime does not reopen them.
- Purpose-scoped secret/identity/federation subsystems retain their own machine
  stores. They are not project-config inheritance.
- `wg migrate project-local-pi` without cleanup is informational. Legacy
  project conversion is deliberately attended through `wg setup` or
  `wg profile select`; this prevents silent route adoption or credential copy.
- Global setup and global routing writes remain hard errors. A global route is
  inert by definition and accepting it would create a plausible but ineffective
  configuration.

## Validation evidence

The following candidate-tree commands ran consecutively in one fail-fast shell
and exited `0`. The complete 211,580-byte combined stdout/stderr capture had
SHA-256 `420aaad464c40c54d73203617e035786f71d924cc900a0699c7803f862a1f1cb`.

| Command | Result |
|---|---|
| `cargo test --test integration_project_local_pi_cli --test integration_project_local_pi_config --test integration_project_local_pi_migrate` | PASS: 5 + 7 + 9 tests |
| `cargo test --lib migrate_project_local_pi::tests` | PASS: 10 tests |
| `cargo fmt --check` | PASS |
| `cargo clippy` | PASS; repository-existing warnings only |
| `PATH=<candidate-target>/debug:$PATH WG_SMOKE_SCENARIO=project_local_pi_e2e bash tests/smoke/scenarios/project_local_pi_e2e.sh` | PASS: two-repo isolation, stale-global fail-loud, fresh-HOME credential-free clone, migration preservation |

The focused source coverage is in `tests/integration_project_local_pi_cli.rs`,
`tests/integration_project_local_pi_config.rs`,
`tests/integration_project_local_pi_migrate.rs`, and
`tests/smoke/scenarios/project_local_pi_e2e.sh`.
