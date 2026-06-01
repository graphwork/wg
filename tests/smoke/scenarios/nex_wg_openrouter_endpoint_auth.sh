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
openrouter_key="test-openrouter-key"
printf '%s\n' "$openrouter_key" >"$key_file"
chmod 600 "$key_file"

redact_file() {
    sed "s/$openrouter_key/<redacted>/g" "$1"
}

assert_no_key_leaked() {
    local file
    for file in "$@"; do
        if [[ -f "$file" ]] && grep -qF "$openrouter_key" "$file"; then
            loud_fail "$file leaked the configured OpenRouter API key"
        fi
    done
}

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
        "Reply with exactly: WG_NEX_FILE_AUTH_OK" >wg-nex-file.out 2>wg-nex-file.err; then
    assert_no_key_leaked wg-nex-file.out wg-nex-file.err
    loud_fail "wg nex eval-mode did not use configured api_key_file credentials. stderr:\n$(redact_file wg-nex-file.err)"
fi

if ! env -u OPENROUTER_API_KEY -u OPENAI_API_KEY timeout 45s nex --wg --eval-mode --minimal-tools \
        --model "$model" --endpoint openrouter --max-turns 1 \
        "Reply with exactly: NEX_WG_FILE_AUTH_OK" >nex-wg-file.out 2>nex-wg-file.err; then
    assert_no_key_leaked nex-wg-file.out nex-wg-file.err
    loud_fail "nex --wg eval-mode did not use configured api_key_file credentials. stderr:\n$(redact_file nex-wg-file.err)"
fi

cat >"$scratch/.wg/config.toml" <<TOML
[agent]
model = "$model"

[dispatcher]
model = "$model"

[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "$endpoint"
api_key_env = "OPENROUTER_API_KEY"
is_default = true
TOML

if ! env -u OPENAI_API_KEY OPENROUTER_API_KEY="$openrouter_key" timeout 45s wg nex --eval-mode --minimal-tools \
        --model "$model" --endpoint openrouter --max-turns 1 \
        "Reply with exactly: WG_NEX_ENV_AUTH_OK" >wg-nex-env.out 2>wg-nex-env.err; then
    assert_no_key_leaked wg-nex-env.out wg-nex-env.err
    loud_fail "wg nex eval-mode did not use configured api_key_env credentials. stderr:\n$(redact_file wg-nex-env.err)"
fi

if ! env -u OPENAI_API_KEY OPENROUTER_API_KEY="$openrouter_key" timeout 45s nex --wg --eval-mode --minimal-tools \
        --model "$model" --endpoint openrouter --max-turns 1 \
        "Reply with exactly: NEX_WG_ENV_AUTH_OK" >nex-wg-env.out 2>nex-wg-env.err; then
    assert_no_key_leaked nex-wg-env.out nex-wg-env.err
    loud_fail "nex --wg eval-mode did not use configured api_key_env credentials. stderr:\n$(redact_file nex-wg-env.err)"
fi

assert_no_key_leaked \
    wg-nex-file.out wg-nex-file.err \
    nex-wg-file.out nex-wg-file.err \
    wg-nex-env.out wg-nex-env.err \
    nex-wg-env.out nex-wg-env.err

grep -q '"status":"ok"' wg-nex-file.out || loud_fail "wg nex api_key_file did not report ok: $(redact_file wg-nex-file.out)"
grep -q '"status":"ok"' nex-wg-file.out || loud_fail "nex --wg api_key_file did not report ok: $(redact_file nex-wg-file.out)"
grep -q '"status":"ok"' wg-nex-env.out || loud_fail "wg nex api_key_env did not report ok: $(redact_file wg-nex-env.out)"
grep -q '"status":"ok"' nex-wg-env.out || loud_fail "nex --wg api_key_env did not report ok: $(redact_file nex-wg-env.out)"

if [[ -e "$scratch/.nex-eval" ]]; then
    loud_fail "WG-scoped eval-mode unexpectedly wrote standalone .nex-eval state"
fi

python3 - "$request_log" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    requests = [json.loads(line) for line in f if line.strip()]

if len(requests) != 4:
    raise SystemExit(f"expected exactly four configured-endpoint requests, got {len(requests)}: {requests}")

for req in requests:
    if req.get("path") != "/v1/chat/completions":
        raise SystemExit(f"wrong request path: {req}")
    if req.get("model") != "minimax/minimax-m2.7":
        raise SystemExit(f"OpenRouter model prefix was not stripped: {req}")
    if req.get("auth_ok") is not True:
        raise SystemExit(f"configured Bearer auth was missing: {req}")
PY

echo "PASS: WG-scoped Nex eval entrypoints attach configured OpenRouter api_key_file and api_key_env credentials"
