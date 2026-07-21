# Symbolic TUI context bar

**Status:** decision-ready design; no implementation is included

**Decision:** adopt **Grammar A, persistent lane rail**

**Supported geometry:** one contextual row, one split seam, zero outer frames; 40 columns remains the fully supported floor

## 1. Decision in one screen

Use one light, workspace-colored row in the dark TUI shell:

```text
 C  T  W  <exact identity>  <context actions>  <cached pulse>  /  =  ?  [ New chat ]
```

- ` C `, ` T `, and ` W ` are three independent, three-cell controls for **Chat**, **Task**, and **Workspace**. They never change identity or disappear at the supported width. Styling, not a changing label, marks the active lane.
- The center is contextual. It shows the exact `.chat-N` or task ID when one exists, followed only by actions that apply to that identity.
- `[ New chat ]` remains a 12-cell labelled exception to the symbolic-control rule. It is allocated first at every supported width and in every surface.
- ` / `, ` = `, and ` ? ` are Search, Controls, and Help. ` = ` opens a small controls palette containing the distinct Config and Settings surfaces; it does not consume permanent identity space.
- ` @ ` is a healthy/unknown service-pulse target and ` ! ` is its text-distinct warning form. Both open Workspace/Dashboard and are built only from cached state.
- All one-character controls include their left and right space in a real three-cell hit region. There are no one-cell touch targets and no decorative action glyphs.
- At 20 columns, the requirements are mathematically incompatible: the three minimum padded lane controls use 9 cells and `[ New chat ]` uses 12. The emergency, below-floor form keeps ` C  T  W  +  ? ` and the `n` binding. At 32 columns the full `[ New chat ]` returns; at 40 columns the full supported contract applies.

This fixes the identity-changing behavior of the current row: Chat remains visible while a task is inspected, and Task and Workspace remain visible while a chat PTY owns the content below it.

## 2. Prior contracts and evidence

This design extends rather than reopens the validated minimal layout:

- The [minimal contextual TUI validation report](reports/validate-minimal-contextual-tui-2026-07-18.md) establishes **one contextual row, one split seam, zero outer frames**, a fully labelled New-chat action at the 40-column supported floor, child ownership of all Chat rows below the context row, and non-mutating startup.
- WG task `fix-one-row` made New chat global, made the pulse cached, and removed dead context glyphs. Its human-flow contract is represented by [`tui_four_sided_layout_mobile.sh`](../tests/smoke/scenarios/tui_four_sided_layout_mobile.sh).
- WG task `make-chat-selector` made selector rows and footer actions real, bounded, clickable hit targets. Its mouse contract is represented by [`tui_chat_selector_mouse_actions.sh`](../tests/smoke/scenarios/tui_chat_selector_mouse_actions.sh).
- The terminal-chat Detail and lifecycle behavior remains pinned by [`tui_chat_close_lifecycle.sh`](../tests/smoke/scenarios/tui_chat_close_lifecycle.sh); empty startup remains pinned by [`tui_open_non_mutating.sh`](../tests/smoke/scenarios/tui_open_non_mutating.sh).
- Printable keys must not be stolen from the embedded terminal; the routing policy remains the one documented in [`tui-keymap-routing.md`](bugs/tui-keymap-routing.md).

The row is not a new global status bar. It is the sole inspector context row already budgeted by the accepted layout.

## 3. Alternatives

All examples reserve the rightmost New-chat action. Spaces inside a control are part of its hitbox. Plain text cannot show background styling; the active item is identified in prose.

### A. Persistent lane rail — **recommended**

```text
 C  T  W  .chat-17  <  >  x  :  A2/4 R1 Q3 @  /  =  ?       [ New chat ]
 C  T  W  build-release-notes  open  <  >  :  A2 R1 Q3 @    [ New chat ]
```

The rail is fixed; the identity and actions change. Each lane remembers its last valid location. This is the only grammar that makes all three return paths visible, stable, and independently touchable without adding a row.

### B. Fully labelled segmented lanes

```text
[ Chat ][ Task ][ Workspace ]  .chat-17  < > x :             [ New chat ]
```

This is the most self-explanatory on first use, but the lane group alone costs 29 cells. At phone widths it must rename itself to symbols or move into overflow, recreating the same identity shift the design is meant to remove. The brackets also read as frames and produce irregular hit widths.

### C. One cycling context control plus identity

```text
 C>  .chat-17  <  >  x  :  @  /  =  ?                       [ New chat ]
```

` C> ` cycles Chat -> Task -> Workspace or opens a picker. It has excellent width efficiency but hides two destinations and makes a single tap state-dependent. A user inspecting a task cannot see that Chat is one step or two steps away. This is the current replacement-selector problem in a smaller costume.

### D. Identity-first command deck

```text
.chat-17  C  <  >  x  :  @  /  =  ?                          [ New chat ]
```

Only the current lane symbol is shown; other lanes live in `:`. This maximizes identity width, but Workspace and Task are not visibly reachable while chatting. It also makes the first control appear to label the identity rather than switch surfaces.

### 3.1 Scored decision matrix

Scores are 1 (poor) to 5 (best). Weighted totals are out of 500.

| Criterion | Weight | A: rail | B: labels | C: cycle | D: deck |
|---|---:|---:|---:|---:|---:|
| All Chat/Task/Workspace return paths visible | 25 | 5 | 5 | 2 | 1 |
| 40-column and Termux fit | 20 | 5 | 1 | 5 | 4 |
| Exact-identity budget | 15 | 4 | 1 | 5 | 5 |
| Stable touch targets | 15 | 5 | 4 | 2 | 3 |
| Learnability without a manual | 10 | 4 | 5 | 2 | 2 |
| Stable grammar across contexts and widths | 10 | 5 | 2 | 3 | 2 |
| PTY-safe, deterministic routing | 5 | 5 | 5 | 3 | 3 |
| **Weighted total / 500** | **100** | **475** | **315** | **320** | **280** |

A is recommended. B is rejected for width discontinuity; C for hidden, stateful navigation; D for losing the required persistent switch.

## 4. Recommended grammar

### 4.1 Cell grammar and allocation

```text
LANES  IDENTITY  CONTEXT-ACTIONS  FLEX  PULSE  GLOBALS  NEW
 C T W  exact-id    < > x :              @       / = ?   [ New chat ]
```

The rendering allocator works in this order:

1. Reserve `[ New chat ]` (12 cells).
2. Reserve the three lane controls (9 cells) at every width where the row is rendered.
3. Reserve a full exact `.chat-N` when Chat or terminal-chat identity exists; otherwise give the identity viewport the remaining mandatory budget.
4. Reserve Help at >= 32, then Controls and Search as width permits.
5. Add warning pulse, context menu, close, previous/next, compact healthy pulse, state text, long pulse, and route in that order of increasing expendability.
6. Put unused cells into the identity/flex region. New chat remains right-aligned and never slides as optional text appears.

The renderer must calculate display-cell width, never byte or scalar count. All symbols in the chosen grammar are ASCII and one cell under normal terminal width rules.

### 4.2 Lane memory and activation

- **Chat (` C `):** opens the remembered live chat. If none is live, it opens the non-mutating Chat empty state. A second activation opens the chat selector. Selecting terminal/archived/abandoned `.chat-N` uses `open_chat_task_or_detail` and moves to Task/Detail; it never relaunches that chat.
- **Task (` T `):** opens the exact selected task's Detail. If no task is selected, it focuses the graph/task selection without inventing a task.
- **Workspace (` W `):** opens the Dashboard for the current repository. Empty-canvas selection still selects Workspace, and dragging still only pans.

The three controls remain present while Config, Settings, search, a menu, or a terminal Chat command mode is active. The active lane is styling state, not a replacement label. Config and Settings temporarily have no active lane; the remembered lane remains available in one tap.

### 4.3 Exact identity

- Chat always renders the canonical `.chat-N`, not a display alias. An optional friendly label may follow only when width remains.
- Task always stores, routes, and exposes the canonical task ID. Status is secondary and may disappear first.
- Terminal chat remains a task identity: ` T ` is active and `.chat-N` is shown exactly with terminal status when it fits.
- Workspace shows a privacy-safe repository label (normally the basename) plus its color. The full canonical path is never put in the bar.
- Config uses `Config:<section>` and Settings uses `Settings:<key>` when space allows. Actions bind to the full underlying section/key snapshot, not clipped display text.

Arbitrarily long task IDs cannot be shown simultaneously with all fixed controls at every width. The identity field is therefore a **viewport over the unmodified ID**, not a renamed ID. At >= 60 it takes all flex cells. When clipped, a leading or trailing ASCII `~` marks hidden cells; focusing the field permits horizontal movement and copy of the full exact value. No two clipped IDs may be treated as equal, and every action captures the full ID when its hit map is built. Canonical `.chat-N` fits in full at 32 columns for all practical `N` lengths up to the available viewport.

## 5. Literal mockups

The examples below are cell-geometry mockups. Background, inverse, bold, underline, and focus are described in section 8; they cannot be represented faithfully in a Markdown code fence. `C`, `T`, and `W` are always three-cell padded controls even when the active style is not visible here.

### 5.1 Wide desktop, 120 columns

**Chat — ` C ` active; exact live identity; Task and Workspace still visible**

```text
 C  T  W  .chat-17  connected  <  >  x  :  A2/4 R1 Q3 @ok  /  =  ?                                          [ New chat ]
```

**Task — ` T ` active; Chat remains one tap away**

```text
 C  T  W  design-symbolic-context-bar  in-progress  <  >  :  A2/4 R1 Q3 @ok  /  =  ?                        [ New chat ]
```

**Workspace — ` W ` active; the pulse is the identity's primary content**

```text
 C  T  W  wg  A2/4 R1 Q3 E1 !daemon-stale D86G  :  /  =  ?                                                  [ New chat ]
```

In a side split, the existing single `|`/vertical seam column remains between graph and inspector; it is not part of the controls. In a stacked split, this colored row *is* the single horizontal seam. No second line of dashes is drawn above or below it.

### 5.2 Laptop, 80 columns

**Config — no lane is stolen; ` = ` is active/focused**

```text
 C  T  W  Config:endpoints  :  @  /  =  ?                           [ New chat ]
```

**Settings — distinct surface reached through the same Controls palette or direct key**

```text
 C  T  W  Settings:tui.workspace_color  @  /  =  ?                  [ New chat ]
```

**Search active — lanes and New chat remain fixed; `/` field owns text input**

```text
 C  T  W  / symbolic context  2/7  [Esc]  =  ?                      [ New chat ]
```

`[Esc]` is a real five-cell cancel target. Search results occupy content below the row; search never adds a second header.

### 5.3 Termux portrait, 60 and 40 columns

**Terminal chat at 60 — routed to Task/Detail, never to a PTY**

```text
 C  T  W  .chat-36  abandoned  :  !  ?          [ New chat ]
```

` T ` is active. ` x ` is omitted because there is no live Chat session to close. ` C ` remains visible and returns to the remembered live Chat or its empty state.

**No chat at 40 — opening the TUI created nothing**

```text
 C  T  W  No chat  ?        [ New chat ]
```

**All chats archived at 40 — creation and the selector remain reachable**

```text
 C  T  W  Archived:4  ?     [ New chat ]
```

Activating ` C ` again opens the selector containing those archived identities; selecting one routes to Detail. Activating New chat opens the launcher and does not create until confirmation.

**Workspace at 40**

```text
 C  T  W  wg  !  ?          [ New chat ]
```

The `!` shape, not color alone, reports a warning and opens Dashboard.

### 5.4 Width edge cases

**32 columns — below the supported floor but full labelled action retained**

```text
 C  T  W  .chat-7 ? [ New chat ]
```

**20 columns — emergency mode, explicitly degraded**

```text
 C  T  W  +  ?
```

The 20-column form retains all lane destinations, a padded New-chat target, Help, and the `n` binding. It cannot retain the 12-cell label: `9 + 12 = 21` before identity or separation. This is an honest below-floor fallback, not a silent redefinition of “fully labelled.” Help identifies `+` as New chat, and the content empty state also spells out the action when content has room.

## 6. Symbol, hitbox, keyboard, and availability table

Coordinates are half-open ratatui rectangles: `Rect(x, row_y, width, 1)` contains columns `x..x+width`. “Padded 3” means the exact rendered cells `space + glyph + space`; all three cells activate. Bindings marked **existing** must remain; proposed F-key/bar-focus aliases are additive.

| Rendered control | Meaning | Exact hitbox | Keyboard equivalent | Disabled or omission rule |
|---|---|---|---|---|
| ` C ` | Chat lane / second activation opens selector | padded 3 | proposed `Alt-C`; `0` keeps its existing Chat/10th-chat behavior; F8 bar focus + `c` | Never omitted at >= 32. With no chat, opens empty state, not launcher. |
| ` T ` | Selected/remembered Task Detail | padded 3 | proposed `Alt-T`; `1`/Enter keep existing behavior; bar focus + `t` | Never omitted at >= 32. If no task, focuses graph and announces “No task selected.” |
| ` W ` | Workspace/Dashboard | padded 3 | proposed `Alt-W`; `6` or command-mode `d` keep existing paths; bar focus + `w` | Never omitted at >= 32. Cached/empty dashboard is still valid. |
| identity text | Exact context selector/detail target | `Rect(start, y, rendered_width, 1)` including clip markers | Enter while identity focused; existing picker/detail keys | Placeholder text is not emitted as an actionable ID. Hidden only at 20-column emergency width. |
| ` < ` | Previous identity in current lane order | padded 3 | Chat `Left`/`[`; Task `Up`; bar focus Left + Enter | Omit at compact widths or first item. If shown disabled at wide width, it remains focusable/clickable and announces “first item.” |
| ` > ` | Next identity in current lane order | padded 3 | Chat `Right`/`]`; Task `Down`; bar focus Right + Enter | Symmetric with Previous. |
| ` x ` | Close exact live Chat; opens identity-pinned lifecycle choice | padded 3 | command-mode `w` / `Ctrl-W` (**existing**) | Only for a live/resumable Chat identity. Omit for Task, Workspace, no-chat, terminal/archived/abandoned Chat, Config, and Settings. |
| ` : ` | Context menu for the exact captured identity | padded 3 | `F10`; existing context commands remain | Omit when the context has no actions. A one-item menu executes only after explicit selection; the glyph itself never mutates. |
| ` @ ` | Healthy/unknown cached service pulse; opens Dashboard | padded 3, or the whole rendered long pulse at wide width | `6` / command-mode `d` | Omit healthy pulse before identity/global controls at narrow width. Unknown remains `@`, never falsely “ok.” |
| ` ! ` | Warning service pulse; opens warning detail in Dashboard | padded 3, or whole warning phrase | same as `@` | Never omit a current actionable warning at >= 40; it preempts healthy pulse, route, status, prev/next, and menu. Shape distinguishes it without color. |
| ` / ` | Search current lane | padded 3 | `/` (**existing**) | Omit the pointer at narrow widths; keyboard remains. Scope label appears in the focused field. |
| search field | Focus/edit current search query | exact rendered field rectangle | `/`, typing, Enter; Esc clears (**existing**) | Exists only while search is active. Does not forward text to PTY because Ctrl+O/host command mode is required first. |
| `[Esc]` | Cancel active search/menu | exact 5 cells | Esc | Render only when it fits and is actionable; otherwise Esc remains documented in content/help. |
| ` = ` | Controls palette: Config, Settings, Appearance | padded 3 | proposed `F2`; direct `3` Config and `8` Settings remain | Omit pointer before lanes, identity, warning, Help, or New chat; direct keys remain. Does **not** reuse global `=` layout binding. |
| ` ? ` | Help and binding legend | padded 3 | `?` (**existing**) | Retained at >= 32 and in 20-column emergency mode. |
| `[ New chat ]` | Open route/profile launcher | exact 12 cells | command-mode `n` (**existing**); `+` where already supported | Always reserved first at >= 32 and all supported widths. Creates nothing until launcher confirmation. |
| ` + ` | Emergency compact New chat | padded 3 | `n` | Only below 32 columns. It never appears beside the full label. |

Text such as `connected`, `open`, `A2/4`, or `D86G` is not styled as a control unless its containing pulse rectangle is registered. No triangle, arrow, punctuation mark, label, or warning may be rendered from an action template unless that same frame registers its hit rectangle and input handler.

## 7. Hit, touch, focus, and input rules

### 7.1 Pointer geometry

1. Build text spans and hit rectangles from one immutable `ContextBarModel` snapshot in the same render pass.
2. Clear every prior rectangle before layout. Resize, lane change, search change, modal open, and PTY attach cannot leave stale coordinates.
3. Rectangles are non-overlapping, half-open, and clipped to the actual row. Padding belongs to the neighboring control only when explicitly assigned; unassigned flex cells are inert.
4. A primary-button down event in any of the three cells activates exactly once. Termux does not require hover, double-click, or a precise glyph-cell tap.
5. Route bar hits before child-PTY, graph-pan, task-select, or content handlers. A bar tap cannot fall through.
6. Mouse wheel over identity navigates that lane; over a modal selector it scrolls its bounded list as defined by `make-chat-selector`; elsewhere current graph/content scrolling remains unchanged.
7. Hover is enhancement only. If `MouseMoved` is absent (common under Termux/tmux configurations), no behavior is lost.

A one-row TUI cannot provide a 44-pixel vertical touch target. The mitigation is the full row height plus three terminal columns horizontally, Termux font scaling, single-tap activation, and no adjacent one-cell targets. Increasing hit height would steal the child PTY's first row and is rejected.

### 7.2 Keyboard and focus

- Existing direct bindings remain authoritative. The table does not repurpose printable keys in text input or the child PTY. Proposed `Alt-C`, `Alt-T`, and `Alt-W` are the laptop direct lane bindings; terminals that cannot distinguish Alt reliably use F8 focus, the existing numeric/context path, or touch.
- Proposed `F8` enters **bar focus** on laptops. Left/Right moves only among currently rendered, enabled hit targets; Home/End jumps first/last; Enter/Space activates; Esc returns to the prior graph/right-panel focus. Tab and Shift-Tab cycle controls only after bar focus is explicitly entered, so the existing panel-focus Tab remains unchanged elsewhere.
- Terminals without F-keys use existing direct bindings or pointer input. Help lists both.
- While an interactive child owns stdin, the only host keyboard escape remains `Ctrl+O`. Until that escape, `c`, `t`, `w`, `/`, `+`, `?`, arrows, and all other printable/navigation input continue to the child. Mouse bar targets remain host-owned as today.
- Focus never changes the selected identity merely by traversing controls. Activation captures the model snapshot's full identity and revalidates it before any lifecycle mutation.

### 7.3 No dead glyph rule

Omission is the default for unavailable actions. A temporarily disabled control may remain only when its stable position prevents dangerous target movement during an in-flight action. It must retain a hit/focus target and announce the reason (“loading,” “first item,” or “service data unavailable”); it may not silently do nothing.

## 8. Visual states in a dark shell

The shell/content background remains dark. The context bar is a light/inverse band using the derived workspace background and a contrast-selected dark foreground.

| State | Background/foreground | Modifier | Geometry |
|---|---|---|---|
| Base bar | workspace light background; contrast-safe black/dark text | normal | unchanged |
| Inactive control | transparent over base bar | normal | padded 3 |
| Active lane/surface | dark shell background with light workspace foreground | bold | unchanged |
| Hover | base/active background darkened 8% | underline when supported | unchanged |
| Keyboard focus | inverse relative to its current state | bold + underline; terminal cursor hidden from content | unchanged |
| Press/touch echo | focus style for 120-180 ms | bold | unchanged |
| Disabled pending | base background; foreground mixed toward background but at least 3:1 | no bold | unchanged and explanatory on activation |
| Warning pulse | bar-safe dark foreground plus literal `!` | bold (red may supplement) | unchanged |

Active, focus, warning, and disabled meaning never depends on hue alone. No focus brackets are inserted and no label changes width, so touch coordinates stay stable during hover/focus.

In a stacked layout the colored context row is the one horizontal seam. In a side layout the one existing vertical seam remains one column and the row starts inside the inspector. Full-inspector mode has the row and no top/side/bottom frame.

## 9. Config and Settings remain distinct

They should **not** be merged:

- **Config** is the operational dashboard: service state, providers, endpoints, models, route tests, and add flows. It may launch tests or service/config commands and has larger side effects.
- **Settings** is the effective merged key/value view with source provenance (built-in/global/local), edit scope, lint, setup, and TUI appearance preferences.

Merging them would obscure provenance and put frequent preferences beside operational mutations. Giving each a permanent top-level icon would consume six cells and compete with exact identity.

Use the ASCII ` = ` Controls symbol instead of an emoji gear. `=` is terminal-stable, suggests adjustable values/sliders, occupies one known cell, and has no VS15 requirement. Activating it opens a bounded palette below the bar:

```text
C  Config     providers, endpoints, models, service
S  Settings   sourced values, TUI behavior, appearance
A  Appearance workspace bar color
```

Every palette row has a full-row hit target and an explicit key. Existing direct `3` and `8` access remains. The palette is transient content overlay, not a second persistent row or an outer frame. `=` does not steal the existing global `=` layout key; its proposed direct keyboard alias is F2, while pointer and bar-focus activation use the visible control.

## 10. Deterministic workspace color

### 10.1 Identity and hash

The color input is exactly the UTF-8 string:

```text
user@hostname:canonical-repo-path
```

Compute:

```text
bytes  = UTF8(user + "@" + hostname + ":" + canonical_repo_path)
digest = BLAKE3-DERIVE-KEY("worksgood.tui.workspace-color.v1", bytes)
u      = little-endian u64(digest[0..8])
hue    = 360 * u / 2^64
```

Using BLAKE3's derive-key mode, rather than a plain prefix concatenation, is the domain separator. The context string and byte order are part of the v1 contract. Test vectors must pin the payload, 32 digest bytes, hue, selected truecolor RGB, 256 index, and 16-color index. A future algorithm uses a new context suffix and never silently changes v1 output.

### 10.2 Canonical repository semantics

Resolve once in the asynchronous bootstrap worker:

1. For Git, resolve the absolute Git **common directory** and canonicalize it. If it is a `.git` directory, use its canonical parent worktree/repository root; for a bare repository, use the common directory itself.
2. Linked Git worktrees share the common directory, so main checkout and WG-managed `.wg-worktrees/agent-*` use the same color. This prevents a worker shell from looking like an unrelated repository.
3. Separate clones have different canonical paths and therefore different colors. Different users or hosts also differ intentionally, helping expose the wrong machine/account.
4. Outside Git, use the canonical project root selected by WG's directory resolver. If canonicalization fails, use the normalized absolute lexical root and mark appearance diagnostics as `uncanonical`; do not block the first frame.
5. Symlink spellings converge through canonicalization. Case follows the host filesystem's canonical result; no cross-platform case folding is invented.

The raw user, host, and path are never rendered, included in telemetry, or written to graph/chat state. A cache, if added, is keyed by the digest and stores only capability plus resolved colors. Hostname changes and moving/cloning a repository intentionally change auto color; the explicit override is the stability mechanism for users who want a cross-host brand color.

### 10.3 Truecolor generation and contrast

1. Start with `OKLCH(L=0.82, C=0.10, h=hue)`.
2. Convert to sRGB. If out of gamut, reduce chroma by `0.005` until all channels are in range; do not independently clamp channels.
3. Compute WCAG 2.1 relative luminance from linearized sRGB. Use black foreground when contrast is >= 7:1. If it is lower, increase `L` by `0.01` (maximum `0.94`) and repeat. The automatic palette must pass 7:1 for normal text.
4. The active tile uses the dark shell background and the same light color as foreground; if that pair is below 4.5:1, use white foreground. Focus/active meaning remains shape/modifier backed.

This produces a light inverse bar with perceptually similar lightness across hues, rather than an HSL rainbow whose blue entries can become unreadably dark.

### 10.4 Capability fallbacks

Capability is detected once at startup and stored with the appearance snapshot. Rendering emits only the selected cached style.

| Capability | Behavior |
|---|---|
| Truecolor | Use generated RGB when `COLORTERM=truecolor/24bit` or terminfo/tmux capability confirms RGB. |
| 256 color | Enumerate canonical xterm indices 16-255, reject candidates with black-text contrast < 7:1, then choose nearest OKLab color to the truecolor target; tie-break on lower index. |
| 16 color | Use `digest[8] mod 4` to select, in order, ANSI bright green (background 102), bright yellow (103), bright cyan (106), or bright white (107), with black foreground and bold. These are the canonical ANSI candidates whose xterm RGB values reach 7:1 against black. Because users may redefine them, pixel contrast cannot be proven; inverse/active styling and text identity remain authoritative. |
| Monochrome / `NO_COLOR` / `TERM=dumb` | Use reverse video for the row, bold active control, underline focus, and literal `!` warnings. No workspace distinction relies on hue. |

Terminal-specific policy:

- **tmux:** honor truecolor only when the outer capability/terminfo path confirms it (`RGB`/`Tc` or equivalent). Otherwise downgrade to 256. Do not emit OSC palette queries.
- **mosh:** default conservatively to 256 unless a tested explicit capability override says truecolor. Use only ordinary SGR state, so packet loss/redraw cannot strand palette mutations.
- **Termux:** use truecolor when advertised; otherwise 256. All control glyphs stay ASCII, so Android fonts and fallback emoji fonts cannot change hit geometry.
- **SSH:** hostname is the remote host obtained by the bootstrap worker, so the color describes where WG is running, not the client terminal.

### 10.5 Override and picker

Precedence is:

1. session `WG_TUI_WORKSPACE_COLOR`;
2. project-local `[tui].workspace_color`;
3. global `[tui].workspace_color`;
4. `auto` hash.

Accepted values are `auto`, `none`, `#RRGGBB`, and `ansi:N`. Config/Settings validation rejects malformed or unsupported values. For an RGB override, choose black or white by the higher contrast; the chosen pair must be at least 4.5:1. The Appearance picker previews Auto and a capability-aware palette, reports the contrast ratio in text, offers “Reset to auto,” and lets the user choose local or global scope. A 16-color override is explicitly labelled “terminal-palette dependent.”

### 10.6 Non-blocking ownership

At process start, the UI immediately uses a neutral cached inverse style. A bootstrap job may read user/hostname/repository/config metadata and compute `WorkspaceAppearance`. It publishes one immutable snapshot through the existing async state channel. A config reload or project switch schedules another job; it does not compute in the event handler.

**Render and input must perform zero hostname calls, environment scans, path resolution, filesystem metadata reads, Git subprocesses, palette queries, or hashing.** They only read the cached appearance/model. The worker's reads are non-mutating: it creates no chat, graph row, route, tmux session, or state file.

## 11. Emoji and width assessment

Do not use emoji for bar controls.

- Emoji such as `⚙️` can be one or two terminal cells depending on font, locale, `wcwidth`, and fallback rendering. Variation Selector-16 requests emoji presentation and commonly widens it.
- Appending Variation Selector-15 to request text presentation (`⚙︎`) is not reliable: some fonts ignore it, some terminal width tables count the base as East Asian Ambiguous, and ratatui/crossterm may disagree with the visible fallback font.
- Unicode `‹`, `›`, `⋯`, `●`, and `▾` are less volatile than emoji but still have ambiguous-width/font gaps and are unnecessary here.
- The selected control alphabet `C T W < > x : @ ! / = ? +` plus `[ New chat ]` is ASCII. Width is deterministic and every glyph has a keyboard mnemonic or documented equivalent.

Unicode may remain inside child-owned content. It is not suitable for geometry-critical controls.

## 12. Responsive survival policy

The allocator is capability-independent and deterministic. “Survives” means visibly rendered; omitted pointer actions retain their keyboard path.

| Width | Guaranteed visible | Added when it fits | Rationale |
|---:|---|---|---|
| 20 | ` C  T  W `, padded ` + `, ` ? ` | a few identity viewport cells only after actions | Emergency, unsupported width. Full label is impossible by a one-cell proof. |
| 32 | ` C  T  W `, full `[ New chat ]`, ` ? `, full ordinary `.chat-N` | identity viewport | Keeps the earlier labelled-action intent below the supported floor. |
| 40 | lanes, exact ordinary `.chat-N`/short task identity, warning, Help, full New chat | menu or narrow identity flex | Validated Termux floor; global action and return paths outrank ambient data. |
| 60 | above plus larger exact-ID viewport, context menu, warning pulse | close, Search, Controls | Enough room for terminal status or a normal task ID without a second row. |
| 80 | above plus `/`, `=`, `?`, close and compact pulse | prev/next, status | Laptop grammar exposes all global destinations. |
| 120 | all controls, full normal ID, state, prev/next, long pulse | route/friendly label | Desktop uses spare width for context, never more chrome. |

Collapse order, first removed to last removed:

1. route and friendly labels;
2. healthy disk/daemon words and long pulse;
3. status text;
4. previous/next at boundaries, then available previous/next;
5. Close (keyboard remains), then context menu;
6. healthy pulse;
7. Controls and Search pointer controls;
8. warning only if width is below the supported floor;
9. identity viewport cells, while keeping canonical backing identity;
10. Help only between 21 and 31 columns;
11. full New-chat label only below 32, replaced by padded `+`;
12. lane rail never collapses.

The exact task ID receives all remaining flex space before optional state. No optional token is partially rendered; the allocator chooses complete candidates.

## 13. State and reachability proof

| Current content | Active styling | Identity | ` C ` result | ` T ` result | ` W ` result | New chat |
|---|---|---|---|---|---|---|
| Live Chat/PT​​Y | Chat | exact `.chat-N` | selector on second activation | selected/remembered Task | Dashboard | visible; pointer routed before PTY |
| Normal Task Detail | Task | exact task ID | remembered Chat/empty Chat | task picker/current Detail | Dashboard | visible |
| Terminal/archived Chat Detail | Task | exact `.chat-N` | remembered live Chat/empty Chat | remains exact Detail | Dashboard | visible; no relaunch |
| Workspace/Dashboard | Workspace | repo label | remembered Chat/empty Chat | selected Task/graph | remains Dashboard | visible |
| Config | none; Controls active | `Config:<section>` | Chat | Task | Workspace | visible |
| Settings | none; Controls active | `Settings:<key>` | Chat | Task | Workspace | visible |
| Search active | remembered lane remains styled | query and count | switches scope and keeps query | same | same | visible |
| No chat | Chat | `No chat` (not `.chat-0`) | empty state/selector | Task/graph | Dashboard | visible; no startup mutation |
| All archived | Chat or Workspace | `Archived:N` | selector with terminal rows | selected terminal task Detail | Dashboard | visible |

Thus:

- inspecting a task cannot remove Chat because ` C ` is invariant;
- chatting cannot remove Task or Workspace because ` T ` and ` W ` are invariant and pointer routing precedes PTY capture;
- Config/Settings cannot trap the user because all three lanes remain;
- New chat is globally visible at every supported width and remains keyboard reachable at every width.

Search defaults to the current lane's scope. Switching lanes while search is active keeps the query but recomputes results and visibly changes the scope; Esc clears. It never silently sends the query to a Chat PTY.

## 14. Behavioral invariants that implementation must preserve

1. **PTY ownership:** for a live interactive Chat, the child owns every cell below this one row. WG adds no composer, tab strip, footer, border, or status row.
2. **Input ownership:** when the child owns stdin, only `Ctrl+O` is the keyboard escape. Bar pointer hits are host-owned and dispatched first. Existing mosh Enter, Ctrl+J multiline, scrollback, resize, and restart behavior is unchanged.
3. **Terminal-chat routing:** terminal/done/archived/abandoned `.chat-N` opens exact Task/Detail through `open_chat_task_or_detail`; it never spawns, resumes, or attaches a PTY.
4. **Exact lifecycle:** Close/Menu actions carry the exact identity captured in the frame model and revalidate state before Stop/Archive/Retry. They never operate on “current” after an asynchronous switch.
5. **Non-mutating startup:** rendering `No chat`, resolving color, or selecting a lane creates no chat/task/history/route/tmux/provider state. Only confirmed New chat creates one.
6. **Layout:** one contextual row, one side or stacked seam, zero outer frames. Layout/command mode may replace row contents temporarily but never allocates another row.
7. **Mouse:** every visible affordance has a current hit rectangle; modal hits cannot fall through; resize invalidates stale regions; selector rows/footer/wheel behavior from `make-chat-selector` remains.
8. **Keyboard:** current `0..8`, `n`, `/`, `?`, Chat arrows/brackets, `w`/Ctrl-W, `Ctrl+O`, graph navigation, layout, and mouse-toggle guarantees remain. New aliases are additive only.
9. **Asynchrony:** pulse, disk, service, graph counts, workspace appearance, and identity lists are coherent cached snapshots. Render/input do no filesystem, hostname, subprocess, graph derivation, or hash work.
10. **Accessibility:** warning, focus, active lane, and disabled state have non-color cues; `NO_COLOR` is complete, not a degraded mystery mode.

## 15. Migration plan

No code should be changed until this grammar is accepted.

### Phase 1 — pure model and geometry

- Introduce a pure `ContextBarModel` containing lane memory, full identity, display viewport, action availability, pulse snapshot, workspace appearance, and immutable hit/action IDs.
- Add width-table golden tests for 20/32/40/60/80/120, including cell widths and non-overlap.
- Keep the existing renderer behind a runtime/development flag for capture comparison; do not add a second visible row.

### Phase 2 — persistent lanes and real controls

- Replace the changing `Chat/Task/Workspace` selector with the fixed three-control rail.
- Route every hit through the current identity-safe action functions. Retain existing key bindings and selector behavior.
- Add active/hover/focus/disabled styles without changing any rectangle.

### Phase 3 — Config/Settings controls palette

- Add ` = ` and its bounded palette; keep Config and Settings content/side effects separate.
- Add F2/F8 aliases only after PTY/keymap conflict tests. Existing numeric access remains the fallback.

### Phase 4 — asynchronous workspace appearance

- Compute v1 identity/hash/color on the bootstrap worker, publish a cached snapshot, and expose override/picker in Settings.
- Add truecolor/256/16/mono vector tests and explicit tmux/mosh/Termux capability fixtures.

### Phase 5 — live human-flow gate

A candidate implementation is not complete without a real tmux/PTY scenario that:

- taps C/T/W/New/Search/Controls/Help and every rendered contextual action at actual cells at 40, 80, and 120 columns;
- proves Task -> Chat and Chat -> Task/Workspace return paths;
- proves no-chat/all-archived/terminal-chat/Config/search states;
- types through a live child before and after `Ctrl+O` and proves no key theft;
- resizes through all breakpoints and rejects stale hit coordinates;
- launches repeatedly on an empty graph and proves zero mutation;
- checks candidate-binary provenance and owns the new scenario in the smoke manifest.

Existing selector, close-lifecycle, non-mutating-startup, immediate-input, mosh, path-unique resume, and four-sided layout scenarios must remain green. Migration removes old decorative hit fields only after the new flow passes; it never leaves compatibility glyphs visible without handlers.

## 16. Rejected details

- **Emoji gear:** rejected for VS15/VS16/font/wcwidth ambiguity; use `=`.
- **A permanent Config and Settings tab pair:** rejected because it steals identity width; use one Controls palette and direct keys.
- **Collapsing C/T/W into a dropdown:** rejected because return paths disappear.
- **Icon-only New chat at 40 columns:** rejected. The label is a deliberate exception and survives at the supported floor.
- **Claiming `[ New chat ]` fits with all lanes at 20 columns:** rejected as arithmetically false. The documented emergency `+` form is preferable to clipping or lying.
- **One-cell hit targets:** rejected for Termux. Padding is part of the hitbox.
- **Unicode arrows/ellipsis/status dots in geometry-critical controls:** rejected because ASCII equivalents are sufficient and more stable.
- **OSC palette mutation or probing:** rejected for tmux/mosh safety and render latency.
- **Hashing only repository basename:** rejected because unrelated clones collide. Hashing raw worktree paths is also rejected because WG worktrees would flicker between colors.
- **Color as identity or warning:** rejected. Exact text, active styling, and `!` remain authoritative.
- **Filesystem/hostname work in render/input:** rejected categorically; bootstrap cache owns it.

## 17. Acceptance checklist for the later implementation

- [ ] Exactly one context row, one split seam, and zero outer frames in full/side/stacked layouts.
- [ ] C/T/W and New chat are reachable from Chat, Task, Workspace, Config, Settings, search, no-chat, all-archived, and terminal-chat states.
- [ ] Every rendered action has a same-frame padded hitbox and keyboard path; every unavailable action is omitted or explanatory.
- [ ] Width goldens pass at 20/32/40/60/80/120, with full `[ New chat ]` at 32 and above and no partial optional tokens.
- [ ] Full backing identity survives clipping; `.chat-N` and ordinary task IDs route exactly.
- [ ] Workspace color v1 test vectors, contrast thresholds, capability fallbacks, override precedence, and async/no-I/O render assertions pass.
- [ ] Config and Settings remain separately named, described, keyed, and tested.
- [ ] Real Termux-like SGR taps and laptop keyboard flows pass through tmux; PTY typing/resize/resume/scrollback remain unchanged.
- [ ] Empty startup and color bootstrap are read-only; confirmed New chat is the only creation path in this flow.
