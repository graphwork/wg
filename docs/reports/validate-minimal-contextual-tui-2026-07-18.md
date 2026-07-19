# Minimal contextual TUI validation — 2026-07-18

**Task:** `validate-four-sided`  
**Validated revision:** `0460ac315ec5e5831d2e51e951bdb6eb3bda23ec` (`main`, merged `implement-four-sided`)  
**Result:** **PASS**

The accepted UI is one contextual row, one split seam, and no outer frame. This report independently validates the merged result on ratatui test buffers and through real isolated tmux/PTY flows. It also updates two pre-existing human-flow scenarios whose assertions still required chrome that this change intentionally removed.

## 1. Static render-path audit

The audit compared `main^` (`1d1e4152`) with `0460ac31` and traced every active call from `draw`:

- `draw` now assigns the full terminal to `main_area`; it no longer allocates the old top status, vitals, or bottom command rows (`src/tui/viz_viewer/render.rs:166-171`).
- Side splits subtract exactly one column and allocate it as the only seam (`render.rs:41-70`). Stacked splits subtract exactly one row and reuse it for context (`render.rs:71-98`).
- Full inspector returns `main_area` unchanged and the fullscreen-border renderer is a no-op (`render.rs:2063-2069`). All fullscreen edge hit rectangles are reset before rendering (`render.rs:475-479`).
- The sole active inspector header is `render_context_row` (`render.rs:2092-2179`). `draw_right_panel` allocates either one context row above content or embeds it into the stacked seam (`render.rs:2185-2258`).
- Normal Chat renders cannot enter the legacy full tab/identity-actions branch: `draw_right_panel` sets a non-empty context area before Chat content, while the compatibility branch is gated by `last_tab_bar_area.height == 0`. The compiler correspondingly reports `draw_tab_bar`, global status, vitals, and action-hint helpers as dead code. These retained definitions/state fields are migration debt, not width-dependent render paths.
- Chat context clears legacy Prev/Next/Choose/Close hit areas each frame and exposes only the fixed New-chat hit target. Task context does not create that target (`render.rs:2145-2179`).
- PTY mode sets WG input/search height to zero. The buffer test requires `last_chat_message_area == Rect(0,1,80,23)` and `last_chat_input_area.height == 0`; the child therefore owns all 23 rows below context.

`rg`/diff audit confirmed that calls to `draw_status_bar`, `draw_vitals_bar`, `draw_action_hints`, and `draw_tab_bar` were removed from the active frame. The old full-screen edge renderer was deleted and replaced by a no-op. This is an allocation/call-graph removal, not CSS-like hiding at one breakpoint.

## 2. Before/after row and seam accounting

The baseline used the pre-change renderer at `1d1e4152`. Its top-level layout always reserved three global rows: status + vitals + command hints. The inspector then added its own tab row; split mode also drew an `ALL` border; Chat additionally carried its nested chat strip/identity controls and a WG composer.

| Layout | Before (`1d1e4152`) | After (`0460ac31`) |
|---|---:|---:|
| Global full-screen WG chrome | 3 rows | 0 rows |
| Full Task inspector | 3 global + 1 inspector tab = 4 rows | 1 Task context row |
| Full Chat/PT​​Y | 3 global + 1 inspector tab + 2 nested Chat rows + 1 WG composer = 7 rows | 1 Chat context row; child owns the rest |
| Side split | 3 global rows + framed inspector/tab (and nested Chat rows when applicable) | 1 contextual row in inspector + exactly 1 seam column |
| Stacked split | 3 global rows + separate border/tab rows | exactly 1 combined seam/context row |
| Full inspector outer frame | edge/restore affordances existed | 0 rows, 0 columns, 0 hit targets |

Ratatui measurements:

- `80×24` full Chat: panel `0,0,80,24`; context `0,0,80,1`; child content `0,1,80,23`.
- `80×24` full Task: context `0,0,80,1`; task content `0,1,80,23`.
- `120×30` right split: `panel.x - graph.right == 1`; every seam cell is `│`.
- `120×30` bottom split: `panel.y - graph.bottom == 1`; `last_tab_bar_area.y == seam_y`; context text occupies that seam.
- Chat width matrix `40, 50, 60, 80, 120`: exactly one row, `Chat`, exact `.chat-7`, and full `[ New chat ]` at each width.
- Live tmux viewports: desktop `160×44`, medium `76×30`, Termux portrait/minimum supported validation width `40×22`. Each had one context row. Resize bursts also traversed `58×24 → 104×32 → target` before measurement.

At 40 columns, optional state text is clipped first while the fixed action remains complete:

```text
 Chat ▾  .chat-0  ● no selec[ New chat ]
```

At 76 columns, secondary state restores but route remains subordinate to the fixed action:

```text
 Chat ▾  .chat-0  ● no selection                                [ New chat ]
```

At wide width, all available context restores without moving New chat:

```text
 Chat ▾  .chat-0  ● no selection                                       [ New chat ]
```

## 3. Captured layouts

The captures below come from the real candidate binary inside isolated tmux, not string fixtures.

### Desktop, side Task split (`160×44`)

```text
layout-fixture-exact  (open│ Task ▾  layout-fixture-exact  ● open  ‹  ›  ⋯
                           │▾ ── layout-fixture-exact ──
                           │Title: layout-fixture-exact-id
                           │Status: Open
```

There is one `│` seam column, no inspector box, and no Chat control in Task context.

### Desktop, stacked Task split

```text
layout-fixture-exact  (open) 0s

 Task ▾  layout-fixture-exact  ● open  ‹  ›  ⋯─────────────────────────────────────
▾ ── layout-fixture-exact ──
Title: layout-fixture-exact-id
Status: Open
```

The horizontal seam and Task context are the same physical row; there is no doubled separator.

### Desktop, full Task

```text
 Task ▾  layout-fixture-exact  ● open  ‹  ›  ⋯
▾ ── layout-fixture-exact ──
Title: layout-fixture-exact-id
Status: Open
```

No top/left/right/bottom frame remains.

### Termux portrait, full Chat (`40×22`)

```text
 Chat ▾  .chat-0  ● no selec[ New chat ]
            No chat selected.

 Create one below, or press n in command
                  mode.
```

No second Chat row, global row, footer, or outer frame appears.

## 4. Context and command behavior

The focused render suite proves:

- Task row: exact task identity, status, `‹ › ⋯`, no New chat, no border glyphs.
- Chat row: exact chat identity, connection state, optional route, fixed `[ New chat ]`, no Task controls.
- Layout command mode contains `h/j/k/l dock` and replaces `Chat ▾` in the same one-row buffer.
- PTY mode owns every row below context and WG has no duplicate composer.

The real terminal-chat lifecycle flow selects terminal `.chat-36` through Choose Chat. It renders `Task ▾`, `.chat-36`, and `abandoned`, does not spawn/reattach a PTY, and does not show `[ New chat ]`. Returning with `0` restores the live Chat context. `Ctrl+O → w` still opens the identity-pinned Close modal; Cancel, Hide, confirmed Stop, and confirmed Archive all pass. Thus removing always-visible Prev/Next/Choose/Close labels did not remove their command-mode behavior.

The mosh PTY flow sets the mosh/Termux policy, sends the literal Ctrl+O byte through a PTY, enters the native composer, and verifies plain Enter and Ctrl+J behavior. It passed with 12 plain CR submissions, 12 mosh-style parsed Shift+Enter submissions, and one preserved Ctrl+J multiline submission, all exactly once.

## 5. PTY isolation, resize, and persistence

All of these existing regressions passed against the merged candidate:

| Scenario | Result / evidence |
|---|---|
| `tui_chat_mosh_plain_enter_pty` | PASS — Enter cohorts exactly once; Ctrl+J multiline preserved |
| `tui_immediate_chat_startup` | PASS — existing pane accepted input in **368 ms** during a real filesystem stall; no duplicate handler; Ctrl+O→New flow passed |
| `tui_open_non_mutating` | PASS — 20 empty opens were graph/session/provider-free; explicit New paths each created one pinned chat |
| `tui_stateful_chat_restart_resume` | PASS — restart reattached `.chat-0`, no new row, history continuous |
| `chat_tmux_path_unique_terminal_resume` | PASS — equal-basename graphs remained session/path isolated; terminal stale proof rejected without mutation |
| `tui_wg_add_output_confinement` | PASS — nested `wg add` stdout/stderr stayed in child PTY; resize/poisoned-state repaint coherent |
| `pty_resize_dedup_no_scrollback_echo` | PASS — burst reflow kept scrollback duplicate-free |
| `pty_initial_spawn_no_scrollback_doubling` | PASS — initial PTY used real dimensions; no first-frame SIGWINCH echo |
| `tui_chat_close_lifecycle` | PASS — Task/Detail route plus command-mode Close lifecycle settled on one borderless contextual frame |
| `tui_four_sided_layout_mobile` | PASS — desktop/medium/Termux, exact Task context, side/stacked/full, resize bursts, rollback/commit, narrow→wide restore |

The output-confinement and path-unique flows jointly cover output leakage and cross-session effects. The resize tests cover reflow, storms/bursts, narrow→wide restoration, and scrollback preservation. Stateful restart covers continued session ownership/history rather than a fresh child.

## 6. Regression maintenance found during independent validation

Two existing scenarios initially failed for the correct product change, not because their underlying flows broke:

1. `tui_chat_close_lifecycle` waited for the deliberately removed always-visible `Close…` label and a border pair.
2. `tui_immediate_chat_startup` waited for removed `0 tasks`/`Connecting active chat`, `Active: … route …`, and `[PTY]` status rows.

They were updated to assert the one contextual row and absence of legacy chrome while retaining the same actual PTY, Detail-routing, Ctrl+O, lifecycle, PID, latency, and mutation checks. Both now self-clear parent worker/graph variables and pass through the real agent smoke gate. `tui_four_sided_layout_mobile` was strengthened to inspect an exact task/status, capture a true side split, and exercise resize bursts; it remains owned by the implementation task because it performs a candidate Cargo build, while this validator's gate owns the two installed-main lifecycle regressions.

## 7. Exact commands and results

```bash
# Source/diff audit
git diff 1d1e4152..0460ac31 -- src/tui/viz_viewer/render.rs
git show 1d1e4152:src/tui/viz_viewer/render.rs
rg -n 'draw_tab_bar|draw_status_bar|draw_vitals_bar|draw_action_hints|...' \
  src/tui/viz_viewer/{render,event,state}.rs
# PASS: active calls/allocations removed; compatibility helpers unreachable

CARGO_BUILD_JOBS=1 cargo test --bin wg tui::viz_viewer::render::tests \
  -- --test-threads=1
# PASS: 166 passed, 0 failed

bash tests/smoke/scenarios/tui_four_sided_layout_mobile.sh
# PASS

env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
  bash tests/smoke/scenarios/tui_chat_close_lifecycle.sh
# PASS

env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
  bash tests/smoke/scenarios/tui_immediate_chat_startup.sh
# PASS: 368 ms

PATH="$PWD/target/debug:$PATH" bash tests/smoke/scenarios/<scenario>.sh
# PASS for all PTY regressions listed in section 5

cargo fmt --check
cargo build
# PASS; repository-existing warnings only

env -i HOME="$HOME" USER="$USER" PATH="$PATH" \
  CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" \
  RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" TERM="$TERM" \
  cargo test -- --test-threads=1
# PASS: bin 3839 passed / 0 failed / 1 ignored;
# all 159 integration targets and doc tests passed

cargo clippy
# PASS (exit 0); 183 repository-existing warnings, no new denial
```

A partially cleaned environment that left other `WG_*` worktree variables set caused service-policy failures and a worktree-cleanup race during early attempts. The authoritative full-suite command above uses `env -i` and passed completely.

## 8. Installation provenance

Installation occurred **only after validation**, and only from the merged `main` checkout:

```bash
cd /home/bot/wg
[[ $(git rev-parse HEAD) == 0460ac315ec5e5831d2e51e951bdb6eb3bda23ec ]]
[[ $(git rev-parse HEAD) == $(git rev-parse main) ]]
cargo install --path . --locked
wg dev-check
```

Result: release build succeeded; `/home/bot/.cargo/bin/{wg,nex}` were replaced; `wg dev-check` on `main` reported `status: OK` with current/main/binary all at `0460ac315ec5`.

## Conclusion

The merged TUI satisfies the approved minimal contract at desktop, medium, Termux portrait, and the 40-column supported validation floor: **one contextual row, one split seam, zero outer frames**. `[ New chat ]` remains fixed and fully labelled while secondary text collapses first. Chat and Task contexts are mutually specific. Command/layout modes replace the row. The child PTY owns all remaining rows, and the tested mosh/tmux input, lifecycle, resize, scrollback, output, restart, and session-isolation behaviors remain intact.
