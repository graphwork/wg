#!/usr/bin/env bash
# Scenario: nex_helper_openrouter_route
#
# Drives the real `wg nex` CLI against a local OpenRouter-shaped
# OpenAI-compatible endpoint and forces the parent model to call the
# `summarize` and `delegate` helper tools. The regression was that those
# helpers ignored the active OpenRouter session and silently created a
# hard-coded Anthropic `sonnet` client.

set -euo pipefail

source "$(dirname "$0")/_helpers.sh"

require_wg

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON" "python3 is required for the local OpenRouter-shaped endpoint"
fi

scratch="$(make_scratch)"
cd "$scratch"
wg init --no-agency >/dev/null

server_py="$scratch/fake_openrouter.py"
port_file="$scratch/port"
request_log="$scratch/requests.jsonl"

cat >"$server_py" <<'PY'
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port_file = sys.argv[1]
request_log = sys.argv[2]

MODEL = "minimax/minimax-m2.7"

def message_text(messages):
    return json.dumps(messages, sort_keys=True)

def system_text(messages):
    for message in messages:
        if message.get("role") == "system":
            content = message.get("content", "")
            return content if isinstance(content, str) else json.dumps(content)
    return ""

def sse(chunks):
    return "".join("data: " + json.dumps(c) + "\n\n" for c in chunks) + "data: [DONE]\n\n"

class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        body = self.rfile.read(length)
        req = json.loads(body)
        messages = req.get("messages", [])
        system = system_text(messages)
        text = message_text(messages)

        with open(request_log, "a", encoding="utf-8") as f:
            f.write(json.dumps({
                "path": self.path,
                "model": req.get("model"),
                "stream": bool(req.get("stream")),
                "system": system,
                "tools": [
                    t.get("function", {}).get("name")
                    for t in req.get("tools", [])
                    if t.get("function", {}).get("name")
                ],
                "text": text,
            }) + "\n")

        if "text summarization agent" in system:
            self.send_text_response(req, "summary inherited openrouter route")
            return

        if "focused sub-agent" in system:
            self.send_text_response(req, "delegate inherited openrouter route")
            return

        if '"role": "tool"' in text:
            self.send_sse_text("helper route completed")
            return

        if "summarize-helper-route" in text:
            self.send_sse_tool("call_summarize", "summarize", {
                "source": "alpha beta gamma",
                "instruction": "summarize-helper-route"
            })
            return

        if "delegate-helper-route" in text:
            self.send_sse_tool("call_delegate", "delegate", {
                "prompt": "delegate-helper-route",
                "max_turns": 1
            })
            return

        self.send_sse_text("unexpected request")

    def send_text_response(self, req, text):
        if req.get("stream"):
            self.send_sse_text(text)
        else:
            self.send_json(text)

    def send_json(self, text):
        payload = {
            "id": "fake-json",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        }
        self.send_body("application/json", json.dumps(payload).encode("utf-8"))

    def send_sse_text(self, text):
        payload = sse([
            {"id": "fake-stream", "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": None}]},
            {"id": "fake-stream", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 1, "completion_tokens": 1}},
        ])
        self.send_body("text/event-stream", payload.encode("utf-8"))

    def send_sse_tool(self, call_id, name, args):
        payload = sse([
            {"id": "fake-tool", "choices": [{"index": 0, "delta": {"role": "assistant", "tool_calls": [{"index": 0, "id": call_id, "type": "function", "function": {"name": name, "arguments": json.dumps(args)}}]}, "finish_reason": None}]},
            {"id": "fake-tool", "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 1, "completion_tokens": 1}},
        ])
        self.send_body("text/event-stream", payload.encode("utf-8"))

    def send_body(self, content_type, payload):
        self.send_response(200)
        self.send_header("content-type", content_type)
        self.send_header("content-length", str(len(payload)))
        self.send_header("connection", "close")
        self.end_headers()
        self.wfile.write(payload)
        self.close_connection = True

    def log_message(self, fmt, *args):
        return

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
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
[[ -s "$port_file" ]] || loud_fail "fake OpenRouter endpoint did not start"

endpoint="http://127.0.0.1:$(cat "$port_file")/v1"
model="openrouter:minimax/minimax-m2.7"

cat >"$scratch/.wg/config.toml" <<TOML
[agent]
model = "$model"

[dispatcher]
model = "$model"

[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "$endpoint"
api_key = "test-openrouter-key"
is_default = true
TOML

if ! timeout 45s wg nex --eval-mode --chat summarize-route --max-turns 4 -m "$model" -e openrouter \
        "summarize-helper-route" >summarize.out 2>summarize.err; then
    loud_fail "wg nex summarize flow failed. stderr:\n$(cat summarize.err)"
fi

if ! timeout 45s wg nex --eval-mode --chat delegate-route --max-turns 4 -m "$model" -e openrouter \
        "delegate-helper-route" >delegate.out 2>delegate.err; then
    loud_fail "wg nex delegate flow failed. stderr:\n$(cat delegate.err)"
fi

grep -q '"status":"ok"' summarize.out || loud_fail "summarize flow did not report ok: $(cat summarize.out)"
grep -q '"status":"ok"' delegate.out || loud_fail "delegate flow did not report ok: $(cat delegate.out)"

if grep -qE 'Anthropic API error|model=sonnet|model="sonnet"|model: sonnet' summarize.err delegate.err; then
    loud_fail "helper flow surfaced Anthropic/sonnet fallback:\n$(cat summarize.err delegate.err)"
fi

python3 - "$request_log" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    requests = [json.loads(line) for line in f if line.strip()]

if len(requests) < 6:
    raise SystemExit(f"expected at least 6 requests, got {len(requests)}")

bad = [r for r in requests if r.get("model") in {"sonnet", "claude:sonnet", "haiku", "opus"}]
if bad:
    raise SystemExit(f"found Anthropic alias helper route: {bad}")

summarize = [r for r in requests if "text summarization agent" in r.get("system", "")]
delegate = [r for r in requests if "focused sub-agent" in r.get("system", "")]
if not summarize:
    raise SystemExit("summarize helper request was not observed")
if not delegate:
    raise SystemExit("delegate helper request was not observed")

for label, group in [("summarize", summarize), ("delegate", delegate)]:
    models = {r.get("model") for r in group}
    if models != {"minimax/minimax-m2.7"}:
        raise SystemExit(f"{label} helper used wrong model(s): {models}")

print("PASS: summarize and delegate helper calls used OpenRouter model, not sonnet/Anthropic")
PY
