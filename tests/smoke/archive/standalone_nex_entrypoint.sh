#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/_helpers.sh"

require_wg

if ! command -v nex >/dev/null 2>&1; then
    loud_skip "MISSING NEX BINARY" "nex not found on PATH; run 'cargo install --path . --locked' from this checkout"
fi

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON" "python3 is required for the local fake OAI-compatible endpoint"
fi

scratch="$(make_scratch)"
cd "$scratch"
wg init --no-agency >/dev/null

nex_help="$(nex --help 2>&1)"
grep -q "Usage: nex" <<<"$nex_help" || loud_fail "nex --help should render standalone usage:\n$nex_help"
for flag in --model --resume --chat --read-only --minimal-tools --eval-mode; do
    grep -q -- "$flag" <<<"$nex_help" || loud_fail "nex --help missing $flag:\n$nex_help"
done

wg_nex_help="$(wg nex --help 2>&1)"
grep -q "Usage: wg nex" <<<"$wg_nex_help" || loud_fail "wg nex --help should remain available:\n$wg_nex_help"
for flag in --model --resume --chat --read-only --minimal-tools --eval-mode; do
    grep -q -- "$flag" <<<"$wg_nex_help" || loud_fail "wg nex --help missing $flag:\n$wg_nex_help"
done

server_py="$scratch/fake_openrouter.py"
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

        chunks = [
            {"id": "fake-1", "choices": [{"index": 0, "delta": {"role": "assistant", "content": "hello from nex"}, "finish_reason": None}]},
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

if ! timeout 30s nex --eval-mode --chat standalone-entrypoint --max-turns 1 -m "$model" -e openrouter \
        "Say hello from standalone nex" >standalone.out 2>standalone.err; then
    loud_fail "standalone nex flow failed. stderr:\n$(cat standalone.err)"
fi

if ! timeout 30s wg nex --eval-mode --chat wg-subcommand-entrypoint --max-turns 1 -m "$model" -e openrouter \
        "Say hello from wg nex" >wg-nex.out 2>wg-nex.err; then
    loud_fail "wg nex compatibility flow failed. stderr:\n$(cat wg-nex.err)"
fi

grep -q '"status":"ok"' standalone.out || loud_fail "standalone nex did not report ok: $(cat standalone.out)"
grep -q '"status":"ok"' wg-nex.out || loud_fail "wg nex did not report ok: $(cat wg-nex.out)"

python3 - "$request_log" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    requests = [json.loads(line) for line in f if line.strip()]

if len(requests) != 2:
    raise SystemExit(f"expected one request from nex and one from wg nex, got {len(requests)}")

models = [r.get("model") for r in requests]
if models != ["minimax/minimax-m2.7", "minimax/minimax-m2.7"]:
    raise SystemExit(f"expected stripped OpenRouter model for both entrypoints, got {models}")

for index, request in enumerate(requests):
    if not request.get("stream"):
        raise SystemExit(f"request {index} did not use native streaming path: {request}")
    tools = request.get("tools", [])
    if not any(t.get("function", {}).get("name") == "bash" for t in tools):
        raise SystemExit(f"request {index} missing native tool surface: {request}")
PY

echo "PASS: standalone nex and wg nex share help/options and native OpenRouter route"
