# UX and concierge decision menu

**Status:** maintainer decision menu; design only; no implementation, command name, CLI topology, execution provider, install, configuration, or service change is approved

**Inputs:** [symbolic context bar][bar], [search navigation][search], [concierge entrypoint][concierge], [push-button configurator][configurator], the [validated minimal TUI][minimal], the [PTY key-routing contract][keymap], and the completed [clickable Chat selector flow][selector]

## One-page recommended default

Approve the following as **independent product contracts**, not as one TUI-plus-CLI bundle.

### Surface contract

| Surface | Recommended default | Meaning and input contract | Source / disagreement |
|---|---|---|---|
| Context row | Keep one row and replace the changing context selector with persistent padded ` C `, ` T `, ` W ` controls. The exact identity and applicable actions occupy the center; `[ New chat ]` is reserved first. | `C` = Chat, `T` = exact Task/Detail, `W` = Workspace/Dashboard. A second `C` activation opens the existing bounded Chat selector. Each one-character control has a three-cell hitbox; New chat has a 12-cell hitbox. | **[Bar] recommendation.** Preserves one row, one seam, zero outer frames, PTY ownership, non-mutating startup, and the completed selector. It **changes** [Minimal]'s earlier mutually-specific changing `Chat`/`Task` row, so the rail needs an explicit approval. |
| Symbols | ` / ` = search current lane; ` = ` = Controls palette; ` ? ` = Help; ` < ` / ` > ` = previous/next exact identity; ` x ` = lifecycle action for the exact live Chat; ` : ` = exact-context menu; ` @ ` = cached healthy/unknown service pulse; ` ! ` = cached warning. `[ New chat ]` remains the labelled exception. | `=` opens distinct Config, Settings, and Appearance entries; it does not merge Config and Settings. Unavailable actions are omitted, not decorative. Warning and active meaning never rely on color alone. | **[Bar].** An idle symbolic `/` is recommended here for a stable compact grammar, but **[Search] instead recommends `/ Search` or `/ History` whenever it fits**. Decision 2 keeps that disagreement open. |
| Keyboard | Existing routes remain authoritative: `/` begins search in a host-owned graph/command context, `?` opens Help, command-mode `n` opens New chat, `3` reaches Config, `8` Settings, `6` Dashboard, and `Ctrl+O` remains the sole keyboard escape from a live child PTY. Later `F8` bar focus, `Alt-C/T/W`, and `F2` Controls are additive candidates only after PTY conflict tests. | Printable keys, `/`, arrows, and lane mnemonics continue to the child until `Ctrl+O`; no new printable-key interception in PTY or editor input. Bar focus uses arrows, Enter/Space, and Esc only after explicit entry. | **[Bar], [Search], [Keymap].** This preserves the approved child-input contract. |
| Mouse/touch | Route same-frame bar hits before graph or PTY hits. A single primary press anywhere in a padded control activates once; selector rows and footer actions retain their completed full-row/bounded hits. Resize clears stale rectangles before redraw. Hover and double-click are never required. | The one row cannot promise a 44-pixel target; mitigation is three terminal columns, single tap, Termux font scaling, no adjacent one-cell targets, and no hit fall-through. | **[Bar], [Selector].** Preserves the one-row height rather than stealing the child's first row. |
| Search | Graph search is a temporary **exact-task transaction**: Inactive -> Editing/filtering -> Enter or exact-row tap commits a stable task ID -> full graph returns with a two-second target indication -> Inactive. Esc totally clears without quitting. | During Editing: Up/`k`/BackTab = previous result; Down/`j`/Tab = next; Enter = commit; Esc = clear; `n/N` are query text. Refresh remaps by stable task ID; deletion fails closed; render colors matches only while Editing. Context change clears graph search. | **[Search] Option B.** Context-clear **conflicts with [Bar]'s carry-query-across-lanes proposal**; Decision 3 exposes it. Persistent accepted graph matches and their `n/N` overload are rejected by default. |
| Chat history search | Separate state and label from task search. It searches the bounded loaded Chat projection; its existing accepted text highlights and occurrence navigation remain. Starting either search clears the other. | Wide active text says `Tasks /query` or `Chat history /query`; compact mode uses the active `T`/`C` style plus `/query`. `/` typed in a live PTY still belongs to the child unless host command mode was entered. | **[Search].** Do not fix graph search by changing Chat-history semantics. |
| Bar color | Use a light/inverse workspace-colored row in the dark shell, with a neutral inverse first frame. Derive auto color asynchronously from `user@hostname:canonical-repo-path` using BLAKE3 derive-key context `worksgood.tui.workspace-color.v1`, then OKLCH/contrast-safe capability fallbacks. | Render/input perform no host, path, Git, filesystem, environment, palette-query, or hash work. Override precedence: session `WG_TUI_WORKSPACE_COLOR` -> project `[tui].workspace_color` -> global -> `auto`; accepted values are `auto`, `none`, `#RRGGBB`, `ansi:N`. `NO_COLOR` uses reverse/bold/underline and literal `!`. | **[Bar].** This is new appearance, not a new status row. It preserves geometry but needs separate approval from the earlier minimal dark-shell validation. |
| Concierge command contract | Use the placeholder **`<concierge>`** only. In an attended TTY, bare `<concierge>` is the first-run/returning-run lifecycle. It is not the setup-neutral TUI command and it selects nothing by detection. | First run: resolve repository and verified absolute WorksGood build -> explain prerequisites -> ask `Graph only` or an explicit profile (Pi prominent, never preselected) -> show one immutable plan -> confirm -> graph init -> selected prerequisite/auth/plugin owner -> apply exact handler-first route -> validate -> service reconcile -> commit -> launch TUI. Returning run revalidates committed readiness, reuses a matching service, and enters TUI. | **[Concierge] plus the requested refinement.** This **differs from [Configurator]'s safer `onboard` verb/help-first baseline**, but approves only an interaction contract, not a spelling or topology. Graph-only remains complete and provider-free. |
| Service and TUI lifecycle | Graph-only: leave service stopped and run the existing setup-neutral TUI. LLM route ready + service down: start. Healthy graph/build/protocol/config match: reuse. Proven stale state: identity-safe repair then start. Build/protocol/config mismatch: show diff, confirm, controlled restart with intended values, and verify handshake. Explicit restart warns first. | Never restart merely because it is up; never imply `--kill-agents`; never launch TUI after failed readiness/reconcile. On TUI exit, leave a detached service running and print status, graceful stop, lifecycle re-entry, TUI-only, and setup commands; graph-only output explicitly says no service runs. | **[Concierge].** Reconcile rejects the earlier literal “if up, restart” request. It also preserves **[Configurator]/[Minimal]'s non-mutating `<verified-wg> --dir <graph> tui`** contract. |
| Execution identity | Let `W` mean the receipt/manifest-verified **absolute** WorksGood executable and `G` the canonical absolute graph directory. Internal argv is always `W --dir G ...`; direct shared-library calls are also acceptable. | Never use `wg`, `command -v`, `which`, a shell string, a basename, or an unknown candidate's `--version` as identity. A service handshake must prove graph, executable, build, protocol, config fingerprint, PID/start time, and socket. | **[Concierge], [Configurator].** This is a release gate, not an implementation detail. |

### Literal geometry

The lines below are exact display-cell widths; all control glyphs are one-cell ASCII. Styling cannot be represented in a code fence: prose identifies the active lane, while the real row uses inverse/bold/underline without changing geometry.

**Wide desktop, 120 columns** — Chat active, then Task active, then task search Editing:

```text
 C  T  W  .chat-17  connected  <  >  x  :  A2/4 R1 Q3 @ok  /  =  ?                                          [ New chat ]
 C  T  W  design-ux-concierge-menu  in-progress  <  >  :  A2 R1 Q3 @ok  /  =  ?                             [ New chat ]
 C  T  W  Tasks /symbolic context  2/7  [Esc]  =  ?                                                         [ New chat ]
```

**Termux portrait, 40 columns (supported floor)** — Chat active, task search Editing, no-chat, then clipped Task identity. Each line is exactly 40 cells:

```text
 C  T  W  .chat-7 ! ?       [ New chat ]
 C  T  W  /fix 2/7          [ New chat ]
 C  T  W  No chat ?         [ New chat ]
 C  T  W  build~ ! ?        [ New chat ]
```

At 40, active `C` or `T` styling supplies the compact search scope; Esc remains the keyboard cancel when `[Esc]` cannot fit. `build~` is a viewport over an unchanged canonical ID, not a renamed ID. Optional `/`, `=`, pulse detail, and context actions collapse before lanes, Help where shown, or `[ New chat ]`.

**Narrow Termux, 32 columns (documented below-floor mode)** — exact 32 cells:

```text
 C  T  W  .chat-7 ? [ New chat ]
 C  T  W  No chat ? [ New chat ]
```

Below 32, the full label cannot coexist with the nine lane cells (`9 + 12 > 20`), so the honest 20-column emergency line is shown between non-rendered ruler bars (the interior is exactly 20 cells):

```text
| C  T  W  +  ?      |
```

The supported floor remains 40 columns. The 20-column `+` is New chat and retains command-mode `n`; it is not permission to weaken the labelled action at supported widths.

### Concierge state card

`<concierge>` is a placeholder, never a PATH call. All choices default to cancel/no mutation until the user acts.

```text
FIRST RUN (attended only)
  observe repo + W + G -> explain -> choose:
    g  Graph only — no route, credential, plugin, or service
    1  Pi — prominent suggested integrated profile; not selected
    …  other configured/available profiles, readiness honestly annotated
    q  Cancel [default]
  -> immutable redacted plan -> confirm once
  -> init graph before project-local setup -> prepare selected prerequisites
  -> apply exact explicit route -> lint/probe -> reconcile -> commit -> W --dir G tui

RETURNING RUN
  re-observe -> validate committed graph/route/readiness/service identity
  -> no-op matching phases -> reuse/start/reconcile as required -> W --dir G tui

RECONCILE
  graph-only: leave stopped | down+ready: start | healthy exact match: reuse
  stale proven: repair+start | mismatch: show diff+confirm+restart intended state
  failure: bounded prior-build recovery or loud service-down; do not open TUI

AFTER TUI
  service-backed: service remains running; print (with the approved spelling):
    Status: <concierge> status | Stop: <concierge> stop  # agents continue
    Re-enter: <concierge> | Viewer: W --dir G tui | Setup: <concierge> setup
  graph-only: say no service is running; omit Stop; print re-entry/viewer/setup
```

Non-TTY bare invocation prints help or `ATTENDED_TTY_REQUIRED` and mutates nothing. Strict dry-run writes no graph, config, journal, usage, cache, service, or TUI state and does not invent an intent. A cancel before confirmation also writes nothing. A restart preserves detached agents, one-shot agency work, chat processes/PTYs, and the outer tmux/mosh session; a message during socket replacement may fail loudly for retry but must not duplicate silently.

---

## Independent maintainer choices

Approve or reject each row separately. No decision has more than three alternatives, and approving a TUI row does not imply approval of any command spelling.

### Decision 1 — persistent lanes

1. **A — Approve persistent ` C  T  W ` rail (recommended).** Stable return paths and padded hits at every supported width; reuse the current selector on second `C` activation.
2. **B — Keep the changing current context selector.** Lowest migration risk, but Chat/Task/Workspace are not all visible and one-tap reachable from every context.

**Conflict requiring approval:** A changes [Minimal]'s validated mutually-specific context presentation while preserving its geometry, PTY ownership, global New-chat action, and non-mutating startup. It must not add a tab row, footer, frame, or child-owned hit area.

### Decision 2 — idle search affordance

1. **A — Symbol-only idle ` / ` (recommended for this integrated grammar).** Active state spells the scope at wide widths; Help and content empty states teach it.
2. **B — `/ Search` and `/ History` whenever they fit ([Search]'s recommendation).** More discoverable, but consumes exact-identity budget and changes width grammar.
3. **C — Keyboard only.** Smallest row, rejected by default because touch users lose visible search reachability.

This decision changes presentation only. All options keep `/` as the host-command binding and never intercept it from a PTY/editor.

### Decision 3 — graph-search context transition

1. **A — Clear on Chat/Task/Workspace change (recommended).** Search remains a scoped exact-task transaction; starting Chat history also clears it.
2. **B — Carry the query and recompute in the new lane ([Bar]'s proposal).** Faster cross-scope repetition, but risks silently changing the meaning/result set of the typed query.

This is a real source disagreement. Do not implement a hybrid that sometimes carries based on result availability.

### Decision 4 — graph-search acceptance model

1. **A — Transactional exact-task commit (recommended; [Search] Option B).** Enter/tap commits stable ID, clears query/filter/match semantics, and leaves only two-second feedback.
2. **B — Persistent vim-style accepted search.** Keeps `n/N` navigation and the state users report as stuck.
3. **C — New modal task palette.** Clearest list geometry, largest change; defer rather than smuggle it into the bug fix.

Chat-history accepted highlights remain independent under every choice.

### Decision 5 — workspace bar appearance

1. **A — Deterministic auto color plus explicit override (recommended).** Async cached BLAKE3/OKLCH contract and truecolor/256/16/mono fallbacks from [Bar].
2. **B — Neutral inverse by default; color opt-in.** Lower visual change but loses automatic workspace differentiation.
3. **C — Keep the current row styling.** Lowest migration; rejects the color proposal without blocking lanes/search.

No option may encode identity, active state, or warnings by hue alone or perform bootstrap work in render/input.

### Decision 6 — lifecycle interaction shape, not its name

1. **A — Bare chosen concierge name runs the attended first-run/returning lifecycle (recommended for usability study).** `up`, if retained, must be an exact alias to the same state machine.
2. **B — Require explicit `<concierge> up`.** Mutation is clearer; bare prints help.
3. **C — Defer a concierge; keep explicit existing primitives only.** No new lifecycle surface.

Approval here authorizes only an isolated usability/reconcile prototype after gates. It does **not** approve `worksg`, `worksgood`, `graphwork`, `wg up`, `onboard`, a wrapper topology, a full rename, or any provider. [Concierge] recommends studying bare lifecycle; [Configurator] prefers explicit `onboard` pending a separate attended UX test.

## Traceability and preserved constraints

| Recommendation | Primary evidence | Earlier constraint or conflict | Required preservation |
|---|---|---|---|
| Persistent C/T/W and symbolic actions | [Bar] §§1, 4, 6, 12–14 | [Minimal] validated context-specific changing labels rather than persistent lanes. | One row, one seam, zero frames; exact backing identity; global labelled New chat at >=32; no startup mutation. |
| Second `C` opens selector | [Bar] §4.2; [Selector] | The selector is already clickable and bounded. | Do not replace, duplicate, or regress full-row/footer/wheel hit behavior. |
| Transactional exact-task search | [Search] Options and state machine | Current accepted state intentionally persists and overloads `n/N`; Chat history intentionally differs. | Stable task ID, latest-wins async derivation, no render scan, total Esc clear, Chat state isolation. |
| Clear on lane change | [Search] refresh/context rules | Direct conflict with [Bar] §13 query carry. | Maintainer chooses A or B explicitly before implementation. |
| Symbol-only idle search | Integrated recommendation from [Bar] grammar | Direct presentation conflict with [Search]'s labelled idle affordance. | Active scope remains visible; pointer remains padded where rendered; `/` remains PTY-safe. |
| Workspace color | [Bar] §§8, 10 | Earlier minimal report validated geometry, not this appearance identity. | Cached snapshot, contrast/non-color cues, capability fallbacks, override, no I/O/hashing in render. |
| Graph-only first run | [Concierge] scope/state table; [Configurator] invariants | No conflict: fresh installs already require explicit execution selection. | No inferred route, credential, plugin, or daemon; TUI still works. |
| Pi prominence without selection | [Concierge] attended flow; [Configurator] detection-is-not-authority | A suggested row can be mistaken for a default/provider choice. | No preselected row, timer choice, detected fallback, or route write before explicit confirmation. |
| Init before local profile apply | [Concierge] attended flow/commit order | [Configurator]'s broader journal stages routing before graph init; local `.wg/config.toml` cannot safely precede graph creation with current init semantics. | UX may plan profile first, but concierge mutation order is graph init -> prerequisites -> local apply -> validate. |
| Reconcile instead of unconditional restart | [Concierge] restart comparison/state table | Rejects the literal “up means restart” behavior. | Reuse healthy match; intended config wins on mismatch; no agent kill; verify handshake. |
| Setup-neutral TUI | [Concierge], [Configurator], [Minimal] | Bare concierge is mutating only after confirmation; `tui` is not. | `W --dir G tui` never init/select/install/auth/start/reconcile. Ordinary bounded UI-state persistence remains its only mutation. |
| Absolute execution identity | [Concierge] authoritative identity; [Configurator] PATH protocol | Current `wg` collides with WireGuard; a facade does not solve it. | No unknown execution, basename trust, shell lookup, overwrite, diversion, or unverified fallback. |
| Post-TUI daemon persistence | [Concierge] lifecycle | Users may assume quitting TUI stops work. | Wait for TUI, leave service detached, print explicit status/stop/re-entry; graph-only message differs. |

## Staged, separately reviewable future work

No stage begins merely because this document lands. Each item is a future task boundary with its own approval, tests, smoke owner, and rollback.

### Stage 0 — decisions and read-only fixtures

1. Record Decisions 1–6 independently; unresolved rows remain off.
2. Add placeholder-labelled golden plans and terminal buffer fixtures only after approval. Specify service handshake/config fingerprint and repository-bounded resolution without a public command/name.
3. Prove strict dry-run and TUI startup have zero setup mutations. Perform name/topology clearance separately.

**Rollback:** documents/fixtures only. **Gate:** no public binary, alias, PATH entry, config key, or daemon action.

### Stage T1 — pure context-bar model and geometry

Future task scope: immutable `ContextBarModel`, exact identity viewport, allocator, same-frame action IDs/hit rectangles, and 20/32/40/60/80/120 goldens. Keep the current renderer behind a development/runtime comparison flag.

**Review boundary:** no search semantics, color bootstrap, Config mutation, or concierge work. **Rollback:** flip to old renderer; no data migration. **Gate:** `[ New chat ]`, existing selector, terminal-chat Detail routing, and empty startup remain green.

### Stage T2 — persistent lanes and input routing

Future task scope: C/T/W lane memory, padded pointer/touch hits, active/focus styles, selector second activation, resize invalidation, and additive key aliases only after conflict tests.

**Review boundary:** preserve all current keys and PTY `Ctrl+O` ownership. **Rollback:** old row flag; lane memory is ephemeral. **Gate:** real tmux/PTY taps at 40/80/120, resize stale-hit rejection, live-child key-through, and Termux-like single taps.

### Stage S1 — graph-search correctness

Future task scope: one authoritative graph-search phase, exact task-node candidates, stable-ID selection/remap/feedback, total clear, context transition chosen in Decision 3, and removal of normal-mode retained-match `n/N` semantics. Leave Chat search code behavior unchanged.

**Review boundary:** independent of the bar color and concierge. **Rollback:** feature flag only during trial; search state is ephemeral and has no persisted migration. **Gate:** failing unit/render tests first plus the real `tui_search_navigation.sh` tmux flow specified by [Search].

### Stage T3 — visible Search and Controls

Future task scope: Decision 2's idle affordance, active scope/query/count/cancel geometry, ` = ` palette, and distinct Config/Settings/Appearance entries. Reuse existing Config/Settings owners; do not merge their side effects.

**Review boundary:** no new setup behavior and no second row/modal frame. **Rollback:** omit `/`/`=` pointer controls while retaining existing direct keys. **Gate:** pointer, bar-focus, compact width, and no-dead-glyph tests.

### Stage T4 — asynchronous workspace appearance

Future task scope: v1 hash vectors, canonical Git-common-directory identity, immutable appearance snapshot, capability mapping, override validation/picker, and no-I/O render assertions.

**Review boundary:** one optional appearance config value only after schema review; no service/provider config. **Rollback:** `none`/neutral inverse and ignore the override without deleting user config. **Gate:** contrast vectors and truecolor/256/16/mono/tmux/mosh/Termux fixtures.

### Stage C0 — concierge prerequisites, still no public name

Future tasks, reviewed separately:

1. repository-bounded resolver and read-only init/setup/service plan APIs;
2. absolute executable/build proof and non-executing identity diagnostics;
3. authenticated daemon handshake plus intended-config reconcile planner;
4. transaction journal/locks/hash-guarded compensation and stable non-TTY/JSON errors.

**Review boundary:** no installed facade and no provider/model default. **Rollback:** read-only APIs first; transaction-created deltas compensate in reverse and never remove later work, external credentials, reused services, or foreign executables.

### Stage C1 — isolated attended lifecycle study

Run a dev-only harness by absolute checkout path with randomized placeholder labels. Exercise graph-only, explicit-profile first run, auth cancel/resume, healthy reuse, mismatch restart, failed recovery, TUI return, tmux/mosh/Termux widths, and non-TTY refusal.

**Gate:** humans correctly predict mutation, graph-only, reuse, and post-exit daemon behavior; actual PTY tests prove agents/one-shots/chats survive restart. This stage may approve Decision 6's **shape only**.

### Stage C2 — topology/name release decision

Only after C1 evidence and the release gates below may maintainers choose a lifecycle facade, a full canonical rename, or continued current topology. Packaging, alias, migration, Pi backend, generated commands, docs, install, and rollback are separate funded tasks from [Configurator]'s staged name plan.

**Rollback:** an isolated harness is removable. No concierge-only experiment may silently become a permanent two-brand product or evidence that a full rename is safe.

## Migration, rollback, and principal risks

| Risk | Migration/rollback rule |
|---|---|
| Width pressure hides identity/actions | At the supported floor reserve full New chat, lanes, warning shape when actionable, Help, and an identity viewport; collapse only optional detail/actions and viewport cells. No partial token; 40 remains supported; 20 is explicit emergency only. Old renderer remains trial fallback. |
| Touch target or stale-hit regression | Text and hit map derive from one immutable frame; resize clears all; primary press activates once; modal/bar hits never fall through. Revert routing/model as one unit, not visible glyphs alone. |
| PTY key theft | No printable alias ships without real child-PTY tests. `Ctrl+O` remains the sole keyboard escape. Revert additive aliases independently. |
| Search stale highlight/retarget | Semantic phase and stable task IDs replace indexes/lines; worker products cannot revive coloring. Search feature flag can revert because state is not persisted. |
| Color drift/latency/contrast | Pin v1 vectors and domain separator; first frame neutral; async cache only; `none` and `NO_COLOR` are complete fallbacks. A future algorithm gets a new version, never a silent v1 change. |
| Config/Settings side effects become conflated | Palette only dispatches to existing distinct surfaces. Remove palette entry without changing either owner. |
| Concierge silently selects Pi/provider | No selection default; detection annotates only; immutable plan names the exact route/source/readiness. On failure pause or graph-only by explicit choice—never fallback. |
| Wrong repo/build/PID | Canonical bounded repo, explicit `G`, verified absolute `W`, and full service handshake. Ambiguity fails closed; never PATH-probe or kill by basename. |
| Config restart preserves old runtime overrides | Reconcile passes intended resolved values and verifies fingerprint; generic restart is insufficient. Failed replacement attempts the verified prior build once or reports service down. |
| Rollback destroys user work | Reverse, hash/identity-guarded compensation. Never delete a graph that gained work, credentials, detached agents, reused daemon, foreign package, or concurrently edited file. |
| Two-brand facade becomes accidental rename | Time-bound isolated study; topology ADR and name clearance are separate. Facade does not solve WireGuard. |
| TUI becomes a hidden mutator | Concierge commits before launching; direct TUI stays setup-neutral and can always be invoked as `W --dir G tui`. |

## No decision yet: names, rename, and release topology

There is **no clearance or approval** for any of the following:

- `worksg`, `worksgood`, `graphwork`, `wg up`, or `onboard` as the public concierge spelling;
- bare no-arg behavior under any concrete executable;
- a concierge-only facade, full canonical CLI rename, dual-bin release, compatibility alias, deprecation, or installer change;
- keeping the current `wg` collision as an accepted permanent risk;
- any execution provider, profile, model, endpoint, credential owner, paid/free policy, or default route.

Known blockers remain: WireGuard owns the cross-platform `wg` command; `worksg` has a third-party GitHub account and incomplete ecosystem/legal clearance; `graphwork` is occupied on PyPI; a concierge-only name does not secure plugins, agents, scripts, services, or later graph commands; and a full rename requires the [Configurator] invocation inventory, absolute Pi backend, generated-command compatibility, service/build handshakes, package-manager coexistence, dual-name field period, and safety-preserving rollback.

Until those gates close, documents and isolated absolute-path studies must use `<concierge>`, `W`, and `G`. They must not install a candidate name, execute an unknown PATH candidate, approve a CLI rename by implication, or turn direct `tui` into setup.

[bar]: design-tui-symbolic-context-bar.md
[search]: design-tui-search-navigation.md
[concierge]: design-concierge-entrypoint-menu.md
[configurator]: design-pushbutton-configurator.md
[minimal]: reports/validate-minimal-contextual-tui-2026-07-18.md
[keymap]: bugs/tui-keymap-routing.md
[selector]: ../tests/smoke/scenarios/tui_chat_selector_mouse_actions.sh
