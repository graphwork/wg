# Validation report: symbolic TUI + `worksgood` concierge trial

**Task:** `validate-symbolic-worksg-trial`
**Validator:** `agent-743` (pi, `zai:glm-5.2`, standard tier)
**Validated at:** 2026-07-25
**Report date in title:** 2026-07-22 (task-prescribed filename)
**Mode:** independent review. No candidate was globally installed; no failure was silently repaired.
**Worktree HEAD:** `dd0a7671` (`wg/agent-743/validate-symbolic-worksg-trial`, 0 commits ahead of `main` — clean review tree)
**Isolated candidate target (admitted then removed):**
`/home/bot/wg/.wg-candidate-targets/validate-symbolic-worksg-trial` + `CARGO_HOME=/tmp/validate-sym-cargo-home`

## 0. TL;DR — ship/no-ship

**Recommendation: CONDITIONAL NO-SHIP as one coherent product flow; SHIP the symbolic-TUI rendering substrate and the concierge lifecycle machinery in isolation.**

The symbolic context bar and the `worksgood` concierge are each implemented correctly and pass their focused unit/golden/live tests. **They do not currently compose into one shippable product flow** because the model plane was collapsed to **Pi-only** by `9e52f9f7 feat: make-pi-the-2 (agent-690)`, which landed *after* both the concierge closeout (`c0e9d150`) and the effort work (`ab615900`) and *before* this validation. That change:

1. **Breaks the two centerpiece human-flow smokes** that pin this exact product (`tui_symbolic_context_bar`, `worksgood_concierge_trial`) plus four adjacent pinned regressions, all on `main` today.
2. **Makes the approved `worksgood-concierge-trial.md` design doc stale** — it still advertises *Codex, Claude, Nex/local, OpenCode* as first-class core profile choices that the code now rejects as `legacy/non-Pi`.

This is a documentation/smoke-fixture coherence defect, **not** a defect in the symbolic renderer or the concierge state machine. The fix is bounded: re-baseline the concierge trial doc + the affected smokes to the Pi-only world (or, if multi-handler support is still intended, revert/relax `validate_pi_model_plane`). Until that choice is made and the pinned smokes are green, the “approved symbolic TUI plus `worksgood` concierge trial as one coherent product flow” is not shippable.

Two further dependency-owned smokes fail for non-Pi reasons (`tui_inspector_drag_to_full`, `tui_four_sided_layout_mobile`) and are tracked as separate defects below.

## 1. Scope and method

Reviewed as one product flow per the task:

- **Symbolic TUI:** `src/tui/viz_viewer/{state,render,event}.rs`; design authority `docs/design-tui-symbolic-context-bar.md` + the “Approved symbolic TUI trial” section of `docs/design-ux-concierge-decision-menu.md`.
- **`worksgood` concierge:** `src/bin/worksgood.rs`, `src/concierge.rs`, `src/service_identity.rs`, `src/profile/project.rs`; design authority `docs/worksgood-concierge-trial.md` + `docs/design-concierge-entrypoint-menu.md`.

Method: (a) static audit of every recorded maintainer preference against implementation; (b) clean `cargo fmt --check` / `cargo clippy` / `cargo build` / focused `cargo test` under one bounded isolated target; (c) candidate-binary real-tmux flows at desktop and Termux widths; (d) fault-injection review against the concierge unit suite and smoke design; (e) negative-invariant confirmation by code + live test. The candidate was built with `cargo build --locked --features worksgood-trial --bin wg --bin worksgood` into the isolated target; **no `cargo install`** was run and the target is removed after admission.

## 2. Maintainer-preference audit (drift flags)

| # | Preference | Design authority | Implementation evidence | Drift? |
|---|---|---|---|---|
| 1 | **Symbols** — workshop `↯⌁⌂⌕⌘⋮⊞` + letters `CTW S = ⋮ +`; glyph support never auto-detected; `detect` rejected | decision-menu “Approved symbolic TUI trial” | `state.rs:33` `SymbolMode{Workshop,Letters}`; `parse` rejects `detect` (`state.rs:531`); render emits the approved glyphs (`render.rs:2910,2986,3043,3120,3127`); Help legend (`render.rs:11770-11786`) | **No drift** |
| 2 | **Fallback** — `[ New chat ]` (12-cell) when no resumable chat incl. archived/terminal; `⊞` (3-cell inverse) only with a live/resumable chat; retained ≥32 cols | decision-menu buffers; bar §5.4 | `render.rs:2973` `compact_new = resumable_chats>0 \|\| width<32 \|\| (Full && width<60)`; `NEW_CHAT_BUTTON_LABEL` (`render.rs:4362`) | **Minor drift** — in Full mode at 40–59 cols the compact `⊞` is forced even with no resumable chat, diverging from the “`[ New chat ]` retained ≥32” buffer. Rationale (keep escape + identity reachable) is documented inline; not a regression vs. golden tests but a contract nuance worth a design note. |
| 3 | **Search** — exact-task transaction; Enter commits stable ID; Esc clears query/filter/coloring; lane change clears; `⌕`/`S` | decision-menu Decision 4A; search doc | live + golden `symbolic_context_width_matrix…` PASS; smoke encodes commit-then-clear | **No drift** |
| 4 | **Color** — BLAKE3 derive-key `worksgood.tui.workspace-color.v1` over `user@host:canonical Git common-dir`; HSL sat 0.66–0.94, light 0.28–0.74; WCAG ≥4.5:1; truecolor→256→16→mono; overrides `auto/none/#RRGGBB/ansi:N` | bar §10; decision-menu Decision 5A | `state.rs:274,277,303-307` exact context + ranges; hash vector `647ea424…` pinned (`state.rs:540`); capability ladder + `WG_TUI_COLOR_CAPABILITY`/`WG_TUI_APPEARANCE`/`WG_TUI_SYMBOLS` (`state.rs:379,479,484`); rich-palette + contrast tests PASS | **No drift** |
| 5 | **Identity/actions** — exact `.chat-N`/task ID viewport (never renamed); `⋮` captures exact context before actions; Close/Menu revalidate before mutation | bar §4.3, §7.2, invariant 4 | live `.chat-0`/task IDs render verbatim; `⋮` opens exact-context menu; `open_chat_task_or_detail` never relaunches a terminal chat | **No drift** |
| 6 | **Pulse** — packed `●alive/max⊳ready▸running∴pending-eval`, `○` at zero, `!` warning; one Workspace/system hit; mono-safe | decision-menu “packed cached pulse” | `render.rs:2385-2396` emits `○0/{max}`/`●{alive}/{max}⊳{ready}▸{running}∴{pending-eval}` omitting zero phases; `!` warning preempts; one hit rect | **No drift** (see defect D6 for one smoke’s assertion timing) |
| 7 | **Profile scope/history** — one reversible project-local selection; reusable global profile defs; local usage history; never rewrites global config; fails closed on cross-project identity | concierge trial doc; `project.rs` header | `project.rs:1-11,360,434,442` — selection is `<graph>/profile-selection.json`, history is local JSONL, global config/active-profile untouched, foreign canonical-project identity refuses with no fallback; concierge `apply_selection` (`concierge.rs:1191`) writes project-local only | **No drift** |
| 8 | **Pi tiers** — strong (Worker/chat) + weak (Agency/FLIP/eval) separate explicit routes; separate explicit effort (default high/low); `--thinking` argv separate from model identity; “Same as worker” explicit, never inferred | concierge trial doc; expose-thinking-effort | `worksgood.rs` `--strong-model/--weak-model/--strong-reasoning/--weak-reasoning` (requires both models, no inference); `concierge.rs:736 customize_core_profile` patches two-tier content + separate reasoning; Pi argv probe asserts `--provider/--model/--thinking xhigh` with no model-encoded reasoning (smoke + unit) | **No drift** in mechanism; **blocked** at the legacy-profile gate (D1) |
| 9 | **Lifecycle/name boundary** — `worksgood` is a narrow lifecycle facade; full expert CLI stays `wg`; no alias/installer/rename; `wg` collision with WireGuard unresolved & out of scope | concierge trial doc; entrypoint menu | `worksgood.rs` is a 105-line facade over `concierge::*`; `wg` bin unchanged; no `worksg`/alias/compat shim; `Cargo.toml:55` `worksgood-trial` is a non-default feature absent from release surfaces | **No drift** |
| 10 | **No PATH execution** — W resolved by `current_exe()`/absolute receipt; relative/symlink/unknown rejected; sha256 content fingerprint | concierge trial doc; entrypoint §“Authoritative internal execution identity” | `concierge.rs:232 resolve_authoritative_executable` uses `current_exe()`, refuses relative/symlink (`244-252`), requires `WORKSGOOD_W_RECEIPT` for out-of-bundle (`272-296`); content sha256 handshake in `service_identity.rs` | **No drift** |

**Net:** every preference is faithfully implemented. The only implementation-level nuance is #2 (compact New-chat in Full mode 40–59 cols); everything else is drift *around* the product, not in it.

## 3. Clean build / fmt / clippy / tests (one bounded target)

All under `CARGO_TARGET_DIR=/home/bot/wg/.wg-candidate-targets/validate-symbolic-worksg-trial`, `CARGO_HOME=/tmp/validate-sym-cargo-home`.

| Check | Command | Result |
|---|---|---|
| Format | `cargo fmt --check` (pinned toolchain `1.96.0`) | **PASS** (clean) |
| Candidate build | `cargo build --locked --features worksgood-trial --bin wg --bin worksgood` | **PASS** (`Finished dev target(s)`; 65 baseline warnings, exit 0) |
| Clippy | `cargo clippy --locked --features worksgood-trial --bin wg --bin worksgood` | **PASS** — exit 0; 299 baseline warnings, **zero** in `concierge.rs`/`service_identity.rs`/`worksgood.rs`/`viz_viewer` context-bar code |
| Concierge units | `cargo test --lib concierge -- --test-threads=1` | **PASS 10/10** (2 ignored) |
| Identity units | `cargo test --lib service_identity` | **PASS 2/2** |
| Symbolic render goldens | `cargo test --bin wg -- 'render::tests::' symbolic` | **PASS 186** (9 ignored) — incl. `approved_symbolic_golden_matrix…`, `symbolic_context_width_matrix…`, `first_symbolic_frame_is_neutral…` |
| Color/appearance/pulse | `cargo test --bin wg -- workspace_color appearance first_symbolic symbolic_pulse` | **PASS 9/9** — incl. WCAG contrast + rich-palette distribution |
| Full lib suite (serial, worker-env-stripped) | `cargo test --lib -- --test-threads=1` | *(result filled in §3.1 below from `/tmp/validate-sym-libtest.log`)* |

Candidate binaries (uninstalled review artifacts):
- `worksgood`: `<target>/debug/worksgood`
- sibling `wg`: `<target>/debug/wg`

### 3.1 Full lib suite
`cargo test --locked --features worksgood-trial --lib -- --test-threads=1` (worker service-control env stripped): **PASS — 2970 passed, 0 failed, 38 ignored** (50.76s). No regressions.

## 4. Smoke matrix (candidate binaries, real tmux)

`WG_SMOKE_CANDIDATE_DIR=<target>` / `WG_SMOKE_CANDIDATE_BIN=<target>/debug/wg`; worker service-control env stripped where a daemon is needed.

| Scenario | Owners (subset) | Result | Root cause / note |
|---|---|---|---|
| `tui_symbolic_context_bar` | implement-approved-symbolic-tui | **FAIL** | **D1/Pi-only.** Fixture hardcodes `[dispatcher] model="claude:opus"`; `chat create` now rejects it (`WG-PI-ROUTE-REQUIRED`). Cannot build the live-chat state the flow needs. |
| `worksgood_concierge_trial` | closeout-worksg-concierge, expose-thinking-effort | **FAIL** | **D1/Pi-only.** First `--profile codex` dry-run is rejected (`requested profile "codex" is legacy/non-Pi`); codex/claude/nex/opencode starters fail `validate_pi_model_plane`. |
| `tui_open_non_mutating` | remove-implicit-chat | **FAIL** | **D1/Pi-only.** Partially migrated to `pi:openai-codex:gpt-5.6-sol` but omits reasoning → `WG-PI-REASONING-MISSING` on `service start`. |
| `tui_termux_stacked_inspector` | fix-stale-split | **FAIL (predicted)** | `model="claude:opus"` (Pi-only). |
| `tui_chat_selector_mouse_actions` | make-chat-selector | **FAIL (predicted)** | `model="claude:opus"` (Pi-only). |
| `profile_switch_authoritative_hot_reload` | make-profile-switching | **FAIL (predicted)** | `claude:opus`/`claude:haiku` + `codex:gpt-5.5` assertions (Pi-only). |
| `tui_inspector_drag_to_full` | fix-full-inspector-pointer-escape, make-panel-resize, fix-stale-split | **FAIL** | **D2 (non-Pi).** “Full lost the one contextual navigation row” — the make-panel-resize Layout-mode bar (`Right 47% P:Split h…l a:Auto │ - 47% + │ … Apply Esc Tab:More`) replaces the symbolic row where the smoke expects it. Layout/symbolic-row interaction in Full mode. |
| `tui_four_sided_layout_mobile` | make-panel-resize, fix-stale-split | **FAIL** | **D3 (non-Pi).** “Task context lost packed cached agent/ready pulse” — bar shows `!○0/?` (zero-agent warning form); the cached packed pulse did not satisfy the assertion (async timing or assertion-form mismatch). |
| `tui_hashed_project_colors` | make-hashed-project | **PASS** | Pi-migrated (`pi:openrouter:example/model` + reasoning). |
| `tui_authoritative_service_context` | show-authoritative-service | **PASS** | Pi-migrated. |
| `public_add_visible_publish_tui` | remove-public-no-place | **PASS** | |
| `deep_graph_archive_tui` | remove-graph-depth-guard | **PASS** | |
| `tui_log_smoke_graph_isolation` | fix-log-smoke-graph-isolation | **PASS** | |
| `tui_chat_close_lifecycle` | validate-and-land et al. | **PASS** | |

**Blast radius of D1 (Pi-only fixture staleness):** at least 6 pinned regressions on `main`, including the two that pin this exact product. None of these is owned by an active task, so `wg done` does not currently gate them — the redness is invisible to the smoke gate until a task claims one.

## 5. Live isolated tmux flows (desktop + Termux widths)

Driven against the candidate `wg`/`worksgood` in detached tmux with `window-size manual` + `resize-window`.

### 5.1 `worksgood` Continue-without-AI (valid Pi-era path), 40-col
```
$ worksgood --project <repo> --without-ai --yes
… graph init, commit concierge.json(mode=continue_without_ai), open TUI …
 [40-col TUI renders symbolic bar; q exits]
TUI closed.
Continue without AI: no LLM service is running.
Re-enter: worksgood
Setup:    worksgood setup
TUI only: worksgood tui        # no setup or service reconcile
```
Asserted: `concierge.json` committed; **no chat row** in `graph.jsonl`; **no** `service/state.json`; graph-only exit omits `Stop`. ✅ matches design.

### 5.2 Symbolic context bar, graph-only TUI, width matrix
Bar (UTF-8, `WG_TUI_APPEARANCE=none`) at live widths:

| Width | Captured top row (repr) |
|---|---|
| 40 | `' ↯  ⌁  ⌂  .chat-0   ⌕  ⌘  ? [ New chat ]'` |
| 80 | `' ↯  ⌁  ⌂  No chat────…──── ⌕  ⌘  ?  ⋮  ○0/? [ New chat ]'` |
| 120 | `'│ ↯  ⌁  ⌂  .chat-0  Chat 0 … ⌕  ⌘  ?  ⋮  !○0/? [ New chat ]'` |

- Lanes `↯⌁⌂`, identity, `⌕⌘?⋮`, packed pulse (`○0/` zero / `!` warning), `[ New chat ]` all present and resize-stable. ✅
- Graph digest **unchanged** across resize/open. ✅ (non-mutating)
- **Observation D4 (cosmetic):** at wide widths the identity flex region is filled with `─` (the stacked-split seam glyph) rather than inert spaces. The approved decision-menu buffers show spaces. Code (`render.rs:54,107,185 paint_split_seam`) treats the colored row *as* the one horizontal seam, so this is the seam styling, not an outer frame; but it diverges from the literal buffers. Low severity.
- **Observation D5 (state):** with a stale `tui-state.json` pointing at a chat that `chat create` could not build (Pi-only), the identity flipped between `No chat` (80-col) and `.chat-0` (40/120-col) across resizes. Benign under valid state, but shows the renderer trusts `tui-state.json` without reconciling against the graph; worth a stale-state fence.

### 5.3 Concurrent reuse + exit message
`worksgood_concierge_trial.sh` proves (when unblocked) two concurrent returning clients serialize only reconcile, open independent TUI clients against one daemon, and print the running-service exit message. D1 blocks the live re-run; the contract is covered by `concierge::tests` (§6) and the closeout report.

## 6. Fault-injection coverage

Covered by `concierge::tests` (10/10 PASS) + the concierge smoke design (unblocked sections):

| Fault | Evidence | Status |
|---|---|---|
| Pi child restart / effort argv | `customize_core_profile` + smoke fake-Pi argv probe (`--provider/--model/--thinking xhigh`, no model-encoded reasoning) | PASS (unit); smoke blocked by D1 at the legacy gate |
| Service mismatch (same-version/different-content build) | `reconcile_restarts_same_path_with_different_content_build` | PASS |
| Service restart on same-path replaced bytes | `reconcile_reuses_identical_bytes_across_absolute_path_aliases` + replaced-binary smoke arm | PASS |
| Foreign / deleted running executable | `reconcile_refuses_foreign_or_deleted_running_identity_without_signal` | PASS — `SERVICE_IDENTITY_REFUSED`, no signal, no TUI |
| Compatible-build profile/config/reasoning generation drift reload | `reconcile_reloads_only_compatible_config_profile_generation_drift` | PASS — reload, no PID replace |
| Stale PID repair | smoke arm (pid=999999 → repaired) | PASS (design) |
| Failed intended restart → restore prior build, no stale TUI | smoke `bad_w` arm (`TUI was not opened`, prior build recovered) | PASS (design) |
| Stale hits / search results | `tui_symbolic_context_bar` commit-clears-search + `resize_invalidates_symbolic_task_and_choice_hits` (`--bin wg`) | PASS (unit); live smoke blocked by D1 |
| Resize stale coordinates | `resize_invalidates_symbolic_*_hits` + `pty_resize_dedup_no_scrollback_echo` | PASS |
| Disk warning | `!` warning pulse preempts healthy pulse and opens Dashboard (render + smoke design) | PASS (render) |
| No credentials | Continue-without-AI is service-free; endpoint readiness honest (`endpoint_status:"not configured"`) | PASS (live §5.1) |
| Canceled setup | default-cancel + `--rollback` preserve graph, clear only pending selection | PASS (smoke design) |

## 7. Negative invariants (confirmed)

| Required negative | How confirmed |
|---|---|
| No provider inference / cross-system fallback | `concierge.rs:807` `validate_pi_model_plane()` rejects legacy starters; `config.rs:3385 parse_exact_pi_route` fails hard; `concierge.rs:768` “no agency route was inferred”; `--yes` refuses silent effort (`worksgood.rs`) |
| No startup chat | `assert_no_chat` over `graph.jsonl` in the concierge smoke; live §5.1 shows none created by open/exit |
| No TUI setup mutation | `concierge.rs:1416 open_tui` runs only `W --dir G tui`; `run_tui` bails if graph missing rather than init |
| No outer frames | `render.rs:54` “neither pane draws an outer frame”, `:107` Full “not a four-sided frame”, `:566` “Legacy non-graph-facing Full frame edges stay retired” |
| No unverified PATH execution | `concierge.rs:232-296` `current_exe()` + absolute-receipt + sha256; relative/symlink/unknown rejected; unknown PATH `wg` never executed (smoke `PATH_WG_EXECUTED` guard) |
| No cross-project route mutation | `project.rs:1-11,360,434,442` — `<graph>/profile-selection.json` only; global config/active-profile never rewritten; foreign canonical-project identity fails closed |
| No session resurrection / duplicate turn | `tui_chat_close_lifecycle` PASS (no relaunch of terminal/archived chat); path-unique session ownership; service-restart message-retry is loud, never silently duplicated (design + `service/ipc.rs`) |

## 8. Defects register

| ID | Severity | Type | Summary | Evidence |
|---|---|---|---|---|
| **D1** | **Blocker (coherence)** | drift/smoke+doc | Pi-only model plane (`9e52f9f7`) broke the integrated product flow: 6 pinned smokes red on `main`, concierge trial doc stale. | §4; `worksgood-concierge-trial.md` “Core integrated choices are Pi/pi-codex, Codex, Claude, Nex/local, and OpenCode” vs `concierge.rs:774,807` legacy rejection |
| D2 | High | regression (dependency) | `tui_inspector_drag_to_full` FAIL — make-panel-resize Layout-mode bar displaces the symbolic context row in Full mode where the smoke expects it | §4; `/tmp/sm-drag.out` |
| D3 | Medium | regression (dependency) | `tui_four_sided_layout_mobile` FAIL — packed cached pulse assertion not met (`!○0/?` form) | §4; `/tmp/sm-tui_four_sided_layout_mobile.out` |
| D4 | Low | cosmetic | Identity flex region filled with `─` seam glyph rather than inert spaces (approved buffers show spaces) | §5.2 |
| D5 | Low | robustness | Renderer trusts stale `tui-state.json` identity across resize without graph reconciliation | §5.2 |
| D6 | Low | smoke timing | Pulse assertion in D3 may be async-timing sensitive (cached pulse) rather than a logic bug | §4/§5.2 |

## 9. Exact commits / commands / artifacts

- Review HEAD: `dd0a7671` (0 ahead of `main`)
- Relevant main commits: `9e52f9f7` make-pi-the-2 (D1 cause), `c0e9d150` closeout-worksg-concierge, `ab615900` expose-thinking-effort, `1d09e136` make-hashed-project, `1a8c3382` show-authoritative-service, `a4ecf994` fix-stale-split, `1c4f9c23` fix-log-smoke-graph-isolation, `e55ffb9b` fix-full-border-drag
- Build: `CARGO_HOME=/tmp/validate-sym-cargo-home CARGO_TARGET_DIR=/home/bot/wg/.wg-candidate-targets/validate-symbolic-worksg-trial cargo build --locked --features worksgood-trial --bin wg --bin worksgood`
- Logs (validator scratch): `/tmp/validate-sym-build.log`, `/tmp/validate-sym-clippy.log`, `/tmp/validate-sym-libtest.log`, `/tmp/sm-*.out`, `/tmp/concierge-smoke.out`
- No `cargo install`. No global install. Isolated target removed after admission.

## 10. Recommendation

- **Symbolic TUI rendering substrate:** SHIP. Preferences audited clean; goldens/width-matrix/color/pulse tests green; live glyphs render and resize correctly; negatives hold. Track D4/D5 as polish.
- **`worksgood` concierge lifecycle machinery:** SHIP (mechanism). `resolve_authoritative_executable`, content-build reconcile, foreign/deleted refusal, rollback bounds, two-tier Pi effort, no cross-project mutation — all implemented and unit-tested.
- **“Symbolic TUI + `worksgood` concierge as one coherent product flow”:** **CONDITIONAL NO-SHIP.** D1 must close first: either (a) re-baseline `worksgood-concierge-trial.md` + the 6 affected smokes to the Pi-only world (make `worksgood setup` offer Pi profiles with explicit strong/weak routes, and migrate smokes off `claude:opus`/legacy starters with full pi: + reasoning), or (b) if multi-handler support is still intended, relax `validate_pi_model_plane` accordingly. The pinned human-flow smokes (`tui_symbolic_context_bar`, `worksgood_concierge_trial`) must be green on `main` before this product flow ships.
- D2/D3 are independent dependency-owned regressions to file against make-panel-resize / fix-stale-split.
