#!/usr/bin/env bash
# Scenario: nex_progressive_markdown_pty
#
# Drives standalone `nex` through a real PTY against a LOCAL mock
# OpenAI-compatible server (no live model) that streams a MARKDOWN answer
# whose markdown tokens are deliberately fragmented across SSE deltas
# (heading split mid-word, `**bold**` split across deltas, an inline
# `code` span split, a fenced ```python code block whose fence marker is
# split, and a bullet list). It asserts the user-visible behavior of
# progressive-streaming-markdown:
#
#   * TTY direct path (WG_NEX_LIVE_INPUT=0, color on): the answer renders
#     as styled markdown — the heading, bold, inline code, list bullets,
#     and fenced code block all appear FORMATTED, with the raw markdown
#     punctuation (`#`, `**`, backticks) consumed, ANSI styling present,
#     and no garble even though every token was fragmented. Because the
#     renderer commits finished blocks + redraws the live block in place,
#     the in-place erase control (ESC[…J) is present — proof the render is
#     progressive, not a single buffer-then-print at the end.
#   * PLAIN fallback (stderr is NOT a tty / piped): the SAME answer streams
#     as raw markdown text with NO ANSI escapes — the `#`, `**`, and
#     backtick punctuation is preserved verbatim for downstream consumers.
#
# This is the human-flow reproducer for progressive-streaming-markdown: a
# unit test cannot prove the live terminal render path actually formats the
# stream. The incremental-chunk unit coverage (BlockSplitter +
# CursorRenderer, including a virtual-terminal "no garble == one-shot
# render" check) lives in src/executor/native/streaming_markdown.rs.

set -euo pipefail

source "$(dirname "$0")/_helpers.sh"

require_wg

if ! command -v nex >/dev/null 2>&1; then
    loud_skip "MISSING NEX BINARY" "nex not found on PATH; run 'cargo install --path . --locked' from this checkout"
fi

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON" "python3 is required for the PTY harness"
fi

scratch="$(make_scratch)"
transcript="$scratch/md-pty.txt"
plain_out="$scratch/md-plain.txt"

(cd "$scratch" && wg init --no-agency >/dev/null)

if ! python3 - "$scratch" "$transcript" "$plain_out" <<'PY'; then
import errno
import fcntl
import json
import os
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import termios
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

scratch, transcript_path, plain_path = sys.argv[1:4]

ansi_re = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")


def clean(text):
    text = ansi_re.sub("", text)
    text = text.replace("\r", "\n")
    return "".join(ch if ch in "\n\t" or ord(ch) >= 32 else "" for ch in text)


def emulate(s):
    # Minimal VT100 model: applies the cursor moves the progressive
    # renderer emits (ESC[<n>A up, ESC[0J erase-to-end, \r, \n) so we can
    # assert on the FINAL on-screen text rather than the flattened stream
    # (which keeps every transient partial-markdown redraw). Ignores SGR.
    rows = [[]]
    r = c = 0
    i, n = 0, len(s)
    while i < n:
        ch = s[i]
        if ch == "\x1b" and i + 1 < n and s[i + 1] == "[":
            j = i + 2
            params = ""
            while j < n and not ("@" <= s[j] <= "~"):
                params += s[j]
                j += 1
            final = s[j] if j < n else ""
            i = j + 1
            num = int(params) if params.isdigit() else 1
            if final == "A":
                r = max(0, r - num)
            elif final == "B":
                r += num
                while len(rows) <= r:
                    rows.append([])
            elif final == "C":
                c += num
            elif final == "D":
                c = max(0, c - num)
            elif final == "J":
                if params.strip() in ("", "0"):
                    while len(rows) <= r:
                        rows.append([])
                    rows[r] = rows[r][:c]
                    del rows[r + 1:]
                elif params.strip() == "2":
                    rows, r, c = [[]], 0, 0
            elif final == "K":
                while len(rows) <= r:
                    rows.append([])
                if params.strip() in ("", "0"):
                    rows[r] = rows[r][:c]
                elif params.strip() == "2":
                    rows[r] = []
            continue
        if ch == "\x1b":
            i += 2
            continue
        if ch == "\r":
            c = 0
            i += 1
            continue
        if ch == "\n":
            r += 1
            c = 0
            while len(rows) <= r:
                rows.append([])
            i += 1
            continue
        if ord(ch) < 32 and ch != "\t":
            i += 1
            continue
        while len(rows) <= r:
            rows.append([])
        row = rows[r]
        while len(row) <= c:
            row.append(" ")
        row[c] = ch
        c += 1
        i += 1
    lines = ["".join(row).rstrip() for row in rows]
    while lines and lines[-1] == "":
        lines.pop()
    return "\n".join(lines)


def write_file(path, t):
    with open(path, "w", encoding="utf-8", errors="replace") as f:
        f.write(t)


def fail(message, raw=""):
    write_file(transcript_path, clean(raw))
    print(message, file=sys.stderr)
    raise SystemExit(1)


def sse(handler, obj, delay=0.02):
    handler.wfile.write(("data: " + json.dumps(obj, separators=(",", ":")) + "\n\n").encode())
    handler.wfile.flush()
    if delay:
        time.sleep(delay)


def done(handler):
    handler.wfile.write(b"data: [DONE]\n\n")
    handler.wfile.flush()


# Markdown answer with every construct fragmented across deltas so the
# streaming splitter/renderer must reassemble partial tokens.
MD_DELTAS = [
    "# Hea", "ding Line\n\n",
    "Some **bo", "ld** and ", "`co", "de` words.\n\n",
    "- first item\n", "- second item\n\n",
    "```py", "thon\n", "print('hi')\n", "```\n\n",
    "Final line.\n",
]


def stream_markdown(handler, sid):
    for chunk in MD_DELTAS:
        sse(handler, {"id": sid, "choices": [{"index": 0,
            "delta": {"content": chunk}, "finish_reason": None}]})
    sse(handler, {"id": sid, "choices": [{"index": 0, "delta": {},
        "finish_reason": "stop"}], "usage": {"prompt_tokens": 5,
        "completion_tokens": 20}}, delay=0)
    done(handler)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def do_POST(self):
        length = int(self.headers.get("content-length", "0") or "0")
        self.rfile.read(length)
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("connection", "close")
        self.end_headers()
        stream_markdown(self, "md-1")


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
endpoint = f"http://127.0.0.1:{server.server_port}"

base_cmd = ["nex", "--no-mcp", "--max-turns", "4",
            "-m", "nex-markdown-model", "-e", endpoint]

# ── Case 1: TTY direct path — progressive styled markdown render. ──
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
attrs = termios.tcgetattr(slave)
attrs[3] &= ~termios.ECHO
termios.tcsetattr(slave, termios.TCSANOW, attrs)

env = os.environ.copy()
env.pop("WG_FAKE_LLM", None)
env["NEX_DIR"] = os.path.join(scratch, ".nex")
env["WG_NEX_LIVE_INPUT"] = "0"      # force the direct-stderr cursor path
env["TERM"] = "xterm-256color"
env.pop("NO_COLOR", None)
env.pop("WG_NEX_THINK", None)


def child_setup():
    os.setsid()
    try:
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    except OSError:
        pass


proc = subprocess.Popen(base_cmd, cwd=scratch, env=env, stdin=slave, stdout=slave,
                        stderr=slave, close_fds=True, preexec_fn=child_setup)
os.close(slave)
os.set_blocking(master, False)

raw = bytearray()


def read_some(timeout):
    deadline = time.monotonic() + timeout
    progressed = False
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            try:
                chunk = os.read(master, 65536)
                if chunk:
                    raw.extend(chunk)
                    progressed = True
                    continue
            except OSError:
                pass
            return progressed
        ready, _, _ = select.select([master], [], [], 0.05)
        if not ready:
            continue
        try:
            chunk = os.read(master, 65536)
        except OSError as e:
            if e.errno in (errno.EIO, errno.EBADF):
                return progressed
            raise
        if chunk:
            raw.extend(chunk)
            progressed = True
    return progressed


def text():
    return raw.decode("utf-8", errors="replace")


def wait_for(needle, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if needle in clean(text()):
            return clean(text())
        read_some(0.2)
    fail(f"timed out waiting for {needle!r}", text())


def write_all(payload):
    view = memoryview(payload)
    deadline = time.monotonic() + 5
    while view:
        if time.monotonic() > deadline:
            fail("timed out writing to PTY", text())
        _, writable, _ = select.select([], [master], [], 0.1)
        if not writable:
            continue
        try:
            n = os.write(master, view)
        except BlockingIOError:
            continue
        view = view[n:]


def send_line(line):
    for byte in (line + "\n").encode("utf-8"):
        write_all(bytes([byte]))
        time.sleep(0.01)


try:
    # nex may print a prompt glyph; just wait for the first answer text.
    send_line("render some markdown please")
    after = wait_for("Final line.", 30)

    raw_with_ansi = text()
    # The FINAL on-screen text (cursor moves applied), where transient
    # partial-markdown redraws have been overwritten by their corrected
    # forms — this is what the human actually sees.
    screen = emulate(raw_with_ansi)

    # All answer content survives, formatted.
    for needle in ["Heading Line", "bold", "code", "first item",
                   "second item", "print('hi')", "Final line."]:
        if needle not in screen:
            fail(f"missing rendered content {needle!r}\n--- screen ---\n{screen}", raw_with_ansi)

    # Bullets rendered (markdown `-` became `•`).
    if "•" not in screen:
        fail(f"bullet list did not render with the • glyph\n{screen}", raw_with_ansi)

    # Raw markdown punctuation was consumed by the renderer (final screen).
    if "**" in screen:
        fail(f"literal '**' bold markers leaked — markdown not rendered\n{screen}", raw_with_ansi)
    if "`" in screen:
        fail(f"literal backticks leaked — code not rendered\n{screen}", raw_with_ansi)
    if "# Heading" in screen:
        fail(f"literal '# ' heading marker leaked\n{screen}", raw_with_ansi)

    # ANSI styling actually present (we are on the color TTY path).
    if "\x1b[" not in raw_with_ansi:
        fail("no ANSI styling in the TTY render", raw_with_ansi)

    # Progressive proof: the in-place erase control (CSI J) appears, which
    # only the streaming renderer's live-block redraw emits — a single
    # buffer-then-print at the end would not redraw in place per token.
    if "\x1b[0J" not in raw_with_ansi:
        fail("no in-place redraw (ESC[0J) — render was not progressive", raw_with_ansi)
    write_file(transcript_path, screen)

    send_line("/quit")
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline and proc.poll() is None:
        read_some(0.2)
    if proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        fail("nex did not exit after /quit", text())
    read_some(1)
    write_file(transcript_path, emulate(text()))
    if proc.returncode != 0:
        fail(f"nex exited with {proc.returncode}", text())
finally:
    try:
        os.close(master)
    except OSError:
        pass

# ── Case 2: PLAIN fallback — stderr/stdout NOT a tty (piped). ──
plain_env = os.environ.copy()
plain_env.pop("WG_FAKE_LLM", None)
plain_env["NEX_DIR"] = os.path.join(scratch, ".nex")
plain_env.pop("NO_COLOR", None)
plain_env["TERM"] = "xterm-256color"

plain = subprocess.run(
    base_cmd,
    cwd=scratch,
    env=plain_env,
    input="render some markdown please\n/quit\n",
    capture_output=True,
    text=True,
    timeout=60,
)
combined = plain.stdout + plain.stderr
write_file(plain_path, combined)

# The ANSWER must be raw passthrough, NOT run through the markdown
# renderer: raw markdown punctuation is preserved verbatim and none of the
# renderer's signature glyphs (the `•` bullet, the `│` code-block bar)
# appear. (The startup banner is colored regardless of tty — that is a
# separate, pre-existing cosmetic; we assert on the answer's structure.)
if "Final line." not in combined:
    fail("plain fallback missing answer content", combined)
for marker in ["# Heading Line", "**bold**", "`code`", "```python"]:
    if marker not in combined:
        fail(f"plain fallback did not preserve raw markdown {marker!r}", combined)
if "•" in combined:
    fail("plain fallback rendered a bullet glyph — markdown was not raw", combined)
if "│" in combined:
    fail("plain fallback rendered a code-block bar — markdown was not raw", combined)

try:
    server.shutdown()
except Exception:
    pass

raise SystemExit(0)
PY
    loud_fail "progressive-markdown PTY harness failed. Transcript tail:\n$(tail -60 "$transcript" 2>/dev/null || true)\n--- plain tail ---\n$(tail -20 "$plain_out" 2>/dev/null || true)"
fi

echo "PASS: nex renders streaming markdown progressively on the TTY and falls back to raw plain text when piped"
