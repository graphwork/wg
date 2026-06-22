#!/usr/bin/env bash
# Scenario: tui_focus_in_reasserts_input_grab (bug-tui-focus-fix)
#
# Pins the general fix for the "wg TUI leaks first keystrokes to tmux after OS
# focus-in" bug (docs/bugs/tui-focus-input-race.md). The wg TUI used to set its
# terminal input grab exactly once at startup and be structurally blind to
# focus changes, so returning OS focus to the window (alt-tab / click) left a
# window in which the user's first keystroke was parsed by tmux instead of wg
# (popping choose-tree, etc.).
#
# The fix has two halves, both directly observable in the TUI's raw output
# stream under a real PTY (no tmux/emulator needed — the assertions are at wg's
# own layer):
#
#   Defect 2 (subscribe to focus): wg now enables DECSET 1004 focus reporting
#     at startup — the byte \x1b[?1004h appears in the startup output.
#   Defect 1 (re-assert grab on focus-in): when wg receives a focus-in report
#     (\x1b[I), it re-emits its own input grab as one burst — bracketed paste
#     (\x1b[?2004h) and mouse capture (\x1b[?1002h) — before the next key.
#
# Both bytes are ABSENT on the pre-fix `main`:
#   * main never emits \x1b[?1004h (no EnableFocusChange anywhere in src), and
#   * main drops Event::FocusGained via a catch-all arm, so a focus-in triggers
#     no re-assert at all (no second \x1b[?2004h, no re-emitted \x1b[?1002h).
# So this scenario fails on main and passes after the fix.
#
# Idle TUI redraws never emit DECSET mode-set sequences (ratatui emits only
# cursor/style/text), so seeing \x1b[?2004h *after* the focus-in injection is an
# unambiguous signal of the re-assert, not a coincidental repaint.
#
# Exit 0  = PASS
# Exit 77 = SKIP (no python3 / no wg)
# Exit 1  = FAIL

set -euo pipefail

source "$(dirname "$0")/_helpers.sh"

require_wg

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON" "python3 is required for the PTY harness"
fi

WG_BIN="$(command -v wg)"

scratch="$(make_scratch)"
(cd "$scratch" && wg init --no-agency >/dev/null)

graph_dir="$(graph_dir_in "$scratch")" || loud_fail "no .wg dir under $scratch after wg init"

if ! python3 - "$WG_BIN" "$graph_dir" <<'PY'; then
import fcntl
import os
import pty
import select
import struct
import sys
import termios
import time

wg_bin, graph_dir = sys.argv[1:3]

FOCUS_ON = b"\x1b[?1004h"      # DECSET 1004 — focus reporting (Defect 2 fix)
ALT_SCREEN = b"\x1b[?1049h"    # alternate screen — proves the TUI actually started
PASTE_ON = b"\x1b[?2004h"      # bracketed paste enable (re-asserted on focus-in)
MOUSE_ON = b"\x1b[?1002h"      # mouse button tracking (re-asserted on focus-in)

pid, fd = pty.fork()
if pid == 0:
    # Child: become the wg TUI on a real PTY slave.
    os.environ["TERM"] = "xterm-256color"
    # Strip any inherited agent/project pins so --dir is authoritative.
    for var in ("WG_DIR", "WG_PROJECT_ROOT", "WG_WORKTREE_PATH", "WG_TASK_ID"):
        os.environ.pop(var, None)
    os.execvp(wg_bin, [wg_bin, "--dir", graph_dir, "tui"])
    os._exit(127)

# Parent: drive + observe the master side.
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))

buf = bytearray()


def drain(seconds):
    """Read whatever the TUI emits for `seconds`, appending to `buf`."""
    end = time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.2)
        if fd in r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                return False
            if not data:
                return False
            buf.extend(data)
    return True


def teardown(rc, msg=""):
    if msg:
        sys.stderr.write(msg + "\n")
    try:
        os.write(fd, b"q")
        time.sleep(0.3)
        os.write(fd, b"\x03")  # Ctrl-C fallback
    except OSError:
        pass
    try:
        os.kill(pid, 9)
    except OSError:
        pass
    try:
        os.waitpid(pid, 0)
    except OSError:
        pass
    sys.exit(rc)


# Phase 1 — startup. Generous window: supports_keyboard_enhancement() blocks
# ~2s on a PTY with no responder, and an initial render animation follows.
drain(8.0)
startup = bytes(buf)

if ALT_SCREEN not in startup:
    teardown(
        1,
        "wg tui never entered the alternate screen — the TUI failed to start "
        "(captured {} bytes)".format(len(startup)),
    )

if FOCUS_ON not in startup:
    teardown(
        1,
        "REGRESSION (Defect 2): wg tui did not enable focus reporting "
        "(DECSET 1004 / \\x1b[?1004h) at startup, so tmux will never forward "
        "focus-in into the pane and the grab can never be re-asserted.",
    )

# Phase 2 — focus-in. Mark the boundary, inject a focus-in report exactly as a
# terminal would after OS focus returns, then watch for the re-assert burst.
boundary = len(buf)
os.write(fd, b"\x1b[I")  # focus-in report, exactly as a terminal sends on OS focus-in
drain(4.0)
post = bytes(buf)[boundary:]

if PASTE_ON not in post:
    teardown(
        1,
        "REGRESSION (Defect 1): focus-in (\\x1b[I) did not trigger an input-grab "
        "re-assert — bracketed paste (\\x1b[?2004h) was not re-emitted after "
        "focus returned. The first post-focus-in keystroke can leak to tmux. "
        "(post-focus-in bytes: {})".format(len(post)),
    )

if MOUSE_ON not in post:
    teardown(
        1,
        "focus-in re-assert is incomplete: mouse capture (\\x1b[?1002h) was not "
        "re-emitted alongside bracketed paste on focus-in.",
    )

sys.stderr.write(
    "PASS: startup enabled DECSET 1004; focus-in re-asserted paste + mouse "
    "grab ({} startup bytes, {} post-focus-in bytes)\n".format(len(startup), len(post))
)
teardown(0)
PY
    loud_fail "focus-in input-grab re-assert assertions failed (see harness output above)"
fi

echo "=== tui_focus_in_reasserts_input_grab: PASS ==="
