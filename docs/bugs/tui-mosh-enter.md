# Mosh plain Enter parsed as Shift+Enter in the TUI Chat composer

## Symptom

Through `mosh` (including `mosh -> tmux -> wg tui`), an unmodified physical
Enter could periodically insert a newline in the native Chat composer instead
of submitting. The next Enter then acted on the now-multiline draft, producing
the reported Backspace/Enter/Enter recovery pattern. Other TUI panels did not
show the same symptom.

## Root-cause capture

`tests/smoke/scenarios/tui_chat_mosh_plain_enter_pty.sh` reproduces the failure
at WG's real outer PTY boundary without credentials. It sets
`MOSH_SERVER_PID`, `MOSH_IP`, and a Kitty-capable `TERM`, then models the exact
server-side bytes observed for the misclassified physical Enter:

```text
raw bytes reaching wg:  1b 5b 31 33 3b 32 75   (CSI 13;2u)
crossterm parse:        KeyCode::Enter, modifiers=SHIFT, kind=Press
active route:           native_composer (InputMode::ChatInput)
```

The event trace now records both the original parsed event and its active Chat
owner. A captured event has this shape (timestamps/task selection omitted):

```json
{"event":{"type":"Key","code":"Enter","modifiers":"Shift"},"state":{"focused_panel":"right_panel","right_panel_tab":"chat","input_mode":"ChatInput","chat_input_route":"native_composer"}}
```

Tracing occurs before capability normalization, so it preserves the evidence
that crossterm parsed `Shift`; key feedback and routing use the normalized
event.

The old startup sequence unconditionally called
`supports_keyboard_enhancement()` when the advertised terminal answered, then
pushed Kitty `DISAMBIGUATE_ESCAPE_CODES`. A positive answer only established
that the terminal beyond mosh understood Kitty keyboard enhancement. It did
**not** establish that mosh was a byte-reliable enhanced-keyboard transport.
That made an unreliable Shift distinction authoritative. The native composer
then intentionally matched `Enter + SHIFT` as its multiline chord.

This is Chat-composer-specific because the three Chat input routes do different
things:

1. **Native composer** (`InputMode::ChatInput`) interprets negotiated
   Shift+Enter as a literal newline and plain Enter as submit. This was the
   visible failure route.
2. **Startup buffer** queues the parsed `KeyEvent` until the embedded pane
   attaches. It must not retain an untrusted Shift bit or manufacture a second
   Enter when flushing.
3. **Embedded vendor PTY** (Pi, Codex, Claude, Nex, etc.) forwards Enter through
   `PtyPane::send_key`; `key_event_to_bytes` emits exactly one CR (`0d`), never
   CR/LF. It does not use the native composer newline mapping.

Plain tmux is not the cause: tmux can forward negotiated extended keys. The
outer transport remains mosh even when a mosh session starts or attaches tmux,
and the mosh environment markers survive that hop.

## Fix and policy

WG now decides one centralized outer-keyboard policy at TUI startup:

- `MOSH_SERVER_PID` or `MOSH_IP`: do not query, push, reassert, or pop Kitty
  keyboard enhancement.
- recording/asciinema: keep the existing no-query policy.
- non-mosh terminals, including ordinary tmux: directly request keyboard
  enhancement without a synchronous capability query. Supporting terminals
  enable it and other ANSI terminals ignore the sequence; this avoids blocking
  the first frame and input for up to two seconds when no query reply arrives.

At the single outer event boundary, WG removes `SHIFT` from Enter unless
keyboard enhancement was requested over a reliable transport.
This normalization happens before all three Chat routes, not in the composer.
Other modifier bits and key event metadata are preserved. Under mosh,
Shift+Enter is therefore deliberately not claimed as distinguishable; Ctrl+J
(and Alt+Enter) remain multiline chords, and the action bar omits the
Shift+Enter hint. Outside mosh, enhanced Shift+Enter remains a newline.

Teardown now pops Kitty flags only when WG successfully pushed them, avoiding a
side effect on an ancestor terminal/tmux stack when policy skipped negotiation.
The policy is computed once from startup environment; no probe, filesystem
access, runtime chat metadata, or relaunch is introduced on input/render.

## Regression coverage

- Pure policy tests cover `MOSH_SERVER_PID`, `MOSH_IP`, ordinary tmux,
  tmux-over-mosh, recording, and reliable non-mosh enablement. The
  mosh/recording branches never request keyboard enhancement.
- Event tests cover native-composer normalization, startup-buffer
  normalization, and a credential-free Pi/vendor `PtyPane` stand-in that
  receives exactly one byte for Enter with no queued late duplicate.
- `tui_chat_mosh_plain_enter_pty` sends twelve literal CR Enter events plus
  twelve repeated misclassified Enter events and proves every message is
  exactly-once and in order, with no newline or late duplicate; it also proves
  Ctrl+J multiline input.
- `tui_chat_composer_newlines_pty` observes the direct Kitty request outside
  mosh and proves Shift+Enter and Ctrl+J insert newlines while plain Enter
  submits once.
