#!/usr/bin/env bash
# Probe-driven conditional minimal-tools default for `wg nex`.
#
# Locks in the wg-nex-probe behavior: when neither --minimal-tools nor
# --full-tools is passed, the lean tool surface auto-enables iff the
# resolved context window is at or below the configured threshold (32k).
# Explicit flags always win, and the auto-path prints a discoverability
# banner on stderr (non-eval only) pointing at `/tools full`.
#
# A fake OpenAI-compatible endpoint advertises a SMALL window via
# llama.cpp's `GET /props` (n_ctx=8192), so the probe resolves 8192 <= 32k
# and the auto-minimal default fires unless overridden.
set -euo pipefail

source "$(dirname "$0")/_helpers.sh"

require_wg

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON" "python3 is required for the local fake OAI-compatible endpoint"
fi

scratch="$(make_scratch)"
cd "$scratch"
wg init --no-agency >/dev/null

server_py="$scratch/fake_oai_server.py"
port_file="$scratch/port"
request_log="$scratch/requests.jsonl"

cat >"$server_py" <<'PY'
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port_file = sys.argv[1]
request_log = sys.argv[2]


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _send_json(self, obj):
        body = json.dumps(obj).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        # The context probe hits llama.cpp `/props` first, then
        # `/v1/models`. Advertise a small runtime window both ways so the
        # resolved context window is 8192 (<= the 32k threshold).
        path = self.path.rstrip("/")
        if path.endswith("/props"):
            self._send_json({"n_ctx": 8192})
            return
        if "/models" in path:
            self._send_json({"data": [{"id": "qwen3-coder", "max_model_len": 8192}]})
            return
        self._send_json({})

    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        body = self.rfile.read(length)
        request = json.loads(body)
        with open(request_log, "a", encoding="utf-8") as f:
            f.write(json.dumps(request) + "\n")

        chunks = [
            {"id": "fake-1", "choices": [{"index": 0, "delta": {"role": "assistant", "content": "ok"}, "finish_reason": None}]},
            {"id": "fake-1", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 1, "completion_tokens": 1}},
        ]
        payload = "".join("data: " + json.dumps(chunk) + "\n\n" for chunk in chunks) + "data: [DONE]\n\n"
        encoded = payload.encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("content-length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def log_message(self, fmt, *args):
        return


server = HTTPServer(("127.0.0.1", 0), Handler)
with open(port_file, "w", encoding="utf-8") as f:
    f.write(str(server.server_port))
server.serve_forever()
PY

python3 "$server_py" "$port_file" "$request_log" &
server_pid=$!
cleanup_fake_server() {
    kill "$server_pid" >/dev/null 2>&1 || true
}
add_cleanup_hook cleanup_fake_server

for _ in $(seq 1 50); do
    [[ -s "$port_file" ]] && break
    sleep 0.1
done
[[ -s "$port_file" ]] || loud_fail "fake endpoint did not start"

endpoint="http://127.0.0.1:$(cat "$port_file")"

# (a) No flag + small window → auto-minimal fires (eval mode = scriptable,
#     deterministic; banner suppressed). (b) --full-tools forces the full
#     surface despite the small window. (c) --minimal-tools forces lean.
wg nex --eval-mode --chat auto-lean      --max-turns 1 -m nex:qwen3-coder -e "$endpoint" "hi" >"$scratch/auto.out"
wg nex --eval-mode --chat forced-full --full-tools --max-turns 1 -m nex:qwen3-coder -e "$endpoint" "hi" >"$scratch/full.out"
wg nex --eval-mode --chat forced-min --minimal-tools --max-turns 1 -m nex:qwen3-coder -e "$endpoint" "hi" >"$scratch/min.out"

python3 - "$request_log" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    requests = [json.loads(line) for line in f if line.strip()]

if len(requests) != 3:
    raise SystemExit(f"expected 3 requests, got {len(requests)}")


def tool_names(request):
    return {
        tool.get("function", {}).get("name")
        for tool in request.get("tools", [])
        if tool.get("function", {}).get("name")
    }


# (a) auto-minimal: no explicit flag + 8k window → lean surface.
auto = tool_names(requests[0])
assert "bash" in auto, auto
assert "web_fetch" not in auto, ("auto path should drop web_fetch on small window", auto)
assert "web_search" not in auto, auto

# (b) --full-tools overrides upward: full surface despite the small window.
full = tool_names(requests[1])
assert "web_fetch" in full, ("--full-tools should keep web_fetch", full)
assert "web_search" in full, full
assert len(full) > len(auto), (len(full), len(auto))

# (c) --minimal-tools forces lean (same surface as the auto path here).
minimal = tool_names(requests[2])
assert "bash" in minimal, minimal
assert "web_fetch" not in minimal, minimal
PY

# Banner is user-visible: in a NON-eval interactive run the auto path must
# print the discoverability line on stderr (and naming `/tools full`); an
# explicit --full-tools run must NOT (no auto-fire, no banner). Drive the
# real REPL with stdin closed so it exits after one turn.
auto_err="$scratch/auto.err"
full_err="$scratch/full.err"
timeout 30 wg nex --chat banner-auto --max-turns 1 -m nex:qwen3-coder -e "$endpoint" "hi" </dev/null >"$scratch/banner_auto.out" 2>"$auto_err" || true
timeout 30 wg nex --chat banner-full --full-tools --max-turns 1 -m nex:qwen3-coder -e "$endpoint" "hi" </dev/null >"$scratch/banner_full.out" 2>"$full_err" || true

grep -q "minimal tool surface" "$auto_err" \
    || loud_fail "auto path did not print the minimal-tools banner on stderr"
grep -q "/tools full" "$auto_err" \
    || loud_fail "minimal-tools banner did not name '/tools full' as the expand path"
if grep -q "minimal tool surface" "$full_err"; then
    loud_fail "--full-tools must not print the auto minimal-tools banner"
fi

grep -q '"status":"ok"' "$scratch/auto.out"
grep -q '"status":"ok"' "$scratch/full.out"
grep -q '"status":"ok"' "$scratch/min.out"
