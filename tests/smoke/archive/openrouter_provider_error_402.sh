#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/_helpers.sh"

require_wg

if ! command -v nex >/dev/null 2>&1; then
    loud_skip "MISSING NEX BINARY" "nex not found on PATH; run 'cargo install --path . --locked' from this checkout"
fi

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON" "python3 is required for the local fake OpenRouter endpoint"
fi

scratch="$(make_scratch)"
cd "$scratch"
wg init --no-agency >/dev/null

server_py="$scratch/fake_openrouter_provider_error.py"
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

        model = request.get("model", "")
        if model.endswith(":free"):
            payload = {
                "error": {
                    "message": "Provider returned error",
                    "code": 402,
                    "metadata": {
                        "provider_name": "Crucible",
                        "raw": json.dumps({
                            "error": {
                                "type": "insufficient_quota",
                                "code": "insufficient_quota",
                                "message": "Out of credits. Top up at /dashboard/billing to continue.",
                            }
                        }),
                    },
                }
            }
            encoded = json.dumps(payload).encode("utf-8")
            self.send_response(402)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)
            return

        chunks = [
            {"id": "paid-ok", "choices": [{"index": 0, "delta": {"role": "assistant", "content": "paid route ok"}, "finish_reason": None}]},
            {"id": "paid-ok", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 1, "completion_tokens": 1}},
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
free_model="openrouter:deepseek/deepseek-v4-flash:free"
paid_model="openrouter:deepseek/deepseek-v4-flash"
secret="sk-or-smoke-secret-must-not-print"

cat >"$scratch/.wg/config.toml" <<TOML
[agent]
model = "$free_model"

[dispatcher]
model = "$free_model"

[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "$endpoint"
api_key = "$secret"
is_default = true
TOML

if timeout 30s nex --eval-mode --chat openrouter-free-provider-error --max-turns 1 -m "$free_model" -e openrouter \
        "Say hello from the free route" >free.out 2>free.err; then
    loud_fail "free OpenRouter route unexpectedly succeeded"
else
    status=$?
    [[ "$status" -ne 124 ]] || loud_fail "free OpenRouter route timed out. stderr:\n$(cat free.err)"
fi

grep -q "API error 402: Provider returned error" free.err \
    || loud_fail "free-route error lost top-level status/message:\n$(cat free.err)"
grep -q "OpenRouter provider Crucible" free.err \
    || loud_fail "free-route error did not include provider name:\n$(cat free.err)"
grep -q "insufficient_quota" free.err \
    || loud_fail "free-route error did not include upstream type/code:\n$(cat free.err)"
grep -q "Out of credits" free.err \
    || loud_fail "free-route error did not include upstream message:\n$(cat free.err)"
grep -q "openrouter:deepseek/deepseek-v4-flash" free.err \
    || loud_fail "free-route error did not suggest the non-free model id:\n$(cat free.err)"
if grep -q "Check your API key" free.err || grep -q "api_key =" free.err; then
    loud_fail "provider-side failure incorrectly pointed at local credentials:\n$(cat free.err)"
fi
if grep -q "$secret" free.err free.out; then
    loud_fail "nex output leaked the configured OpenRouter API key"
fi

if ! timeout 30s nex --eval-mode --chat openrouter-paid-ok --max-turns 1 -m "$paid_model" -e openrouter \
        "Say hello from the paid route" >paid.out 2>paid.err; then
    loud_fail "paid OpenRouter route should still succeed against fake endpoint. stderr:\n$(cat paid.err)"
fi

grep -q '"status":"ok"' paid.out \
    || loud_fail "paid OpenRouter route did not report ok: stdout=$(cat paid.out) stderr=$(cat paid.err)"

python3 - "$request_log" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    requests = [json.loads(line) for line in f if line.strip()]

models = [r.get("model") for r in requests]
expected = ["deepseek/deepseek-v4-flash:free", "deepseek/deepseek-v4-flash"]
if models != expected:
    raise SystemExit(f"expected stripped free and paid OpenRouter models {expected}, got {models}")
PY

echo "PASS: nex renders OpenRouter provider-side 402 metadata and paid route still succeeds"
