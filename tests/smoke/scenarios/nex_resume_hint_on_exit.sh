#!/usr/bin/env bash
# Scenario: nex_resume_hint_on_exit
#
# Pins `print-resume-command`: when an INTERACTIVE `wg nex` session exits
# cleanly (Ctrl-D/EOF, /quit, normal end-of-interactive), nex must print a
# one-line hint telling the human exactly how to resume THAT session with
# its REAL resolved id — never a placeholder:
#
#     Resume this session with:  wg nex --resume <actual-session-id>
#
# and, when the session is chat-bound (--chat / --chat-id), the chat-aware
# form:
#
#     Resume this session with:  wg nex --chat <ref> --resume
#
# The hint must NOT pollute non-interactive contracts:
#   * --eval-mode (stdout reserved for the JSON summary, stderr pristine)
#   * piped / scripted output (non-tty stdin/stderr — no human watching)
#
# This is a live human-flow guard: it drives a real PTY (kernel echo off,
# the way a terminal actually behaves), so it exercises the interactive
# exit path a human hits — not a CLI/unit substitute. It also proves the
# printed command is real by COPY-PASTING it back: the second run must
# resume the same session (same conversation.jsonl, both turns present).
#
# The fixture talks to a local fake OpenAI-compatible server, so it needs
# no credentials and no network.

set -euo pipefail

source "$(dirname "$0")/_helpers.sh"

require_wg

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON" "python3 is required for the PTY harness"
fi

scratch="$(make_scratch)"
transcript_dir="$scratch/transcripts"
mkdir -p "$transcript_dir"
results="$scratch/results.env"

(cd "$scratch" && wg init --no-agency >/dev/null)

if ! python3 - "$scratch" "$transcript_dir" "$results" <<'PY'; then
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

scratch, transcript_dir, results_path = sys.argv[1:4]

MODEL = "resume-hint-smoke-model"
COMMON = ["--no-mcp", "--minimal-tools", "--max-turns", "8", "-m", MODEL]

ansi_re = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")


def clean(text):
    text = ansi_re.sub("", text)
    text = text.replace("\r", "\n")
    return "".join(ch if ch in "\n\t" or ord(ch) >= 32 else "" for ch in text)


def fail(message, extra=""):
    print(f"nex_resume_hint_on_exit: {message}", file=sys.stderr)
    if extra:
        print("---- transcript ----", file=sys.stderr)
        print(clean(extra)[-4000:], file=sys.stderr)
    raise SystemExit(1)


# ── Fake OpenAI-compatible streaming server ─────────────────────────
# Every chat-completion request gets a tiny text reply that finishes with
# stop, so the agent loop ends each turn at a stable interactive boundary
# (EndTurn) and then waits for the next human input. No tool calls.
request_count = [0]


def sse(handler, obj, delay=0.01):
    payload = "data: " + json.dumps(obj, separators=(",", ":")) + "\n\n"
    handler.wfile.write(payload.encode("utf-8"))
    handler.wfile.flush()
    if delay:
        time.sleep(delay)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, _fmt, *_args):
        pass

    def do_POST(self):
        length = int(self.headers.get("content-length", "0") or "0")
        _ = self.rfile.read(length)
        request_count[0] += 1
        idx = request_count[0]

        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("connection", "close")
        self.end_headers()
        sse(self, {
            "id": f"smoke-{idx}",
            "choices": [{
                "index": 0,
                "delta": {"role": "assistant", "content": f"ACK_{idx} reply ready\n"},
                "finish_reason": None,
            }],
        })
        sse(self, {
            "id": f"smoke-{idx}",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 2},
        }, delay=0)
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
endpoint = f"http://127.0.0.1:{server.server_port}"


def base_env():
    env = os.environ.copy()
    env.pop("WG_FAKE_LLM", None)
    env.pop("NO_COLOR", None)
    env["TERM"] = "xterm-256color"
    # Use plain rustyline (not the live-input editor) so /quit and Ctrl-D
    # are read deterministically. The resume banner does not depend on the
    # live editor — only on stdin/stderr being a tty.
    env["WG_NEX_LIVE_INPUT"] = "0"
    return env


def run_pty(cmd, inputs, exit_method, label, settle=1.2):
    """Run `cmd` under a PTY, type each line in `inputs`, then exit via
    `exit_method` ('quit' → send /quit, 'eof' → send Ctrl-D). Returns the
    cleaned transcript."""
    import fcntl as _fcntl
    cmd = [endpoint if part == "__ENDPOINT__" else part for part in cmd]
    master, slave = pty.openpty()
    _fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
    attrs = termios.tcgetattr(slave)
    attrs[3] &= ~termios.ECHO
    termios.tcsetattr(slave, termios.TCSANOW, attrs)

    def child_setup():
        os.setsid()
        try:
            _fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
        except OSError:
            pass

    proc = subprocess.Popen(
        cmd, cwd=scratch, env=base_env(),
        stdin=slave, stdout=slave, stderr=slave,
        close_fds=True, preexec_fn=child_setup,
    )
    os.close(slave)
    os.set_blocking(master, False)
    raw = bytearray()

    def pump(timeout):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            ready, _, _ = select.select([master], [], [], 0.05)
            if not ready:
                if proc.poll() is not None:
                    try:
                        chunk = os.read(master, 65536)
                        if chunk:
                            raw.extend(chunk)
                            continue
                    except OSError:
                        pass
                    return
                continue
            try:
                chunk = os.read(master, 65536)
            except OSError:
                return
            if chunk:
                raw.extend(chunk)

    def text():
        return raw.decode("utf-8", errors="replace")

    def wait_for(needle, timeout):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if needle in clean(text()):
                return
            pump(0.2)
        fail(f"[{label}] timed out waiting for {needle!r}", text())

    def wait_for_acks(n, timeout):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if clean(text()).count("ACK_") >= n:
                return
            pump(0.2)
        fail(f"[{label}] timed out waiting for ack #{n}", text())

    def send(data):
        view = memoryview(data)
        deadline = time.monotonic() + 5
        while view:
            if time.monotonic() > deadline:
                fail(f"[{label}] timed out writing to PTY", text())
            _, w, _ = select.select([], [master], [], 0.1)
            if not w:
                continue
            n = os.write(master, view)
            view = view[n:]

    try:
        # Wait for the first idle prompt.
        wait_for(">", 15)
        for i, line in enumerate(inputs):
            send((line + "\n").encode("utf-8"))
            # Wait for that turn's ack before the next input so turns don't
            # collapse into one request.
            wait_for_acks(i + 1, 25)
            pump(0.4)
        pump(settle)
        if exit_method == "quit":
            send(b"/quit\n")
        elif exit_method == "eof":
            send(b"\x04")
        else:
            raise ValueError(exit_method)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline and proc.poll() is None:
            pump(0.2)
        if proc.poll() is None:
            os.killpg(proc.pid, signal.SIGTERM)
            fail(f"[{label}] nex did not exit after {exit_method}", text())
        pump(1.0)
        out = clean(text())
        with open(os.path.join(transcript_dir, f"{label}.txt"), "w") as f:
            f.write(out)
        if proc.returncode not in (0, None):
            fail(f"[{label}] nex exited with {proc.returncode}", out)
        return out
    finally:
        try:
            os.close(master)
        except OSError:
            pass


def run_pty_chat_release(ref_name, label, settle=1.5):
    """Start a chat-bound `wg nex --chat <ref>` under a PTY (the chat
    surface reads turns from inbox.jsonl, so there is no stdin prompt),
    then trigger the realistic clean-exit path a human uses for a
    chat-bound handler: `wg session release <ref>`. The handler's inbox
    read returns None → EOF → clean exit → resume banner. Returns the
    cleaned transcript."""
    import fcntl as _fcntl
    cmd = ["wg", "nex", "--chat", ref_name, *COMMON, "-e", endpoint]
    master, slave = pty.openpty()
    _fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
    attrs = termios.tcgetattr(slave)
    attrs[3] &= ~termios.ECHO
    termios.tcsetattr(slave, termios.TCSANOW, attrs)

    def child_setup():
        os.setsid()
        try:
            _fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
        except OSError:
            pass

    proc = subprocess.Popen(
        cmd, cwd=scratch, env=base_env(),
        stdin=slave, stdout=slave, stderr=slave,
        close_fds=True, preexec_fn=child_setup,
    )
    os.close(slave)
    os.set_blocking(master, False)
    raw = bytearray()

    def pump(timeout):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            ready, _, _ = select.select([master], [], [], 0.05)
            if not ready:
                if proc.poll() is not None:
                    try:
                        chunk = os.read(master, 65536)
                        if chunk:
                            raw.extend(chunk)
                            continue
                    except OSError:
                        pass
                    return
                continue
            try:
                chunk = os.read(master, 65536)
            except OSError:
                return
            if chunk:
                raw.extend(chunk)

    def text():
        return raw.decode("utf-8", errors="replace")

    try:
        # Wait for the startup banner — proves the handler is up and (in
        # chat mode) blocked on the inbox.
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline and "interactive session with" not in clean(text()):
            pump(0.2)
        if "interactive session with" not in clean(text()):
            fail(f"[{label}] chat-bound nex never printed its startup banner", text())
        # Wait for the handler lock so `wg session release` finds a live
        # holder to signal.
        lock = os.path.join(scratch, ".wg", "chat", ref_name, ".handler.pid")
        ldeadline = time.monotonic() + 10
        while time.monotonic() < ldeadline and not os.path.exists(lock):
            pump(0.2)
        pump(settle)
        rel = subprocess.run(
            ["wg", "session", "release", ref_name, "--wait", "12"],
            cwd=scratch, env=base_env(),
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30,
        )
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline and proc.poll() is None:
            pump(0.2)
        if proc.poll() is None:
            os.killpg(proc.pid, signal.SIGTERM)
            fail(f"[{label}] chat-bound nex did not exit after release. "
                 f"release stderr: {rel.stderr.decode('utf-8', 'replace')}", text())
        pump(1.0)
        out = clean(text())
        with open(os.path.join(transcript_dir, f"{label}.txt"), "w") as f:
            f.write(out)
        return out
    finally:
        try:
            os.close(master)
        except OSError:
            pass


def run_piped(cmd, message, label):
    """Run `cmd <message>` with stdin=/dev/null and stdout/stderr captured
    as pipes (non-tty). Returns (stdout, stderr, returncode)."""
    cmd = [endpoint if part == "__ENDPOINT__" else part for part in cmd]
    proc = subprocess.run(
        cmd + [message], cwd=scratch, env=base_env(),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        timeout=60,
    )
    out = proc.stdout.decode("utf-8", errors="replace")
    err = proc.stderr.decode("utf-8", errors="replace")
    with open(os.path.join(transcript_dir, f"{label}.out.txt"), "w") as f:
        f.write(out)
    with open(os.path.join(transcript_dir, f"{label}.err.txt"), "w") as f:
        f.write(err)
    return out, err, proc.returncode


RESUME_RE = re.compile(r"Resume this session with:\s+(wg nex --resume \S+)")
CHAT_RESUME_RE = re.compile(r"Resume this session with:\s+wg nex --chat (\S+) --resume")
PLACEHOLDERS = ("<id>", "<session", "<actual", "<pattern>", "PATTERN", "placeholder")

results = {}

# ── Case 1: fresh interactive session, /quit → resume hint with real id ──
t1 = run_pty(["wg", "nex", *COMMON, "-e", "__ENDPOINT__"],
             inputs=["FIRST_SESSION_MARKER"], exit_method="quit", label="fresh_quit")
m = RESUME_RE.search(t1)
if not m:
    fail("interactive /quit exit did not print a 'Resume this session with: wg nex --resume <id>' hint", t1)
resume_cmd = m.group(1)
resume_id = resume_cmd.split()[-1]
if not resume_id or any(p in resume_id for p in PLACEHOLDERS):
    fail(f"resume id looks like a placeholder, not a real id: {resume_id!r}", t1)
results["RESUME_ID"] = resume_id

# ── Case 2: copy-paste the printed command → it actually resumes ─────────
# Run the EXACT printed `wg nex --resume <id>` (plus the test endpoint/model
# wiring) and a fresh marker. It must announce it is RESUMING the session.
t2 = run_pty(["wg", "nex", "--resume", resume_id, *COMMON, "-e", "__ENDPOINT__"],
             inputs=["SECOND_SESSION_MARKER"], exit_method="quit", label="resume_roundtrip")
if "resuming session" not in t2:
    fail(f"copy-pasted `wg nex --resume {resume_id}` did not resume the prior session "
         f"(no 'resuming session' banner)", t2)

# ── Case 3: chat-bound session prints the chat-aware form ────────────────
t3 = run_pty_chat_release("smoke-handle", label="chat_bound")
cm = CHAT_RESUME_RE.search(t3)
if not cm:
    fail("chat-bound session did not print the chat-aware resume hint "
         "'wg nex --chat <ref> --resume'", t3)
if cm.group(1) != "smoke-handle":
    fail(f"chat-aware resume hint named the wrong ref: {cm.group(1)!r} (want 'smoke-handle')", t3)
if RESUME_RE.search(t3):
    fail("chat-bound session printed the plain --resume form instead of the chat-aware form", t3)

# ── Case 4: fresh interactive session, Ctrl-D/EOF → resume hint ──────────
t4 = run_pty(["wg", "nex", *COMMON, "-e", "__ENDPOINT__"],
             inputs=["EOF_MARKER"], exit_method="eof", label="fresh_eof")
if not RESUME_RE.search(t4):
    fail("Ctrl-D/EOF exit did not print the 'wg nex --resume <id>' hint", t4)

# ── Case 5: piped / non-tty output is NOT polluted by the banner ─────────
out5, err5, rc5 = run_piped(["wg", "nex", *COMMON, "-e", "__ENDPOINT__"],
                            message="PIPED_MARKER", label="piped")
combined5 = out5 + err5
if "Resume this session with" in combined5:
    fail("piped / non-tty `wg nex` leaked the interactive resume banner", combined5)

# ── Case 6: --eval-mode output is NOT polluted; JSON summary intact ──────
out6, err6, rc6 = run_piped(["wg", "nex", "--eval-mode", *COMMON, "-e", "__ENDPOINT__"],
                            message="EVAL_MARKER", label="eval")
if "Resume this session with" in (out6 + err6):
    fail("--eval-mode leaked the interactive resume banner", out6 + err6)
if '"status"' not in out6:
    fail("--eval-mode did not emit its stdout JSON summary (eval contract broken)", out6 + err6)

with open(results_path, "w") as f:
    for k, v in results.items():
        f.write(f"{k}={v}\n")

server.shutdown()
print("python harness: all resume-hint cases passed")
PY
    loud_fail "nex_resume_hint_on_exit PTY harness failed. Transcripts under: $transcript_dir"
fi

# ── Journal-level proof that the copy-pasted resume reused the SAME session ──
# Both the first session's marker and the resumed session's marker must live
# in ONE conversation.jsonl — that is the on-disk proof that `--resume <id>`
# pointed at the same session, not a fresh one.
joined_journal=""
while IFS= read -r journal; do
    if grep -q "FIRST_SESSION_MARKER" "$journal" 2>/dev/null \
        && grep -q "SECOND_SESSION_MARKER" "$journal" 2>/dev/null; then
        joined_journal="$journal"
        break
    fi
done < <(find "$scratch/.wg/chat" -name conversation.jsonl 2>/dev/null)

if [[ -z "$joined_journal" ]]; then
    tree="$(find "$scratch/.wg/chat" -name conversation.jsonl -exec sh -c 'echo "== $1 =="; tail -5 "$1"' _ {} \; 2>/dev/null)"
    loud_fail "copy-pasted resume did NOT land in the same session journal — \
no single conversation.jsonl holds both FIRST_SESSION_MARKER and SECOND_SESSION_MARKER.
$tree"
fi

echo "PASS: interactive wg nex prints a real resume command on clean exit (EOF + /quit + chat-bound), copy-paste resumes the same session, and piped/--eval-mode output stays clean"
