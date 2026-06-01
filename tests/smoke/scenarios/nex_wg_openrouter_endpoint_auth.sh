#!/usr/bin/env bash
# Scenario: nex_wg_openrouter_endpoint_auth
#
# WG-scoped Nex eval invocations must load the WG endpoint config and attach
# the configured OpenRouter API key. This pins both entrypoints that users run
# from a WG project: `wg nex --eval-mode ...` and `nex --wg --eval-mode ...`.

set -euo pipefail

source "$(dirname "$0")/_helpers.sh"

require_wg

if ! command -v nex >/dev/null 2>&1; then
    loud_skip "MISSING NEX" "standalone nex binary is required for this scenario"
fi
if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON" "python3 is required for the local OpenRouter-shaped endpoint"
fi

scratch="$(make_scratch)"
cd "$scratch"
wg init --no-agency >/dev/null

server_py="$scratch/fake_openrouter_auth.py"
port_file="$scratch/port"
request_log="$scratch/requests.jsonl"
key_file="$scratch/openrouter.key"
printf '%s\n' "test-openrouter-key" >"$key_file"
chmod 600 "$key_file"

cat >"$server_py" <<'PY'
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port_file = sys.argv[1]
request_log = sys.argv[2]

EXPECTED_AUTH = "Bearer test-openrouter-key"

def sse(text):
    chunks = [
        {"id": "fake-stream", "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": None}]},
        {"id": "fake-stream", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 1, "completion_tokens": 1}},
    ]
    return "".join("data: " + json.dumps(c) + "\n\n" for c in chunks) + "data: [DONE]\n\n"

class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        raw = self.rfile.read(length)
        try:
            req = json.loads(raw)
        except Exception:
            req = {}
        auth_ok = self.headers.get("authorization") == EXPECTED_AUTH
        with open(request_log, "a", encoding="utf-8") as f:
            f.write(json.dumps({
                "path": self.path,
                "model": req.get("model"),
                "auth_ok": auth_ok,
            }) + "\n")

        if not auth_ok:
            payload = {"error": {"message": "No cookie auth credentials found", "type": "invalid_request_error"}}
            self.send_body("application/json", json.dumps(payload).encode("utf-8"), status=401)
            return

        self.send_body("text/event-stream", sse("auth ok").encode("utf-8"))

    def send_body(self, content_type, payload, status=200):
        self.send_response(status)
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
api_key_file = "$key_file"
is_default = true
TOML

if ! env -u OPENROUTER_API_KEY -u OPENAI_API_KEY timeout 45s wg nex --eval-mode --minimal-tools \
        --model "$model" --endpoint openrouter --max-turns 1 \
        "Reply with exactly: WG_NEX_AUTH_OK" >wg-nex.out 2>wg-nex.err; then
    loud_fail "wg nex eval-mode did not use configured OpenRouter credentials. stderr:\n$(cat wg-nex.err)"
fi

if ! env -u OPENROUTER_API_KEY -u OPENAI_API_KEY timeout 45s nex --wg --eval-mode --minimal-tools \
        --model "$model" --endpoint openrouter --max-turns 1 \
        "Reply with exactly: NEX_WG_AUTH_OK" >nex-wg.out 2>nex-wg.err; then
    loud_fail "nex --wg eval-mode did not use configured OpenRouter credentials. stderr:\n$(cat nex-wg.err)"
fi

grep -q '"status":"ok"' wg-nex.out || loud_fail "wg nex did not report ok: $(cat wg-nex.out)"
grep -q '"status":"ok"' nex-wg.out || loud_fail "nex --wg did not report ok: $(cat nex-wg.out)"

if [[ -e "$scratch/.nex-eval" ]]; then
    loud_fail "WG-scoped eval-mode unexpectedly wrote standalone .nex-eval state"
fi

python3 - "$request_log" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    requests = [json.loads(line) for line in f if line.strip()]

if len(requests) != 2:
    raise SystemExit(f"expected exactly two configured-endpoint requests, got {len(requests)}: {requests}")

for req in requests:
    if req.get("path") != "/v1/chat/completions":
        raise SystemExit(f"wrong request path: {req}")
    if req.get("model") != "minimax/minimax-m2.7":
        raise SystemExit(f"OpenRouter model prefix was not stripped: {req}")
    if req.get("auth_ok") is not True:
        raise SystemExit(f"configured Bearer auth was missing: {req}")
PY

echo "PASS: WG-scoped Nex eval entrypoints attach configured OpenRouter endpoint credentials"
