# Bug: wg TUI leaks first keystrokes to tmux after OS focus-in

**Status:** FIXED in `bug-tui-focus-fix` (see "Fix applied" at the bottom).
Diagnosis below is preserved as the historical record.
**Component:** `wg tui` own input grab (`src/tui/viz_viewer`)
**Task:** `bug-tui-focus-diagnose` → fix tracked by `bug-tui-focus-fix`

---

## Symptom

Running `wg tui` inside tmux (inside WezTerm, over SSH), when the user returns
**OS-level** focus to the terminal window (alt-tab or click) and immediately
types, the **first keystroke(s) are interpreted by tmux instead of wg** — most
visibly popping the tmux session selector (`choose-tree`). Two workarounds
reliably avoid it:

1. **Wait a few seconds** after refocusing before typing.
2. **Press an arrow key first**, then type normally.

This is an **OS focus-in** event (alt-tab / click on the window), *not* a tmux
pane/window switch. The terminal emulator is incidental — WezTerm is merely
where it reproduces. The fix must be **general** (no emulator branching).

---

## Root cause

**wg's TUI establishes its terminal input grab exactly once, at startup, and
never re-asserts it; and it is structurally blind to focus changes, so it has
no hook on which a re-assert could happen.** There is no code path by which wg
re-acquires raw mode / the kitty keyboard protocol / bracketed paste / mouse
reporting after an OS focus-out→focus-in cycle. The window between focus-in and
"wg's grab is back in force at the outer-terminal boundary" is owned by tmux,
and tmux interprets whatever the user types during it.

Two concrete, verifiable defects combine to produce this:

### Defect 1 — the input grab is set once and never refreshed

`src/tui/viz_viewer/mod.rs::run` sets every input mode a single time at startup:

| Mode | Sequence | Site |
|---|---|---|
| raw mode | `tcsetattr(RAW)` | `mod.rs:84` `enable_raw_mode()` |
| alternate screen | `\x1b[?1049h` | `mod.rs:88` `EnterAlternateScreen` |
| bracketed paste | `\x1b[?2004h` | `mod.rs:88` `EnableBracketedPaste` |
| kitty keyboard (disambiguate) | `\x1b[>1u` | `mod.rs:99-104` `PushKeyboardEnhancementFlags(DISAMBIGUATE_ESCAPE_CODES)` |
| mouse (button + SGR) | `\x1b[?1002h\x1b[?1006h` | `src/tui/viz_viewer/event.rs:228` `set_mouse_capture(true, …)` |

After this, **none of these are ever re-emitted** for the lifetime of the TUI.
The *only* place any mode is re-asserted is `set_mouse_capture` when the user
presses the `m` mouse-toggle key (`event.rs:2408`) or via specific
graph-view interactions — raw mode, the kitty flags, and bracketed paste are
never re-asserted under any condition. (Teardown — `mod.rs:158
restore_terminal` — pops everything; it is not relevant to the live race.)

### Defect 2 — wg is blind to focus: it neither requests focus reports nor handles them

- **wg never enables DECSET 1004 (focus reporting).** There is **no
  `EnableFocusChange` anywhere in `src/`** (only `tui_pty.rs`/`tui_nex.rs` and
  the trace-replay path reference focus at all, and none of those is the main
  TUI). Because the inner application (wg) never requests focus events, tmux —
  even with `set -g focus-events on` — will **not forward** the outer terminal's
  focus-in/out reports into wg's pane. wg therefore cannot observe that focus
  returned.

- **Even a focus event that did arrive would be dropped.**
  `event.rs::dispatch_event` (`event.rs:391-408`) matches only
  `Event::Key(Press)`, `Event::Paste`, `Event::Mouse` (when mouse is enabled),
  and `Event::Resize`; everything else — including `Event::FocusGained` and
  `Event::FocusLost` — falls through the catch-all `_ => {}` arm and is silently
  discarded. (crossterm parses `\x1b[I`/`\x1b[O` into
  `FocusGained`/`FocusLost` **unconditionally** —
  `crossterm-0.29.0/.../event/sys/unix/parse.rs:170-171` — so the events exist
  in the stream; wg just ignores them.)

**Net effect:** wg has *neither the signal* (Defect 2) *nor the action* (Defect
1) needed to re-establish its input grab on focus-in. This is the timing/
ordering defect: the re-grab that *should* happen at the focus-in boundary,
before the user's first key, **never happens at all**.

### Why focus-in specifically opens the leak window

The grab wg asserts is not a property of wg's process alone — in the
`WezTerm → SSH → tmux → wg` chain it is **mediated by tmux per pane and mirrored
onto the outer terminal**. tmux tracks each pane's requested modes (extended/
kitty keys via its `extended-keys` option, bracketed paste, mouse) and, for the
active pane, applies the corresponding modes to the real terminal (WezTerm) so
that WezTerm encodes input the way the pane's app expects.

An OS focus-out/in cycle forces a **re-synchronization** at that boundary:
WezTerm runs its own focus handling and (with `focus-events on`) emits
`\x1b[O`/`\x1b[I`, and tmux re-applies the active pane's mode set to the
outer terminal. For a brief window right after focus-in, the pane's
kitty/extended-keys (and mouse) grab is **not yet re-applied** at the
outer-terminal boundary. Bytes the user types in that window are parsed by
**tmux in its own key/mouse context** rather than being forwarded into the wg
pane — which is exactly how a stray sequence reaches a tmux binding such as
`choose-tree`.

A correctly-behaved full-screen app closes this window by **re-asserting its
modes on focus-in** (this is why many TUIs request DECSET 1004 even when they
don't otherwise care about focus). wg does not, so the window stays open until
something *else* closes it — see the workaround analysis below.

> Note on the exact tmux artifact: which tmux action surfaces (`choose-tree`
> vs. a swallowed key vs. a window switch) depends on the user's tmux key/mouse
> bindings and the precise bytes that leak (a letter vs. the SGR mouse report
> from a refocusing *click*, e.g. `\x1b[<0;C;R M`). The **invariant** — and the
> root cause — is "the first post-focus-in input is consumed by tmux, not the
> pane," because wg failed to re-assert its grab. The session-selector pop is
> the user-visible instance of that invariant.

---

## Minimal failing sequence (terminal events)

```
# 1. wg tui starts in a tmux pane. It emits (once):
        \x1b[?1049h        alternate screen          (mod.rs:88)
        \x1b[?2004h        bracketed paste           (mod.rs:88)
        \x1b[>1u           kitty: disambiguate       (mod.rs:99-104)
        \x1b[?1002h\x1b[?1006h   mouse btn + SGR      (event.rs:228)
   #    NOTE: it does NOT emit \x1b[?1004h  (focus reporting)

# 2. User alt-tabs AWAY:
        WezTerm -> tmux:  \x1b[O   (focus-out; only if focus-events on)
   #    tmux consumes it; wg never asked for it, so wg sees nothing.

# 3. User alt-tabs / clicks BACK:
        WezTerm -> tmux:  \x1b[I   (focus-in)
   #    tmux begins re-applying the pane's kitty/mouse modes to WezTerm.

# 4. User types immediately (within the re-sync window):
        keystroke byte(s) reach tmux BEFORE the pane grab is re-applied
        -> tmux parses them in ITS key/mouse table
        -> e.g. pops choose-tree / swallows the first key.

# 5. Re-sync completes (or escape-time elapses):
        subsequent keys route to the wg pane normally.
```

The defining feature is step 4 happening **before** the grab is back, with no
wg action between steps 3 and 4 because wg is blind to step 3.

---

## Why the workarounds work (mechanistically)

Both workarounds close the leak window *without* wg doing anything — which is
itself evidence that the missing actor is wg's re-assert:

- **Waiting a few seconds.** The fix is purely time-based at the layer below
  wg: tmux/WezTerm finish re-applying the pane's modes to the outer terminal,
  and any partial/pending `ESC`-introduced sequence sitting in tmux's input
  parser is resolved when tmux's `escape-time` elapses. After that, the pane
  grab is back in force and keys route to wg.

- **Pressing an arrow key first.** An arrow key is itself a CSI escape
  sequence (`\x1b[A`/`\x1b[B`/…). Sent as the first input during the unsynced
  window it acts as a **sacrificial keystroke**: it is the input that gets
  consumed/misrouted instead of a "real" one, and its `\x1b[` introducer
  forces tmux's parser to complete/realign on a known sequence. By the time the
  user's *next* key is pressed, re-sync has completed and routing is correct.
  (Confirmed *not* a wg-side re-assert: arrow keys do **not** call
  `set_mouse_capture` or re-push any mode — only the `m` toggle at
  `event.rs:2408` does. So the arrow workaround is a terminal/tmux-parser
  effect, not wg recovering.)

Both are accidental side effects of the window closing on its own. Neither
depends on wg, which is precisely the problem.

---

## Proposed fix (general — no emulator branching)

The fix re-asserts **wg's own** input grab whenever focus returns. It detects
nothing about WezTerm or tmux; it only re-emits the modes wg already owns.

1. **Subscribe to focus events.** At startup (`mod.rs::run`, alongside the
   existing setup at `mod.rs:88`) emit DECSET 1004 via
   `crossterm::event::EnableFocusChange`, and `DisableFocusChange` in
   `restore_terminal`. With `focus-events on`, tmux then forwards `\x1b[I`/
   `\x1b[O` into the wg pane. This is general: any terminal that supports focus
   reporting benefits; any that does not is unaffected (identical to today).

2. **Re-assert the grab on focus-in.** Add an `Event::FocusGained` arm to
   `dispatch_event` (`event.rs:391`) that calls a single shared
   `assert_input_grab()` helper which (re-)emits, in **one buffered write +
   one flush** so they land as a contiguous burst before any user key:
   - raw mode (idempotent `enable_raw_mode()`),
   - the kitty keyboard flags,
   - bracketed paste (`\x1b[?2004h`),
   - mouse capture (`set_mouse_capture(app.mouse_enabled, app.any_motion_mouse)`).

   Have **startup and focus-in call the same `assert_input_grab()`** so the two
   paths cannot drift. `Event::FocusLost` can stay a no-op (or be used only to
   pause animations).

3. **Make the re-assert idempotent and stack-safe.** Re-asserting must *set*,
   not *push*: use a set/replace for the kitty flags (e.g. emit the
   set-flags form `\x1b[=1;1u`) rather than another
   `PushKeyboardEnhancementFlags`, which would grow the terminal's flag stack
   on every focus-in and leave residue after the single matching `Pop` at
   teardown. Only ever assert modes **on**; never toggle off→on (that risks
   dropping an in-flight paste — see risks).

This is general because it operates entirely at wg's own layer: it closes the
post-focus-in window deterministically, on every focus-in, for every terminal
that reports focus — instead of relying on time or a sacrificial arrow key.

### Known limitation of this fix

The re-assert is only triggered when wg actually *receives* a focus-in event,
which requires `set -g focus-events on` in the user's tmux. If focus-events is
off, tmux will not forward focus-in and wg cannot re-assert. That is acceptable
and still general (it strictly improves the common case and regresses nothing),
but it should be documented. A belt-and-suspenders, *also general* option — out
of scope to mandate here — is a debounced "re-assert grab once on the first
input after an idle gap"; this does not depend on focus events but carries
higher paste/double-input risk (below), so it should only be considered if the
focus-event path proves insufficient.

---

## Risks the fix must address

- **Bracketed-paste corruption / double handling.** Re-emitting `\x1b[?2004h`
  is harmless when already enabled (idempotent at the terminal). The hazard is
  *toggling* paste mode (off→on) or re-asserting *mid-paste*, which could split
  a paste so part is delivered as raw keystrokes — the very keystroke-leak
  class `handle_paste` already guards against (`event.rs:744`). Mitigation:
  only ever assert "on," only on the discrete `FocusGained` event, never toggle.

- **Kitty enhancement-flag stack growth / double-input.** crossterm's
  `PushKeyboardEnhancementFlags` *pushes* onto the terminal's flag stack;
  re-pushing on every focus-in grows the stack while teardown pops only once,
  leaving stale flags after exit and potentially altering key encoding.
  Mitigation: re-assert via set/replace, not push (see fix step 3). Re-asserting
  must not synthesize input, so it cannot itself cause double-input.

- **Focus-event regressions / spurious re-asserts.** Some terminals emit a
  focus-in at startup or send duplicate/rapid focus events; re-asserting on
  each is cheap and idempotent but could cause redundant writes or flicker
  under a focus-event flood. Mitigation: debounce (skip re-assert if the last
  one was <~100 ms ago); idempotent modes make the worst case a few extra
  bytes. Also ensure newly-received focus events do not accidentally feed
  interaction-tracking paths (e.g. bumping `last_interaction_at`) — they are
  not user input.

- **Embedded PTY / chat-takeover interaction.** When a chat PTY pane is focused
  (`chat_pty_mode`), wg forwards keys to the child and the child manages its
  own terminal modes. The focus-in re-assert must re-assert **wg's outer grab
  only** and must not clobber or race the child pane's mode negotiation
  (`src/tui/pty_pane.rs`). Scope `assert_input_grab()` to wg's own modes.

---

## Suggested validation for the fix (`bug-tui-focus-fix`)

Per the repo's "user-visible behavior fixes require live human-flow validation"
rule, a CLI/unit test is insufficient. The reproducer should drive the real
input path:

- A scripted PTY/tmux harness that: starts `wg tui`, injects a focus-out then
  focus-in report (`\x1b[O` … `\x1b[I`) followed immediately by a key, and
  asserts the **key reached wg** (e.g. observed via `wg tui dump` /
  `screen_dump`, or the event trace) rather than being consumed upstream.
- The reproducer must fail on `main` (key lost / mis-routed) and pass after the
  fix.
- Add it to `tests/smoke/scenarios/` and list `bug-tui-focus-fix` in the
  `owners` of `tests/smoke/manifest.toml` so the smoke gate catches regressions.
- `cargo build` + `cargo test` green with no regressions.

---

## Code references

- `src/tui/viz_viewer/mod.rs:84` — `enable_raw_mode()` (once)
- `src/tui/viz_viewer/mod.rs:88` — `EnterAlternateScreen, EnableBracketedPaste` (once)
- `src/tui/viz_viewer/mod.rs:94-104` — kitty `supports_keyboard_enhancement()` + `PushKeyboardEnhancementFlags` (once)
- `src/tui/viz_viewer/mod.rs:158-173` — `restore_terminal()` teardown
- `src/tui/viz_viewer/event.rs:198-211` — `set_mouse_capture()` (raw DECSET writes)
- `src/tui/viz_viewer/event.rs:222-258` — event loop + background `event::read()` reader thread
- `src/tui/viz_viewer/event.rs:382-408` — `dispatch_event` (no `FocusGained`/`FocusLost` arm; catch-all `_ => {}`)
- `src/tui/viz_viewer/event.rs:2406-2409` — `m` toggle, the only live mouse re-assert
- `crossterm-0.29.0/src/event/sys/unix/parse.rs:170-171` — `\x1b[I`/`\x1b[O` → `FocusGained`/`FocusLost` parsed unconditionally
- `crossterm-0.29.0/src/terminal/sys/unix.rs:213-267` — keyboard-enhancement query (`\x1b[?u\x1b[c`), shows the kitty negotiation is a query/response dance over the tmux channel
- `docs/pi-integration/terminal-host-research.md` — *adjacent* layer (wg **hosting** child terminals); confirms the team's model of tmux/kitty/DA mode mediation, but the bug here is wg's **own** TUI input grab, not the PTY-host path.

---

## Fix applied (`bug-tui-focus-fix`)

General, emulator-agnostic — no WezTerm/kitty special-casing. Both diagnosed
defects are closed at wg's own layer:

1. **Subscribe to focus (Defect 2).** `src/tui/viz_viewer/mod.rs::run` now emits
   `EnableFocusChange` (DECSET 1004) alongside the existing startup setup, and
   `restore_terminal()` emits `DisableFocusChange`. With tmux `focus-events on`,
   the outer terminal's `\x1b[I`/`\x1b[O` are forwarded into wg's pane. Terminals
   that don't report focus are unaffected (identical to before).

2. **Re-assert the grab on focus-in (Defect 1).** A single shared
   `assert_input_grab()` helper (`src/tui/viz_viewer/event.rs`) re-emits, as one
   buffered `write_all` + `flush`, the modes wg owns: raw mode (idempotent
   `enable_raw_mode()`), kitty disambiguation, bracketed paste (`\x1b[?2004h`),
   and mouse capture. **Both** startup (`run_event_loop`) and the new
   `Event::FocusGained` arm in `dispatch_event` call it, so the two paths cannot
   drift. `Event::FocusLost` is an explicit no-op.

3. **Idempotent + stack-safe.** Kitty flags are re-asserted with the **set**
   form (`CSI = 1 ; 1 u`, i.e. `\x1b[=1;1u`), never another
   `PushKeyboardEnhancementFlags`, so repeated focus-ins don't grow the
   terminal's flag stack (teardown still pops exactly once). Modes are only ever
   asserted **on**; nothing is toggled off→on, so an in-flight paste can't be
   split. The byte payload is built by the pure, unit-tested `input_grab_bytes()`
   so `assert_input_grab()` (which does real terminal I/O) is never driven from a
   unit test.

A focus event is treated as a terminal control signal, **not** user input: the
`FocusGained` arm only re-asserts the grab — it never bumps interaction tracking
and is never forwarded to an embedded chat PTY child (that child negotiates modes
against the in-process emulator in `src/tui/pty_pane.rs`, a separate terminal, so
wg's outer re-assert can't clobber it).

### Known limitation

The re-assert fires only when wg actually *receives* a focus-in, which requires
`set -g focus-events on` in the user's tmux. With focus-events off, tmux won't
forward focus-in and wg can't re-assert — this is strictly no worse than before
and still general (it regresses nothing). If it ever proves insufficient, a
*also-general* follow-up is a debounced "re-assert once on the first input after
an idle gap" (higher paste/double-input risk — only if needed).

### Tests / regression cover

- Unit: `input_grab_tests` in `src/tui/viz_viewer/event.rs` pins the exact re-grab
  byte payload (kitty *set* not push; paste + mouse re-asserted; mouse-off path;
  any-motion mode 1003; paste never disabled).
- Smoke (human-flow): `tests/smoke/scenarios/tui_focus_in_reasserts_input_grab.sh`
  (owned by `bug-tui-focus-fix`, `bug-tui-focus-verify`) drives the **real**
  `wg tui` under a PTY and asserts, in the raw output stream, that startup
  enables DECSET 1004 and that an injected focus-in (`\x1b[I`) re-emits the grab
  burst (`\x1b[?2004h` + `\x1b[?1002h`) before the next key. Verified to **fail on
  pre-fix `main`** (no `\x1b[?1004h`; no post-focus-in re-assert) and **pass after
  the fix**.

### Manual repro checklist (confirm on your machine)

Environment: WezTerm → SSH → tmux → `wg tui`, with `set -g focus-events on` in
tmux (`tmux show -g focus-events` to check; add to `~/.tmux.conf` if unset).

1. Start `wg tui` inside a tmux pane in your WezTerm+SSH session.
2. Alt-tab (or click) **away** from the WezTerm window to another app.
3. Alt-tab (or click) **back** to the WezTerm window.
4. **Immediately** type a normal key (e.g. `j`/`k` to move the selection, or `s`
   to cycle sort) — do *not* wait and do *not* press an arrow first.
5. Expected (fixed): the keystroke is handled by wg; the tmux session selector
   (`choose-tree`) does **not** pop and no key is swallowed.
6. Repeat a few times, and also try refocusing by **clicking** into the pane
   (exercises the SGR mouse-report path).
7. Regression sanity in the same session: bracketed paste still works (paste a
   multi-line block into a chat/launcher field — it arrives as one paste, not
   split), normal typing is unaffected, and focus-out has no visible effect.
