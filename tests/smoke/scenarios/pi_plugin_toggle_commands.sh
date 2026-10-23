#!/usr/bin/env bash
# Scenario: pi_plugin_toggle_commands
#
# Pins PLUGIN ON/OFF AS COMMANDS — now SESSION-SCOPED ACTIVATION
# (wg-session-activation): the pi plugin exposes `/wg on`, `/wg off`, and
# `/wg status` so a human never hand-edits ~/.pi/agent/settings.json to get WG
# in (or out of) a session.
#
# This scenario used to pin the DESTRUCTIVE form of these verbs (`on` ran
# `wg pi-plugin install`, `off` removed the settings entry). That form was a
# catch-22 — the next session lost `/wg` itself. The contract pinned here is the
# replacement:
#
#   /wg on      activates the wg_* tool family in THIS session via
#               `pi.setActiveTools` and marks the project active with a marker
#               file INSIDE an existing .wg/.
#   /wg off     deactivates the tools, unmounts the surfaces, and clears the
#               marker — it NEVER touches settings.json, NEVER removes the
#               extension, and NEVER touches .wg/ or the daemon.
#   /wg status  one compact truth line: session · project · tools · daemon.
#
# Human-flow proof (credential-free): the built plugin's REAL registered `wg`
# command handler is invoked exactly as pi would invoke it (fake pi that models
# pi's name-replacing setActiveTools + fake ctx). `wg pi-plugin disable` stays
# the CLI-only uninstall and is asserted NOT to be called.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v node >/dev/null 2>&1 || loud_skip "MISSING NODE" "node is required for the session-activation contract"

REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null || echo "$HERE/../..")"
plugin="$REPO_ROOT/worksgood-pi"

# Require the built plugin artifact (npm test/build produce pi-worksgood/).
if [ ! -f "$plugin/pi-worksgood/commands.js" ] || [ ! -f "$plugin/pi-worksgood/activation.js" ]; then
    command -v npm >/dev/null 2>&1 || loud_skip "MISSING NPM" "npm needed to build worksgood-pi/pi-worksgood"
    if [ ! -d "$plugin/node_modules" ]; then
        npm --prefix "$plugin" ci >/tmp/pi-worksgood-toggle-npm-ci.log 2>&1 || \
            loud_skip "PI PLUGIN DEPS UNAVAILABLE" "npm ci failed: $(tail -20 /tmp/pi-worksgood-toggle-npm-ci.log)"
    fi
    npm --prefix "$plugin" run build >/tmp/pi-worksgood-toggle-build.log 2>&1 || \
        loud_fail "pi-worksgood build failed: $(tail -40 /tmp/pi-worksgood-toggle-build.log)"
fi

scratch="$(make_scratch)"
fake_home="$scratch/home"
project_dir="$scratch/project"
mkdir -p "$fake_home" "$project_dir/.wg"
settings="$fake_home/.pi/agent/settings.json"

log="$scratch/handler.log"
if ! node --input-type=module - "$plugin" "$fake_home" "$project_dir" \
        >"$log" 2>&1 <<'NODE'
const [plugin, fakeHome, projectDir] = process.argv.slice(2);

const { registerWgCommands } = await import(`${plugin}/pi-worksgood/commands.js`);
const { registerWgTools, WG_TOOL_NAMES } = await import(`${plugin}/pi-worksgood/tools.js`);
const { installSessionActivation, activationMarkerPath } = await import(`${plugin}/pi-worksgood/activation.js`);
const { WgBackend } = await import(`${plugin}/pi-worksgood/wg-backend.js`);

const assert = (cond, msg) => { if (!cond) throw new Error(msg); };
const fs = await import("node:fs/promises");
const exists = async (p) => { try { await fs.access(p); return true; } catch { return false; } };
// The settings.json a console pi would load — this contract must never write it.
const settings = `${fakeHome}/.pi/agent/settings.json`;

// The plugin always shells `wg`; record the argv so "never the plugin
// lifecycle / never the daemon" is proven, not assumed.
const wgCalls = [];
const host = {
  exec: async (command, args) => {
    wgCalls.push([command, ...args].join(" "));
    return { stdout: "", stderr: "unused in this contract", code: 1, killed: false };
  },
};
const backend = new WgBackend(host, {});

// A fake pi that models pi's ACTUAL active-set contract: setActiveTools REPLACES
// the set by name, unknown names are ignored, and `defaultActive: false` tools
// are registered but not activated.
const tools = [];
let active = ["read", "bash", "edit", "write"];
const commands = new Map();
const handlers = new Map();
const pi = {
  registerTool: (tool) => { tools.push(tool.name); },
  registerCommand: (name, opts) => commands.set(name, opts),
  on: (event, handler) => {
    const list = handlers.get(event) ?? [];
    list.push(handler);
    handlers.set(event, list);
  },
  sendUserMessage: () => {},
  setModel: async () => true,
  getActiveTools: () => [...active],
  getAllTools: () => tools.map((n) => ({ name: n, active: active.includes(n) })),
  setActiveTools: (names) => {
    active = names.filter((n) => tools.includes(n) || ["read", "bash", "edit", "write"].includes(n));
  },
};

registerWgTools(pi, backend);
const activation = installSessionActivation(pi, backend, {}, {});
registerWgCommands(pi, backend, activation);

const notes = [];
const ctx = { hasUI: true, ui: { notify: (t, type) => notes.push({ t, type }) }, signal: undefined, cwd: projectDir, mode: "tui" };
const last = () => notes[notes.length - 1].t;
const all = () => notes.map((n) => n.t).join("\n");
const wgVisible = () => active.filter((n) => n.startsWith("wg_"));
const wg = commands.get("wg");

// 0. The command surface still advertises the verbs, and the tools are dormant.
const subs = new Set((wg.getArgumentCompletions?.("") ?? []).map((c) => c.value));
for (const want of ["on", "off", "status"]) assert(subs.has(want), `completions missing ${want}`);
assert(wgVisible().length === 0, `wg tools must be inactive at load, saw ${wgVisible()}`);
assert(tools.length === WG_TOOL_NAMES.length, `expected ${WG_TOOL_NAMES.length} registered wg tools, got ${tools.length}`);

// 1. `/wg on` activates exactly the wg family in this session — and touches no
//    settings.json (the old `on` ran `wg pi-plugin install`).
await wg.handler("on", ctx);
assert(wgVisible().length === WG_TOOL_NAMES.length, `on did not activate the wg family: ${wgVisible()}`);
assert(active.includes("read"), "on must keep the built-in tools active");
assert(/WG active in this session/.test(last()), `on output is not a confirmation: ${last()}`);
assert(/context re-priced/.test(last()), `on should note the one-time context repricing: ${last()}`);
assert(!(await exists(settings)), `on must NOT create/rewrite settings.json: ${settings}`);
assert(!wgCalls.some((c) => /pi-plugin (install|disable)/.test(c)), `on must not run the plugin lifecycle: ${wgCalls.join(", ")}`);

// 2. The per-project marker was written INSIDE the existing .wg/.
const marker = activationMarkerPath(projectDir);
assert(await exists(marker), `on must write the project marker at ${marker}`);
assert((await fs.readFile(marker, "utf8")) === "active\n", "marker contents drifted from the documented 'active\\n'");
assert(await exists(`${projectDir}/.wg`), "on must never remove .wg/");

// 3. A NEW session of the marked project starts ACTIVE (persistence).
{
  let active2 = ["read", "bash", "edit", "write"];
  const tools2 = [];
  const h2 = new Map();
  const pi2 = {
    registerTool: (t) => tools2.push(t.name),
    registerCommand: () => {},
    on: (e, h) => h2.set(e, [...(h2.get(e) ?? []), h]),
    getActiveTools: () => [...active2],
    setActiveTools: (names) => { active2 = names.filter((n) => tools2.includes(n) || ["read", "bash", "edit", "write"].includes(n)); },
    getAllTools: () => tools2.map((n) => ({ name: n, active: active2.includes(n) })),
  };
  registerWgTools(pi2, new WgBackend(host, {}));
  installSessionActivation(pi2, new WgBackend(host, {}), {}, {});
  const startCtx = { mode: "tui", hasUI: true, cwd: projectDir, ui: { notify: () => {}, setWidget: () => {} }, signal: undefined };
  for (const handler of h2.get("session_start") ?? []) await handler({}, startCtx);
  await new Promise((r) => setTimeout(r, 10));
  const visible2 = active2.filter((n) => n.startsWith("wg_"));
  assert(visible2.length === WG_TOOL_NAMES.length, `a marked project must start active in a new session, saw ${visible2.length}`);
}

// 4. `/wg off` deactivates, clears the marker, and touches nothing else.
const callsBeforeOff = wgCalls.length;
await wg.handler("off", ctx);
assert(wgVisible().length === 0, `off did not deactivate the wg family: ${wgVisible()}`);
assert(active.includes("read"), "off must keep the built-in tools active");
assert(!(await exists(marker)), "off must clear the project marker");
assert(await exists(`${projectDir}/.wg`), "off must never remove .wg/");
assert(!(await exists(settings)), "off must never touch settings.json");
assert(!wgCalls.some((c) => /pi-plugin (disable|install)/.test(c)), `off must not run the plugin lifecycle: ${wgCalls.join(", ")}`);
assert(wgCalls.length === callsBeforeOff, `off must not shell wg at all, saw: ${wgCalls.slice(callsBeforeOff).join(", ")}`);
assert(/WG dormant in this session/.test(last()), `off output is not a confirmation: ${last()}`);
assert(/settings\.json/.test(last()) && /untouched/.test(last()), `off must state what it left alone: ${last()}`);
assert(/running agents are separate processes/.test(last()), `off must state that running agents are unaffected: ${last()}`);

// 5. `/wg off` is idempotent and never strands `/wg` (the old catch-22).
await wg.handler("off", ctx);
assert(/WG dormant in this session/.test(last()), `second off should stay honestly dormant: ${last()}`);
assert(commands.has("wg") && commands.has("wg-model"), "the /wg commands must survive every toggle");
assert(!(await exists(settings)), "repeated off must still never touch settings.json");

// 6. `/wg on` works again after an off (self-referential safety), and
//    `/wg status` reports the real state in the required shape.
await wg.handler("on", ctx);
assert(wgVisible().length === WG_TOOL_NAMES.length, "on after off must reactivate the wg family");
await wg.handler("status", ctx);
const statusLine = last();
assert(!statusLine.includes("\n"), `status is not one line: ${statusLine}`);
assert(/^session: (active|dormant) · project: (marked active|dormant) · tools: \d+ wg tools visible · daemon: (running|stopped|unknown)$/.test(statusLine),
  `status shape wrong: ${statusLine}`);
assert(/^session: active/.test(statusLine), `status must report the session as active: ${statusLine}`);
assert(/project: marked active/.test(statusLine), `status must report the project marker: ${statusLine}`);
assert(/tools: 12 wg tools visible/.test(statusLine), `status must report the visible tool count: ${statusLine}`);
assert(/daemon: (stopped|unknown)/.test(statusLine), `the fake wg has no daemon: ${statusLine}`);

console.log("pi_plugin_toggle_commands: handler flow OK");
NODE
then
    loud_fail "pi /wg on|off|status session-activation human-flow contract failed: $(tail -80 "$log")"
fi

grep -q "handler flow OK" "$log" || loud_fail "handler flow did not complete: $(tail -40 "$log")"

echo "pi_plugin_toggle_commands: PASS"
exit 0
