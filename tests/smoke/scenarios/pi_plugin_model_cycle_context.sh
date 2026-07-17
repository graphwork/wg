#!/usr/bin/env bash
# Regression: a globally installed WorksGood extension also loads in standalone
# Pi consoles. Ctrl-P must remain a local Pi model change unless WG explicitly
# launched the process with WG_CHAT_ID / WG_CHAT_REF.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

command -v npm >/dev/null 2>&1 || loud_skip "MISSING NPM" "npm is required for the pi plugin regression"
command -v node >/dev/null 2>&1 || loud_skip "MISSING NODE" "node is required for the pi plugin regression"

repo="$(cd "$HERE/../../.." && pwd)"
plugin="$repo/pi-plugin"
if [ ! -d "$plugin/node_modules" ]; then
    npm --prefix "$plugin" ci >/tmp/fix-pi-plugin-npm-ci.log 2>&1 || \
        loud_skip "PI PLUGIN DEPS UNAVAILABLE" "npm ci failed: $(tail -20 /tmp/fix-pi-plugin-npm-ci.log)"
fi
npm --prefix "$plugin" run build >/tmp/fix-pi-plugin-build.log 2>&1 || \
    loud_fail "pi-plugin build failed: $(tail -80 /tmp/fix-pi-plugin-build.log)"

# Always-on credential-free event flow. `fire` models Pi's documented Ctrl-P
# order: Pi changes its selected model, then emits model_select(source=cycle).
node --input-type=module - "$plugin" <<'NODE' || loud_fail "plugin model-cycle context contract failed"
const plugin = process.argv[2];
const { installModelBridge, readWgEnv, WgBackend } = await import(`${plugin}/dist/index.js`);

function runner() {
  let handler;
  let selected;
  const pi = {
    registerProvider() {},
    on(event, fn) { if (event === "model_select") handler = fn; },
  };
  return {
    pi,
    get selected() { return selected; },
    async ctrlP(model) {
      selected = `${model.provider}:${model.id}`;
      await handler({ type: "model_select", model, source: "cycle" });
    },
  };
}

// Standalone: OpenRouter and local llama.cpp follow the identical context gate.
const standaloneCalls = [];
const standaloneErrors = [];
const standaloneHost = { exec: async (command, args) => {
  standaloneCalls.push([command, args]);
  return { stdout: "", stderr: "", code: 0, killed: false };
}};
const oldError = console.error;
console.error = (...args) => standaloneErrors.push(args);
try {
  const run = runner();
  installModelBridge(run.pi, new WgBackend(standaloneHost, readWgEnv({})), {});
  await run.ctrlP({ provider: "openrouter", id: "qwen/qwen3.6-flash" });
  if (run.selected !== "openrouter:qwen/qwen3.6-flash") throw new Error("OpenRouter selection did not change");
  await run.ctrlP({ provider: "llamacpp", id: "llama-3.3-local" });
  if (run.selected !== "llamacpp:llama-3.3-local") throw new Error("local llama selection did not change");
} finally {
  console.error = oldError;
}
if (standaloneCalls.length !== 0) throw new Error(`standalone made WG calls: ${JSON.stringify(standaloneCalls)}`);
if (standaloneErrors.length !== 0) throw new Error(`standalone logged errors: ${JSON.stringify(standaloneErrors)}`);

// Canonical managed identity: exactly one command to the exact .chat-N.
const managedCalls = [];
const managedHost = { exec: async (command, args) => {
  managedCalls.push([command, args]);
  return { stdout: "ok", stderr: "", code: 0, killed: false };
}};
const managed = runner();
installModelBridge(
  managed.pi,
  new WgBackend(managedHost, readWgEnv({ WG_CHAT_ID: ".chat-12", WG_DIR: "/project/.wg" })),
  {},
);
await managed.ctrlP({ provider: "openrouter", id: "qwen/qwen3.6-max-preview" });
if (managedCalls.length !== 1) throw new Error(`managed write count=${managedCalls.length}`);
const expected = ["--dir", "/project/.wg", "chat", "model", ".chat-12", "openrouter:qwen/qwen3.6-max-preview", "--warm-pi-writeback"];
if (JSON.stringify(managedCalls[0]) !== JSON.stringify(["wg", expected])) {
  throw new Error(`wrong managed target: ${JSON.stringify(managedCalls[0])}`);
}

// Compatibility alias normalizes to the same canonical graph task id.
const aliasCalls = [];
const alias = runner();
installModelBridge(alias.pi, new WgBackend({ exec: async (command, args) => {
  aliasCalls.push([command, args]);
  return { stdout: "ok", stderr: "", code: 0, killed: false };
}}, readWgEnv({ WG_CHAT_REF: "chat-9" })), {});
await alias.ctrlP({ provider: "llamacpp", id: "llama-3.3-local" });
if (aliasCalls.length !== 1 || aliasCalls[0][1][2] !== ".chat-9") {
  throw new Error(`WG_CHAT_REF alias did not target .chat-9: ${JSON.stringify(aliasCalls)}`);
}

// Genuine managed failure: visible once, one line, no Error object/stack spam.
const failureLines = [];
console.error = (...args) => failureLines.push(args);
try {
  const failure = runner();
  installModelBridge(failure.pi, new WgBackend({ exec: async () => ({
    stdout: "",
    stderr: "daemon refused write\n  at noisy-internal-frame",
    code: 7,
    killed: false,
  }) }, { chatId: ".chat-4" }), {});
  await failure.ctrlP({ provider: "openrouter", id: "qwen/qwen3.6-flash" });
} finally {
  console.error = oldError;
}
if (failureLines.length !== 1 || failureLines[0].length !== 1) {
  throw new Error(`managed failure was not exactly one console line: ${JSON.stringify(failureLines)}`);
}
const failureText = String(failureLines[0][0]);
if (!failureText.includes(".chat-4") || failureText.includes("\n") || failureText.includes("    at ")) {
  throw new Error(`managed failure was not concise/actionable: ${failureText}`);
}
NODE

# Real WG/TUI launch path: a fake interactive Pi process receives the child
# environment, loads the built bridge, and emits one cycle event. The current
# worktree binary is preferred so this gate does not require installing an
# unmerged agent branch globally.
if command -v tmux >/dev/null 2>&1; then
    wg_bin="$repo/target/debug/wg"
    if [ ! -x "$wg_bin" ]; then
        wg_bin="$(command -v wg 2>/dev/null || true)"
        if [ -n "$wg_bin" ] && [ "$("$wg_bin" pi-plugin compat-version 2>/dev/null || true)" != "0.1.1" ]; then
            echo "pi_plugin_model_cycle_context: managed TUI sub-check deferred (installed wg predates compat 0.1.1)" >&2
            wg_bin=""
        fi
    fi

    if [ -n "$wg_bin" ]; then
    managed_scratch="$(make_scratch)"
    managed_home="$managed_scratch/home"
    managed_graph="$managed_scratch/project/.wg"
    managed_bin="$managed_scratch/bin"
    managed_env="$managed_scratch/pi-env"
    managed_done="$managed_scratch/model-cycle-done"
    mkdir -p "$managed_home" "$managed_graph" "$managed_bin"
    : >"$managed_graph/graph.jsonl"
    printf '[dispatcher]\nmodel = "pi:openrouter:qwen/old"\n' >"$managed_graph/config.toml"

    HOME="$managed_home" "$wg_bin" --dir "$managed_graph" chat create \
      --name managed-pi --exec pi --model pi:openrouter:qwen/old --json \
      >"$managed_scratch/chat-create.json" 2>&1 || \
      loud_fail "could not create managed Pi chat: $(cat "$managed_scratch/chat-create.json")"

    cat >"$managed_scratch/emit-cycle.mjs" <<'NODE'
import { execFile } from "node:child_process";
import { promisify } from "node:util";
const execFileP = promisify(execFile);
const mod = await import(process.env.MANAGED_PLUGIN_ENTRY);
let handler;
const pi = {
  registerProvider() {},
  on(event, fn) { if (event === "model_select") handler = fn; },
};
const host = { async exec(_command, args) {
  try {
    const r = await execFileP(process.env.MANAGED_WG_BIN, args, { env: process.env });
    return { stdout: r.stdout, stderr: r.stderr, code: 0, killed: false };
  } catch (err) {
    return { stdout: err.stdout ?? "", stderr: err.stderr ?? String(err), code: err.code ?? 1, killed: false };
  }
}};
const backend = new mod.WgBackend(host, mod.readWgEnv(process.env));
mod.installModelBridge(pi, backend, process.env);
await handler({
  type: "model_select",
  source: "cycle",
  model: { provider: "openrouter", id: "qwen/qwen3.6-flash" },
});
NODE
    cat >"$managed_bin/pi" <<'SH'
#!/bin/sh
printf 'WG_CHAT_ID=%s\nWG_CHAT_REF=%s\nWG_DIR=%s\n' \
  "${WG_CHAT_ID-}" "${WG_CHAT_REF-}" "${WG_DIR-}" >"${MANAGED_ENV_FILE:?}"
node "${MANAGED_EMITTER:?}" >"${MANAGED_EMITTER_LOG:?}" 2>&1 && : >"${MANAGED_DONE_FILE:?}"
printf 'MANAGED_PI_MODEL_CYCLE_DONE\n'
while IFS= read -r _line; do :; done
SH
    chmod +x "$managed_bin/pi"

    managed_sock="wgsmoke-pi-model-context-$$"
    managed_session="wgsmoke-pi-model-context-$$"
    cleanup_managed_tmux() { tmux -L "$managed_sock" kill-server 2>/dev/null || true; }
    add_cleanup_hook cleanup_managed_tmux
    HOME="$managed_home" PATH="$managed_bin:$PATH" \
      MANAGED_ENV_FILE="$managed_env" MANAGED_DONE_FILE="$managed_done" \
      MANAGED_EMITTER="$managed_scratch/emit-cycle.mjs" \
      MANAGED_EMITTER_LOG="$managed_scratch/emitter.log" \
      MANAGED_PLUGIN_ENTRY="file://$plugin/dist/index.js" MANAGED_WG_BIN="$wg_bin" \
      tmux -L "$managed_sock" new-session -d -s "$managed_session" -x 160 -y 40 \
      "$wg_bin --dir $managed_graph tui"

    for _ in $(seq 1 80); do
        [ -f "$managed_done" ] && break
        sleep 0.25
    done
    [ -f "$managed_done" ] || loud_fail "WG TUI Pi child did not complete model cycle: $(cat "$managed_scratch/emitter.log" 2>/dev/null)"
    grep -q '^WG_CHAT_ID=\.chat-0$' "$managed_env" || loud_fail "TUI Pi child missing canonical WG_CHAT_ID: $(cat "$managed_env")"
    grep -q '^WG_CHAT_REF=chat-0$' "$managed_env" || loud_fail "TUI Pi child missing WG_CHAT_REF alias: $(cat "$managed_env")"
    python3 - "$managed_graph" <<'PY' || loud_fail "managed TUI cycle did not persist exact override"
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
state = json.loads((root / "service/coordinator-state-0.json").read_text())
assert state["executor_override"] == "pi", state
assert state["model_override"] == "pi:openrouter:qwen/qwen3.6-flash", state
assert not (root / "service/coordinator-state-1.json").exists(), "write leaked to another chat"
PY
    fi
fi

# Optional real terminal flow. It starts a genuine standalone Pi console with
# two dummy-auth custom models, sends the actual Ctrl-P byte repeatedly through
# a PTY, and records every non-handshake `wg` invocation. No model request is
# submitted, so this needs no credentials or live llama server.
if command -v pi >/dev/null 2>&1 && command -v python3 >/dev/null 2>&1; then
    scratch="$(make_scratch)"
    home="$scratch/home"
    fakebin="$scratch/bin"
    mkdir -p "$home/.pi/agent" "$fakebin"
    calls="$scratch/wg-calls"
    output="$scratch/pi-output"
    cat >"$fakebin/wg" <<'SH'
#!/bin/sh
if [ "$*" = "pi-plugin compat-version" ]; then
  echo 0.1.1
  exit 0
fi
printf '%s\n' "$*" >>"${MODEL_CYCLE_WG_CALLS:?}"
exit 0
SH
    chmod +x "$fakebin/wg"
    cat >"$home/.pi/agent/settings.json" <<JSON
{
  "quietStartup": true,
  "defaultProjectTrust": "never",
  "enabledModels": ["qwen/qwen3.6-flash", "llama-3.3-local"]
}
JSON
    cat >"$home/.pi/agent/models.json" <<'JSON'
{
  "providers": {
    "openrouter": {
      "baseUrl": "https://openrouter.invalid/v1",
      "api": "openai-completions",
      "apiKey": "dummy",
      "models": [{"id":"qwen/qwen3.6-flash"}]
    },
    "llamacpp": {
      "baseUrl": "http://127.0.0.1:1/v1",
      "api": "openai-completions",
      "apiKey": "dummy",
      "models": [{"id":"llama-3.3-local"}]
    }
  }
}
JSON
    env -u WG_CHAT_ID -u WG_CHAT_REF \
      HOME="$home" PATH="$fakebin:$PATH" PI_OFFLINE=1 \
      MODEL_CYCLE_WG_CALLS="$calls" PLUGIN_ENTRY="$plugin/dist/index.js" PI_CAPTURE="$output" \
      python3 <<'PY'
import os, pty, select, signal, subprocess, time
cmd = ["pi", "--provider", "openrouter", "--model", "qwen/qwen3.6-flash", "-e", os.environ["PLUGIN_ENTRY"], "-ne"]
master, slave = pty.openpty()
p = subprocess.Popen(cmd, stdin=slave, stdout=slave, stderr=slave, env=os.environ.copy(), close_fds=True)
os.close(slave)
data = bytearray()
def drain(seconds):
    end = time.time() + seconds
    while time.time() < end:
        ready, _, _ = select.select([master], [], [], 0.1)
        if ready:
            try: data.extend(os.read(master, 65536))
            except OSError: return
try:
    drain(2.5)
    for _ in range(4):
        os.write(master, b"\x10")  # real app.model.cycleForward / Ctrl-P
        drain(0.5)
    os.write(master, b"\x04")
    drain(1.0)
finally:
    if p.poll() is None:
        p.terminate()
        try: p.wait(timeout=3)
        except subprocess.TimeoutExpired:
            p.kill(); p.wait()
    os.close(master)
    open(os.environ["PI_CAPTURE"], "wb").write(data)
PY
    [ ! -s "$calls" ] || loud_fail "standalone Ctrl-P invoked wg: $(cat "$calls")"
    if grep -aEq 'model write-back failed|cannot write model override|[[:space:]]at WgBackend' "$output"; then
        loud_fail "standalone Pi printed model write-back stack noise: $(tail -30 "$output")"
    fi
else
    echo "pi_plugin_model_cycle_context: live Ctrl-P sub-check unavailable (pi/python3 missing); always-on event flow passed" >&2
fi

echo "PASS: standalone OpenRouter/local Ctrl-P cycles are graph-inert; managed canonical/alias writes target once; failures stay concise"
