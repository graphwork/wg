# Bug diagnosis: wg TUI keymap + input routing

**Task:** `bug-tui-keymap-audit` — Diagnose: wg TUI keymap + input routing
(`+` opens add-chat; Ctrl+T collides with thinking-toggle).
**Status:** Investigation only — no code changes in this task.
**Primary file:** `src/tui/viz_viewer/event.rs`

---

## TL;DR

Two user-visible symptoms share one root cause: **the host TUI's
"escape-hatch" hotkeys are checked _before_ keystrokes are forwarded to the
focused embedded REPL**, so a printable (`+`) and a vendor-reserved chord
(`Ctrl+T`) are stolen from the child instead of reaching it.

- **Symptom 1 — `+` opens "add chat" while typing to claude.** Root cause:
  `handle_key()` → `vendor_pty_active` branch, the `is_plus_launcher` check at
  **`src/tui/viz_viewer/event.rs:619-624`**. A *bare printable* `+` is matched
  as a global hotkey and routed to `app.open_launcher()` before it is forwarded
  to the embedded child's stdin.
- **Symptom 2 — `Ctrl+T` collides with codex/pi "toggle thinking".** Root
  cause: the same branch's `is_toggle` check at
  **`src/tui/viz_viewer/event.rs:617-618`**, which lets `Ctrl+T` fall through to
  `toggle_chat_pty_mode()` (the host "command mode" focus toggle) at
  **`event.rs:2038-2043`** / **`event.rs:3209-3214`**. `Ctrl+T` is never
  forwarded to the embedded REPL, so codex/pi's thinking-toggle convention can
  never fire.

The host-TUI *composer* (`handle_chat_input`, `event.rs:1864`) already does the
right thing — printables fall through to the editor. Only the **embedded-PTY
forward path** violates the policy. The fix is to make that path obey the same
rule the composer already follows: **printables and vendor-reserved chords go to
the focused input/child; host hotkeys fire only when no input/child is
focused.**

---

## 1. How the wg TUI dispatches key events

### 1.1 Event pipeline

```
crossterm event reader thread
  → mpsc channel
    → run_event_loop_inner()                       event.rs:241
      → dispatch_event(app, ev)                     event.rs:382
        ├─ Event::Key(Press) → handle_key(...)      event.rs:392 → 472
        ├─ Event::Paste(t)   → handle_paste(...)    event.rs:400 → 744
        ├─ Event::Mouse(...) → handle_mouse(...)    event.rs:403
        ├─ Event::Resize     → (redraw)             event.rs:406
        └─ _                  → ignored             event.rs:407   ← FocusGained/FocusLost land here
```

`dispatch_event` only handles `KeyEventKind::Press` (good — avoids
double-counting key Release on kitty-protocol terminals). **`Event::FocusGained`
/ `Event::FocusLost` fall into the `_ => {}` arm and are dropped** — relevant to
the sibling focus-race bug (§6).

### 1.2 `handle_key` precedence ladder

`handle_key()` (`event.rs:472`) is a **flat precedence ladder of global
intercepts, checked top-to-bottom _before_ the per-`InputMode` dispatch at the
bottom**. The order is the whole problem. Abbreviated:

| Order | Guard | Action | Line |
|------:|-------|--------|------|
| 1 | `show_help` | swallow all but `?`/`Esc`/`q` | 474 |
| 2 | `service_health.panel_open` | service panel keys | 484 |
| 3 | `service_health.detail_open` | popup scroll keys | 489 |
| 4 | `InputMode::ScrollMode` | scrollback nav; **swallow all, no PTY forward** | 508 |
| 5 | **`vendor_pty_active`** | **host escapes, then forward to embedded child** | **603** |
| 6 | global `Ctrl+N` | open launcher | 675 |
| 7 | global `Ctrl+W` | close chat tab | 688 |
| 8 | bare `w` (Normal) | close chat tab | 701 |
| 9 | `match app.input_mode { … }` | per-mode handlers (incl. `ChatInput`, `Search`, `Launcher`, …) | 715 |

The per-`InputMode` dispatch (step 9) is where text inputs *should* receive
their keys. But steps 5–8 run first. **Any global intercept above step 9 that
matches a bare printable will steal it from a focused text input.** That is
exactly what happens to `+`.

### 1.3 The `vendor_pty_active` branch (the heart of both bugs)

`vendor_pty_active` (`event.rs:603-611`) is true when the embedded chat PTY owns
focus and should receive raw keystrokes:

```rust
let vendor_pty_active = app.chat_pty_mode
    && app.chat_pty_forwards_stdin
    && app.right_panel_tab == RightPanelTab::Chat
    && app.focused_panel == FocusedPanel::RightPanel
    && !app.chat_pty_observer
    && matches!(app.input_mode, InputMode::Normal)
    && !app.chat_agent_death.contains_key(&app.active_coordinator_id);
```

This is the "you are typing to claude/codex/nex" state. Inside the branch
(`event.rs:612-668`) the order is:

1. compute `is_toggle` = `Ctrl+T` — **event.rs:617-618**
2. compute `is_plus_launcher` = bare `+` (no Ctrl/Alt/Meta) — **event.rs:619-621**
3. **if `is_plus_launcher` → `app.open_launcher(); return;`** — **event.rs:622-624** ← **Symptom 1**
4. `is_scroll_toggle` = `Ctrl+]` → enter `ScrollMode` — event.rs:626-631
5. bare `PageUp/PageDown/Home/End` → scroll the pane — event.rs:632-648
6. `if !is_toggle { … forward key to pane via pane.send_key() … return; }` — event.rs:650-667
7. (else) `Ctrl+T` falls through to `handle_normal_key` → `toggle_chat_pty_mode` — **event.rs:668** ← **Symptom 2**

So while the child has focus, **everything is forwarded to the child via
`pane.send_key()` EXCEPT** `+` (→ launcher), `Ctrl+T` (→ command-mode toggle),
`Ctrl+]` (→ scroll mode), bare PageUp/Down/Home/End (→ scroll), and `Ctrl+C`
(→ `pane.interrupt_foreground()`). `+` is the **only bare printable** in that
exception list — which is precisely why `+` is the one printable users see
hijacked.

---

## 2. Root cause: Symptom 1 (`+` captured as a global hotkey)

**File/function:** `src/tui/viz_viewer/event.rs`, `handle_key()`, the
`is_plus_launcher` check at **lines 619-624**:

```rust
let is_plus_launcher = matches!(code, KeyCode::Char('+'))
    && !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::META);
if is_plus_launcher {
    app.open_launcher();
    return;
}
```

**Why it fires while you are "typing to claude":** when the embedded REPL owns
focus, `vendor_pty_active` is true, and this check runs *before* the
forward-to-child code at `event.rs:650`. A bare `+` matches, so the host opens
the new-chat launcher and `return`s — the `+` never reaches the child's stdin.

**Why `+` specifically (and not other printables):** the design intent
(comment at `event.rs:612-616`, from task `implement-tui-modal`, commit
`e1441c22`) was to keep two host escapes alive while the REPL has focus —
`Ctrl+T` (command mode) and `+` (new chat, because the visible Chat tab renders
a clickable `[+]` button). Every other letter/digit/symbol correctly falls to
the `pane.send_key()` forward. The mistake is treating a **bare printable** as a
host hotkey: there is no modifier to distinguish "the user wants a new chat"
from "the user typed a plus sign into their prompt", so the printable is
unconditionally stolen. The regression is locked in by the test
`pty_mode_plus_opens_launcher` (`event.rs:9237-9258`), which asserts the *buggy*
behavior.

**Confirmation that the rest of the stack is correct:** the host-TUI composer
`handle_chat_input()` (`event.rs:1864-1960`) handles `+` correctly — its `match`
only special-cases `Esc`, `Enter`, `Ctrl+C`, `Ctrl+V`, and arrows; everything
else (including `+`) falls through to
`app.editor_handler.on_key_event(...)` which inserts the character. So the
non-PTY input path already obeys the correct policy. **Only the embedded-PTY
forward path is broken.**

### Secondary `+` bindings (correct — context-gated, no input focused)

- `handle_right_panel_key`: `KeyCode::Char('+') if right_panel_tab == Chat`
  (`event.rs:3271-3278`) — opens launcher (or `create_coordinator_with_defaults`
  on `Shift+Plus` with keyboard enhancement). This only runs in `InputMode::Normal`
  with no embedded child focused → acceptable.
- `handle_chat_manager_input`: `KeyCode::Char('+')` (`event.rs:1255-1258`) —
  launcher inside the chat-manager overlay; no text field focused → acceptable.

These are fine under the proposed policy because no text input/child has focus
when they fire. The bug is exclusively the `vendor_pty_active` interception.

---

## 3. Root cause: Symptom 2 (`Ctrl+T` collides with thinking-toggle)

**What "command mode" is:** there is **no `InputMode::Command`**. "Command
mode" is the state where the embedded chat PTY is *rendered but not focused*
(`focused_panel == Graph` while on the Chat tab). In that state the host's
single-key bindings (`n` new chat, `w` close tab, `+`, `[`/`]`, digits, …) act
as host commands instead of being forwarded to the child. `toggle_chat_pty_mode`
(`event.rs:3460-3489`) flips `focused_panel` between `RightPanel` (REPL focused)
and `Graph` (command mode).

**File/function:** `Ctrl+T` is bound to that toggle in two places:

- `handle_normal_key()` global pre-check — **event.rs:2035-2043**:
  ```rust
  if modifiers.contains(KeyModifiers::CONTROL)
      && matches!(code, KeyCode::Char('t'))
      && app.right_panel_tab == RightPanelTab::Chat
  { toggle_chat_pty_mode(app); return; }
  ```
- `handle_right_panel_key()` match arm — **event.rs:3209-3214** (same call).

**Why it clobbers codex/pi:** in the `vendor_pty_active` branch, `is_toggle`
(`event.rs:617-618`) is the one chord that is **not** forwarded to the child —
the `if !is_toggle { … forward … }` guard at `event.rs:650` deliberately skips
it so it can fall through (`event.rs:668`) to `toggle_chat_pty_mode`. Net
effect: while you are inside codex / pi / claude, pressing `Ctrl+T` flips the
host's focus instead of toggling the executor's thinking blocks. `Ctrl+T` is the
established "toggle thinking" convention in codex/pi, so wg silently clobbers it.

This binding was introduced by task `implement-tui-modal` (commit `e1441c22`)
and the command-mode single-key aliases by `implement-tui-command`
(`90cfcb9e`). At that time the "embedded executor reserves `Ctrl+T`" convention
was not accounted for.

---

## 4. Catalog of global bindings + collisions

### 4.1 Bindings that fire while the embedded REPL has focus (`vendor_pty_active`)

These are the only keys NOT forwarded to the child — the "host escape" set. This
is the collision-critical list, because the child (claude/codex/pi/nex REPL)
expects to receive these.

| Key | Host action | Site | Collision with executor convention? |
|-----|-------------|------|-------------------------------------|
| **`+`** (bare) | open new-chat launcher | event.rs:619-624 | **YES — printable. Steals a typed `+`.** (Symptom 1) |
| **`Ctrl+T`** | toggle command mode | event.rs:617-618 → 2038/3209 | **YES — codex/pi "toggle thinking blocks".** (Symptom 2) |
| `Ctrl+]` | enter scroll mode | event.rs:626-631 | Low. `Ctrl+]` is the telnet/nc "escape to host" char; rarely used inside REPLs. |
| `PageUp/PageDown/Home/End` (bare) | scroll pane | event.rs:632-648 | Low–med. Some full-screen REPL UIs use PageUp/Down; `Home`/`End` are line-edit keys in readline but rarely sent as the bare `KeyCode::Home/End` here. Worth review. |
| `Ctrl+C` | `pane.interrupt_foreground()` | event.rs:652-654 | Intended — SIGINT to the child. Correct. |

### 4.2 Global bindings active in "command mode" / graph focus (no input focused)

These are *not* bugs (no text input is focused when they fire) but are catalogued
for the policy and to flag any that are surprising. Source: `handle_key`
pre-dispatch (`event.rs:672-712`), `handle_graph_key` (`event.rs:2166+`),
`handle_right_panel_key` (`event.rs:2799+`), `handle_normal_key`
(`event.rs:2034+`).

Bare single-key (Normal mode, no input focused):

| Key | Action | Site |
|-----|--------|------|
| `q` | quit | event.rs:2184, 2800 |
| `?` | help overlay | event.rs:2181, 2799 |
| `Esc` | clear search / dismiss toast / quit | event.rs:2185 |
| `/` | search | event.rs:2227 |
| `Tab` | toggle panel focus | event.rs:2236 |
| `t` | toggle trace | event.rs:2248 |
| `T` | toggle token display | event.rs:2253 |
| `.` | toggle system tasks | event.rs:2258 |
| `<` | toggle running-system-tasks | event.rs:2264 |
| `*` | toggle touch echo | event.rs:2270 |
| `\` | toggle right panel | event.rs:2278 |
| `=` / `BackTab` | cycle layout mode | event.rs:2283 |
| `i` / `v` | grow / shrink viz pane | event.rs:2287/2291 |
| `n` / `N` | new chat (no matches) / next-prev search match | event.rs:2297/2310 |
| `w` | close chat tab (Chat tab) | event.rs:701-712 |
| `+` / `-` | new chat / close tab (Chat tab) | event.rs:3271/3280 |
| `~` / `` ` `` | coordinator picker (Chat tab) | event.rs:3269 |
| `[` / `]` | cycle chat tabs / iterations | event.rs:3217/3245 |
| `1`–`9` | jump to chat tab / panel tab | try_chat_tab_navigation, event.rs:2047 |
| `R` | toggle raw JSON (Detail tab) | event.rs:3320 |
| `r`/`e`/`x` | death-panel recovery (when chat agent died) | event.rs:2066-2085 |

Modifier chords (mostly fine — modifiers disambiguate from printables):

| Key | Action | Site | Note |
|-----|--------|------|------|
| `Ctrl+N` | open launcher | event.rs:675-685 | legacy alias of `n` |
| `Ctrl+W` | close chat tab | event.rs:688-700 | readline "delete word" — but only fires when NOT PTY-focused, so child keeps its `Ctrl+W` |
| `Ctrl+C` | interrupt coordinator / kill agent | event.rs:2799+, 2063 | |
| `Ctrl+H` | history browser | event.rs:2207 | readline "backspace" — same gating caveat as `Ctrl+W` |
| `Ctrl+R` | resume after provider error | event.rs:2210 | readline "reverse-search" — same gating caveat |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | cycle chat tabs | event.rs:2113, 2814 | |
| `Alt+1..9` | jump to chat tab | try_chat_tab_navigation | |
| `Alt+Up/Down`, `Alt+Left/Right` | panel focus / inspector cycle | event.rs:2316+ | |

> **Note on `Ctrl+W` / `Ctrl+H` / `Ctrl+R`:** these collide with readline
> editing semantics *in principle*, but they are gated to fire only when the
> embedded REPL does **not** have focus (the `vendor_pty_active` branch returns
> before they are reached). So inside the REPL the child still receives them.
> They are listed for completeness; they are not the reported bug. The only
> chords that reach into the REPL-focused state are the §4.1 set.

### 4.3 Collision summary

- **`+`** — printable stolen from focused input. **Must fix** (Symptom 1).
- **`Ctrl+T`** — clobbers codex/pi thinking-toggle. **Must fix** (Symptom 2).
- `PageUp/PageDown/Home/End` — review whether full-screen child UIs need these
  (secondary; see policy §5).
- `Ctrl+]`, `Ctrl+C` — acceptable host/child escapes; keep.

---

## 5. Proposed keymap policy

A single coherent rule set that the embedded-PTY path should obey (the host
composer `handle_chat_input` already obeys it):

### P1 — Printables always go to the focused input/child.
A bare `KeyCode::Char(c)` with no `CONTROL`/`ALT`/`META` modifier is **text**.
It must reach the focused text input (`ChatInput`, `Search`, launcher fields, …)
or the focused embedded child, and must **never** be consumed as a global
hotkey. This directly removes the `is_plus_launcher` interception
(`event.rs:619-624`). The `[+]` new-chat affordance stays reachable two correct
ways: by **mouse click** on the rendered `[+]` button, and by `+` **in command
mode** (when no child/input is focused).

> Single-key host commands that exist today in *command mode* (`n`, `w`, `+`,
> `t`, `.`, …) stay legal there **because no text input is focused in that
> state** — P1 only forbids stealing a printable *from a focused input/child*.

### P2 — Global hotkeys fire only when no text input/child is focused, OR require a non-printable modifier chord.
Two acceptable forms for a global hotkey:
1. A bare key that only fires when `focused_panel != RightPanel`/no input mode
   active (i.e. command mode / graph focus) — today's `n`/`w`/`+` command-mode
   keys.
2. A modifier chord (`Ctrl+…`, `Alt+…`) that a bare printable can never produce.
   Even these must be checked against executor conventions (§P3).

### P3 — Reserved passthrough keys go to the embedded executor per its conventions.
Maintain an explicit **reserved set** of chords that, while a child is focused,
are forwarded to the child rather than consumed by the host, because downstream
executors bind them:

- `Ctrl+T` → **forward** (codex/pi: toggle thinking blocks).
- `Ctrl+R` → reverse-search (readline) — already forwarded (host `Ctrl+R` is
  gated off when PTY-focused); keep it that way.
- `Ctrl+C`/`Ctrl+D`/`Ctrl+Z`/`Ctrl+L`/`Ctrl+A`/`Ctrl+E`/`Ctrl+U`/`Ctrl+W`/
  `Ctrl+K` — readline/job-control; already forwarded (except host `Ctrl+C`,
  which is intentionally translated to `pane.interrupt_foreground()` — keep).
- Consider `PageUp/PageDown` for full-screen child TUIs.

The host keeps a **small, deliberate** escape set that does NOT overlap the
reserved set (see §6 rebinding). The telnet/tmux model is the right mental
model: one well-known "escape to host" chord, everything else passes through.

### P4 — One escape chord, symmetric.
Entering and leaving command mode should use the same, single, non-printable,
non-reserved chord (today `Ctrl+T`, which violates P3). Pick one chord that is
not a readline editing key, not a vendor thinking/interrupt/EOF key, and ideally
not the user's tmux prefix (wg is frequently run inside tmux — see §6).

**Net effect on the two bugs:**
- P1 deletes the `+` interception → `+` reaches claude. ✅ Symptom 1.
- P3 adds `Ctrl+T` to the reserved-passthrough set → codex/pi thinking-toggle
  works. ✅ Symptom 2.
- P4 moves command-mode toggle to a safe chord → keyboard escape still exists.

---

## 6. Command-mode rebinding options (free `Ctrl+T`)

Goal: forward `Ctrl+T` to the embedded REPL (restore thinking-toggle) and move
the command-mode focus toggle to a chord that does not collide with
codex/pi/claude/readline conventions — and ideally not with the tmux prefix
(per `bug-tui-focus-diagnose`, wg runs inside tmux/WezTerm/SSH; the user's
environment is also Termux→Mosh→Tmux→TUI).

Constraints a candidate must satisfy: non-printable; not a readline editing key
(`Ctrl+A/B/E/F/H/K/N/P/R/U/W/D/L`); not a vendor reserved key
(`Ctrl+T` thinking, `Ctrl+C` int, `Ctrl+D` EOF, `Ctrl+G` abort); not the tmux
default prefix (`Ctrl+B`) or common custom prefixes (`Ctrl+A`, `Ctrl+Q`).

### Option A (recommended): `Ctrl+O` as the command-mode toggle
- **Pros:** `Ctrl+O` is readline `operate-and-get-next` — a niche, almost-never
  used binding; no vendor (codex/pi/claude) assigns it; not a tmux prefix;
  single chord; symmetric toggle; minimal code change (swap the two
  `Char('t')+CONTROL` sites and the `is_toggle` computation to `Char('o')`).
  Mnemonic: "**O**uter / **O**perate host."
- **Cons:** the rare readline `operate-and-get-next` user loses it inside the
  host layer (but it's *not* forwarded today either when toggling). Slightly
  less discoverable than `Ctrl+T` was; needs a help-overlay/status-bar update.

### Option B: `Ctrl+]` prefix/leader (telnet/tmux model) — unify host escapes
- Promote `Ctrl+]` (already the in-house "escape to host" chord for scroll mode,
  `event.rs:626-631`) into a **leader**: `Ctrl+]` then a command key —
  `Ctrl+] s` = scroll, `Ctrl+] c` = command mode (focus toggle), `Ctrl+] n` =
  new chat, `Ctrl+] w` = close tab, `Ctrl+] +` = new chat, etc.
- **Pros:** most principled, future-proof: it frees **every** bare/single chord
  (`Ctrl+T`, `+`, …) for the child, matching tmux's prefix discipline exactly.
  `Ctrl+]` is the classic telnet/nc "escape to host". One reserved chord instead
  of a scattered escape set.
- **Cons:** biggest change (introduces a leader/pending-prefix state machine and
  a `InputMode`/flag for "leader pressed"); changes muscle memory for existing
  scroll-mode users; needs a visible "PREFIX" indicator in the status bar. Some
  terminals send `Ctrl+]` as `Ctrl+5`/`Esc`-ish on certain layouts — verify.

### Option C: a function key (`F8` or `F12`) as the command-mode toggle
- **Pros:** F-keys are essentially never bound by REPLs/readline → maximal
  collision safety; unambiguous; trivially a single chord.
- **Cons:** discoverability is poor; **tmux/mosh/Termux may intercept or fail to
  pass F-keys** reliably (directly relevant to this user's
  Termux→Mosh→Tmux→TUI chain), and some F-keys need `Fn` on laptops/mobile.
  Riskiest for the actual deployment environment.

### Recommendation
**Adopt Option A (`Ctrl+O`) now** as the low-risk, minimal-diff fix that frees
`Ctrl+T` immediately and keeps a single symmetric keyboard escape. **Track
Option B (`Ctrl+]` leader) as the principled follow-up** if more host commands
need to coexist with embedded executors — it is the only option that scales
without re-litigating each new chord against vendor conventions. Avoid Option C
given the tmux/mosh/Termux passthrough fragility.

Regardless of which is chosen, also apply **P1** (drop the bare-`+`
interception) and **P3** (add `Ctrl+T` to the reserved-passthrough set so it
reaches the child). The rebind only addresses *how you enter command mode*; it
does not by itself fix the `+` capture.

---

## 7. Overlap with the focus-in fix (sequencing)

The sibling task **`bug-tui-focus-diagnose`** (agent-5498, in progress) owns
**`docs/bugs/tui-focus-input-race.md`** — referenced by this task but **not yet
present on disk at the time of writing** (it is that task's deliverable). Its
bug: returning OS focus to the terminal and typing immediately leaks the first
keystrokes to tmux (pops `choose-tree`), i.e. a raw-mode / keyboard-protocol
re-grab race on focus-in.

### Same layer, different functions — shared file is `event.rs`

| | This bug (keymap routing) | Focus-in race |
|---|---|---|
| Layer | **Key *dispatch / routing*** (host-vs-child arbitration) | **Terminal *mode setup / re-grab*** (raw mode, kitty protocol, focus reporting) |
| Primary code | `handle_key()` `vendor_pty_active` branch — `event.rs:603-668`, `2038`, `3209` | terminal init in `src/tui/viz_viewer/mod.rs:84-105`; `dispatch_event` `Event::FocusGained/FocusLost` arm — `event.rs:407` |
| Touches `event.rs`? | **Yes** (`handle_key`) | **Likely yes** (`dispatch_event` focus arms) |
| Touches `mod.rs`? | No | **Yes** (raw-mode / `EnableBracketedPaste` / `PushKeyboardEnhancementFlags`; note **no `EnableFocusChange`/DECSET 1004 is enabled today** — `mod.rs:84-105`) |

**Conflict assessment:** both fixes edit `src/tui/viz_viewer/event.rs`, but
**different functions** — this task changes `handle_key` (and the two
`Ctrl+T`→`toggle_chat_pty_mode` sites); the focus fix changes `dispatch_event`'s
`Event::FocusGained/FocusLost` handling (currently `_ => {}` at `event.rs:407`)
and the terminal-mode setup in `mod.rs`. They do **not** overlap line-for-line,
so a textual merge conflict is unlikely, but they reason about the *same input
arbitration* and must be coordinated:

- If the focus fix begins **handling `Event::FocusGained/FocusLost`** (e.g.
  re-asserting raw mode / re-pushing keyboard flags, or forwarding focus events
  to the embedded child), it must not re-introduce a path that forwards focus
  bytes into the child while a host modal is open — respect the same modal/PTY
  gating this doc relies on (`vendor_pty_active`, `handle_paste` modal gate at
  `event.rs:744-762`).
- If this keymap fix changes which chords are forwarded to the child, the focus
  fix's "re-grab then replay/flush early bytes" logic must route those replayed
  bytes through the *same* `handle_key` path so the new routing rules apply
  uniformly.

### Suggested sequencing

1. **Land the focus-in fix first** (`bug-tui-focus-fix`): it is lower in the
   stack (terminal-mode acquisition) and changes `mod.rs` + the
   `Event::FocusGained/FocusLost` arms — areas this keymap fix does not touch.
2. **Then land the keymap fix** on top: it edits `handle_key`'s dispatch ladder.
   Rebasing the keymap change over the focus change is clean because the focus
   change leaves `handle_key`'s body untouched.
3. Both should add **human-flow PTY/tmux smoke scenarios** (the keymap fix:
   "`+` reaches the child", "`Ctrl+T` reaches the child", "command-mode chord
   toggles focus"; the focus fix: "no keystroke leak after focus-in"). Keep them
   as **separate** scenarios in `tests/smoke/manifest.toml` so each gate fails
   independently.

If schedule forces the reverse order, the only coordination point is the
`Event::FocusGained/FocusLost` arm in `dispatch_event` — whoever touches it
second should re-read the other's change first.

---

## 8. Validation pointers for the downstream fix task

This is a diagnosis; the fix is a separate task. When it is implemented, per the
repo's "user-visible behavior fixes require live human-flow validation" rule,
the reproducers must drive the real keystroke path (not a CLI/unit substitute):

- **`+` reaches the child:** start `wg tui` in tmux, open a chat with an
  embedded executor (claude/codex/nex), focus the PTY, `tmux send-keys '+'`,
  assert the child's stdin received `+` (PtyPane `child_input_bytes_written()`
  advanced) and the launcher did **not** open. Contrast: the existing
  `pty_mode_plus_opens_launcher` test (`event.rs:9237`) asserts the *buggy*
  behavior and must be inverted.
- **`Ctrl+T` reaches the child:** focus the PTY, send `Ctrl+T`, assert the child
  received it and `focused_panel` did **not** flip.
- **Command-mode escape still works:** send the new escape chord (e.g.
  `Ctrl+O`), assert `focused_panel` toggled to `Graph` and back.
- Add the scenarios to `tests/smoke/scenarios/` and list the fix task in
  `owners` of `tests/smoke/manifest.toml`.

---

## Appendix: key file/line index

| Symbol | Location |
|--------|----------|
| `dispatch_event` (FocusGained/Lost dropped at `_ =>`) | event.rs:382, 407 |
| `handle_key` (global precedence ladder) | event.rs:472 |
| `vendor_pty_active` definition | event.rs:603-611 |
| `is_toggle` (Ctrl+T) | event.rs:617-618 |
| **`is_plus_launcher` (Symptom 1 root cause)** | **event.rs:619-624** |
| `is_scroll_toggle` (Ctrl+]) | event.rs:626-631 |
| forward-to-child (`pane.send_key`) | event.rs:650-667 |
| Ctrl+T fall-through comment | event.rs:668 |
| global Ctrl+N / Ctrl+W / bare `w` | event.rs:675 / 688 / 701 |
| `handle_chat_input` (correct printable handling) | event.rs:1864-1960 |
| `handle_normal_key` Ctrl+T → toggle (Symptom 2) | event.rs:2035-2043 |
| `handle_graph_key` (command-mode bare keys) | event.rs:2166+ |
| `handle_right_panel_key` Ctrl+T → toggle | event.rs:3209-3214 |
| `handle_right_panel_key` `+` (command mode) | event.rs:3271-3278 |
| `toggle_chat_pty_mode` (command mode = focus flip) | event.rs:3460-3489 |
| `handle_paste` modal gating | event.rs:744-762 |
| terminal setup (raw/paste/kbd; no DECSET 1004) | mod.rs:84-105 |
| `pty_mode_plus_opens_launcher` (locks buggy behavior) | event.rs:9237-9258 |

**Introduced by:** `implement-tui-modal` (commit `e1441c22`) — the
`vendor_pty_active` escape set incl. `Ctrl+T` + `+`; `implement-tui-command`
(`90cfcb9e`) — command-mode single-key aliases.
