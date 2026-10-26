# Audit: reachability of the agency federation / library verbs and remote config

**Task:** `audit-agency-federation-reachability`
**Found by:** `survey-config-agency` (C1 of `.wg/survey/survey-config-agency.md`)
**Downstream consumer:** `retire-agency-composition`
**Method:** read-only. `grep` call-site tracing over `src/` (excluding `tests/`),
`git log -1` per file, `wg --help` surface, and the live store
(`/home/bot/wg/.wg/agency/**`) + checked-in `worksgood.toml`. No code changed,
no build run, nothing deleted.
**Snapshot:** branch `wg/agent-213/audit-agency-federation-reachability`
(based on `4ed9da4a`).

---

## 0. Verdict (binary)

> **RETIRE.** The `wg agency {pull,push,merge,remote,create,migrate,import,export,scan}`
> verb family and the `[agency]` remote-federation config fields
> (`agency_server_url`, `agency_token_path`, `assignment_source`,
> `agency_project_id`, `upstream_url`, plus `placer_agent`) have **no live
> runtime consumer**. Every one is reachable only from the user-invoked CLI
> dispatch table (`src/main.rs`), from tests, or from a *config-gated and
> config-disabled* daemon touchpoint. Nothing in the daemon, dispatch, TUI, or
> concierge path calls them unconditionally.

**Two carve-outs** must be severed before the first deletion commit (both are
already dead because their gating config is `false`/unset, but they are the only
structural links from a live path into this surface):

1. `build_auto_create_task` creates a `.create-*` task whose `exec` string is
   literally `"wg agency create"` — `src/commands/service/coordinator.rs:1611`
   (built at `:1506`, gated `config.agency.auto_create` at `:3452`).
2. `agency_init::try_upstream_pull` calls `agency_import::run_import` from the
   daemon bootstrap — `src/commands/service/mod.rs:3115` →
   `src/commands/agency_init.rs:305` (gated `config.agency.auto_evolve` at
   `:3106` and `agency.upstream_url` at `:286`).

A **one-release deprecation** is the safe path (warn loudly, then delete), not a
same-commit removal. Exact `DEPRECATED_KEYS` additions are in §5.

---

## 1. Reachability definitions used

| State | Meaning in this report |
|---|---|
| **live** | Called from a daemon/dispatch/TUI/concierge code path and **not** gated off by the live config (`worksgood.toml`). |
| **live-gated** | Structurally reachable from a live path but gated by a config knob that is `false`/unset in the live project. |
| **test-only** | The only callers are `tests/**` or smoke scenarios; no production caller besides the CLI dispatch table. |
| **dead** | No caller in `src/` (outside its own module), no test caller, no CLI dispatch. |

The CLI dispatch table itself (`src/main.rs`) is *user-invoked*: it is not a
daemon/dispatch/TUI/concierge path, so a verb that only appears there is **not**
"live" for this audit.

---

## 2. CLI verbs — call sites and state

Target files: `src/commands/agency_*.rs`. Total target verb-file LOC **6,449**
(`pull` 288, `push` 522, `merge` 128, `remote` 156, `create` 396, `migrate`
737, `import` 1,249, `scan` 132, plus `init`/`human`/`stats` out of scope).

| Verb | CLI dispatch | Production caller (non-main) | Test caller | State |
|---|---|---|---|---|
| `pull` | `src/main.rs:2574,2585` | **none** (`agency_pull::run` non-main refs = 0) | `tests/integration_untested_commands.rs:822,862,1014` (CLI e2e) | **test-only** |
| `push` | `src/main.rs:2678` | **none** (`agency_push::run` = 0) | `tests/integration_untested_commands.rs:908,955,997,1014` (CLI e2e) | **test-only** |
| `merge` | `src/main.rs:2592,2598` | **none** (`agency_merge::run` = 0) | `tests/integration_untested_commands.rs:678,753,804` (CLI e2e) | **test-only** |
| `remote` | `src/main.rs:2605,2612,2614,2616` | **none** (`agency_remote::run_*` = 0) | **none** (`agency remote` / `RemoteCommands` absent from `tests/`) | **dead** |
| `create` | `src/main.rs:2620` | `coordinator.rs:1611` builds `.create-*` with `exec="wg agency create"` — **live-gated** on `config.agency.auto_create` (`:3452`, live value `false`) | **none** (no `tests/` caller of `agency_create::run`) | **live-gated** |
| `migrate` | `src/main.rs:2529` | **none** (`agency_migrate::run` = 0) | `tests/integration_untested_commands.rs:114,127,166`; `tests/integration_agency_hash.rs:189,226`; `tests/smoke/scenarios/simple_local_recovery.sh:125` (CLI) | **test-only** |
| `import` | `src/main.rs:2642,2653` | `agency_init.rs:305` from daemon bootstrap `service/mod.rs:3115` — **live-gated** on `auto_evolve` (`:3106`) + `agency.upstream_url` (`:286`, unset) | `tests/integration_agency_csv_roundtrip.rs:25`; `tests/smoke/scenarios/agency_csv_roundtrip.sh:19`; `agency_import_dedup_collision.sh:34,64` | **live-gated** |
| `export` | `src/main.rs:2660` | **none** (`agency_push::run_export` = 0) | `tests/integration_agency_csv_roundtrip.rs:44`; `tests/smoke/scenarios/agency_csv_roundtrip.sh:23` | **test-only** |
| `scan` | `src/main.rs:2562` | **none** (`agency_scan::run` = 0) | **none** (`agency scan` absent from `tests/`) | **dead** |

Verification commands (all run in the retained worktree):

```
grep -rn "agency_pull::run" src/ tests/ | grep -v '^src/main.rs'   # 0
grep -rn "agency_push::run_export" src/ tests/ | grep -v '^src/main.rs'  # 0
grep -rn "agency_merge::run" src/ tests/ | grep -v '^src/main.rs'  # 0
grep -rn "agency_remote::run_" src/ tests/ | grep -v '^src/main.rs' # 0
grep -rn "agency_scan::run" src/ tests/ | grep -v '^src/main.rs'   # 0
grep -rn "agency create" tests/ | grep -v manifest  # 0 (create untested)
grep -rn "agency remote\|agency scan" tests/        # 0
```

### 2.1 The core library is *not* the same as the CLI wrappers

The transfer engine is `LocalStore` in `src/agency/store.rs` (554 LOC, last
touched 2026-08-09) and the pull/push/merge semantics are covered by
`tests/integration_agency_federation.rs` (which calls `LocalStore` directly, not
the CLI wrappers — e.g. `pull_new_entities_all_copied` at `:227`,
`push_to_existing_store_merges` at `:534`, `remote_add_list_remove_lifecycle`
at `:622`). So retiring the *CLI wrappers* is a smaller, lower-risk change than
retiring the store engine, and the two decisions should not be conflated.

### 2.2 git archaeology (`git log -1`)

| file | last commit |
|---|---|
| `src/commands/agency_create.rs` | 2026-06-16 |
| `src/commands/agency_import.rs` | 2026-06-16 |
| `src/commands/agency_merge.rs` | 2026-06-16 |
| `src/commands/agency_migrate.rs` | 2026-06-16 |
| `src/commands/agency_pull.rs` | 2026-06-16 |
| `src/commands/agency_push.rs` | 2026-06-16 |
| `src/commands/agency_remote.rs` | 2026-06-25 |
| `src/commands/agency_scan.rs` | 2026-08-09 (mechanical) |
| `src/agency/agency_bridge.rs` | 2026-05-04 |
| `src/agency/run_mode.rs` | 2026-05-04 |

The federation CLI is frozen since mid-June while `config.rs`/`setup.rs` churn
weekly. That is the signature of a compatibility-only subsystem.

---

## 3. `AgencyConfig` remote fields — call sites and state

Live project config (`worksgood.toml` `[agency]`) contains **none** of these
fields: `auto_assign=false`, `auto_create=false`, `auto_evolve=false`,
`assignment_source`/`agency_server_url`/`agency_token_path`/`agency_project_id`/
`upstream_url` all unset. There is **no `wg config set` setter and no `wg config`
display** for any of the five remote fields — `grep` over
`src/commands/config_cmd.rs` returns **0 hits** for all five, so they can only be
populated by hand-editing TOML.

| Field | Definition | Non-test readers | State |
|---|---|---|---|
| `agency_server_url` | `src/config.rs:4553` (default `:4663`) | `src/commands/assign.rs:235,239` (in `run_auto_assign`, gated `assignment_source=="agency"` at `:234`, itself reachable from daemon admission hook `prepare_automatic_intent_if_configured` `:192` ← `spawn/execution.rs:1435` + `claim.rs:23`, gated `auto_assign` at `:194`; and from `wg assign --auto` at `:173`); `src/agency/eval.rs:191` (in `record_evaluation_with_inference`) | **live-gated / CLI** |
| `agency_token_path` | `src/config.rs:4557` (default `:4664`) | `src/agency/agency_bridge.rs:60,171` — only reachable through `post_evaluation_to_agency` | **live-gated / CLI** |
| `assignment_source` | `src/config.rs:4562` (default `:4665`) | `src/commands/assign.rs:234` (same dispatch gate) | **live-gated** |
| `agency_project_id` | `src/config.rs:4566` (default `:4666`) | `src/agency/agency_bridge.rs:166` | **live-gated / CLI** |
| `upstream_url` | `src/config.rs:4570` (default `:4667`) | `src/commands/agency_import.rs:665` (`wg agency import --upstream`); `src/commands/agency_init.rs:286` (daemon bootstrap, gated) | **live-gated / CLI** |
| `placer_agent` | `src/config.rs:4414` (default `:4635`) | **none** — only the struct field and its default initializer | **dead** |

### 3.1 The `agency_server_url` → bridge → `agency_token_path`/`agency_project_id` chain

The bridge is reachable **only** from the scored-evaluation recorder:

```
commands/evaluate.rs:961  record_evaluation_with_inference(...)   # wg evaluate record
commands/evolve/mod.rs:553 agency::record_evaluation(...)         # wg evolve
        └─ agency/eval.rs:182 record_evaluation
              └─ agency/eval.rs:191  if config.agency_server_url.is_some()
                    └─ agency_bridge::post_evaluation_to_agency   (:196)
                          ├─ agency_bridge.rs:55-64  reads agency_server_url + agency_token_path
                          └─ agency_bridge.rs:160-169 reads agency_server_url + agency_project_id
```

Both entry points are CLI (`wg evaluate`, `wg evolve`) and both are further
downstream of the *already-suspect* scored-evaluation store (1 lifetime
evaluation; see `survey-config-agency` §2.3 and prior audit `AGENCY-004`). So the
token/project/server triple is **doubly** dead: no live caller, and its only
caller is itself a retirement candidate (C4).

### 3.2 `placer_agent` — fully dead

`grep -rn "placer_agent" src/` returns exactly two hits, both in `config.rs`:
the field declaration (`:4414`) and the default initializer (`:4635`). Zero
non-config references, including tests. The related `agency.auto_place` knob is
read only by `wg config` display/set (`commands/config_cmd.rs:320,1215`) and the
TUI config editor — **never** by any placement decision. Real placement now comes
from WG-Exec typed metadata: `dispatch::plan::placement_from_task`
(`src/dispatch/plan.rs:639-647`) reads `Task.remote_provider`, entirely
independent of `agency.auto_place`/`placer_agent`. `placer_agent` is dead.

---

## 4. The `.wg/agency/primitives/**` and `cache/**` store

Live store facts (read-only):

- `.wg/agency/primitives/**` — **672 files, all mtime 2026-08-07** (one-time
  `wg agency init`/import; never updated).
- `.wg/agency/cache/**` — **13 files, all mtime 2026-08-07**.
- `.wg/agency/import-manifest.yaml` — `schema: agency-12col-v1.2.4`,
  `agency_compat_version: 1.2.4`, `imported_at: 2026-08-07`, counts
  `{role_components: 328, desired_outcomes: 95, trade_off_configs: 195}`.
- `.wg/agency/adaptive/v1/assignment-receipts/**` — **150/150 receipts
  `decision.kind == "uncomposed"`**; `worksgood.toml` `assignment_source` unset
  and `agency_server_url` unset, so the provider branch never fired.

### 4.1 Readers of the primitive/cache store *outside* `wg agency`

| Reader | Path | State |
|---|---|---|
| `agency::load_all_agents_or_warn(cache/agents)` | `src/commands/assign.rs:274` (`run_auto_assign`) | reachable via `wg assign --auto`; daemon admission hook gated `auto_assign=false` |
| `cache/roles` + `primitives/components` scope/eligibility filter | `src/commands/assign.rs:292-293` | same path |
| `agency::load_all_roles(cache/roles)` | `src/commands/service/coordinator.rs:1468` (`build_auto_evolve_task`) | gated `auto_evolve=false` (`:3446`) |
| `agency::load_all_roles(cache/roles)` | `src/commands/service/mod.rs:3110` (daemon bootstrap) | gated `auto_evolve=false` (`:3106`) |
| `load_agency_role(primitives/components)` | `src/commands/nex.rs:351` (fn `:914`) | only when `wg spawn-task --role <name>` is passed explicitly, **or** `wg nex --role`; for `.coordinator-*` tasks the handler uses `build_system_prompt` (`nex.rs:337-346`), **not** the primitives store |
| `wg role` / `wg tradeoff` / `wg evolve` | `commands/role.rs:57,249`, `commands/tradeoff.rs:252`, `commands/evolve/*` | CLI-only |

**Conclusion:** the primitive/cache store is **not** read exclusively by
`wg agency`, but *every* non-`wg agency` reader is either CLI-only or
config-gated-off. The store is effectively frozen input (all files 2026-08-07)
consumed only by dormant or CLI paths.

Note: the coordinator prompt path *does* read a live `.wg/agency/` subdirectory —
`service/coordinator_prompt.rs:51` joins `agency/coordinator-prompt` — but that
is **not** `primitives/` or `cache/`, so it does not keep this audit's store
alive; it must be preserved (or relocated) separately.

---

## 5. Retirement blockers and exact removals

### 5.1 Smoke scenarios that pin the target surface

| Scenario | Pins | Blocking? |
|---|---|---|
| `tests/smoke/scenarios/agency_csv_roundtrip.sh` (manifest `:20`) | `wg agency import --format agency-csv` (`:19`) + `wg agency export --format agency-csv` (`:23`) | **Yes** if `import`/`export` are retired — retire the scenario in the same change. |
| `tests/smoke/scenarios/agency_import_dedup_collision.sh` (manifest `:30`) | `wg agency import` default warn-and-skip + `--strict` (`:34,64`) | **Yes** if `import` is retired. |
| `tests/smoke/scenarios/simple_local_recovery.sh` (manifest `:3179`) | `wg agency migrate` (`:125`) + `wg agency stats` (`:93`) | **Yes** if `migrate` is retired — drop the `agency migrate` line; `stats` is out of scope. |

No smoke scenario exercises `pull`, `push`, `merge`, `remote`, `create`, or
`scan`.

### 5.2 `WG_AGENCY_COMPAT_VERSION` and import manifests

`WG_AGENCY_COMPAT_VERSION = "1.2.4"` (`src/agency/mod.rs:18`) is shared by
still-live surfaces: `agency_stats` JSON (`commands/agency_stats.rs:994`),
`agency_import` manifest write/read (`commands/agency_import.rs:66,154`),
`agency_init` (`:915`), and mirrored by `identity/mod.rs:120`. The only
*import-manifest* consumers are `import` + `init`:

```
read_manifest      src/commands/agency_import.rs:154
write_manifest     src/commands/agency_import.rs:56
--check / content_hash   src/commands/agency_import.rs:627,665
import-manifest.yaml     .wg/agency/import-manifest.yaml  (schema agency-12col-v1.2.4)
```

- **No compat bump is required** to retire the verbs/fields in §2/§3: the const
  is not an on-wire handshake for the CLI verbs, and the other consumers
  (`stats`, `eval`, `init`) can keep it.
- If `import`/`export` are retired, retire the two smoke scenarios in §5.1 and
  make `read_manifest`/`ImportManifest` dead-code removals; leave a migration
  that ignores a stale `.wg/agency/import-manifest.yaml` rather than erroring.
- The `agency_compat_version` field stays at `1.2.4` until/unless `agency stats`
  (C4) is also retired; only then consider `1.2.4 → 1.3.0` with a loud-fail
  handshake mirroring the WG-Fed pattern.

### 5.3 Exact `DEPRECATED_KEYS` additions

Add to `DEPRECATED_KEYS` in `src/config_migrate.rs:57` so `wg migrate config`
drops the remote fields from older/project docs:

```rust
// Agency federation/remote surface retired: no live consumer.
("agency", "agency_server_url"),
("agency", "agency_token_path"),
("agency", "assignment_source"),
("agency", "agency_project_id"),
("agency", "upstream_url"),
("agency", "placer_agent"),
```

Then delete the struct fields + defaults:
`src/config.rs:4414,4553,4557,4562,4566,4570` and `:4635,4663,4664,4665,4666,4667`.
Also remove the `--upstream` flag (`src/cli.rs:5944`) and the two daemon
touchpoints in §0.

Note: `assignment_source` also exists as an unrelated enum field in
`src/agency/types.rs:976` (`AssignmentSource`) and is written as `Native` at
`src/commands/assign.rs:536`. The **config** string field is the one to drop;
the task-record enum stays.

### 5.4 Requisite order for a safe retirement

1. Sever the `.create-*` exec string in `build_auto_create_task`
   (`coordinator.rs:1611`) and the `agency_init::try_upstream_pull` call
   (`agency_init.rs:305`), replacing them with a loud warn (or removing
   `auto_create`/`upstream_url` handling entirely).
2. Add the `DEPRECATED_KEYS` entries and remove the config fields.
3. Retire the two import/export smoke scenarios and drop the `agency migrate`
   line from `simple_local_recovery.sh`; remove the CLI verb arms from
   `src/main.rs:2529-2680` and the `AgencyCommands` variants (`src/cli.rs:5801+`).
4. One-release deprecation: keep the subcommands emitting a loud
   `--deprecated` warning before removal (matches the existing
   `wg profile use` → `wg profile select` precedent).

---

## 6. Residual risk / what would change the verdict

| Risk | Mitigation |
|---|---|
| `LocalStore` engine (store.rs, 554 LOC) is separately test-covered and may be wanted for `wg agency stats`/onboarding | Retire the **CLI wrappers** first; keep `store.rs` until C4 confirms the stats/analytics consumer is also dead. |
| `nex --role <name>` reads `primitives/components` | Only reachable by an explicit `--role`; not a live dispatch caller. If interactive `nex` role-loading is still desired, keep the *reader* (`nex.rs:914`) and delete only the federation verbs. |
| A user may have a hand-edited `agency_server_url` in a private config | The deprecation warning + `wg migrate config` drop makes it visible; no silent behavior change until removal. |
| Compat const shared with identity/`stats` | Do **not** bump `WG_AGENCY_COMPAT_VERSION` when retiring only the verbs/fields. |

**Bottom line:** the target surface is dead or test-only. The only production
links are two config-gated, config-disabled daemon touchpoints that must be
severed first. Retirement is safe with a one-release deprecation and the
`DEPRECATED_KEYS` additions in §5.3.

---

## 7. Reproduce

```
# verb call-site counts (production, excluding main.rs CLI + tests)
for s in agency_pull::run agency_push::run agency_push::run_export \
         agency_merge::run agency_remote::run_add agency_create::run \
         agency_migrate::run agency_import::run_import agency_scan::run; do
  printf '%s: %s\n' "$s" "$(grep -rn "$s" src/ --include=*.rs | grep -v '^src/main.rs' | wc -l)"
done

# config field readers
grep -rn 'agency_server_url\|agency_token_path\|assignment_source\|agency_project_id\|upstream_url\|placer_agent' src/ --include=*.rs

# store readers outside the agency module
grep -rn 'load_all_roles\|load_all_agents\|primitives/components\|cache/roles' src/commands/ --include=*.rs

git diff --check refs/heads/main..HEAD
```
