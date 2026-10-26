#!/usr/bin/env bash
# Smoke: `wg login openrouter --from-stdin` wires a secret-backed OpenRouter
# endpoint without embedding the key in config, then unblocks `wg models fetch
# --no-cache` through the WG credential path.
# owner: impl-wg-login-openrouter
set -euo pipefail
. "$(dirname "$0")/_helpers.sh"
require_wg

SMOKE_HOME=$(mktemp -d)
PROJECT_ROOT=$(mktemp -d)
SERVER_DIR=$(mktemp -d)
add_cleanup_hook "rm -rf $SMOKE_HOME $PROJECT_ROOT $SERVER_DIR"
export HOME="$SMOKE_HOME"
unset WG_GLOBAL_DIR WG_DIR 2>/dev/null || true
WG_DIR="$PROJECT_ROOT/.wg"
mkdir -p "$WG_DIR" "$HOME/.wg"

cat >"$WG_DIR/graph.jsonl" <<'EOF'
EOF

PORT_FILE="$SERVER_DIR/port"
python3 - "$PORT_FILE" >"$SERVER_DIR/server.log" 2>&1 <<'PY' &
import http.server
import json
import socketserver
import sys

port_file = sys.argv[1]
requests = []
body = json.dumps({
    "data": [{
        "id": "openai/gpt-4o-mini",
        "name": "GPT-4o mini",
        "description": "test",
        "context_length": 128000,
        "pricing": {"prompt": "0.00000015", "completion": "0.0000006"},
        "supported_parameters": ["tools"],
    }]
}).encode()

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        requests.append(self.headers.get("Authorization", ""))
        if self.path != "/api/v1/models":
            self.send_response(404)
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        return

with socketserver.TCPServer(("127.0.0.1", 0), Handler) as httpd:
    with open(port_file, "w", encoding="utf-8") as fh:
        fh.write(str(httpd.server_address[1]))
    httpd.timeout = 0.5
    while True:
        httpd.handle_request()
PY
SERVER_PID=$!
add_cleanup_hook "kill $SERVER_PID 2>/dev/null || true"

for _ in $(seq 1 50); do
    if [[ -s "$PORT_FILE" ]]; then
        break
    fi
    sleep 0.1
done
[[ -s "$PORT_FILE" ]] || loud_fail "mock OpenRouter server did not start"
PORT=$(cat "$PORT_FILE")
BASE_URL="http://127.0.0.1:${PORT}/api/v1"

cat >"$WG_DIR/config.toml" <<TOML
[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "$BASE_URL"
is_default = true
TOML

LOGIN_OUT=$(printf '%s' 'sk-or-smoke-login-test' | OPENROUTER_BASE_URL="$BASE_URL" wg --dir "$WG_DIR" login openrouter --from-stdin --backend keystore --local 2>&1)
echo "$LOGIN_OUT" | grep -q "secret: present (keystore:openrouter)" \
    || loud_fail "login output did not report the keystore secret ref:\n$LOGIN_OUT"
echo "$LOGIN_OUT" | grep -q "auth: ok" \
    || loud_fail "login output did not report successful auth:\n$LOGIN_OUT"

CONFIG="$WG_DIR/config.toml"
grep -q 'api_key_ref = "keystore:openrouter"' "$CONFIG" \
    || loud_fail "config.toml is missing the expected api_key_ref:\n$(cat "$CONFIG")"
if grep -q 'sk-or-smoke-login-test' "$CONFIG"; then
    loud_fail "config.toml leaked the OpenRouter key:\n$(cat "$CONFIG")"
fi
if grep -q 'api_key =' "$CONFIG"; then
    loud_fail "config.toml should not embed an inline api_key:\n$(cat "$CONFIG")"
fi

FETCH_OUT=$(OPENROUTER_BASE_URL="$BASE_URL" wg --dir "$WG_DIR" models fetch --no-cache 2>&1)
echo "$FETCH_OUT" | grep -q "Benchmark registry updated:" \
    || loud_fail "wg models fetch --no-cache did not succeed via the logged-in secret:\n$FETCH_OUT"

CHECK_OUT=$(OPENROUTER_BASE_URL="$BASE_URL" wg --dir "$WG_DIR" login openrouter --check 2>&1)
echo "$CHECK_OUT" | grep -q "OpenRouter (WG)" \
    || loud_fail "check output missing WG section:\n$CHECK_OUT"
echo "$CHECK_OUT" | grep -q "OpenRouter (Pi)" \
    || loud_fail "check output missing Pi section:\n$CHECK_OUT"

kill "$SERVER_PID" 2>/dev/null || true
echo "PASS: wg login openrouter --from-stdin configures a secret-backed endpoint"
