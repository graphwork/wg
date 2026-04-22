#!/usr/bin/env python3
"""
Minimal OpenAI-compatible fake LLM server for end-to-end smoke tests.

Responds to POST /chat/completions and /v1/chat/completions. Streams
SSE chunks that mimic the shape `wg nex` (and any OpenAI-compatible
client) expects. Ignores the request body content — the responses it
serves are driven entirely by a script file passed on the CLI.

## Script format

Plain text, one turn per entry, separated by blank lines. The server
serves the Nth entry as the response to the Nth incoming request.
After exhausting the script, it loops back to the first entry.

Example (`responses.txt`):

    Hello! How can I help you?

    Sure — I can help with that. What specifically are you after?

    Alright, that's done.

## Usage

    python3 fake_llm_server.py --port 18080 --responses responses.txt

Listens on 127.0.0.1. Writes a one-line JSON startup log to stdout
when ready so callers can `read` from it to sync. SIGTERM exits
cleanly. Also accepts `--ready-file <path>` which touches the path
once the server is accepting connections (simpler than parsing stdout).

## Non-goals

- Auth (it accepts any / no Authorization header — local-only by design)
- Tool-use responses (returns plain text only for now)
- Multi-client session tracking (global turn counter; one client at a
  time is the expected use)

Runs on Python 3.8+ with only stdlib.
"""

from __future__ import annotations

import argparse
import http.server
import json
import signal
import socketserver
import sys
import threading
import time
import uuid
from pathlib import Path


def parse_script(path: Path) -> list[str]:
    raw = path.read_text()
    # Split on blank lines; keep non-empty chunks.
    chunks = [chunk.strip() for chunk in raw.split("\n\n")]
    return [c for c in chunks if c]


class FakeState:
    def __init__(self, responses: list[str]) -> None:
        self.responses = responses
        self.turn = 0
        self.lock = threading.Lock()
        self.received: list[dict] = []

    def next_response(self, body: dict) -> str:
        with self.lock:
            if not self.responses:
                return "OK"
            r = self.responses[self.turn % len(self.responses)]
            self.turn += 1
            self.received.append(body)
            return r


STATE: FakeState | None = None


class Handler(http.server.BaseHTTPRequestHandler):
    # Silence default access-log spam; keep 'em on stderr under --verbose.
    def log_message(self, format: str, *args) -> None:  # noqa: A002
        if getattr(self.server, "verbose", False):
            sys.stderr.write("[fake] " + (format % args) + "\n")

    def do_POST(self) -> None:  # noqa: N802
        if self.path not in ("/chat/completions", "/v1/chat/completions"):
            self.send_response(404)
            self.end_headers()
            return

        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        try:
            body = json.loads(raw.decode() or "{}")
        except json.JSONDecodeError:
            body = {}
        text = STATE.next_response(body) if STATE else "OK"
        streaming = bool(body.get("stream"))

        if streaming:
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.end_headers()
            cid = f"chatcmpl-{uuid.uuid4().hex[:8]}"
            # Yield text as one chunk (keep wire-format simple).
            first = {
                "id": cid,
                "object": "chat.completion.chunk",
                "choices": [
                    {"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": None}
                ],
            }
            self.wfile.write(f"data: {json.dumps(first)}\n\n".encode())
            final = {
                "id": cid,
                "object": "chat.completion.chunk",
                "choices": [
                    {"index": 0, "delta": {}, "finish_reason": "stop"}
                ],
            }
            self.wfile.write(f"data: {json.dumps(final)}\n\n".encode())
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
            return

        # Non-streaming fallback.
        payload = {
            "id": f"chatcmpl-{uuid.uuid4().hex[:8]}",
            "object": "chat.completion",
            "choices": [
                {"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}
            ],
            "usage": {"prompt_tokens": 0, "completion_tokens": len(text.split()), "total_tokens": 0},
        }
        data = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


class ThreadedServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--responses", type=Path, required=True, help="path to script file")
    parser.add_argument("--ready-file", type=Path, help="touch this path once listening")
    parser.add_argument("--verbose", action="store_true", help="echo access log to stderr")
    args = parser.parse_args()

    global STATE
    STATE = FakeState(parse_script(args.responses))

    server = ThreadedServer(("127.0.0.1", args.port), Handler)
    server.verbose = args.verbose

    # Clean shutdown on SIGTERM/SIGINT.
    def shutdown(_sig, _frame):
        threading.Thread(target=server.shutdown, daemon=True).start()

    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT, shutdown)

    # Signal readiness so the caller can sync without racing.
    print(json.dumps({"ready": True, "port": args.port, "turns_loaded": len(STATE.responses)}), flush=True)
    if args.ready_file:
        args.ready_file.parent.mkdir(parents=True, exist_ok=True)
        args.ready_file.write_text(str(args.port))

    try:
        server.serve_forever(poll_interval=0.1)
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
