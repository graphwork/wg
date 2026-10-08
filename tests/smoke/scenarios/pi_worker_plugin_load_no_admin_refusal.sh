#!/usr/bin/env bash
# Scenario: pi_worker_plugin_load_no_admin_refusal
#
# Regression for hotfix-worker-plugin-load. Commit 198025aa ("self-heal the
# console plugin cache at console launch") added a load-time self-heal to
# worksgood-pi/src/index.ts that shells `wg pi-plugin compat-version` / `digest`
# in EVERY pi session. A WG-managed WORKER session carries WG_AGENT_ID (trusted
# control mode), and `src/worker_cli.rs` refuses admin verbs there:
#
#   worker_control.admin_operation_refused: command is outside trusted local
#   graph coordination
#
# so the console feature killed every fresh worker spawn before it ran a tool
# (observed on agent-231 / agent-232). The fix makes the self-heal CONSOLE-ONLY:
# detect the worker context (WG_AGENT_ID / WG_WORKER_CAPABILITY) and skip
# entirely — the daemon's JIT pre-spawn `ensure-pi-plugin` already keeps a
# worker's cache current.
#
# Credential-free. Loads the real built plugin bundle with a node harness whose
# `pi.exec` shells the REAL candidate `wg`, so the assertions exercise the exact
# refusal path the regression came from.
#
# Checks:
#   1. control: the candidate `wg` DOES refuse `pi-plugin digest` under the
#      worker env (the gate this regression weaponized is active);
#   2. worker session (WG_AGENT_ID set) — plugin load runs ZERO wg subprocesses;
#   3. worker session — a real tool call (wg_ready) reaches `wg` and its output
#      never contains admin_operation_refused;
#   4. console session (no worker env) — the self-heal still runs (wg
#      subprocesses include pi-plugin digest/compat-version).

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v node >/dev/null 2>&1 || loud_skip "MISSING NODE" "node is required for the pi-plugin worker-load contract"

repo="$(cd "$HERE/../../.." && pwd)"
plugin="$repo/worksgood-pi"
[ -f "$plugin/package.json" ] || loud_fail "missing worksgood-pi/package.json"

if [ ! -f "$plugin/pi-worksgood/index.js" ]; then
    command -v npm >/dev/null 2>&1 || loud_skip "MISSING NPM" "npm is required to build the pi-plugin bundle"
    if [ ! -d "$plugin/node_modules" ]; then
        npm --prefix "$plugin" ci >/tmp/pi-worker-plugin-npm-ci.log 2>&1 || \
            loud_skip "PI PLUGIN DEPS UNAVAILABLE" "npm ci failed: $(tail -20 /tmp/pi-worker-plugin-npm-ci.log)"
    fi
    npm --prefix "$plugin" run build >/tmp/pi-worker-plugin-build.log 2>&1 || \
        loud_fail "pi-plugin build failed: $(tail -40 /tmp/pi-worker-plugin-build.log)"
fi

# Candidate wg binary — MUST carry the fix's semantics (a worker refuses
# pi-plugin). Prefer the worktree/candidate build over a stale installed one.
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" && -x "$WG_SMOKE_CANDIDATE_BIN" ]]; then
    WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
elif [[ -x "${CARGO_TARGET_DIR:-$repo/target}/debug/wg" ]]; then
    WG_BIN="${CARGO_TARGET_DIR:-$repo/target}/debug/wg"
else
    WG_BIN="$(command -v wg)"
fi

scratch="$(make_scratch)"
bindir="$scratch/bin"
fake_home="$scratch/home"
project="$scratch/project"
mkdir -p "$bindir" "$fake_home/.config/workgraph" "$project"
ln -sf "$WG_BIN" "$bindir/wg"

# Real graph for the worker tool call to reach.
(
    cd "$project" || exit 1
    env -u WG_AGENT_ID -u WG_TASK_ID -u WG_DIR -u WG_WORKER_CONTROL_MODE \
        HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" PATH="$bindir:$PATH" \
        "$WG_BIN" init --no-agency >/dev/null 2>&1
) || loud_fail "wg init failed for the fixture project"

worker_env=(
    "WG_AGENT_ID=agent-hotfix-test"
    "WG_TASK_ID=hotfix-fixture-task"
    "WG_WORKER_CONTROL_MODE=trusted"
    "WG_DIR=$project"
)

# Any worker vars inherited from the surrounding session are cleared before
# each assertion so only the intended worker context is exercised.
strip_worker_env=(
    -u WG_GRAPH_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC
    -u WG_WORKER_CONTROL_PROTOCOL -u WG_PI_PLUGIN_COMPAT_VERSION
    -u WG_WORKER_ATTEMPT_ID -u WG_WORKER_ATTEMPT_FENCE
)

# ── 1. control: the worker gate really refuses pi-plugin ────────────
control_out="$(env "${strip_worker_env[@]}" \
    HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" PATH="$bindir:$PATH" \
    "${worker_env[@]}" "$WG_BIN" pi-plugin digest 2>&1)" || true
grep -q "admin_operation_refused" <<<"$control_out" \
    || loud_fail "control: candidate wg did NOT refuse 'pi-plugin digest' under the worker env; got: $control_out"

# ── 2/3/4. drive the real plugin bundle through a node harness ──────
result="$(env "${strip_worker_env[@]}" HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" \
    XDG_CACHE_HOME="$scratch/cache" PATH="$bindir:$PATH" \
    PLUGIN_DIR="$plugin" PROJECT="$project" WG_BIN="$WG_BIN" \
    node --input-type=module - <<'NODE'
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";

const pluginDir = process.env.PLUGIN_DIR;
const project = process.env.PROJECT;
const wgBin = process.env.WG_BIN;
const mod = await import(`${pluginDir}/pi-worksgood/index.js`);

function makePi() {
  const calls = [];
  const tools = new Map();
  const pi = {
    calls,
    tools,
    registerTool: (t) => { tools.set(t.name, t); },
    registerCommand: () => {},
    registerProvider: () => {},
    on: () => {},
    exec: async (command, args) => {
      calls.push([command, ...args]);
      const r = spawnSync(command, args, {
        encoding: "utf8",
        env: process.env,
      });
      return { stdout: r.stdout ?? "", stderr: r.stderr ?? "", code: r.status ?? 1, killed: false };
    },
  };
  return pi;
}

function withEnv(overrides, fn) {
  const saved = {};
  for (const key of Object.keys(overrides)) {
    saved[key] = process.env[key];
    if (overrides[key] === undefined) delete process.env[key];
    else process.env[key] = overrides[key];
  }
  try {
    return fn();
  } finally {
    for (const key of Object.keys(overrides)) {
      if (saved[key] === undefined) delete process.env[key];
      else process.env[key] = saved[key];
    }
  }
}

function toolText(res) {
  const parts = res?.content ?? [];
  return parts.map((p) => (typeof p === "string" ? p : p?.text ?? "")).join("\n");
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const out = {};

// Worker session: WG_AGENT_ID set, no WG_PI_PLUGIN_COMPAT_VERSION.
await withEnv(
  {
    WG_AGENT_ID: "agent-hotfix-test",
    WG_TASK_ID: "hotfix-fixture-task",
    WG_WORKER_CONTROL_MODE: "trusted",
    WG_DIR: project,
    WG_WORKER_CAPABILITY: undefined,
    WG_PI_PLUGIN_COMPAT_VERSION: undefined,
    WG_GRAPH_ID: undefined,
    WG_WORKER_IPC: undefined,
    WG_WORKER_CONTROL_PROTOCOL: undefined,
    WG_WORKER_ATTEMPT_ID: undefined,
    WG_WORKER_ATTEMPT_FENCE: undefined,
  },
  async () => {
    const pi = makePi();
    mod.default(pi);
    await sleep(250); // let any (buggy) async self-heal fire
    out.workerLoadCalls = pi.calls.map((c) => c.join(" "));
    const ready = pi.tools.get("wg_ready");
    const res = await ready.execute("id", {}, undefined);
    out.workerToolText = toolText(res);
    out.workerToolCalls = pi.calls.map((c) => c.join(" "));
  },
);

// Console session: no worker env → self-heal must still run.
await withEnv(
  {
    WG_AGENT_ID: undefined,
    WG_TASK_ID: undefined,
    WG_WORKER_CAPABILITY: undefined,
    WG_WORKER_CONTROL_MODE: undefined,
    WG_PI_PLUGIN_COMPAT_VERSION: undefined,
  },
  async () => {
    const pi = makePi();
    mod.default(pi);
    await sleep(500);
    out.consoleLoadCalls = pi.calls.map((c) => c.join(" "));
  },
);

console.log(JSON.stringify(out));
NODE
)" || loud_fail "node harness failed: $result"

if ! python3 - "$result" <<'PY'
import json, sys
out = json.loads(sys.argv[1])
load = out.get("workerLoadCalls", [])
# 2. worker load must invoke NO wg subprocess at all.
assert load == [], f"worker plugin load invoked wg subprocesses: {load}"
# 3. a real tool call must reach wg and never surface the admin refusal.
tool_text = out.get("workerToolText", "")
assert "admin_operation_refused" not in tool_text, f"worker tool call hit the admin refusal: {tool_text!r}"
tool_calls = out.get("workerToolCalls", [])
assert any(c.startswith("wg ") and " ready" in c for c in tool_calls), \
    f"worker tool call did not reach `wg ready`: {tool_calls}"
# 4. console load still self-heals (a pi-plugin subprocess runs).
console = out.get("consoleLoadCalls", [])
assert any("pi-plugin" in c for c in console), f"console self-heal did not run: {console}"
PY
then
    loud_fail "worker/console plugin-load assertions failed (see traceback above)"
fi

echo "PASS: pi worker plugin loads with zero wg subprocesses and reaches a tool call without admin refusal; console self-heal still runs"
exit 0
