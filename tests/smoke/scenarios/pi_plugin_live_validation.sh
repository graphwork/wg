#!/usr/bin/env bash
# Credentialed live validation for the pi plugin-in-the-loop RPC path.
#
# SKIPs loudly without a pi binary or OpenRouter credentials. When enabled, this
# drives the real pi RPC JSONL protocol with the WG plugin loaded from this repo:
#   - first turn streams to agent_end, calls a WG tool, and yields non-empty
#     get_last_assistant_text;
#   - the pi process is killed, restarted with the same --session-id, and the
#     session recalls a token from the previous turn while the plugin re-attaches;
#   - a large bash output request surfaces a fullOutputPath on a tool result.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v pi >/dev/null 2>&1 || \
    loud_skip "MISSING PI BINARY" "pi not on PATH; install pi.dev CLI to run the live plugin RPC smoke"
command -v npm >/dev/null 2>&1 || \
    loud_skip "MISSING NPM" "npm is required to build/load the WG pi plugin"
command -v node >/dev/null 2>&1 || \
    loud_skip "MISSING NODE" "node is required to build/load the WG pi plugin"

# Resolve OpenRouter credentials from WG endpoint/secret config. The WG
# contract: a configured WG endpoint/key is sufficient — no separate
# OPENROUTER_API_KEY export required. We only SKIP when WG cannot resolve a
# usable OpenRouter key.
_wg_openrouter_key=""
if [ -n "${OPENROUTER_API_KEY:-}" ]; then
    _wg_openrouter_key="$OPENROUTER_API_KEY"
fi
if [ -z "$_wg_openrouter_key" ]; then
    # Try resolving from WG config: `wg secret list` / `wg endpoints list`.
    _wg_scratch_home="$(mktemp -d)"
    _wg_test_dir="$_wg_scratch_home/wg-test"
    mkdir -p "$_wg_test_dir"
    if (cd "$_wg_test_dir" && HOME="$_wg_scratch_home" wg init --no-agency >/dev/null 2>&1); then
        _wg_resolved="$(cd "$_wg_test_dir" && HOME="$_wg_scratch_home" wg endpoints list 2>/dev/null | grep -i 'openrouter' | head -1)"
        if [ -n "$_wg_resolved" ]; then
            # WG has an openrouter endpoint configured; resolve its key via
            # `wg secret list` (keystore-backed). The key is exported into the
            # child env so the pi process (which reads OPENROUTER_API_KEY)
            # receives it via WG's env injection.
            _wg_key_val="$(cd "$_wg_test_dir" && HOME="$_wg_scratch_home" wg secret get openrouter 2>/dev/null || true)"
            if [ -n "$_wg_key_val" ]; then
                _wg_openrouter_key="$_wg_key_val"
                export OPENROUTER_API_KEY="$_wg_openrouter_key"
            fi
        fi
    fi
    rm -rf "$_wg_scratch_home"
fi
if [ -z "$_wg_openrouter_key" ]; then
    loud_skip "NO WG-RESOLVABLE OPENROUTER CREDENTIALS" \
        "WG could not resolve an OpenRouter key from endpoint/secret config, and OPENROUTER_API_KEY is not set. Configure a WG openrouter endpoint (wg endpoint add + wg key set) to run the credentialed pi RPC smoke."
fi

repo="$(cd "$HERE/../../.." && pwd)"
plugin="$repo/worksgood-pi"
[ -f "$plugin/package-lock.json" ] || loud_fail "missing worksgood-pi/package-lock.json"

if [ ! -d "$plugin/node_modules" ]; then
    npm --prefix "$plugin" ci >/tmp/wg-pi-live-npm-ci.log 2>&1 || \
        loud_skip "PI PLUGIN DEPS UNAVAILABLE" "npm ci failed: $(tail -20 /tmp/wg-pi-live-npm-ci.log)"
fi
npm --prefix "$plugin" run build >/tmp/wg-pi-live-build.log 2>&1 || \
    loud_fail "pi-plugin build failed: $(tail -80 /tmp/wg-pi-live-build.log)"

scratch=$(make_scratch)
fake_home="$scratch/home"
project="$scratch/project"
mkdir -p "$fake_home/.pi/agent" "$project"

cat >"$fake_home/.pi/agent/settings.json" <<JSON
{
  "extensions": [
    "$plugin/pi-worksgood/index.js"
  ]
}
JSON

(
    cd "$project" || exit 1
    HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" wg init -m claude:opus --no-agency >/dev/null 2>&1
) || loud_fail "wg init failed"

# Give wg_ready a real graph surface and make the large-output request cheap to
# satisfy through pi's built-in bash tool.
(
    cd "$project" || exit 1
    HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" wg add "pi live ready sentinel" --id pi-live-ready --no-place >/dev/null 2>&1
) || loud_fail "wg add sentinel failed"

driver="$scratch/pi_live_driver.py"
cat >"$driver" <<'PY'
import json
import os
import select
import signal
import subprocess
import sys
import time

scratch, project, session_dir, model = sys.argv[1:5]
session_id = "wg-pi-live-validation"
token = "WG_PI_RESUME_TOKEN_5572"

base_env = os.environ.copy()
base_env.update({
    "HOME": os.path.join(scratch, "home"),
    "XDG_CONFIG_HOME": os.path.join(scratch, "home", ".config"),
    "WG_DIR": os.path.join(project, ".wg"),
    "WG_PROJECT_DIR": os.path.join(project, ".wg"),
    "WG_TASK_ID": "pi-plugin-impl-live-validation",
    "WG_AGENT_ID": "pi-live-smoke",
})

def spawn():
    args = [
        "pi",
        "--mode", "rpc",
        "--provider", "openrouter",
        "--model", model,
        "--session-id", session_id,
        "--session-dir", session_dir,
        "--no-approve",
    ]
    return subprocess.Popen(
        args,
        cwd=project,
        env=base_env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
    )

def send(proc, payload):
    proc.stdin.write(json.dumps(payload) + "\n")
    proc.stdin.flush()

def read_json_line(proc, deadline):
    while time.time() < deadline:
        if proc.poll() is not None:
            err = proc.stderr.read() if proc.stderr else ""
            raise RuntimeError(f"pi exited early rc={proc.returncode}: {err[-4000:]}")
        ready, _, _ = select.select([proc.stdout], [], [], 0.25)
        if not ready:
            continue
        line = proc.stdout.readline()
        if not line:
            continue
        try:
            return json.loads(line)
        except json.JSONDecodeError:
            continue
    raise TimeoutError("timed out waiting for pi RPC line")

def contains_value(obj, needle):
    if isinstance(obj, str):
        return needle in obj
    if isinstance(obj, dict):
        return any(contains_value(v, needle) for v in obj.values())
    if isinstance(obj, list):
        return any(contains_value(v, needle) for v in obj)
    return False

def turn(proc, prompt, want_tool=False, want_full_output=False, want_token=False):
    send(proc, {"id": f"prompt-{time.time_ns()}", "type": "prompt", "message": prompt})
    deadline = time.time() + 180
    saw_agent_end = False
    saw_delta = False
    saw_wg_tool = False
    saw_full_output = None
    events = []

    while time.time() < deadline:
        ev = read_json_line(proc, deadline)
        events.append(ev)
        ty = ev.get("type")
        if ty == "message_update" and ev.get("assistantMessageEvent", {}).get("type") == "text_delta":
            if ev.get("assistantMessageEvent", {}).get("delta", "").strip():
                saw_delta = True
        if ty and str(ty).startswith("tool_") and contains_value(ev, "wg_ready"):
            saw_wg_tool = True
        if contains_value(ev, "fullOutputPath"):
            def find_path(x):
                if isinstance(x, dict):
                    if isinstance(x.get("fullOutputPath"), str) and x["fullOutputPath"]:
                        return x["fullOutputPath"]
                    for v in x.values():
                        p = find_path(v)
                        if p:
                            return p
                if isinstance(x, list):
                    for v in x:
                        p = find_path(v)
                        if p:
                            return p
                return None
            saw_full_output = saw_full_output or find_path(ev)
        if ty == "agent_end":
            saw_agent_end = True
            break

    if not saw_agent_end:
        raise AssertionError(f"agent_end not observed; events={events[-5:]}")

    send(proc, {"id": f"last-{time.time_ns()}", "type": "get_last_assistant_text"})
    final = ""
    for _ in range(64):
        ev = read_json_line(proc, time.time() + 30)
        if ev.get("type") == "response" and isinstance(ev.get("data"), dict):
            final = str(ev["data"].get("text") or "").strip()
            break
    if not final:
        raise AssertionError("get_last_assistant_text returned empty text")
    if not saw_delta:
        raise AssertionError(f"no streaming text_delta observed before agent_end; final={final!r}")
    if want_tool and not saw_wg_tool:
        raise AssertionError(f"wg_ready tool did not execute; final={final!r}")
    if want_token and token not in final:
        raise AssertionError(f"resumed session did not recall token {token}; final={final!r}")
    if want_full_output:
        if not saw_full_output:
            raise AssertionError("large output turn did not expose fullOutputPath")
        if not os.path.exists(saw_full_output):
            raise AssertionError(f"fullOutputPath does not exist: {saw_full_output}")
    return final

p = spawn()
try:
    first = turn(
        p,
        "Call the wg_ready tool exactly once. Remember this exact token for the next turn: "
        f"{token}. Then reply with 'READY ' plus the token.",
        want_tool=True,
    )
finally:
    p.kill()
    try:
        p.wait(timeout=10)
    except subprocess.TimeoutExpired:
        p.send_signal(signal.SIGKILL)
        p.wait(timeout=10)

p = spawn()
try:
    second = turn(
        p,
        "This is a resumed RPC process. Call the wg_ready tool exactly once, then answer with "
        "only the exact token I asked you to remember in the previous turn.",
        want_tool=True,
        want_token=True,
    )
    bash_prompt = (
        "Run this exact bash command and wait for it to finish: "
        "python3 -c \"import sys; [print('WG_PI_LARGE_OUTPUT_%04d' % i) for i in range(7000)]\". "
        "The output is intentionally large. After the command, reply BASH_DONE."
    )
    third = turn(p, bash_prompt, want_full_output=True)
finally:
    p.kill()
    try:
        p.wait(timeout=10)
    except subprocess.TimeoutExpired:
        p.send_signal(signal.SIGKILL)
        p.wait(timeout=10)

print(json.dumps({"first": first, "second": second, "third": third}))
PY

model="${WG_PI_LIVE_OPENROUTER_MODEL:-openai/gpt-4o-mini}"
session_dir="$scratch/pi-sessions"
mkdir -p "$session_dir"

if ! python3 "$driver" "$scratch" "$project" "$session_dir" "$model" >"$scratch/live-driver.out" 2>"$scratch/live-driver.err"; then
    loud_fail "pi live RPC validation failed for OpenRouter model $model.
stdout:
$(tail -40 "$scratch/live-driver.out" 2>/dev/null)
stderr:
$(tail -80 "$scratch/live-driver.err" 2>/dev/null)"
fi

echo "PASS: live pi RPC plugin validation streamed, resumed with plugin re-attached, and surfaced fullOutputPath"
