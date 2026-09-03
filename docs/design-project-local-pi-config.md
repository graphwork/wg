# Project-local-by-default Pi configuration

**Status:** Proposed clean cutover

**Owner task:** `project-local-pi-design`

**Implementation consumer:** `project-local-pi-core`
**Scope:** Pi-routed project configuration, profile application, migration, and provenance

## 1. Decision

A WG project has one authoritative configuration document:

```text
<project-root>/worksgood.toml
```

It is ordinary project source, may be checked in, and is the only persistent
configuration layer consulted for project execution. `~/.wg/config.toml` is no
longer merged into a project. `<graph>/config.toml` and
`<graph>/profile-selection.json` are compatibility inputs only when
`worksgood.toml` is absent; they are never merged with the new document.
Command/task overrides remain higher-precedence, and schema defaults remain the
lowest-precedence source.

The decisive precedence is therefore:

```text
task field > explicit command flag > project worksgood.toml > built-in structural default
```

There is no global-config position in that chain. In particular, **global
non-routing settings do not inherit**. This deliberately removes the most
surprising mode rather than adding a fourth overlay rule.

Reusable profile definitions remain machine-global inputs at
`~/.wg/profiles/<name>.toml`. `wg profile select <name>` resolves a Pi-only,
closed model/reasoning projection from one definition and writes that projection
into `worksgood.toml`. Runtime loading does not reopen the profile definition.
A later edit, rename, or deletion of the reusable definition therefore cannot
change or disable an already configured project. Re-running `profile select` is
the explicit update operation.

The live `.wg/` directory remains an untracked, protected control plane. It is
**not** made check-in-able. Checked-in determinism comes from the sibling source
file `worksgood.toml`, not from weakening the `.wg` boundary.

## 2. Goals and non-goals

### Goals

1. Opening project A must never inherit a route, capacity, guardrail, or agency
   choice left globally by project B.
2. A clean checkout containing `worksgood.toml` must resolve the same WG route
   and reasoning without any named-profile file or active-profile pointer.
3. Profile selection must preserve project guardrails and must not import
   unrelated fields from a reusable definition.
4. A route-less project stays graph-only even if an old global config contains a
   valid route.
5. Migration must be reversible, idempotent, narrowly scoped, and unable to
   touch secrets, identity, or federation state.
6. Every surface that calls a setting “effective” must report its winning
   source and any profile origin.

### Non-goals

- Moving Pi provider authentication, model discovery, endpoint ownership, or
  reported cost into WG.
- Making `.wg` Git-owned.
- Converting native Claude/Codex routes into Pi routes automatically.
- Inferring a project route from launcher history, an installed profile, Pi
  login state, an endpoint, or a global active profile.
- Providing a new global-preferences overlay. If reusable non-routing presets
  are wanted later, they require a separate design and an explicit apply step.

## 3. Current implementation audit

The implementation already contains useful project isolation primitives, but
its effective configuration is still partly machine-global and not portable.

| Area requested for audit | Current behavior and consequence | Current code/tests |
|---|---|---|
| Project association and fingerprints | A selection is `<graph>/profile-selection.json`, containing a profile name, semantic full-profile fingerprint, selection time, canonical-path-derived project digest, and version. The project digest binds the association to one filesystem location. The selected definition is reopened on every config load; a move, missing definition, or content drift fails closed. This prevents silent rerouting but means a clone is not self-contained. | `src/profile/project.rs:1-9,31-45,179-245,330-465`; apply preimage checks at `src/profile/project.rs:694-730,836-945`; `tests/smoke/scenarios/project_profile_history.sh`. |
| Global/local merge | `Config::load_merged` deep-merges global then local, conditionally suppresses global endpoints, and finally overlays a selected project profile. Without a project profile, a local `agent.model` triggers a special strip of global-only role models. This is several precedence modes, not one project authority. | `src/config.rs:5580-5688,5984-6180`; source replay in `src/config.rs:6769-6875`; `docs/config-precedence.md`; `tests/smoke/scenarios/config_local_sticks_under_profile.sh`. |
| Profile overlay breadth | The project overlay strips routing keys but imports profile values for absent non-routing keys. The shipped Pi profile includes `dispatcher.max_agents` and `dispatcher.resource_management.disk_sentinel_enabled`, so selecting it can import more than model/reasoning intent. | `src/profile/named.rs:451-558`; `src/profile/templates/pi.toml`; overlay tests in `src/profile/named.rs`. |
| Setup defaults | `SetupScope` still exposes global/local/both. Non-interactive setup defaults to global, and a fresh interactive setup also defaults to global when no local config exists. Global/both writes a reusable profile and active pointer. Setup performs a Console-mode Pi plugin ensure before committing config. | `src/commands/setup.rs:78-150,1435-1570,1580-1768,1922-1969`; `tests/smoke/scenarios/explicit_execution_selection.sh`; `tests/smoke/scenarios/worksgood_one_model_setup.sh`. |
| Config source reporting | `ConfigSource` has `Global`, `Local`, `ProjectProfile`, and `Default`. `load_with_sources` records layers and then value-matches to repair some labels. `wg config --show`, `--list`, `get`, and `--models` expose source in different shapes. `execution_selection` still accepts a global winning route and may attribute it to the global active profile. | `src/config.rs:5490-5512,6769-6875`; `src/commands/config_cmd.rs:118-173,1459-1533,1700-1882,3113-3177`; `src/execution_selection.rs:117-220`. |
| Existing migration | `wg migrate config` canonicalizes global/local files separately, reports changes, and backs up a changed file. It does not establish a project-only authority boundary or remove the active-profile pointer. | `src/commands/migrate.rs:694-888`; pure predicates in `src/config_migrate.rs`. |
| Pi plugin | The embedded, compatibility-locked plugin has Hermetic mode (cache only, never touches `~/.pi`) and Console mode (also edits global Pi settings). Today setup and global `profile use` use Console mode; JIT spawn can use Hermetic mode. Plugin state is correctly machine-owned, but selecting project routing should not silently edit console-global settings. | `src/pi_plugin/mod.rs:1-31,38-70,113-130`; `src/commands/pi_plugin_install.rs`; `tests/smoke/scenarios/pi_plugin_install_hermetic.sh`, `pi_handler_plugin_transports.sh`. |
| Secrets and custody | Secret backend policy is currently read from `~/.wg/config.toml`; keyring/file custody is under the user home. Thus simply deleting the whole global config would incorrectly delete unrelated secret policy. Identity private keys also share the keystore namespace and must never be classified as routing. | `src/secret.rs:72-130`; `src/identity/keys.rs`. |
| Identity and federation | Public local identity state, replay/freshness memory, and loaded state live under `<graph>/identity`; peer/remotes/trust/node settings live in `<graph>/federation.yaml`. Private keys live only in custody. These are independent of model routing. | `src/commands/identity_cmd.rs:57-205`; `src/federation.rs:15-115`; federation smoke scenarios including `federation_spark_two_graphs.sh`. |
| Checked-in `.wg` | Init adds the graph directory to repository `.gitignore`. More importantly, the control-plane boundary rejects any Git tree or index entry whose path contains a normalized `.wg` component. Tracking `.wg/config.toml` would break candidate sealing and weaken a deliberate safety property. | `src/commands/init.rs:6-18,316-366`; `src/control_plane.rs:1-7,64-75,154-210`; tests in `src/control_plane.rs:1016-1205`. |

### Audit conclusion

The existing association is safe against silent profile drift, but it is a
pointer to machine state, not deterministic project state. The global/local
merge and “profile supplies non-routing defaults” rule also mean a project can
change when unrelated machine-global state changes. The cutover must replace
runtime overlay with apply-time materialization.

## 4. Ownership model

### 4.1 Project-owned settings

Every setting that can affect this graph's behavior is project-owned and comes
from `worksgood.toml` when explicitly set:

- exact Pi model route and reasoning for every dispatch role;
- dispatcher capacity, timing, retry, worktree, archive, and resource policy;
- agency/evaluation policy and project agent bindings;
- worker-control, observer, watchdog, execution-failure, replay, checkpoint,
  guardrail, and log policy;
- project metadata;
- MCP/tool permission declarations as **requests**, never as self-authorizing
  machine grants;
- TUI/viz/help/chat/bash presentation or behavior overrides for this project.

A missing project key uses a documented built-in schema default. A default is
reported as `builtin-default`; it is not described as inherited.

No field from `~/.wg/config.toml`, including UI preferences or daemon tuning,
is inherited. This is intentional. It prevents “non-routing” from becoming an
open-ended escape hatch whose members silently change as `Config` grows.

### 4.2 Machine-owned state that is not a config layer

The following remains machine-global but is never merged into project config:

| State | Canonical ownership | Rule |
|---|---|---|
| Reusable profile definitions | `~/.wg/profiles/*.toml` | Read only while planning/applying `profile select`; never read for runtime resolution. |
| Profile usage history | `~/.wg/profile-usage.jsonl` | Ranking/audit hint only; never selection authority. |
| Pi integration | versioned cache plus `~/.pi/agent/settings.json` | Hermetic runtime preparation is machine capability state. Console wiring requires the explicit `wg pi-plugin install` command. |
| Pi provider login/model catalog | Pi-owned files/services | WG stores exact `pi:` route strings only and never copies credentials. |
| Secret policy and values | `[secrets]` in `~/.wg/config.toml`, OS keyring, `~/.wg/keystore`, or `~/.wg/secrets` | The purpose-scoped secret subsystem may keep reading its machine policy directly; the table is never merged into project config. No secret value or credential path may enter `worksgood.toml`. A later file split is out of scope. |
| Identity private keys | custody backend | Never inspected, copied, or deleted by config migration. |
| Notification/account credentials | their existing dedicated stores | Not imported by profiles or project migration. |

`~/.wg/config.toml` remains readable as **legacy migration data**, not as a
project preference layer. Purpose-scoped subsystem readers may still read an
explicitly machine-owned namespace such as `[secrets]`; that is not Config
inheritance. This distinction must be present in help and JSON.

### 4.3 Project control state that is not source configuration

`<graph>/identity/**`, `<graph>/federation.yaml`, graph/task data, service state,
receipts, usage, `authorization.toml`, and `profile-selection.json` remain under
`.wg`. They are not merged with `worksgood.toml` and are not Git candidates. The
current protection in `control_plane::is_protected_repo_path` remains
unchanged.

### 4.4 Legacy top-level disposition

The cutover must classify every current `Config` top-level section. “Preserve
inactive” means leave the source bytes available for rollback/migration but do
not copy them into, or consult them for, a Pi project.

| Current section | New owner/effect | Project migration action | Global cleanup action |
|---|---|---|---|
| `agent`, `dispatcher` | Project behavior; model keys are routing projection | Copy explicit non-model keys; copy exact Pi model via closed projection | Remove model/provider/executor selectors; preserve other bytes inactive |
| `project`, `help`, `agency`, `evaluation`, `log`, `replay`, `guardrails`, `viz`, `tui`, `checkpoint`, `chat`, `bash`, `worktree_observer`, `pi_watchdog` | Project document | Copy explicit local values | Preserve legacy bytes inactive |
| `worker_control` | Project request bounded by protected operator/control-plane ceiling | Copy as request; never raise effective authority | Preserve legacy bytes inactive |
| `models`, `tiers`, top-level `profile`, `execution` | Project Pi routing | Resolve/flatten exact Pi routes and allowed same-system fallbacks; fail on native/non-Pi ambiguity | Remove models/tiers/profile and exact fallback declarations |
| `llm_endpoints` | Legacy native-handler capability, not used by Pi route | Do not copy; report retained inactive; reject any attempted credential export | Preserve unchanged |
| `model_registry` | Legacy catalog metadata, not route authority | Do not copy; exact Pi identity is already in the projection | Preserve unchanged |
| `tag_routing` | Inert compatibility data | Do not copy; report inert | Preserve unchanged |
| `openrouter` | Legacy native cost/credential policy; Pi owns provider policy | Do not copy; report retained inactive | Remove only route-bearing `fallback_model`; preserve caps/metadata |
| `native_executor` | Native-handler settings outside the Pi-only design | Do not copy; report retained inactive | Preserve unchanged |
| `mcp` | Project request bounded by protected machine approval | Copy name/command/args/enabled only; reject nonempty inline `env`; new schema permits typed secret/environment references, never values | Preserve legacy bytes inactive |
| `secrets` | Typed machine secret-backend policy | Never copy; continue `SecretsConfig::load_global` | Preserve byte/semantically unchanged and active only for secret subsystem |
| `auth` | Legacy handler credential source; Pi/other CLIs own auth | Never copy or inspect values; report credential-bearing section retained | Preserve unchanged and inactive for Pi |

Matrix notification credentials remain at the current documented
`~/.config/worksgood/matrix.toml`; Pi login/plugin settings and other
account-specific stores remain owned by their respective subsystem. No unknown
future top-level section is copied: migration fails closed and names the key so
ownership can be designed before data moves.

## 5. Canonical project document

`worksgood.toml` is rooted next to `.wg`, not inside it. Project-root
discovery is one canonical algorithm:

1. an attempt uses the project-root identity frozen in its launch permit;
2. otherwise explicit `--project` wins after canonicalization and must match the
   graph binding;
3. otherwise `<graph>/project.json`, written by `wg init`, supplies a relative
   root (`..` for the ordinary `<root>/.wg` layout);
4. for an old ordinary graph with no binding, and only then, basename `.wg`
   implies its parent and is persisted on migration;
5. an arbitrary external/nonstandard `--dir` without a binding fails with
   `WG-PROJECT-ROOT-REQUIRED` instead of guessing.

Symlinks are resolved before binding comparison; display paths remain relative
to the logical project root. Commands from subdirectories and linked worktrees
reuse the graph binding, not the current working directory. A mismatch names
both redacted paths and performs no write. Clone portability comes from the
relative ordinary binding and source document, not a canonical-path digest.

The document is stable TOML with a mandatory schema version and optional
profile-origin record:

```toml
schema_version = 1

[profile_origin]
name = "pi"
definition_fingerprint = "b3:..."   # semantic digest of the reusable input
projection_fingerprint = "b3:..."   # digest of the closed block below

[agent]
model = "pi:openai-codex:gpt-5.6-sol"

[dispatcher]
model = "pi:openai-codex:gpt-5.6-sol"
max_agents = 4                       # existing project choice; not from profile

[dispatcher.resource_management]
disk_sentinel_enabled = true         # existing project guardrail; preserved

[models.default]
model = "pi:openai-codex:gpt-5.6-sol"
reasoning = "high"

[models.task_agent]
model = "pi:openai-codex:gpt-5.6-sol"
reasoning = "high"

[models.evaluator]
model = "pi:openai-codex:gpt-5.6-luna"
reasoning = "low"

[models.assigner]
model = "pi:openai-codex:gpt-5.6-luna"
reasoning_mode = "provider-default" # mutually exclusive with `reasoning`
# ...one explicit resolved entry for every dispatchable role...
```

`selected_at` is intentionally absent: selecting the same definition twice
must be byte-idempotent. Time belongs in local usage/audit history, not in the
checked-in declaration.

### 5.1 Closed Pi projection

`profile select` does not copy the profile file. It parses the definition in an
isolated resolver that has no global/local inputs and produces this allowlist:

1. `agent.model` and `dispatcher.model`;
2. `models.default` and every member of `DispatchRole::ALL`, with the final
   exact `model` and reasoning instruction written explicitly;
3. `profile_origin.{name,definition_fingerprint,projection_fingerprint}`.

Every resolved model must be an exact `pi:<provider>:<model>` route. Legacy
`provider`, `endpoint`, and `executor` fields are rejected or derived, never
copied. Tier aliases are resolver inputs but are flattened into explicit role
entries. Reasoning has exactly two TOML forms: a current `ReasoningLevel` in
`reasoning`, or `reasoning_mode = "provider-default"`; the fields are mutually
exclusive and one is required in a profile-produced role entry. The latter is
not a new `ReasoningLevel`: the runtime omits Pi's `--thinking` flag and
provenance records that deliberate omission. Schema evolution that adds a
dispatch role must fail with `WG-CONFIG-UPGRADE-REQUIRED` until the manifest is
upgraded, rather than silently applying a new role default.

The following profile fields are deliberately **not** imported:

- dispatcher capacity/timers/resource management;
- agency/evaluation/guardrail/worker-control policy;
- project, TUI, viz, chat, checkpoint, log, replay, bash, or MCP settings;
- endpoints, API key references, auth, native-executor settings, registry
  entries, OpenRouter settings, secret policy, and notification settings;
- unknown or future fields.

This rule is an allowlist in one function, not a “routing-like key name”
heuristic. Additions require a schema/version change and tests.

### 5.2 Guardrail preservation and direct edits

Apply edits the TOML tree, removes only the prior managed projection keys, and
inserts the new closed projection in canonical order. All non-projection bytes
are preserved semantically; comment-preserving editing should retain comments
where the TOML editor permits it. Before write, the whole candidate is parsed,
Pi-only validated, and compared with the plan preimages.

If a user later changes a managed route directly with `wg config set`, that
command atomically removes `profile_origin`; the route remains valid project
configuration but is now reported as `project-file (manual)`. A hand edit that
leaves stale origin metadata is detected by the projection fingerprint. It does
not consult the reusable profile or a global fallback: config inspection shows
`profile-origin-drift`, and LLM execution fails until the user runs either
`wg profile select <name>` or `wg profile select --clear` (which keeps the
current routes and removes only origin metadata).

### 5.3 Transaction and dry-run

`wg profile select NAME --dry-run` performs no cache, plugin, history, profile,
or config write. The plan includes:

- project document path and preimage;
- definition source and semantic fingerprint;
- exact role routes/reasoning and projection fingerprint;
- exact project keys replaced, added, and preserved;
- `global_config_changed=false`, `global_active_profile_changed=false`, and
  `console_plugin_changed=false`.

Apply reacquires a project-config lock, rechecks document and definition
preimages, writes a recoverable backup, atomically renames one complete
`worksgood.toml`, and only then records bounded usage history. Failure leaves
the prior document active. There is no two-authority runtime window.

A built-in profile may still be materialized once into
`~/.wg/profiles/<name>.toml` using atomic create-new semantics, but the exact
projection is committed to the project document in the same attended apply.
If project write fails, remove only the exact just-created definition preimage,
matching the existing rollback discipline.

### 5.4 Attempt freezing and Git authority

Making configuration source-owned must not let a running worker widen its own
authority. The daemon resolves `worksgood.toml` from the canonical root paired
with the graph, snapshots its fingerprint before issuing a launch permit, and
passes only that frozen configuration to the attempt. A candidate-worktree edit
of `worksgood.toml` is an ordinary proposed source diff: it does not change the
running attempt, the graph owner, or a service generation. It becomes eligible
for later reload only after landing through the normal completion boundary.
Worker/service-admin restrictions continue to apply independently of file
contents. Readiness and status name both the landed config fingerprint and the
attempt's frozen fingerprint when they differ.

### 5.5 Repository requests versus machine authorization

A checked-in document can request behavior; it cannot grant host authority.
For every MCP server, filesystem root, network destination, secret reference,
external command, and elevated worker-control capability:

```text
effective capability = project request ∩ built-in ceiling ∩ operator approval
```

The operator approval is a digest-bound allowlist in protected
`<graph>/authorization.toml`, written only by an attended
`wg config authorize` flow under the existing worker/admin authority checks.
The record binds command/endpoint/root identities (including requested
`bash.path`) rather than trusting a server name, never contains a secret value,
and defaults to deny when absent, stale, or unreadable. Project MCP `env`
literals are forbidden; a typed environment/secret reference is resolved only
after the ceiling authorizes that exact binding. Profiles cannot write the
approval. Migration cannot infer approval from an old MCP declaration or from
the fact that a tool was previously installed.

Provenance for an effective capability reports the project request and the
`operator-ceiling` decision separately. A cloned repository therefore remains
deterministic about what it requests but safely needs local authorization for
host access. Denial of optional capabilities does not silently choose a
different model; readiness names the missing authorization before spawn.

## 6. Resolution behavior

### 6.1 New projects

- `wg init` remains graph-only and creates no route.
- `wg setup`, interactive or non-interactive, defaults to project scope and
  writes `worksgood.toml`.
- `wg setup --route pi --model ...` and `wg profile select pi` share the same
  closed-projection writer.
- `--scope local` is accepted as a deprecated synonym for project scope.
- No route is inferred from global config, an active profile, Pi auth, catalog
  state, launcher history, or plugin availability.

### 6.2 Existing project with no local route and stale global routing

It is **unselected**. Graph reads/edits and setup-neutral TUI continue to work.
Every LLM entry point returns `WG-EXEC-UNSELECTED` before creating service,
claim, session, or worktree state. The diagnostic says, for example:

```text
No project Pi route is selected.
Ignored legacy machine routing: ~/.wg/config.toml (agent.model),
~/.wg/active-profile (pi). These do not configure this project.
Run: wg profile select pi
 or: wg setup --route pi --model pi:<provider>:<model>
```

The exact legacy route may be shown because it is not a credential, but secret
references and endpoint paths remain redacted. There is no prompt-free “adopt
global” action. An attended migration may offer the old route as a choice; the
user must confirm it like any other profile/model selection.

### 6.3 Exclusive legacy compatibility read

For one release only, when `worksgood.toml` is absent:

1. If any legacy association record exists, it must be readable, supported,
   project-bound, fingerprint-valid, and resolvable. A valid association plus
   `<graph>/config.toml` uses the existing project-profile compatibility path
   with global input replaced by an empty table. An invalid, missing-definition,
   drifted, or unsupported association is a hard execution failure.
2. Only when no association record exists is `<graph>/config.toml` alone read.
3. Otherwise the project is route-less.

This is an **exclusive fallback**, not a merge: the presence of
`worksgood.toml` disables both legacy project inputs, and global config is never
read as an effective layer. Every fallback result carries
`legacy-project-source=true` and a migration command. The following release
removes execution from these legacy inputs after telemetry-free warning time;
read-only migration remains.

Native Claude/Codex legacy project routes are reported but do not get rewritten
to Pi. LLM execution remains blocked until the operator explicitly selects an
exact Pi route. Shell task execution is orthogonal and remains allowed.

## 7. Compatibility and deprecation

| Surface | Cutover behavior | Removal path |
|---|---|---|
| `wg profile select NAME` | Canonical project operation. Pi-only. Materializes the closed projection into `worksgood.toml`; does not mutate global config, active pointer, or Pi console settings. | Permanent. |
| `wg profile select --clear` | Removes only `profile_origin`; by default keeps current explicit project routes. `--clear-route` is a separate destructive, confirmed action that returns the project to graph-only. | Permanent. |
| `wg profile use NAME` | One-release deprecated alias for `profile select NAME` when a project root is available. It prints that scope changed and never performs the old global mutation. Without project context it fails with actionable guidance. | Remove after one release. |
| `wg profile use --clear` | Deprecated alias for `profile select --clear`; it must not silently resurrect the old active-pointer semantics. | Remove with `profile use`. |
| `wg profile create/edit/pi/init-starters` | Continue managing reusable global definitions. Output says “definition only; no project selected.” | Permanent. |
| `wg config --global --show`, `wg config lint --global` | Read-only legacy inspection. Project-behavior values are labeled `legacy-global (inactive)`; retained subsystem namespaces such as `[secrets]` are labeled `machine-setting` and are never described as project-effective. | Keep through migration window. |
| Any global config write (`config set --global`, `config --global --model`, setup scope `global` or `both`) | Hard error before write. Guidance points to project config or profile-definition editing. Do not accept a command that exits success while having no current effect. | Remove obsolete flags after one release. |
| `wg setup` without scope | Project scope. `--scope local` warns but behaves identically. `global` and `both` fail before plugin/config mutation. | Make `--scope` unnecessary after one release. |
| Existing `~/.wg/active-profile` | Ignored for project resolution; surfaced only as legacy inactive state and migration input. | Removed by explicit global cleanup. |

This preserves the intent of common `profile use` scripts—select this project's
profile—without preserving cross-project mutation. Explicit global write
scripts fail loudly rather than producing plausible but inert state.

## 8. Pi plugin behavior

Project selection and plugin installation are separate authorities:

- `profile select` only validates that the selected route is Pi-shaped and
  reports bounded readiness. It does not edit `~/.pi`.
- Worker spawn retains the existing JIT `EnsureMode::Hermetic` preparation and
  compatibility handshake. This may materialize the embedded cache but never
  changes console settings.
- `wg pi-plugin install` remains the only command that intentionally performs
  Console wiring. Attended `wg setup` may ask whether to invoke it, defaulting
  to no when Hermetic execution is already ready; the plan must list that
  machine-global side effect separately.
- Plugin status is capability/readiness metadata, never a configuration source
  and never a reason to choose another model or handler.

## 9. Migration and cleanup

Add:

```text
wg migrate project-local-pi [--dry-run] [--cleanup-global-routing]
wg migrate project-local-pi --rollback <receipt>
```

Dry-run is the default when invoked interactively without `--yes`; JSON carries
all preimages and proposed paths but no secret values.

### 9.1 Project migration algorithm

1. Lock the project config transition and hash `worksgood.toml` (or absence),
   `<graph>/config.toml`, and `<graph>/profile-selection.json`.
2. If `worksgood.toml` exists, validate it and return no-op. Never merge legacy
   files into it implicitly.
3. Read explicit keys from legacy `<graph>/config.toml` only. Do not fill
   omitted keys from global config.
4. If a valid project association exists, resolve its selected definition once
   and generate the closed Pi projection. If it is missing/drifted, stop and
   require an explicit profile/model choice; do not use global routing.
5. If the only project route is native Claude/Codex or a bare provider route,
   stop with a redacted report. Never manufacture a Pi provider/model identity.
6. Refuse to copy inline credentials, credential paths, `[auth]`, endpoint
   secrets, `[secrets]`, or nonempty `mcp.servers[*].env` values into the
   checked-in document. Tell the operator to move provider auth to Pi and MCP
   environment material to typed machine secret references, naming keys but not
   values.
7. Canonicalize the remaining explicit project settings, overlay only the
   closed Pi projection, validate, write a backup/receipt under
   `<graph>/migrations/project-local-pi/`, then atomically create
   `worksgood.toml`.
8. Leave legacy `<graph>/config.toml` and `profile-selection.json` in place as
   inactive rollback inputs. The new file's presence makes them non-authority.
9. Re-read through the real loader, compare every effective project leaf and
   route with the plan, then mark the receipt complete. A second run is a
   byte-for-byte no-op and creates no new backup.

The migration must not auto-edit `.gitignore`; `worksgood.toml` is outside the
ignored/protected `.wg` component and appears as ordinary source for the user to
stage. Output explicitly reports `git: untracked|tracked|not-a-repository`.

### 9.2 Optional global routing cleanup

`--cleanup-global-routing` is a separate, explicit machine-wide phase. It
backs up `~/.wg/config.toml` and `~/.wg/active-profile`, then removes only:

- top-level legacy `profile`;
- `agent.model` and `agent.executor`;
- `dispatcher/coordinator.model`, `.provider`, and `.executor`;
- the complete `[tiers]` and `[models]` route/reasoning tables;
- `[[execution.fallbacks]]` (each entry authorizes an exact alternate route);
- `openrouter.fallback_model` while preserving the rest of `[openrouter]`;
- `~/.wg/active-profile` itself.

Empty tables created by those removals may be dropped. It preserves every
other byte semantically, including:

- `~/.wg/profiles/**` and profile usage history;
- `[secrets]`, keyring index, OS-keyring entries, `keystore/**`, and
  `secrets/**`;
- endpoints/API-key references, native-executor data, model registry, cost
  policy, notification/account configuration, and unrelated non-routing keys;
- all project `.wg/**`, especially `identity/**` and `federation.yaml`;
- Pi settings and plugin caches.

Preserving endpoints and registries is deliberate: they may be useful migration
or non-Pi data, and they are not by themselves project selection authority.
The current binary ignores the remaining global file for project resolution.
A later, separately designed cleanup may remove obsolete provider data.

Preservation is proven without reading custody material: cleanup constructs an
allowlisted write-set limited to the global config and active pointer, records
path identity/mode/mtime for protected filesystem roots, and asserts those
paths never enter an open/write/remove operation. OS-keyring entries are not
enumerated or hashed. Tests may hash synthetic fixtures, but production never
reads secret/key bytes merely to prove non-mutation. A plan that names a
keystore, secret value, identity, federation, profile-definition, or Pi-settings
path is rejected as an internal bug.

The global phase acquires the canonical global-config and active-profile locks
in fixed path order (after releasing any project lock), hashes both preimages,
and revalidates them immediately before each atomic replace. Profile
definition/history locks are not needed because those paths are outside the
write-set. A concurrent writer changes a preimage and causes the entire global
phase to refuse; cleanup never rebases its deletion onto newly written bytes.

### 9.3 Backup, crash recovery, rollback, and idempotence

- Project backups live below `<graph>/migrations/project-local-pi/`. Global
  cleanup backups and receipts live only below mode-`0700`
  `~/.wg/migrations/project-local-pi/<receipt>/`, never inside a project graph.
  File copies preserve mode and have mode at most `0600`; receipts record
  pre/post BLAKE3 values, command version, and write-ahead transaction state.
  They contain no newly rendered or enumerated secret values.
- Project and global phases have separate receipts. Global failure cannot roll
  back or corrupt a successful project migration.
- Atomic replace is used per file; the journal records `prepared`,
  `project-committed`, `global-config-committed`,
  `active-pointer-committed`, and `complete`. Recovery has a specified action
  for every state: discard an uncommitted temp after preimage verification,
  finish the remaining replace only when all preimages still match, or restore
  committed postimages by CAS. It never guesses after an unknown state.
- `--rollback <receipt>` uses compare-and-swap: it restores only when current
  bytes equal the receipt's postimage. A later user edit causes a refusal with
  manual paths, never an overwrite.
- Rolling back a newly created `worksgood.toml` removes it only if its exact
  postimage is still present. Rolling back global cleanup restores the exact
  global config and active pointer needed by an older binary.
- No-op apply and repeated cleanup create no backup and do not change mtimes.

## 10. Provenance contract

Introduce one shared representation used by CLI, status, service planning, and
TUI view models:

```json
{
  "key": "models.task_agent.model",
  "value": "pi:openai-codex:gpt-5.6-sol",
  "effective": true,
  "source": {
    "kind": "project-profile-import",
    "path": "worksgood.toml",
    "profile": "pi",
    "definition_fingerprint": "b3:...",
    "projection_fingerprint": "b3:..."
  }
}
```

Allowed project-setting `source.kind` values are `task`, `command`,
`project-file`, `project-profile-import`, `builtin-default`,
`legacy-project-source`, and `unset`. `legacy-global` is allowed only with
`effective=false` in a separate `ignored_sources` array. Purpose-scoped
subsystems may report `machine-setting`; derived host capabilities report an
`inputs` array containing the `project-request`, `builtin-ceiling`, and
`operator-ceiling` decisions rather than pretending any one input granted the
result. None of these machine kinds may source a model route.

Absolute project paths may be omitted/redacted in JSON; relative
`worksgood.toml` plus a project digest is enough. Secret values, secret names not
already requested by the user, credential paths, and Pi login data are never
included.

Every effective-setting surface must use this representation:

| Surface | Required provenance output |
|---|---|
| `wg config --show` / `--list` / `get` | Source for every displayed leaf; header names the authoritative `worksgood.toml` and schema. Scoped global view says inactive. |
| `wg config --models` | Exact Pi route, reasoning, source kind, and profile fingerprints for every role. No generic “explicit” label without a file/origin. |
| `wg profile show` / `list` | Distinguish reusable definition source from current project import. Show definition and applied projection fingerprints and `ready`, `origin-drift`, or `not-selected`. |
| `wg profile select --dry-run/apply` | Definition source, exact projection, keys preserved/replaced, all side-effect booleans, and post-apply project source. |
| `wg status`, `wg check`, execution-selection errors | Effective route/reasoning and source; when unset, list ignored stale global selectors as inactive diagnostics. |
| `wg service start/status/reload` | Persist and show the project config fingerprint and source path used for the daemon generation. A mismatch is “disk config changed”, not a profile/global inference. |
| TUI profile/status views and concierge plans | Consume the same structured view model; compact display may abbreviate fingerprints but must expose detail on demand. |
| Config setters and setup summaries | Written value, effective value, project source, profile-origin removal/preservation, reload result, and machine-global side effects separately. Permission requests show the operator-ceiling intersection. |
| Migration/lint/upgrade | For each key: old source, action (`copy`, `flatten`, `ignore-global`, `preserve`, `remove-routing`), new source, backup/receipt. |

`ConfigSource::Global` and `ProjectProfile` are retired from effective new-mode
results. `ProjectProfileImport` describes bytes in the project document; it is
origin metadata, not a live overlay.

## 11. Phased implementation plan

### Phase 0 — schema and exclusive loader

**Code:** new `src/project_config.rs` and `src/project_authorization.rs`;
`src/config.rs`; `src/execution_selection.rs`; `src/service_identity.rs`;
`src/main.rs` project root resolution; `src/control_plane.rs` ceiling enforcement;
`src/secret.rs` typed machine-policy reader.

- Before disabling global merge, preserve and test the independent,
  partial-deserialization `SecretsConfig::load_global` path. It reads only the
  typed `[secrets]` machine namespace and can never return routes, project
  behavior, or the full global `Config`.
- Parse/version `worksgood.toml` and profile origin; parse protected
  `authorization.toml` separately and intersect every repository capability
  request before it can reach spawn/tool construction.
- Make loader source selection exclusive: new project document, else legacy
  project-only compatibility, else defaults. Remove global merge from project
  execution without changing secret-backend policy.
- Require exact Pi routes in new documents and return route-less state for stale
  global-only projects.
- Add structured `EffectiveSettingSource` and project config fingerprint.

**Tests:** new `tests/integration_project_local_pi_config.rs`; extend
`tests/smoke/scenarios/explicit_execution_selection.sh` with a stale global
config and active pointer that remain ignored before all LLM side effects.

### Phase 1 — profile materialization

**Code:** `src/profile/project.rs`, `src/profile/named.rs`,
`src/commands/profile_cmd.rs`, `src/cli.rs`.

- Add the isolated, allowlisted, closed Pi projection.
- Replace runtime association overlay with transactional `worksgood.toml`
  editing and profile-origin verification.
- Keep definition create/edit/history behavior, but remove definition drift as a
  runtime dependency.
- Make `profile use` the warned project alias.

**Tests:** replace assumptions in
`tests/smoke/scenarios/project_profile_history.sh` and
`config_local_sticks_under_profile.sh`; add clone-without-profile, unrelated
profile-field exclusion, guardrail preservation, idempotent reselect, direct
route edit/origin clearing, and dry-run no-write cases.

### Phase 2 — setup and plugin separation

**Code:** `src/commands/setup.rs`, `src/config_defaults.rs`,
`src/commands/pi_plugin_install.rs`, `src/pi_plugin/mod.rs`, `src/concierge.rs`,
`src/bin/worksgood.rs`.

- Default all setup paths to project scope and the shared projection writer.
- Reject global/both before mutation.
- Use Hermetic plugin readiness for WG execution; make Console installation an
  explicit separately reported choice.
- Update concierge-generated profiles to materialize their closed projection in
  the project document.

**Tests:** update `worksgood_one_model_setup.sh`, `worksgood_concierge.sh`,
`pi_plugin_install_hermetic.sh`, and `pi_handler_plugin_transports.sh`.

### Phase 3 — migration and compatibility CLI

**Code:** `src/commands/migrate.rs`, `src/config_migrate.rs`,
`src/commands/config_cmd.rs`, `src/commands/upgrade.rs`, `src/secret.rs`.

- Implement project migration, optional surgical global cleanup, receipts,
  rollback, CAS, allowlisted write-set proofs, and protected-path metadata.
- Keep the purpose-scoped `SecretsConfig::load_global` reader working and prove
  that routing cleanup never deletes or rewrites `[secrets]` or secret stores.
- Hard-error global project-config writes; label project-behavior reads inactive
  while reporting purpose-scoped subsystem settings separately.

**Tests:** new integration migration suite plus
`tests/smoke/scenarios/project_local_pi_migration.sh`, including byte hashes of
profiles, key stores, identity, federation, and Pi settings before/after.

### Phase 4 — provenance convergence and documentation

**Code:** `src/commands/config_cmd.rs`, `src/commands/status.rs`,
`src/commands/service/*`, TUI snapshot/view models, quickstart/COMMANDS/docs.

- Route every effective-value display through the shared provenance type.
- Remove prose and JSON that call global state active for a project.
- Add JSON compatibility fields only as aliases derived from the new source;
  never emit two disagreeing authorities.

**Tests:** JSON schema assertions and one scripted terminal/TUI flow showing a
fresh checkout, ignored stale global route, project selection, daemon start,
and provenance detail. Register scenarios in `tests/smoke/manifest.toml` under
`project-local-pi-core`.

### Phase 5 — end compatibility read

After one release, remove execution from legacy `<graph>/config.toml` and
`profile-selection.json`. Keep `wg migrate project-local-pi` capable of reading
them. Remove `profile use`, global write flags, `ConfigSource::Global`,
`overlay_project_profile`, and the active-profile runtime read. Do not remove
profile definitions, migration backups, or inactive user data automatically.

## 12. Acceptance matrix

| ID | Scenario | Expected result | Named validation location |
|---|---|---|---|
| A1 | Fresh project; no files; stale valid global Pi route + active pointer | Graph operations work; every LLM entry point returns `WG-EXEC-UNSELECTED`; no service/session/claim/worktree is created; ignored legacy sources shown inactive. | `tests/smoke/scenarios/explicit_execution_selection.sh`; `src/execution_selection.rs` unit tests. |
| A2 | `worksgood.toml` has a Pi route while global selects another Pi route | Project route wins; global value is never present in effective source map. | `tests/integration_project_local_pi_config.rs`. |
| A3 | Global non-routing `max_agents=99`, project omits it | Built-in default is used and labeled `builtin-default`; no global inheritance. | `tests/integration_project_local_pi_config.rs`. |
| A4 | Project sets guardrails/capacity; selected profile contains different values | Existing project values remain byte/semantically unchanged; only model/reasoning projection changes. | `config_local_sticks_under_profile.sh`; profile projection unit tests. |
| A5 | Profile contains endpoint, auth, secret policy, MCP, unknown section, max agents, and resource policy | None are copied. Plan lists them as ignored unrelated fields without values for sensitive fields. | `src/profile/project.rs` projection tests. |
| A6 | Profile tiers/cascade imply routes for roles not explicitly present | `profile select` writes exact Pi model/reasoning entries for every dispatchable role; runtime does not consult the profile. | `tests/integration_project_local_pi_config.rs`. |
| A7 | Select same profile twice | Second apply writes nothing, creates no backup, preserves mtime, and emits identical projection fingerprint. | `project_profile_history.sh`. |
| A8 | Edit/delete global profile after selection; copy checkout to machine with no profiles | Project remains ready with identical route/reasoning; `profile show` may report definition unavailable but runtime is unaffected. | `project_profile_history.sh`. |
| A9 | Clone project to a different path | `worksgood.toml` works unchanged; no canonical-path digest rejection. | `project_profile_history.sh`. |
| A10 | Direct supported config setter changes managed route | `profile_origin` is cleared atomically; source becomes `project-file (manual)`. | `config_local_sticks_under_profile.sh`. |
| A11 | Hand edit changes projection but leaves origin | Inspection reports origin drift and LLM execution fails without global fallback; graph-only commands work. | `tests/integration_project_local_pi_config.rs`. |
| A12 | `profile select --dry-run` with missing built-in definition | No config/profile/history/cache/plugin/lock file changes; plan discloses prospective definition creation and exact project delta. | `project_profile_history.sh`. |
| A13 | `profile use pi` in a project | Warns once, applies project selection, does not write global config/active pointer, exits success. | new compatibility section in `project_profile_history.sh`. |
| A14 | Global config write command | Fails before write with project/profile-definition guidance; no inert-success behavior. | new `tests/smoke/scenarios/project_local_pi_global_write_refused.sh`. |
| A15 | Setup without scope, interactive and `--yes` | Writes/updates `worksgood.toml`, never global config/active pointer. | `worksgood_one_model_setup.sh`; PTY setup flow. |
| A16 | Project selection without Console plugin install | Selection succeeds; `~/.pi` remains byte-identical; Hermetic JIT prepares compatible cache at spawn. | `pi_plugin_install_hermetic.sh`, `pi_handler_plugin_transports.sh`. |
| A17 | Migrate legacy local project profile | Closed projection and explicit local non-routing keys enter `worksgood.toml`; no global omitted key is copied. | `project_local_pi_migration.sh`. |
| A18 | Migrate global-only route | Project remains unselected unless user explicitly confirms a Pi route; dry-run offers but does not adopt. | `project_local_pi_migration.sh`. |
| A19 | Migration sees native Claude/Codex or bare-provider route | Fails with actionable route guidance; no automatic Pi conversion and no partial write. | migration integration tests. |
| A20 | Migration sees inline credential/path | Refuses checked-in copy and redacts value; no file changes. | migration integration tests; secret smoke tests. |
| A21 | Global cleanup with profiles, `[secrets]`, endpoints, keystore, identity, federation, Pi settings/cache | Removes only enumerated selectors and active pointer; allowlisted write-set/path-identity evidence proves protected roots were unopened and unchanged without reading secret values. | `project_local_pi_migration.sh`; federation and secret scenario owners added to the new scenario only where relevant. |
| A22 | Run migration/cleanup twice | Second run is no-op, creates no backup, changes no mtime. | migration integration tests and smoke. |
| A23 | Rollback immediately | Exact project/global bytes and active pointer restored; generated manifest removed only by matching postimage. | migration integration tests. |
| A24 | Edit a migrated file, then rollback | CAS refusal; user edit is not overwritten; manual backup path printed. | migration integration tests. |
| A25 | Every config/profile/status/service/TUI JSON surface | Same effective route/reasoning/source/origin; global appears only as `effective=false`; no credentials leak. | provenance snapshot tests in `config_cmd`, `status`, service, and TUI modules. |
| A26 | Attempt to track `.wg/config.toml` or any `.wg` child | Existing control-plane rejection remains unchanged; `worksgood.toml` is trackable as ordinary source. | `src/control_plane.rs` tests plus a project-config Git integration test. |
| A27 | Running worker edits `worksgood.toml` in its candidate worktree | Its frozen route/authority and current service generation do not change; status identifies the candidate-vs-landed fingerprint difference. | service launch-permit tests plus project-config Git integration test. |
| A28 | Legacy association exists but is corrupt, missing its definition, path-invalid, unsupported, or fingerprint-drifted | Hard execution failure; it never falls through to local-only config or global routing. | `project_profile_history.sh`; loader integration tests. |
| A29 | Manifest schema is unsupported or `DispatchRole::ALL` gains an unmaterialized role | `WG-CONFIG-UPGRADE-REQUIRED`; no role/default inference and no launch side effect. | `tests/integration_project_local_pi_config.rs`. |
| A30 | Definition or project document changes after select/migration dry-run but before apply | Preimage refusal; no partial project/profile/history write. | project transaction integration tests. |
| A31 | Concurrent command updates global config or active pointer during cleanup | Global preimage recheck refuses; concurrent bytes survive exactly. | migration concurrency integration test. |
| A32 | Hostile checkout requests network/filesystem/secret/MCP access with no matching operator approval | Effective capability is deny; no process/tool/network access occurs; provenance shows project request and absent operator ceiling. | new capability-ceiling integration test and `project_local_pi_config.sh`. |
| A33 | Existing `[secrets] allow_plaintext/default_backend` across Phase 0 cutover | Secret commands retain identical policy via typed machine reader; project loader cannot observe any other global key; OS keyring is not enumerated. | `src/secret.rs` unit tests and existing secret smoke scenarios. |
| A34 | Process crashes in each migration journal state | Recovery follows the specified CAS finish/restore action, reaches old or complete new state, and never mixes authorities or overwrites changed bytes. | migration fault-injection integration tests. |
| A35 | Subdirectory, symlink, linked task worktree, ordinary `.wg`, and external `--dir` project discovery | All valid forms resolve the bound landed root; unbound external graph fails `WG-PROJECT-ROOT-REQUIRED`; no cwd or home fallback. | project-root table tests and service launch-permit tests. |

## 13. Requirement-to-rule traceability

| Assignment requirement | Design rule | Primary implementation/test locations |
|---|---|---|
| Decide project-owned vs globally inherited settings | Sections 4.1-4.3: every effective `Config` field is project/default; no global routing or non-routing inheritance; machine capabilities are not layers. | `src/project_config.rs`, `src/config.rs`; A2-A3. |
| Apply reusable profile deterministically without erasing guardrails/importing unrelated settings | Sections 5.1-5.3: isolated closed Pi allowlist, apply-time materialization, preserve all non-projection project keys, no runtime profile read. | `src/profile/project.rs`, `src/profile/named.rs`, `profile_cmd.rs`; A4-A12. |
| Existing route-less project under stale global routing | Section 6.2: unselected, global ignored, no side effects/fallback. | `src/execution_selection.rs`; A1/A18. |
| Compatibility/deprecation for `profile use` and explicit global operations | Section 7: warned project alias; global reads inactive; global writes hard-error; setup project-default. | `src/commands/profile_cmd.rs`, `config_cmd.rs`, `setup.rs`, `cli.rs`; A13-A15. |
| Safe migration removing only stale global routing/activation | Section 9: two-phase project migration plus explicit surgical cleanup; exact removal/preservation sets. | `src/commands/migrate.rs`, `src/config_migrate.rs`; A17-A24. |
| Preserve profiles, unrelated secrets, identity/federation | Sections 4.2-4.3 and 9.2: immutable preservation set and before/after hashes. | `src/secret.rs`, `identity_cmd.rs`, `federation.rs`; A21. |
| Source/provenance on every effective-setting surface | Section 10 shared schema and complete surface inventory. | `config_cmd.rs`, `status.rs`, `execution_selection.rs`, service/TUI models; A25. |
| Checked-in project determinism | Sections 1 and 5: checked-in `worksgood.toml`; `.wg` stays protected; no path-bound runtime association. | `src/project_config.rs`, `src/control_plane.rs`; A8-A9/A26. |
| Phased implementation and explicit acceptance matrix | Sections 11-12. | Downstream task plan and smoke manifest. |

## 14. Final invariants

1. A machine-global file can define a reusable choice or hold a capability, but
   cannot silently configure a project.
2. One project document determines project behavior; compatibility readers are
   exclusive and temporary, never merged precedence layers.
3. Selecting a profile is a copy-by-value of a narrow, closed Pi projection,
   not a durable pointer.
4. Absence of a project route means unselected, regardless of stale global
   state.
5. `.wg` remains outside Git authority; deterministic source configuration is
   outside `.wg`.
6. Migration deletes no profile definition, secret/custody material,
   identity/federation state, endpoint record, or Pi state.
7. An “effective” value without source provenance is a bug.
