# Design menu: attended WorksGood concierge entrypoint

**Task:** `design-concierge-entrypoint`

**Status:** decision menu only — **no executable name, command shape, wrapper, or canonical-CLI rename is approved**

**Evidence checked:** 2026-07-21 (name registries, local command/package snapshot, and current checkout); WireGuard/package evidence carried forward from 2026-07-18

**Build examined:** checkout commit `be6701987d119bf5b3dcc849967fd1ffc75373e7`

This document narrows the larger [push-button configurator study](design-pushbutton-configurator.md) to one attended repository flow:

1. locate and visibly identify the repository root;
2. create a **graph-only** `.wg` if it is absent;
3. if the user asks for LLM work, obtain or reconfirm an **explicit execution-system selection**;
4. reconcile the project service to the intended executable build and resolved configuration;
5. enter the TUI.

It compares names and command shapes. It does not create a binary, install anything, change configuration, start or restart a daemon, or rename the current CLI.

## Decision summary

1. **Prefer reconcile over “restart whenever already up.”** Start a down service; leave a healthy daemon alone when graph identity, executable build, protocol, and effective configuration all match; repair/restart only on stale state or a build/config mismatch, or when the user explicitly requests `--restart`. This avoids a needless dispatch gap on every TUI visit.
2. **Treat the bare candidate as the requested attended lifecycle wizard.** On first run it explains prerequisites, asks the user to choose graph-only or an explicit suggested profile, initializes and validates, reconciles service, and opens TUI. On return it skips satisfied phases and goes through readiness/reconcile directly to TUI. `worksg up` remains a useful explicit-shape control or optional alias, but is not the only concierge experience.
3. **No candidate name is approved.** `worksg` is concise but has unresolved name clearance and a third-party GitHub account; `worksgood` is coherent but longer; `graphwork` is already occupied on PyPI. The bare-wizard requirement does not waive those gates.
4. **A concierge/lifecycle name is not a canonical rename.** A limited family such as bare `worksg` plus `setup/status/stop/tui` can invoke an absolute, verified WorksGood executable while the full CLI remains `wg`; that is trialable but leaves the WireGuard collision for all other commands. Making `worksg` the full canonical CLI is a separate, much larger release decision.
5. **Use one narrow orchestration/reconcile layer, not new copies of domain logic.** The concierge should compose reusable plan/apply functions behind existing `init`, `setup`/profiles, plugin/readiness checks, `service`, and `tui`. It needs a transaction coordinator and service identity/reconcile plan, but not a second graph initializer, route writer, daemon manager, or TUI launcher. The broader installer/model-scout `onboard` design remains separate.
6. **Execution identity is data, never a PATH guess.** Every internal invocation is an argv call to an authoritative absolute WorksGood executable plus `--dir <absolute-graph-dir>`. A concierge must never run `wg`, `command -v wg`, or an unknown `wg --version` to decide what it found.
7. **Graph-only/no-credential is a complete result.** If the user wants only the graph/TUI—or has no usable credentials yet—no provider is selected and no service is started. If the user requests LLM work, final confirmation is blocked until they explicitly choose a profile/handler-first route and readiness succeeds. Detection and prominence never become selection.
8. **Opening and exiting the TUI have explicit lifecycle semantics.** The underlying `<absolute-wg> --dir <graph> tui` remains setup-neutral. The concierge commits first, waits for TUI to return, leaves the detached service running, and prints concise stop/status/re-entry commands. Ordinary documented TUI-state persistence is not concierge setup authority.

## Scope and invariants

The proposed concierge is for an **attended TTY in a repository**. It explains and checks prerequisites and may hand off to existing profile/auth/plugin setup, but it is not a bootstrap installer, a new secret store, a package manager, a second daemon supervisor, or a replacement for all current commands.

Non-negotiable invariants inherited from the parent study and the [explicit execution-system design](design-explicit-execution-system.md):

- Fresh installation remains graph-only until a handler-first execution route is explicitly confirmed.
- “Open the graph” and “enable LLM work” are different choices. The first never implies the second.
- No provider, model, profile, endpoint, or paid/free policy is inferred from installed binaries, environment variables, credentials, or a previous global default.
- Existing graph content is never cleared or reinitialized.
- The default service action is **reconcile**, not unconditional restart.
- A restart never implies `--kill-agents`.
- A strict dry run writes nothing: no graph, usage history, config, transaction journal, service state, TUI state, or cache.
- Pi may be shown prominently as a suggested integrated profile, and configured/available Claude, Codex, nex, or other systems may be annotated, but the default choice is cancel/graph-only—not Pi or any detected system.
- Non-TTY invocation never prompts, starts a TUI, or mutates by default.
- The plan names every target path, prerequisite/profile action, config source, service impact, and rollback limit before confirmation.
- A dirty repository is observed, not committed, stashed, reset, or “cleaned.”

## The attended flow

The product-level sequence is deliberately narrower than the parent study's broad installer/model-scout `onboard`:

```text
Observe
  -> ResolveRepositoryRoot
  -> ResolveAuthoritativeExecutable
  -> ExplainAndInspectPrerequisites
  -> InspectGraphAndRepo
  -> AskIntent(GraphOnly | ExplicitProfile)
  -> SuggestProfilesWithoutSelecting     # Pi prominent; all readiness annotated
  -> SelectOrConfirmExplicitRoute        # still read-only
  -> InspectServiceIdentityAndHealth
  -> PlanAllMutationsAndDisruption
  -> ConfirmOnce
  -> InitGraphIfAbsent                   # MUST precede local setup apply
  -> PrepareSelectedProfilePrerequisites # auth/plugin owners; route not winning yet
  -> ApplyProfileAndRouteIfNeeded        # existing setup/profile/config primitive
  -> ValidateRouteReadiness
  -> ReconcileServiceIfLlmWork           # existing service primitives
  -> CommitTransaction
  -> LaunchAndWaitForTui                 # existing TUI entry, post-commit
  -> PrintLifecycleMessage               # service stays detached
```

The route/profile choice occurs before the combined confirmation, but is not written while merely choosing. **Apply ordering is different from UX ordering:** the user experiences one plan, yet graph init must apply before project-local profile/config setup. Today a local config belongs under `.wg`, and creating `.wg/config.toml` first can make the existing init primitive see an already-existing graph directory and refuse rather than merge. Therefore selection/planning may precede init, but mutation order is `init -> selected-profile prerequisites -> local setup apply -> validate -> service`. A separately confirmed global profile change does not technically require init first, but the concierge should still defer it until after graph creation so cancellation cannot change unrelated projects.

If the current setup implementation cannot plan without writing, it must first expose its existing planner as a reusable API; the concierge must not recreate route/profile resolution.

### Source-verified current seams

| Current behavior | Checkout evidence | Design consequence |
|---|---|---|
| Generic graph resolution walks upward to `.wg`/`.workgraph`, then may fall back to `~/.wg`. | `src/main.rs:27-133` | Concierge mutation needs repository-bounded mode; it must not independently reimplement discovery. |
| Graph-only init and route setup already have dry-run/planning and backup behavior. | `src/commands/init.rs`; `src/commands/setup.rs`; source audit in the [parent study](design-pushbutton-configurator.md#source-verified-current-behavior) | Reuse/refactor those planners rather than writing files directly. |
| Service start refuses missing explicit execution selection, is idempotent when a recorded live daemon exists, uses `current_exe()`, and detaches with `setsid`. | `src/commands/service/mod.rs:1133-1364` | Reconcile can use existing service ownership, but needs a stronger build/config handshake. |
| Graceful restart stops with `kill_agents=false`; current restart captures and reuses prior `CoordinatorState` values. | `src/commands/service/mod.rs:3400-3540`; `src/commands/service/ipc.rs:409-418,889-903` | Agents survive by design, but a config mismatch needs intended-value reconcile rather than a blind restart. |
| Current service status removes a dead PID's stale state as part of inspection. | `src/commands/service/mod.rs` `run_status` | Concierge plan/dry-run and `worksg status` need a strictly read-only observe mode; cleanup belongs to a confirmed repair plan. |
| Inline evaluation/FLIP/assignment wrappers are detached with `setsid`.  | [`docs/design-agency-tasks-on-claude.md`](design-agency-tasks-on-claude.md) | Daemon restart must preserve and re-observe them; it need not cancel them. |
| `tui` directly enters the viewer rather than init/setup/service. | `src/main.rs` command dispatch; parent source audit | Keep TUI setup-neutral and post-commit. |

The wizard first explains the readiness layers:

```text
WorksGood can open a graph without an AI account.
LLM work additionally needs one execution profile, that profile's own
authentication/endpoint, any required WorksGood integration, and a ready service.
Nothing detected below will be selected automatically.
```

Its first choice combines intent and explicit profile selection. Illustrative labels—not a hard-coded provider list—come from existing profile/setup catalogs and read-only readiness probes:

```text
Choose how this repository should run:
  g. Graph only — no credentials, no service
  1. Pi (suggested integrated experience)  installed; auth unknown; plugin missing
  2. Codex profile                         installed; auth check pending
  3. Claude profile                        installed; auth check pending
  4. nex/local profile                     configured endpoint; probe pending
  m. More configured/available systems…
  q. Cancel [default]
```

“Suggested” controls explanation/order only. There is no preselected row, timeout default, detection-based activation, or fallback. A first-run global active profile is shown as context but is not silently copied into the repository. On a returning run, a project-local explicit profile/route previously committed by this flow may be labeled “current” and reused without re-asking the whole catalog unless it is unavailable, unready, or the user enters setup.

Readiness annotations must be honest. The parent study found no credential-safe, noninteractive Pi auth-status or forced-tool probe primitive. Until those exist, Pi must say `auth status unknown — attended check required`, not infer readiness by reading secret files or merely finding `pi`. Similar handler checks distinguish `installed`, `configured`, `authenticated/probed`, and `ready`; those words are never interchangeable.

For `g`, a missing graph can be initialized graph-only and opened with no credential. For any profile, the combined plan must contain its exact handler-first route, config source/scope, auth owner/status, plugin/integration status, and readiness actions before mutation.

## Name and availability evidence

### What the checks do and do not prove

The following are read-only exact-name observations, not legal clearance. Registry absence can change immediately and says nothing about trademark, domains, unindexed executables, private packages, app stores, social names, or common-law use. A release still needs counsel/maintainer review and a broader package/platform search.

Evidence collected on **2026-07-21**:

| Surface | `worksg` | `worksgood` | `graphwork` | Meaning |
|---|---|---|---|---|
| npm exact package | [404](https://registry.npmjs.org/worksg) | [404](https://registry.npmjs.org/worksgood) | [404](https://registry.npmjs.org/graphwork) | Exact npm names appeared unregistered at check time only. |
| crates.io exact crate | [404](https://crates.io/api/v1/crates/worksg) | [404](https://crates.io/api/v1/crates/worksgood) | [404](https://crates.io/api/v1/crates/graphwork) | The checkout package is locally named `worksgood`; no exact public crate record was returned. |
| PyPI exact project | [404](https://pypi.org/pypi/worksg/json) | [404](https://pypi.org/pypi/worksgood/json) | **Occupied:** [`graphwork` 0.1.1](https://pypi.org/project/graphwork/) | `graphwork` already has an unrelated package owner and is not a clean cross-ecosystem name. |
| Homebrew core formula API | [404](https://formulae.brew.sh/api/formula/worksg.json) | [404](https://formulae.brew.sh/api/formula/worksgood.json) | [404](https://formulae.brew.sh/api/formula/graphwork.json) | No exact core formula was returned; taps/casks and other managers were not cleared. |
| GitHub exact account | **Occupied:** [`worksg`](https://github.com/worksg), user created 2018 | [404](https://api.github.com/users/worksgood) | **Occupied by this project's current organization:** [`graphwork`](https://github.com/graphwork) | Project ownership of one account is useful but not global clearance; `worksg` has known third-party ownership uncertainty. |
| This Linux host's PATH | no command found | no command found | no command found | Snapshot only. `wg` resolved to the installed WorksGood build on this host. No candidate was executed for identification. |
| This host's APT cache, exact package | no package found | no package found | no package found | Snapshot only. `wireguard-tools` was present in the same cache. |

Unchecked release surfaces include Debian/Fedora/Arch/Alpine repositories beyond the local cache, Homebrew taps/casks, MacPorts, FreeBSD pkg, Nix attributes, Cargo binary names from differently named crates, Go install paths, Winget/Scoop/Chocolatey, Windows/macOS application names, Android/Termux packages, container image names, shell functions/aliases in other environments, domains, and trademark classes. These are explicit release gates, not implied negatives.

### WireGuard remains the hard collision

The current `wg` name is already owned in operating-system command space by WireGuard's official [`wg(8)`](https://man7.org/linux/man-pages/man8/wg.8.html). Its bare command means `show`, while WorksGood's bare command currently prints help. The 2026-07-18 audit in the [parent study's platform/package matrix](design-pushbutton-configurator.md#platformpackage-collision-matrix) documents Debian/Ubuntu, RPM distributions, Arch, Alpine/containers, Homebrew, MacPorts/FreeBSD, Nix, Cargo, HPC modules, Termux, and Windows.

Consequences for this menu:

- no candidate concierge may locate WorksGood by trying `wg` from PATH;
- adding a concierge under another name does **not** fix agents, plugins, scripts, services, or humans that later type `wg`;
- a full canonical rename can address long-term coexistence, but only after the much larger invocation and compatibility migration;
- no installer may replace, divert, shadow, or force-overwrite WireGuard to make this flow work.

## Scored name/shape menu

Scores are `1` (poor) through `5` (strong). Weights are collision/coexistence 25%, provisional name availability 15%, memorability 15%, typing cost 10%, WorksGood product coherence 20%, and clarity that the command may mutate/reconcile 15%. Totals rank an experiment; they are **not name approval**.

Typing counts include the separating space and exclude Enter.

| Exact requested shape | Collision 25 | Availability 15 | Memory 15 | Typing 10 | Coherence 20 | Mutation clarity 15 | Weighted /100 | Assessment |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| bare **`worksg`** (6 chars), lifecycle concierge | 4 | 3 | 4 | 5 | 4 | 2 | **73** | Matches the requested first-run/returning wizard, but bare mutation is surprising and the exact GitHub account is third-party owned. Known naming uncertainty remains. |
| **`worksg up`** (9 chars), explicit lifecycle shape | 4 | 3 | 4 | 4 | 4 | 5 | **80** | Signals start/reconcile clearly and is the strongest shape comparator. It does not clear the name or solve the canonical `wg` collision. |
| bare **`worksgood`** (9 chars), concierge only | 5 | 3 | 5 | 3 | 5 | 2 | **81** | Highest brand recognition and no exact registry hit in this snapshot, but long for a daily command and bare mutation remains unclear. It can also be confused with the package/product name rather than a limited concierge. |
| bare **`graphwork`** (9 chars), concierge only | 4 | 1 | 3 | 3 | 3 | 2 | **56** | Describes graphs but reverses the familiar “work graph,” weakens WorksGood coherence, and is already occupied on PyPI. The GitHub org is project-owned, not universal clearance. |
| **`wg up`** (5 chars), current-name control | 1 | 1 | 4 | 5 | 3 | 5 | **57** | Cheap and explicit, but the `wg` command is already WireGuard's. It remains unsafe as an unconditional cross-platform entrypoint. |

The scores expose a real tension: `worksg up` communicates mutation better, while the requester wants the bare chosen name to feel like an attended product lifecycle. The recommended experiment therefore treats **bare `worksg` as the primary wizard cell and `worksg up` as the explicit-shape comparator**, with `worksgood` as a brand-recognition control. That experimental role is not approval of any name. The current-name `wg up` control is not recommended: WireGuard gives it a release-blocking coexistence score regardless of convenience.

### Shape semantics

| Shape | Attended TTY | Non-TTY | Repeat invocation | Help expectation |
|---|---|---|---|---|
| bare `worksg` | Runs the requested first-run/returning prerequisite/setup/reconcile/TUI wizard; no write before a visible plan. | Help/attended-required error only, no mutation. | Skips completed phases, revalidates, reuses healthy service, opens TUI. | Deliberately departs from no-arg-help; must be tested and documented. |
| `worksg up` | Explicit spelling of the same lifecycle state machine, if retained. | Refuses mutation; `--dry-run` may print a plan. | Same idempotent reconcile, not a stronger restart. | Useful for scripts/docs clarity, but must not drift from bare behavior. |
| bare `worksgood` | Same lifecycle wizard under a clearer, longer brand candidate. | Help/attended-required error only. | Same as bare `worksg`. | Same no-arg departure. |
| bare `graphwork` | Same lifecycle wizard under the graph-first candidate. | Help/attended-required error only. | Same as bare `worksg`. | Same no-arg departure. |

## Concierge-only wrapper versus canonical CLI rename

These are different product topologies and must not share an approval checkbox.

| Topology | What the name exposes | Migration size | WireGuard result | User mental model | Disposition |
|---|---|---|---|---|---|
| **Concierge/lifecycle facade** | Bare wizard plus a small coherent family (`setup`, `status`, `stop`, `tui`, optionally `up`). Other graph commands remain `wg ...`. | Larger than one wrapper but still trialable if every action invokes shared logic or the verified absolute CLI. | **Does not solve the collision.** It avoids it only for lifecycle commands. | Two command brands: “lifecycle with X, graph operations with wg.” | Useful only as a time-bounded experiment/bridge. Do not call it a rename. |
| **Full canonical CLI rename** | Every command: e.g. `worksg show`, `worksg setup`, `worksg service`, `worksg tui`, and `worksg up`. | Large: release assets, Pi backend, generated task exec, prompts, scripts, services, docs, upgrade/rollback, and old graphs. | Can solve daily/package coexistence if core packages omit `wg` and any alias is strictly verified/optional. | One coherent name after migration. | Long-term option only after the parent study's Stages 0–3 and explicit ADR. |
| **Keep canonical `wg`; add `wg up`** | One new subcommand on today's full CLI. | Smallest code migration. | Permanently ambiguous and not cleanly packageable beside WireGuard. | One short name, but wrong binary risk remains. | Control only; requires explicit acceptance of permanent namespace risk. |

The topology itself is also scored, separately from spelling. Criteria are long-term WireGuard resolution 30%, one-name product coherence 20%, near-term migration safety 20%, achievable internal execution safety 15%, and ease of a reversible experiment 15% (`1` poor, `5` strong):

| Topology score | WG resolution 30 | One-name 20 | Near-term safety 20 | Internal identity 15 | Experiment 15 | Weighted /100 |
|---|---:|---:|---:|---:|---:|---:|
| Bare `worksg` lifecycle facade, canonical graph CLI still `wg` | 1 | 2 | 4 | 4 | 5 | **57** |
| Full canonical `worksg` CLI, including `worksg up` | 5 | 5 | 1 | 5 | 2 | **75** |
| Canonical `wg` with `wg up` | 1 | 4 | 5 | 2 | 4 | **60** |

The full rename scores best only as a funded long-term topology; its near-term safety score is deliberately poor. This score does not approve the name or bypass its migration gates. A lifecycle-facade experiment must not be cited later as evidence that a full rename is safe. Conversely, full-rename migration work is not necessary to learn whether the bare attended lifecycle is understandable.

### Lifecycle command coherence

Using `worksg` only as the illustrative candidate, the requested family must be internally coherent even if it remains a limited facade:

Scores in this family table are `1` (incoherent/surprising) through `5` (clear and product-coherent); they score the subcommand shape, not name clearance.

| Command | Shape /5 | Exact concierge meaning | Existing owner underneath | Must not do |
|---|---:|---|---|---|
| bare `worksg` | **4** | First-run or returning lifecycle wizard: prerequisites/profile, transactional graph/setup, readiness, reconcile, TUI, then lifecycle message. | The one concierge state machine. | Treat a detected profile as selected; restart a healthy match. |
| `worksg up` | **4** | Optional explicit spelling of that same state machine. It is not “restart harder.” | Exact same entry function as bare form. | Develop separate flags/defaults or bypass readiness. |
| `worksg setup` | **5** | Re-enter prerequisite/profile selection and apply a confirmed config transaction; stop before service/TUI unless separately requested. | Existing `setup`, `profile`, config, auth handoff, and plugin primitives. | Start service merely because setup succeeded; replace an explicit route silently. |
| `worksg status` | **4** | Read-only combined repository/graph, selected-profile readiness, and service identity/health summary. | Existing config/profile/plugin/service status APIs. | Mutate stale state merely by inspecting; conflate “profile configured” with “auth ready.” |
| `worksg stop` | **4** | Gracefully stop only this graph's proven daemon with `kill_agents=false`; report that detached running work continues. | Existing `service stop` primitive with explicit graph and identity. | Kill agents/chats, remove config, or stop a PID based only on its name. |
| `worksg tui` | **5** | Open the existing TUI directly for an existing graph. No prerequisite/setup/service reconcile. | Existing setup-neutral `tui`. | Become a hidden synonym for the bare lifecycle wizard. |

This family is more than a one-shot wrapper but less than a canonical CLI rename: `worksg add/show/done/...` would still be absent or would need the old `wg`, which is precisely the two-brand coherence cost. If maintainers want every graph command under `worksg`, that is the full rename topology and must meet its migration gates.

After a service-backed TUI returns, the bare wizard waits for the viewer and prints, using the approved name:

```text
TUI closed. Service remains running (PID 4812).
Status:   worksg status
Stop:     worksg stop        # running agents continue
Re-enter: worksg             # readiness + reconcile + TUI
Viewer:   worksg tui         # TUI only; no reconcile
Setup:    worksg setup
```

A graph-only exit instead says `No service is running (graph-only)` and omits the stop instruction. The concierge must not `exec` away its parent if doing so would make this lifecycle message impossible.

## Repository and graph resolution contract

The concierge accepts no implicit global-graph fallback. Its repository policy is stricter than a generic CLI command because it may initialize files.

1. An explicit project argument, if the eventual command has one, wins and is canonicalized.
2. Otherwise, start at the physical current directory and find the **nearest enclosing repository/worktree root**. A Git worktree's own root wins; its `.git` file is a valid boundary.
3. Stop upward traversal at the nearest nested-repository boundary. Never skip a nested repo and attach it to a parent's `.wg`.
4. If that repository already contains `.wg`, use its canonical absolute path. A legacy `.workgraph` is reported and handled only by existing migration/compatibility policy; do not create a competing `.wg` silently.
5. If no repository root is found, stop and ask for an explicit target. Do not fall back to `~/.wg` or create `.wg` in an arbitrary current directory.
6. Show logical and physical paths, worktree identity, nested-parent discovery, ownership, symlink traversal, and whether the target is dirty before confirmation.
7. Every internal operation receives `--dir <canonical-root>/.wg` explicitly. `WG_DIR`, current directory discovery, and a global `~/.wg` cannot redirect it.

The existing generic resolver walks upward to any `.wg` and can fall back to `~/.wg`; it should be reused only after gaining a repository-bounded mode, not copied into the concierge.

## Authoritative internal execution identity

The concierge's display name is not its execution identity. The transaction records:

```json
{
  "product": "WorksGood",
  "canonical_executable": "/opt/worksgood/releases/0.2.0/bin/wg",
  "canonical_graph_dir": "/home/alex/src/acme/.wg",
  "build_id": "<signed-build-id>",
  "sha256": "<release-manifest-hash>",
  "install_receipt": "/home/alex/.wg/install-receipt.toml"
}
```

The basename may eventually be `worksg`; it is not trusted because of that spelling. Authority comes from one of the non-executing identity proofs defined in the [parent study](design-pushbutton-configurator.md#non-executing-path-identity-protocol): same verified executable/inode, a receipt-owned canonical target, or bytes matching a signed release manifest for the intended build.

A concierge-only binary has two acceptable packaging shapes:

- it is the same signed executable/library bundle and directly calls shared command functions; or
- its signed manifest/receipt names an absolute sibling WorksGood CLI path and expected build ID/hash.

It has no PATH fallback. In particular, production logic never does any of the following:

```text
Command::new("wg")
command -v wg
which wg
wg --version          # unknown candidate execution
sh -c "wg ..."        # shell resolution and quoting ambiguity
```

Conceptually, every operation is an argv vector like:

```text
["/opt/worksgood/releases/0.2.0/bin/wg",
 "--dir", "/home/alex/src/acme/.wg",
 "service", "status", "--json"]
```

The service handshake must return canonical graph identity, executable path, build ID, protocol/compat version, resolved config fingerprint, PID/start time, and socket identity. Current status output does not prove all of those fields; adding them is a release gate before automatic build reconciliation.

## Mutation and state contract

“Default action” below assumes an attended TTY and a confirmed combined plan. Before confirmation, every row is observation only.

| Observed state | Default planned action | Confirmation / rollback ownership | Running-work effect |
|---|---|---|---|
| No repository root | Stop. Ask for an explicit repository path. | None; no graph/global fallback. | None. |
| Missing `.wg` at the resolved root | Plan existing graph-only `init` with no route/agency implication. Show every file the current init primitive may touch, including repository docs/ignore files. | Transaction owns only exact created deltas; restore preimages or remove unchanged created files on rollback. Never remove a graph after it gains tasks/events. | None until a later service action. |
| Existing valid `.wg` | Reuse and validate; do not call destructive/rejecting init. | Existing graph is never transaction-owned. Only separately approved config/service deltas can roll back. | None by itself. |
| Existing legacy `.workgraph` or both names | Stop and show compatibility/migration choices. Never create a second graph implicitly. | Existing migration primitive only after a separate plan. | None. |
| Dirty repository | Continue observation, but list dirty paths that overlap planned init/setup writes. Never stash/reset/commit. Require explicit confirmation for overlaps. | Hash-guard exact preimages; abort rollback rather than overwrite later edits. | None by itself. |
| Nested repository | Nearest repository root wins; show ignored parent graph/root. | Changing to the parent requires explicit target selection and a new plan. | Prevents work from dispatching against the wrong graph. |
| Git worktree | Treat the worktree root and its `.wg` as the project identity, not the main checkout's root. Show linked-worktree relationship. | No shared/main-worktree mutation unless explicitly targeted. | Existing agents in another worktree are unrelated and untouched. |
| Profile catalog/prerequisite inspection | Explain WorksGood, repo, handler CLI/endpoint, auth owner/status, plugin compatibility, route lint, and service layers. Order Pi prominently and annotate all configured/available systems. | Inspection is read-only; package/credential presence is evidence, never authority. | None. |
| Profile choice canceled before confirmation | Exit cleanly with no journal or graph/config/service changes. | Nothing to roll back. | None. |
| Graph-only config; user chooses **Graph only** | Keep route unselected, skip auth/plugin/service, launch TUI after graph commit. This is the no-credential path. | No execution config is written; rollback covers init only. | No LLM agents/evaluations start. |
| Graph-only config; user chooses **LLM work** | Invoke existing setup planning and require an exact explicit handler-first selection before confirmation. | Config preimage and source/scope are journaled; no provider is inferred. | Service remains stopped/unmodified until route readiness passes. |
| Existing explicit route, user chooses LLM work | Display exact profile/handler/model/source/scope. A previously committed project selection is the returning default; changing it enters `setup`. | A reused route is not owned; a changed route has setup's own backup plus transaction hash guards. | Service reconcile may be needed only if effective config differs. |
| Returning graph/profile fully ready | Skip init, profile catalog, auth, plugin install, and config writes after revalidating their receipts/status. Reconcile service and enter TUI. | No completed setup phase is re-owned or repeated. | Healthy matching service is reused, so reopening TUI has no dispatch gap. |
| Selected handler CLI/package missing | Pause before route apply/service. Explain the owning installer and exact requirement; the narrow concierge does not invent a package install. | If no mutations occurred, cancel cleanly; otherwise journal `PrerequisitePending` for resume. | Existing service remains untouched. |
| Profile/config apply fails | Restore setup/config preimages. If this transaction created an unchanged empty graph, offer rollback or preserve graph-only and resume later. | Journal `ProfileApplyFailed`; never activate a different profile as fallback. | Service is not started/restarted. |
| Authentication missing or canceled | Hand off only to the selected system's existing auth owner (for example Pi's interactive auth); never read/log the secret. Pause `AuthPending`. | Credentials are externally owned and never auto-deleted; graph-only may be preserved. | Service is not started/restarted; no cross-system fallback. |
| Required Pi/core plugin missing | Include the existing embedded, compat-locked `pi-plugin` action in the confirmed plan after Pi is explicitly selected. | Back up only its managed settings delta; no npm lookalike. | Service waits for plugin readiness. |
| Plugin install/compat check fails | Roll back only the transaction-owned plugin/config delta or pause for repair; preserve credentials and graph. | Journal `PluginPending`/`PluginFailed`; resume re-observes bytes and compat. | Service is not started/restarted. |
| Endpoint/model/readiness probe fails | Pause with the selected profile and exact failure. Offer retry, setup/change profile, graph-only, or rollback—never automatic fallback. | No ready receipt; route is not called executable-ready. | Existing matching old service is not replaced by an unready plan. |
| Missing route in non-TTY | Exit `EXPLICIT_EXECUTION_REQUIRED`; print absolute, fully scoped existing primitive examples. | No prompt or mutation. | None. |
| Service down, LLM route ready | Start once with the authoritative executable, explicit graph, and intended resolved config. Verify handshake before commit. | `service_owned=started`; stop on rollback only if PID/start/socket/build still match and no operator adopted it. | New dispatch begins after readiness. Existing detached agents, if any, are not killed. |
| Service down, Graph-only intent | Leave down. | No service ownership. | None. |
| Healthy service, all identities/config match | **Reuse; no reload or restart.** | `service_owned=false`. | No dispatch gap; running agents, inline evaluations, chats, and IPC stay undisturbed. |
| Dead PID/stale state/socket | Use the existing identity-safe stale cleanup, then start and verify. Never delete arbitrary state or kill by basename. | Journal exact stale artifacts and service identity. | No live daemon should be affected; detached live agents remain. |
| Orphan daemon or PID ambiguity | Stop and show PID/cmdline/graph/socket proof. Require explicit repair; never guess or use generic `--force`. | Existing service diagnostics/cleanup own the operation. | Potentially disruptive; no action until identity is proven. |
| Live daemon build/protocol mismatch | Plan a controlled graceful stop/start under the authoritative build. Never kill agents. | Capture old executable/config/handshake; best-effort rollback restarts the verified prior build if still available and safe. | Brief dispatch/IPC gap; detached work continues. |
| Live daemon effective-config mismatch | Show exact source-aware diff. After confirmation, apply the intended config and perform a controlled restart by default. A future reload optimization is allowed only if the service primitive can prove the intended fingerprint took effect atomically. | Capture old effective config and config files. Restore only on postimage hash match. | Brief dispatch/IPC gap on the default restart; already-running work keeps its launch-time route. |
| Healthy matching service plus explicit `--restart` | Warn that restart is unnecessary, show active counts, then gracefully restart without `--kill-agents`. | User-requested disruption is journaled; old state captured for recovery. | Brief dispatch/IPC gap despite no functional change. |
| Restart/start fails | Do not launch TUI as “ready.” Restore config postimages where safe and attempt one restart of the verified prior service if the transaction stopped it. Otherwise leave the service down and print recovery commands using the absolute executable. | Result is `RolledBack`, `ServiceDownNeedsRepair`, or `NeedsManualMerge`, never false success. | Existing detached agents/evals/chats are preserved, but no new dispatch occurs until repair. |
| TUI exits after service-backed flow | Wait for TUI, leave the detached service running, then print concise `status`, `stop`, re-entry, pure-`tui`, and `setup` commands. | TUI exit is not rollback. | Agents/service continue; outer mosh/tmux shell regains the prompt. |
| TUI exits on graph-only flow | Print that no service is running and show re-entry/setup commands. | No service action. | No background LLM work exists. |
| stdin/stdout not a TTY | Bare forms print help or an attended-required error. `--dry-run` may emit a read-only machine plan. The concierge itself does not mutate or launch TUI noninteractively; automation uses existing explicit primitives. | No journal for strict dry-run. | None. |

## Literal restart policy versus reconcile

The requester's literal rule is:

```text
if down: start
if up:   restart
then:    tui
```

It is simple, but “up” proves neither health nor mismatch, and restarting a healthy matching daemon adds disruption without moving the project closer to the intended state.

The recommended policy is:

```text
if graph-only intent:
    leave service alone/down; open TUI
else if down and route ready:
    start intended build+config
else if stale/orphaned:
    identity-safe repair, then start
else if healthy and graph+build+protocol+config match:
    reuse
else if build/protocol/config mismatch:
    show diff; controlled restart/reconcile after confirmation
else if explicit --restart:
    warn; controlled restart after confirmation
else:
    fail closed with diagnostics
```

| Case | Literal start/restart | Reconcile | Why reconcile is safer |
|---|---|---|---|
| Healthy matching daemon | Stops and replaces it on every entry. | Leaves it alone. | Avoids dropped in-flight IPC and a dispatcher gap. |
| Healthy daemon with config mismatch | Restarts, but a generic restart may preserve the old effective config. | Plans the intended config explicitly and verifies the new handshake/fingerprint. | “Process changed” is not “configuration converged.” |
| Old build | Restarts whatever executable the generic command resolves. | Starts the authoritative absolute build and verifies its build ID. | Avoids PATH and old/new parent-child skew. |
| Dead state file/socket | “Restart” may stop nothing, then start; behavior is incidental. | Classifies and cleans only proven stale state. | Avoids PID reuse and arbitrary-file deletion. |
| Orphan live daemon | May force-kill based on incomplete evidence. | Stops for explicit identity proof/repair. | An unrelated process must not be killed. |
| Failed replacement | Can leave service down with no captured preimage. | Transaction captures prior build/config and attempts bounded recovery. | Makes disruption owned and visible. |

A current implementation detail is important: `service restart` reads the old daemon's `CoordinatorState`, stops gracefully, and passes the old max-agents/executor/model/interval to `service start`. Calling it blindly after a disk config change can preserve the very runtime config the concierge intended to replace. The production reconcile layer must therefore either enhance that primitive to accept/verify the intended resolved configuration or compose its existing stop/start/reload internals with explicit intended values. It must not duplicate daemon logic in the concierge.

### Effects on live work

Current graceful service shutdown defaults to `kill_agents=false`; task agents are detached with `setsid`, and inline evaluation/FLIP/assignment children are also launched in separate sessions. The following are the design guarantees to preserve and test, not reasons to restart casually:

| Surface | Healthy reuse | Graceful reconcile restart | Caveat / required UX |
|---|---|---|---|
| Running task agents | Unaffected. | Continue independently; no `--kill-agents`. New daemon re-observes registry/graph. | Already-running agents keep the executable/model/environment they launched with; config changes apply to later dispatch, not retroactively. |
| Inline evaluations / FLIP / assignments | Unaffected. | Their detached scripts/one-shot calls continue. | No new inline spawn during the daemon gap; completion writes may race with restart and must be recovered from graph/registry, not memory alone. |
| Chat agents/supervisors | Unaffected. | Detached chat process should continue; daemon-hosted message/IPC coordination pauses briefly and re-observes graph chats on boot. | A message submitted exactly during socket replacement may fail and must be retried, never silently duplicated. |
| TUI process | Unaffected. | Remains in its PTY; service indicators/IPC may transiently disconnect and reconnect. | Do not close/recreate the user's TUI or terminal just to restart the daemon. |
| Embedded chat PTYs / tmux chat sessions | Unaffected. | They are separate from the service process and must not be killed. | Session ownership is graph/path based; service restart is not chat archive/delete. |
| mosh/SSH transport | Unaffected. | Daemon remains detached from the transport. A lost TUI can be reopened and should reuse the service. | Restart does not repair mosh key limitations; see [mosh Enter behavior](bugs/tui-mosh-enter.md). |
| Outer tmux session | Unaffected. | Current pane/session stays alive; no nested tmux is created. | The concierge opens TUI in the current pane. Existing chat tmux sessions are reused by their own ownership rules. |

A forced stop, `--kill-agents`, chat archive/delete, or tmux-session cleanup is outside concierge reconciliation and must never be inferred from `--restart`.

## Transaction, cancel/resume, rollback, and dry run

### Lifecycle phases

```text
Observed
  -> PrerequisitesExplained
  -> IntentSelected(GraphOnly | Profile)
  -> ProfilePlanned | GraphOnlyPlanned
  -> Confirmed
  -> GraphReady
  -> AuthReady
  -> PluginReady
  -> ProfileApplied
  -> RouteReady
  -> ServiceReady | ServiceLeftStopped
  -> Committed
  -> TuiRunning
  -> TuiExited
```

A failure after confirmation becomes `Paused { phase, code, repair }`; it is not rounded up to success and does not choose another profile. The illustrative lifecycle flags are:

```text
worksg --resume [TRANSACTION]
worksg --rollback [TRANSACTION]
worksg status
```

- Cancel before confirmation writes no journal and needs no rollback.
- Cancel during external auth records `AuthPending`; auth-side changes remain owned by that system. The user may resume, preserve the graph as graph-only, or request transaction rollback.
- Resume re-resolves repository/executable identity and re-observes every completed phase. It never trusts a stale checkpoint or repeats an idempotent plugin/profile action blindly.
- Rollback is available from any post-confirmation pause, but may honestly preserve an externally authenticated credential, a graph that gained work, or detached agents.
- A returning normal bare invocation is not the same as `--resume`: it uses committed readiness evidence, revalidates it, skips completed first-run phases, reconciles service, and enters TUI.

### Plan and journal

Before confirmation, the concierge produces an immutable redacted plan containing:

- authoritative executable path/build/hash/receipt;
- logical and canonical repository/graph paths and worktree/nested-repo evidence;
- repository dirty paths and overlap with planned writes;
- graph existence/validity and all init-created/modified paths;
- prerequisite status and the explicitly selected profile (or graph-only), exact handler-first route, auth owner/status, plugin action, winning config source/scope, and diff;
- service PID/socket/start time/graph/build/protocol/config fingerprint and active work counts;
- chosen service action (`leave-down`, `start`, `reuse`, `repair-start`, or `restart`);
- TUI action;
- every backup, compensation, and non-rollbackable boundary.

After confirmation, store a 0700 transaction under `${XDG_STATE_HOME:-~/.local/state}/worksgood/concierge/<id>/`, not inside `.wg` because the graph may not exist. Files are 0600 and contain no credentials or environment dump. The journal uses stable product/build identity, not the typed candidate name.

Acquire repository/config/service locks in one documented order. Re-observe immediately before each mutation; drift invalidates the plan and returns to confirmation.

### Commit order

1. Record preimages and ownership.
2. Run the existing graph-only init apply step if absent.
3. Complete the selected system's existing auth handoff and required integration/plugin action, without ingesting secrets. The selected route is not yet made winning.
4. Apply the explicitly selected profile/route through existing setup/config logic. Project-local apply occurs only after init and prerequisite readiness.
5. Lint/re-resolve the winning config and validate CLI/endpoint/auth/plugin/model readiness against the plan.
6. Perform the planned service action with the authoritative executable.
7. Verify the service handshake when service readiness is required.
8. Commit the transaction.
9. Launch and wait for the existing TUI entrypoint in the current terminal.
10. Print the post-TUI lifecycle message; do not stop the detached service.

TUI launch is post-commit. TUI exit, terminal loss, or mosh reconnection does not roll back a valid graph/route/service. Conversely, a failed profile/auth/plugin/service phase cannot be hidden by opening a graph-only-looking TUI and calling the flow successful.

### Rollback

Compensations run in reverse and are hash/identity guarded:

1. A reused daemon is never stopped.
2. A daemon started by the transaction is stopped only if PID/start/socket/graph/build still match.
3. A replaced prior daemon is restored only from its verified absolute executable and captured effective config; no PATH lookup and no unbounded restart loop.
4. Route/profile/config preimages are restored only while current bytes equal the transaction postimage; otherwise report `NeedsManualMerge`.
5. A required integration/plugin delta is removed/restored only if this transaction created it and its settings/cache bytes remain unchanged and unreferenced elsewhere.
6. A newly created graph is removed only if it is still transaction-identical and has no tasks/events/user changes. Otherwise preserve it and report partial rollback.
7. Repository doc/ignore files use exact preimages or marker-aware deltas; dirty or concurrently edited files are never overwritten.
8. No credential, external package, external agent worktree, running task agent, chat PTY, or tmux/mosh session is deleted. The selected auth owner's logout remains a separate attended choice.

A restart cannot be perfectly atomic across process uptime. The honest outcomes are `Committed`, `RolledBack`, `ServiceDownNeedsRepair`, or `NeedsManualMerge`.

### Strict dry-run behavior

Illustrative proposed output:

```console
$ worksg --dry-run
WorksGood concierge plan (READ ONLY)
Repository: /home/alex/src/acme (git worktree; dirty: README.md)
Graph:      /home/alex/src/acme/.wg (absent)
Intent:     not selected in noninteractive dry-run
Executable: /opt/worksgood/releases/0.2.0/bin/wg
Build:      0.2.0+abc123 (receipt verified)
Would ask: graph-only or LLM work
Would write: nothing
Would start/restart: nothing
Would open TUI: no
```

`--dry-run` does not create a journal or usage record and never resolves its missing intent by choosing a default.

## Illustrative attended transcripts

These are proposed interaction transcripts, not commands implemented by this task. `worksg` is the primary bare-wizard experiment label; `worksg up` is its explicit-shape comparator only.

### Fresh repository, graph-only

```console
$ cd ~/src/acme
$ worksg
Repository: /home/alex/src/acme
Graph:      /home/alex/src/acme/.wg (missing)
WorksGood:  /opt/worksgood/releases/0.2.0/bin/wg (verified build abc123)

Choose how this repository should run:
  g. Graph only — no credentials, no service
  1. Pi (suggested integrated experience) — installed; auth unknown
  2. Codex profile — installed; auth check pending
  3. Claude profile — installed; auth check pending
  m. More configured/available systems…
  q. Cancel [default]
Choice: g

Plan
  CREATE graph-only .wg using WorksGood init
  UPDATE .gitignore and marked project guide blocks (exact diff follows)
  KEEP execution system unselected
  KEEP service stopped
  AFTER COMMIT run:
    /opt/worksgood/releases/0.2.0/bin/wg \
      --dir /home/alex/src/acme/.wg tui
Proceed? [y/N] y

Graph initialized; no execution system selected.
Transaction committed. Opening TUI…

# …user exits the TUI…
TUI closed. No service is running (graph-only).
Re-enter: worksg
Viewer:   worksg tui
Setup:    worksg setup
```

No provider/auth/plugin question is asked because graph-only is a valid no-credential destination.

### Fresh repository with LLM work

```console
$ worksg
Repository: /home/alex/src/acme
Graph:      missing

WorksGood can open a graph without an AI account.
Nothing detected below will be selected automatically.

Choose how this repository should run:
  g. Graph only                              no credential required
  1. Pi (suggested integrated experience)   installed; auth unknown; plugin missing
  2. Codex profile                          installed; auth check pending
  3. Claude profile                         installed; auth check pending
  4. nex/local profile                      configured; probe pending
  q. Cancel [default]
Choice: 2

Combined plan
  CREATE graph-only .wg FIRST
  CHECK selected Codex authentication through its owner
  THEN WRITE project-local profile/route: codex:gpt-5.5
  VALIDATE exact winning route readiness
  START service with verified build abc123 and config fingerprint 98f…
  OPEN TUI after service handshake succeeds
No provider/model fallback is authorized.
Proceed? [y/N] y
```

The exact menu remains owned by `setup`/profiles; the concierge embeds/reuses it rather than maintaining another provider list.

### Pi selected, auth canceled, resume later

```console
Choice: 1  # Pi — explicit
Plan: CREATE graph, hand auth to Pi, ENSURE embedded compat plugin,
      APPLY local Pi profile, validate, start, open TUI
Proceed? [y/N] y
Graph ready. Pi selection journaled; route not applied yet.
Pi authentication canceled by user.
Paused: AuthPending (graph remains graph-only; no service change; no fallback selected)
Resume:   worksg --resume 20260721-1420
Rollback: worksg --rollback 20260721-1420
Or keep the graph-only project and run: worksg tui
```

On resume, successful auth advances to the existing embedded plugin action. If plugin install/compat then fails, the transaction pauses `PluginFailed`, does not start the service, preserves Pi-owned credentials, and offers the same resume/rollback/graph-only choices. A profile apply failure instead restores its config preimage before pausing. None of these failures activates Codex, Claude, nex, or another Pi model.

### Existing graph and healthy matching service

```console
$ worksg
Repository: /home/alex/src/acme
Graph:      existing, valid, 14 open tasks
Route:      codex:gpt-5.5 (project-local, explicitly selected)
Service:    healthy PID 4812
             graph/build/protocol/config all match
Plan:       REUSE service; no config or graph writes; open TUI
Proceed? [Y/n] y
Service reused (PID 4812). Opening TUI…

# …user exits the TUI…
TUI closed. Service remains running (PID 4812).
Status:   worksg status
Stop:     worksg stop        # running agents continue
Re-enter: worksg
Viewer:   worksg tui         # no readiness/reconcile
Setup:    worksg setup
```

This is the common return path: completed setup is skipped, the healthy service is not restarted, and TUI exit does not tear it down.

### Build/config mismatch

```console
$ worksg
Service mismatch:
  running build:  0.1.9+old789 at /opt/worksgood/releases/0.1.9/bin/wg
  intended build: 0.2.0+abc123 at /opt/worksgood/releases/0.2.0/bin/wg
  running route:  codex:gpt-5.4
  intended route: codex:gpt-5.5 (project-local)
  live work:      3 task agents, 1 inline evaluation, 1 chat PTY

Plan: gracefully restart dispatcher; KEEP agents/evaluation/chat alive;
      verify new build+config handshake; then open TUI.
Messages sent during socket replacement may need retry.
Proceed? [y/N] y
```

### Explicit restart of an already matching daemon

```console
$ worksg --restart
Service already matches the intended build and config.
Restart is unnecessary and creates a brief dispatch/IPC gap.
Running work will not be killed: 3 agents, 1 inline evaluation, 1 chat.
Restart anyway? [y/N]
```

### Failed restart and bounded recovery

```console
Restart failed: intended daemon did not complete its build/config handshake.
Preserved live work: 3 task agents, 1 inline evaluation, 1 chat PTY.
Recovery: restored the prior config preimage and attempted the verified prior
          executable /opt/worksgood/releases/0.1.9/bin/wg once.
Result: SERVICE_DOWN_NEEDS_REPAIR (no TUI launched)
Run:
  /opt/worksgood/releases/0.1.9/bin/wg \
    --dir /home/alex/src/acme/.wg service status --json
Transaction: /home/alex/.local/state/worksgood/concierge/20260721-1430
```

The command reports service-down rather than looping, choosing another binary, or killing work.

### Non-TTY

```console
$ worksg </dev/null
error[ATTENDED_TTY_REQUIRED]: bare lifecycle setup/reconcile/TUI requires a TTY
hint: use `worksg --dry-run` for a read-only plan
hint: automation should call the verified absolute `wg init/setup/service`
      primitives with explicit `--dir`, route, scope, and noninteractive flags
```

No partial init occurs before this check.

## Composition decision: a new concierge action, not new domain primitives

### Recommendation

Create, if later approved, **one bare lifecycle-concierge state machine**, with `up` only as an optional explicit alias to the same entry function. Its production implementation composes existing primitives. Do **not** create a second broad `onboard` implementation for this request.

The distinction is semantic:

- `onboard` in the [parent study](design-pushbutton-configurator.md#exact-proposed-cli) can own missing-package installation, open-ended provider/model scouting, optional package policy, and a larger bootstrap transaction.
- This concierge assumes the verified WorksGood bundle already exists. It explains/detects prerequisites, offers existing configured/available profiles, hands auth/plugin work to their current owners, and focuses on repository graph readiness, explicit profile selection, service convergence, and TUI lifecycle.
- Bare invocation and optional `up` are repeatable; their steady state is readiness revalidation, no-op service reuse, TUI, and a post-exit lifecycle message.

### Ownership map

| Concern | Existing owner to reuse | Concierge-owned logic |
|---|---|---|
| Repository-bounded target | Refactor the current graph resolver to expose a bounded mode | Choose policy, display evidence, prohibit global fallback. |
| Graph init/dry-run | `init` planner/apply | Decide whether absent; include its exact diff in the transaction. |
| Execution selection/config | `setup`, profile catalog/use, config resolution and lint | Explain/annotate profiles; require explicit selection; combine plan/rollback. |
| Auth/integration readiness | Selected handler's auth owner; existing `pi-plugin`/handler probes | Coordinate/record readiness without reading secrets or inventing fallback/install logic. |
| Service status/start/stop/reload/restart | Existing service and IPC functions | Compare graph/build/protocol/config fingerprints; select `start/reuse/repair/restart`; journal disruption. |
| Running-work inventory | Service registry/coordinator/chat status | Summarize impact before restart; never kill. |
| TUI | Existing `tui` entrypoint | Launch post-commit in current TTY only. |
| Backups/atomic writes | Existing setup/init config writers where present | One cross-step journal, lock order, ownership flags, and bounded compensation. |
| Executable proof | Installer receipt/release manifest/current executable | Require authoritative absolute identity and put it in every operation/handshake. |

A shell wrapper that screen-scrapes prose from four commands is not sufficient for production rollback or identity checks. A prototype may exercise read-only JSON plans by spawning the **absolute verified executable**, but production should expose shared structured `plan`/`apply` functions or a versioned machine protocol. Either way, domain behavior stays in the existing primitives.

## Recommended staged experiment

No stage approves or reserves a name.

### Stage 0 — read-only contract fixtures

- Add no public binary or alias.
- Build golden plan fixtures for every state-table row using a disposable home/repository and a placeholder label `<concierge>`.
- Specify the service handshake fields and config fingerprint.
- Refactor only where necessary to expose read-only init/setup/service plans; verify `tui` remains setup-neutral.
- Run name clearance beyond the provisional exact-name snapshot.

**Exit evidence:** deterministic plans, zero writes in dry-run, repository-bounded root selection, and no unverified PATH execution.

### Stage 1 — attended PTY usability study, isolated invocation

- Invoke an internal/dev-only harness by an absolute checkout path; do not install `worksg`, `worksgood`, or `graphwork` on PATH.
- Randomize labels/screens for the four requested shapes: bare `worksg`, `worksg up`, bare `worksgood`, bare `graphwork`.
- Test fresh graph-only, missing-route LLM, healthy reuse, mismatch restart, cancellation, and failed restart.
- Measure: correct prediction of mutation, name recall after a delay, typing errors, whether users understand “graph-only,” whether they expect healthy service reuse, and whether they confuse concierge-only with all CLI commands.

**Go signal for shape only:** users reliably predict that the bare name is an attended lifecycle wizard, understand that `up` is at most an explicit synonym, understand healthy reuse and post-TUI service persistence, and do not believe the facade renamed the full CLI.

### Stage 2 — feature-gated reconcile prototype under the current verified binary

- Still no new installed name. Expose the orchestration only to maintainers behind an explicit unstable flag or test harness.
- Drive the actual TTY via tmux/PTY, not only library tests.
- Prove task agents, inline evaluation, chats, TUI PTY, and outer tmux survive controlled restart; inject IPC at socket replacement and verify loud retry behavior.
- Prove config mismatch converges to the intended fingerprint rather than preserving prior `CoordinatorState`.
- Prove failed restart restores the prior verified daemon or reports service-down without killing work.

### Stage 3 — choose topology and name separately

Maintainers make two explicit decisions:

1. interaction: approve/reject the bare lifecycle wizard and decide whether `up` is an exact alias;
2. topology: lifecycle-facade bridge, full canonical rename, or keep canonical `wg` with accepted collision risk.

Only then may release packaging work begin. A concierge-only trial cannot silently turn into a full rename, and a full rename must pass the parent study's staged migration.

## Unresolved release gates

No public implementation proceeds until all applicable gates are closed:

1. **Name/legal/ecosystem clearance:** `worksg`, `worksgood`, and `graphwork` across the unchecked package/platform/domain/trademark surfaces; decide how the third-party `worksg` GitHub account and occupied PyPI `graphwork` affect acceptability.
2. **Topology ADR:** explicitly choose concierge-only versus full canonical CLI rename versus continued `wg`; record that a wrapper does not solve WireGuard.
3. **WireGuard policy:** complete the non-executing PATH/ownership protocol and prohibit overwrite/shadow across every supported installer/package channel.
4. **Bare-command policy:** approve or reject no-arg as the attended lifecycle wizard and decide whether `up` is an alias. Non-TTY no-arg must remain non-mutating.
5. **Lifecycle facade policy:** approve exact `setup/status/stop/tui/resume/rollback` semantics and decide whether the two-brand facade is acceptable or implies a funded full rename.
6. **Repository root policy:** approve nearest nested repo/worktree semantics, no global fallback, legacy `.workgraph` handling, symlink/ownership rules, and the no-repository error.
7. **Init/setup order and mutation scope:** approve `init -> prerequisites -> local profile apply`, and decide whether init's `.gitignore`, `AGENTS.md`, and `CLAUDE.md` writes remain acceptable for “create graph-only `.wg`.”
8. **Profile/readiness UX:** identify the existing setup/profile planning API; define Pi's prominent-but-unselected presentation, auth/plugin probes, failure/pause codes, and how a committed explicit route is reused without silent selection.
9. **Absolute executable proof:** define signed build ID/hash/receipt resolution for source builds, package-manager installs, upgrades, Windows, and concierge-only packaging. No PATH fallback.
10. **Daemon handshake:** add/approve graph identity, executable path, build, protocol, config fingerprint, PID/start/socket fields and old-daemon compatibility behavior.
11. **Config reconcile semantics:** decide when a live reload is sufficient and when restart is mandatory. Fix/enhance the current restart path so intended config is not replaced by captured old runtime overrides.
12. **Running-work contract:** PTY tests must prove agents and inline agency calls survive, chats reconnect without duplicate messages, and service restart never archives/kills chat tmux sessions.
13. **Post-TUI lifecycle:** prove the service remains detached, the wizard regains control and prints correct name-dependent commands, graph-only output is distinct, and mosh/tmux return to the same shell/pane.
14. **Failure recovery:** define the supported previous-build retention window, one-attempt recovery, postimage hash guards, operator-adoption detection, and service/profile/auth/plugin failure codes.
15. **Dry-run and non-TTY:** prove absolutely no usage/journal/cache/TUI write; decide stable JSON/error codes. Automation should continue to use explicit existing primitives unless separately approved.
16. **Full rename only:** complete Pi absolute-backend migration, generated exec compatibility, release/install/upgrade/rollback work, dual-name field period, and package-manager coexistence gates from the parent study.
17. **Lifecycle-facade only:** define how help/transcripts avoid teaching ambiguous PATH `wg`, and set an experiment sunset so the two-name product does not become an accidental steady state.

Until those gates are resolved, the only recommendation is the staged, uninstalled experiment. **This document does not approve `worksg`, `worksgood`, `graphwork`, `wg up`, the bare lifecycle wizard, its `setup/status/stop/tui/up` facade, a wrapper, or a canonical rename.**
