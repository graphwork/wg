#!/usr/bin/env bash
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

    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        body = self.rfile.read(length)
        request = json.loads(body)
        with open(request_log, "a", encoding="utf-8") as f:
            f.write(json.dumps(request) + "\n")

        system = ""
        for message in request.get("messages", []):
            if message.get("role") == "system":
                content = message.get("content", "")
                system = content if isinstance(content, str) else json.dumps(content)
                break

        if "requires current real-world data" in system and "fabricates current data" in system:
            text = "I should fetch live data first using web_fetch when available, or bash curl/wget when that is the HTTP path."
        else:
            text = "fn main() { println!(\"fake Copenhagen weather\"); }"

        chunks = [
            {"id": "fake-1", "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": None}]},
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
prompt="Copenhagen weather forecast for June 28-July 3, 2026"

wg nex --eval-mode --chat qwen-current-data --max-turns 1 -m nex:qwen3-coder -e "$endpoint" "$prompt" >"$scratch/qwen.out"
wg nex --eval-mode --chat noncoder-current-data --max-turns 1 -m gpt-5.4-mini -e "$endpoint" "$prompt" >"$scratch/noncoder.out"
wg nex --eval-mode --chat minimal-current-data --minimal-tools --max-turns 1 -m nex:qwen3-coder -e "$endpoint" "$prompt" >"$scratch/minimal.out"

python3 - "$request_log" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    requests = [json.loads(line) for line in f if line.strip()]

if len(requests) != 3:
    raise SystemExit(f"expected 3 requests, got {len(requests)}")

def system_text(request):
    for message in request["messages"]:
        if message["role"] == "system":
            content = message["content"]
            return content if isinstance(content, str) else json.dumps(content)
    raise AssertionError("missing system message")

def tool_names(request):
    return {
        tool.get("function", {}).get("name")
        for tool in request.get("tools", [])
        if tool.get("function", {}).get("name")
    }

for index, request in enumerate(requests):
    system = system_text(request)
    assert "requires current real-world data" in system, (index, system)
    assert "curl https://wttr.in/<location>" in system, (index, system)
    assert "Do not write code or prose that fabricates current data" in system, (index, system)

normal_tools = tool_names(requests[0])
assert "web_fetch" in normal_tools, normal_tools
assert "web_search" in normal_tools, normal_tools
assert "bash" in normal_tools, normal_tools

noncoder_tools = tool_names(requests[1])
assert "web_fetch" in noncoder_tools, noncoder_tools
assert "bash" in noncoder_tools, noncoder_tools

minimal_system = system_text(requests[2])
minimal_tools = tool_names(requests[2])
assert "web_search and web_fetch are not available" in minimal_system, minimal_system
assert "bash" in minimal_tools, minimal_tools
assert "web_fetch" not in minimal_tools, minimal_tools
assert "web_search" not in minimal_tools, minimal_tools
PY

grep -q '"status":"ok"' "$scratch/qwen.out"
grep -q '"status":"ok"' "$scratch/noncoder.out"
grep -q '"status":"ok"' "$scratch/minimal.out"
