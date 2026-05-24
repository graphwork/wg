#!/usr/bin/env bash
# Scenario: nex_live_input_pty
#
# Drives standalone `nex` and `wg nex` through a real PTY against a local
# OpenAI-compatible test server. The transcript covers:
#   * a first user turn that triggers a real bash tool call
#   * a normal stable-boundary follow-up prompt, rendered as plain `>`
#   * a third line typed while the assistant is streaming, visibly marked
#     as queued and delivered once as the next turn
#
# The PTY has kernel echo disabled. Live rustyline must therefore own the
# editable input area; otherwise text typed while output is active would be
# invisible or lost.

set -euo pipefail

source "$(dirname "$0")/_helpers.sh"

require_wg

if ! command -v nex >/dev/null 2>&1; then
    loud_skip "MISSING NEX BINARY" "nex not found on PATH; run 'cargo install --path . --locked' from this checkout"
fi

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON" "python3 is required for the PTY harness"
fi

run_case() {
    local label="$1"
    shift
    local scratch transcript
    scratch="$(make_scratch)"
    transcript="$scratch/pty-transcript-${label// /-}.txt"

    (cd "$scratch" && wg init --no-agency >/dev/null)

    if ! python3 - "$scratch" "$transcript" "$label" "$@" <<'PY'; then
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

scratch, transcript_path, label = sys.argv[1:4]
cmd = sys.argv[4:]
first_turn = "first tool prompt smoke"
stable_followup = "stable follow-up after tool"
queued_turn = "SECOND_QUEUED_SMOKE while stream active"
first_header = f"[assistant for: {first_turn}]"
stable_header = f"[assistant for: {stable_followup}]"
queued_header = f"[assistant for: {queued_turn}]"

ansi_re = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")


def clean(text):
    text = ansi_re.sub("", text)
    text = text.replace("\r", "\n")
    return "".join(ch if ch == "\n" or ch == "\t" or ord(ch) >= 32 else "" for ch in text)


def compact_marker(text):
    return "".join(ch for ch in clean(text) if ch.isalnum() or ch == "_")


def contains_marker(haystack, needle):
    if needle == ">":
        return needle in clean(haystack)
    cleaned = clean(haystack)
    return needle in cleaned or compact_marker(needle) in compact_marker(cleaned)


def write_transcript(transcript):
    with open(transcript_path, "w", encoding="utf-8", errors="replace") as f:
        f.write(transcript)


def fail(message, transcript=""):
    write_transcript(clean(transcript))
    print(f"{label}: {message}", file=sys.stderr)
    raise SystemExit(1)


request_bodies = []


def sse(handler, obj, delay=0.015):
    payload = "data: " + json.dumps(obj, separators=(",", ":")) + "\n\n"
    handler.wfile.write(payload.encode("utf-8"))
    handler.wfile.flush()
    if delay:
        time.sleep(delay)


def done(handler):
    handler.wfile.write(b"data: [DONE]\n\n")
    handler.wfile.flush()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, _fmt, *_args):
        pass

    def do_POST(self):
        length = int(self.headers.get("content-length", "0") or "0")
        raw = self.rfile.read(length)
        try:
            body = json.loads(raw.decode("utf-8"))
        except Exception:
            body = {}
        request_bodies.append(body)
        idx = len(request_bodies)

        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("connection", "close")
        self.end_headers()

        if idx == 1:
            sse(self, {
                "id": "smoke-1",
                "choices": [{
                    "index": 0,
                    "delta": {"role": "assistant", "content": "FIRST_TOOL_INTRO\n"},
                    "finish_reason": None,
                }],
            })
            sse(self, {
                "id": "smoke-1",
                "choices": [{
                    "index": 0,
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call_bash_1",
                            "type": "function",
                            "function": {"name": "bash", "arguments": ""},
                        }]
                    },
                    "finish_reason": None,
                }],
            })
            sse(self, {
                "id": "smoke-1",
                "choices": [{
                    "index": 0,
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "function": {"arguments": json.dumps({"command": "printf TOOL_RESULT_MARKER"})},
                        }]
                    },
                    "finish_reason": None,
                }],
            })
            sse(self, {
                "id": "smoke-1",
                "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
                "usage": {"prompt_tokens": 3, "completion_tokens": 2},
            }, delay=0)
            done(self)
            return

        if idx == 2:
            sse(self, {
                "id": "smoke-2",
                "choices": [{
                    "index": 0,
                    "delta": {"role": "assistant", "content": "FIRST_DONE_MARKER tool result observed\n"},
                    "finish_reason": None,
                }],
            })
            sse(self, {
                "id": "smoke-2",
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 4, "completion_tokens": 2},
            }, delay=0)
            done(self)
            return

        if idx == 3:
            sse(self, {
                "id": "smoke-3",
                "choices": [{
                    "index": 0,
                    "delta": {"role": "assistant", "content": "SECOND_RESPONSE_MARKER\n"},
                    "finish_reason": None,
                }],
            })
            for i in range(140):
                sse(self, {
                    "id": "smoke-3",
                    "choices": [{
                        "index": 0,
                        "delta": {"content": f"streaming filler {i:03d}: queued input should remain editable\n"},
                        "finish_reason": None,
                    }],
                }, delay=0.01)
            sse(self, {
                "id": "smoke-3",
                "choices": [{
                    "index": 0,
                    "delta": {"content": "SECOND_DONE_MARKER\n"},
                    "finish_reason": None,
                }],
            })
            sse(self, {
                "id": "smoke-3",
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 5, "completion_tokens": 4},
            }, delay=0)
            done(self)
            return

        sse(self, {
            "id": "smoke-4",
            "choices": [{
                "index": 0,
                "delta": {"role": "assistant", "content": "QUEUED_RESPONSE_MARKER queued turn received\n"},
                "finish_reason": None,
            }],
        })
        sse(self, {
            "id": "smoke-4",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 6, "completion_tokens": 2},
        }, delay=0)
        done(self)


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
endpoint = f"http://127.0.0.1:{server.server_port}"
cmd = [endpoint if part == "__NEX_ENDPOINT__" else part for part in cmd]

master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 120, 0, 0))

# Disable kernel echo before spawning. Live rustyline still paints the
# editable buffer itself; a boundary-only REPL has nothing to paint while
# the model stream is in flight.
attrs = termios.tcgetattr(slave)
attrs[3] &= ~termios.ECHO
termios.tcsetattr(slave, termios.TCSANOW, attrs)

env = os.environ.copy()
env.pop("WG_FAKE_LLM", None)
env["NEX_DIR"] = os.path.join(scratch, ".nex")
env["WG_NEX_LIVE_INPUT"] = "1"
env.setdefault("TERM", "xterm-256color")


def child_setup():
    os.setsid()
    try:
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    except OSError:
        pass


proc = subprocess.Popen(
    cmd,
    cwd=scratch,
    env=env,
    stdin=slave,
    stdout=slave,
    stderr=slave,
    close_fds=True,
    preexec_fn=child_setup,
)
os.close(slave)
os.set_blocking(master, False)

raw = bytearray()


def read_some(timeout):
    deadline = time.monotonic() + timeout
    made_progress = False
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            try:
                chunk = os.read(master, 65536)
                if chunk:
                    raw.extend(chunk)
                    made_progress = True
                    continue
            except OSError:
                pass
            return made_progress
        ready, _, _ = select.select([master], [], [], 0.05)
        if not ready:
            continue
        try:
            chunk = os.read(master, 65536)
        except OSError as e:
            if e.errno in (errno.EIO, errno.EBADF):
                return made_progress
            raise
        if chunk:
            raw.extend(chunk)
            made_progress = True
    return made_progress


def text():
    return raw.decode("utf-8", errors="replace")


def wait_for(needle, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if contains_marker(text(), needle):
            return clean(text())
        read_some(0.2)
    fail(f"timed out waiting for {needle!r}", text())


def assert_no_next_prompt(transcript):
    if "next>" in clean(transcript):
        fail("transcript should not render the old next> prompt", transcript)


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
    wait_for(">", 15)
    send_line(first_turn)

    tool_seen = wait_for("TOOL_RESULT_MARKER", 25)
    assert_no_next_prompt(tool_seen)
    if "bash(" not in tool_seen:
        fail("tool call preview was not visible in the PTY transcript", tool_seen)

    first_done = wait_for("FIRST_DONE_MARKER", 25)
    assert_no_next_prompt(first_done)
    time.sleep(0.8)
    read_some(0.5)
    send_line(stable_followup)

    wait_for("SECOND_RESPONSE_MARKER", 25)
    send_line("")
    send_line(queued_turn)

    before_second_done = wait_for("SECOND_DONE_MARKER", 40)
    assert_no_next_prompt(before_second_done)
    if compact_marker(queued_turn) not in compact_marker(before_second_done):
        fail(
            "queued input was not visibly preserved in the PTY while the assistant streamed",
            before_second_done,
        )
    if "[queued for next turn]" not in before_second_done:
        fail("queued input did not receive an explicit queued affordance", before_second_done)
    if "[blank queued input ignored]" not in before_second_done:
        fail("blank input during streaming was not explicitly ignored", before_second_done)

    full = wait_for("QUEUED_RESPONSE_MARKER", 35)
    assert_no_next_prompt(full)
    if first_header in full:
        fail("normal first turn should not print a noisy assistant-for label", full)
    if stable_header in full:
        fail("stable-boundary follow-up should not print a noisy assistant-for label", full)
    if queued_header not in full:
        fail("queued follow-up should keep an assistant-for association label", full)
    if full.find("QUEUED_RESPONSE_MARKER") < full.find(queued_header):
        fail("queued assistant response was not separated after its turn header", full)

    send_line("/quit")
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline and proc.poll() is None:
        read_some(0.2)
    if proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        fail("nex did not exit after /quit", text())
    read_some(1)
    final = clean(text())
    write_transcript(final)
    if proc.returncode != 0:
        fail(f"nex exited with {proc.returncode}", final)
    if len(request_bodies) < 4:
        fail(f"expected at least four model requests, saw {len(request_bodies)}", final)
finally:
    try:
        server.shutdown()
    except Exception:
        pass
    try:
        os.close(master)
    except OSError:
        pass
PY
        loud_fail "$label PTY live-input harness failed. Transcript tail:\n$(tail -100 "$transcript" 2>/dev/null || true)"
    fi

    journal="$(
        find "$scratch/.wg/chat" "$scratch/.nex/sessions" \
            -name conversation.jsonl -print -quit 2>/dev/null || true
    )"
    if [[ -z "$journal" ]]; then
        scratch_tree="$(find "$scratch" -maxdepth 5 -print 2>/dev/null | sort)"
        loud_fail "$label did not create a conversation journal. Scratch tree:
$scratch_tree"
    fi
    grep -q "stable follow-up after tool" "$journal" || loud_fail "$label stable follow-up was not journaled. Journal tail:\n$(tail -20 "$journal")"
    grep -q "SECOND_QUEUED_SMOKE" "$journal" || loud_fail "$label queued user turn was not journaled. Journal tail:\n$(tail -20 "$journal")"
    grep -q "TOOL_RESULT_MARKER" "$journal" || loud_fail "$label bash tool result was not journaled. Journal tail:\n$(tail -20 "$journal")"
    local queued_count
    queued_count="$(grep -c "SECOND_QUEUED_SMOKE" "$journal" || true)"
    [[ "$queued_count" == "1" ]] || loud_fail "$label queued user turn was journaled $queued_count times; expected exactly once. Journal:\n$(cat "$journal")"

    echo "PASS: $label PTY prompt labels are plain at stable boundaries, queued input is explicit, and tool/follow-up turns are preserved"
}

run_case "standalone nex" nex --no-mcp --max-turns 8 -m nex-live-input-smoke-model -e __NEX_ENDPOINT__
run_case "wg nex" wg nex --no-mcp --max-turns 8 -m nex-live-input-smoke-model -e __NEX_ENDPOINT__

echo "PASS: nex and wg nex PTY live-input transcript UX is polished"
