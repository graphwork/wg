#!/usr/bin/env bash
#
# fleet_bottom_panel_contract.sh — pins the FleetView-style bottom panel.
#
# Credential-free. Proves, against the BUILT + EMBEDDED plugin bundle:
#   * the focused fleet unit tests pass (glyph/colour formatting + config gate);
#   * the embedded bundle carries the panel (`fleet-view.js`) and its
#     `installFleetView` wiring in `index.js`;
#   * the panel is DISABLED by default (no mount) and, when enabled via the
#     extension-config/env gate, mounts a multi-line widget with
#     `placement: "belowEditor"` and clears/never mounts in non-TUI modes.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v npm >/dev/null 2>&1 || loud_skip "MISSING NPM" "npm is required for the fleet bottom-panel contract"
command -v node >/dev/null 2>&1 || loud_skip "MISSING NODE" "node is required for the fleet bottom-panel contract"

repo="$(cd "$HERE/../../.." && pwd)"
plugin="$repo/worksgood-pi"
[ -f "$plugin/package-lock.json" ] || loud_fail "missing worksgood-pi/package-lock.json" \
    "worksgood-pi/package-lock.json"

if [ ! -d "$plugin/node_modules" ]; then
    npm --prefix "$plugin" ci >/tmp/fleet-bottom-panel-npm-ci.log 2>&1 || \
        loud_skip "PI PLUGIN DEPS UNAVAILABLE" "npm ci failed: $(tail -20 /tmp/fleet-bottom-panel-npm-ci.log)"
fi

# 1. Build + focused unit tests (read model + config gate).
( cd "$plugin" && npm run build >/tmp/fleet-bottom-panel-build.log 2>&1 ) || \
    loud_fail "worksgood-pi build failed" "$(tail -40 /tmp/fleet-bottom-panel-build.log)"
( cd "$plugin" && npx vitest run test/fleet.test.ts >/tmp/fleet-bottom-panel-vitest.log 2>&1 ) || \
    loud_fail "fleet unit tests failed" "$(tail -60 /tmp/fleet-bottom-panel-vitest.log)"

# 2. The embedded bundle carries the panel.
[ -f "$plugin/embedded/pi-worksgood/fleet-view.js" ] || \
    loud_fail "embed missing fleet-view.js" "worksgood-pi/embedded/pi-worksgood/fleet-view.js"
grep -q "installFleetView" "$plugin/embedded/pi-worksgood/index.js" || \
    loud_fail "embed missing installFleetView wiring" "embedded/pi-worksgood/index.js"
grep -q "FLEET_WIDGET_KEY" "$plugin/embedded/pi-worksgood/fleet-view.js" || \
    loud_fail "embed missing FLEET_WIDGET_KEY" "embedded/pi-worksgood/fleet-view.js"

# 3. Behavioural probe against the EMBEDDED bundle: default-off gate + below-editor mount.
node --input-type=module - "$plugin" <<'NODE' >/tmp/fleet-bottom-panel-probe.log 2>&1 || \
    loud_fail "fleet bottom-panel embedded probe failed" "$(cat /tmp/fleet-bottom-panel-probe.log)"
const plugin = process.argv[2];
const { mkdtempSync } = await import("node:fs");
const { tmpdir } = await import("node:os");
const { join } = await import("node:path");
const mod = await import(`${plugin}/embedded/pi-worksgood/index.js`);

// Hermetic config gate: an empty agent dir with no config file (true default off).
process.env.PI_CODING_AGENT_DIR = mkdtempSync(join(tmpdir(), "wg-fleet-smoke-"));
// Force the read-only CLI fallback (never touch a real project daemon).
process.env.WG_DAEMON_SOCKET = join(process.env.PI_CODING_AGENT_DIR, "none.sock");
delete process.env.WG_PI_FLEET_VIEW;

const AGENTS = JSON.stringify([
  { id: "agent-7", task_id: "task-a", executor: "pi", model: "pi:openrouter:x/y", status: "working", uptime: "3m", process_alive: true },
]);
const TASKS = JSON.stringify([{ id: "task-a", title: "Task A", status: "in-progress", after: [], before: [] }]);

function makeHarness(mode, enabled) {
  const calls = [];
  const commands = [];
  const handlers = {};
  const ui = { setWidget: (...a) => calls.push(a), notify: () => {}, theme: undefined };
  const pi = {
    registerTool: () => {},
    registerCommand: (name) => commands.push(name),
    registerProvider: () => {},
    on: (event, handler) => { (handlers[event] ||= []).push(handler); },
    exec: async (_cmd, args) => {
      const verb = Array.isArray(args) ? args[args.length - 1] : "";
      void verb;
      const joined = Array.isArray(args) ? args.join(" ") : "";
      if (joined.includes("agents")) return { stdout: AGENTS, stderr: "", code: 0, killed: false };
      return { stdout: TASKS, stderr: "", code: 0, killed: false };
    },
  };
  if (enabled) process.env.WG_PI_FLEET_VIEW = "1";
  else delete process.env.WG_PI_FLEET_VIEW;  mod.default(pi);
  return { calls, commands, handlers, ctx: { mode, hasUI: mode === "tui", ui, signal: undefined } };
}

async function settle() { for (let i = 0; i < 40; i++) await new Promise((r) => setTimeout(r, 25)); }
const fireStart = (h) => { for (const handler of h.handlers.session_start ?? []) handler({}, h.ctx); };
const fleetCalls = (h) => h.calls.filter((c) => c[0] === "wg-fleet");

// Default OFF: no fleet widget on a TUI session (the /wg-viz widget is separate).
const off = makeHarness("tui", false);
fireStart(off);
await settle();
if (fleetCalls(off).length !== 0) throw new Error(`disabled panel mounted something: ${JSON.stringify(fleetCalls(off))}`);

// Enabled + TUI: a multi-line widget below the editor.
const on = makeHarness("tui", true);
fireStart(on);
await settle();
const last = fleetCalls(on).at(-1);
if (!last) throw new Error("enabled panel did not mount");
if (last[0] !== "wg-fleet") throw new Error(`unexpected widget key ${last[0]}`);
if (!last[2] || last[2].placement !== "belowEditor") throw new Error(`expected belowEditor, got ${JSON.stringify(last[2])}`);
if (!Array.isArray(last[1]) || last[1].length < 2) throw new Error(`expected multi-line widget, got ${JSON.stringify(last[1])}`);
if (!String(last[1][0]).includes("wg fleet")) throw new Error(`header missing: ${last[1][0]}`);

// Enabled + non-TUI: silent degrade (no mount).
const json = makeHarness("json", true);
fireStart(json);
await settle();
if (fleetCalls(json).length !== 0) throw new Error(`non-TUI mode mounted a widget: ${JSON.stringify(fleetCalls(json))}`);

console.log("probe ok: default-off, belowEditor mount, non-TUI silent degrade");
// Tear down the session-scoped intervals so this probe process exits cleanly.
for (const h of [off, on, json]) {
  for (const handler of h.handlers.session_shutdown ?? []) handler({}, h.ctx);
}
process.exit(0);
NODE

echo "PASS: fleet bottom panel is config-gated, mounts below the editor, embeds, and stays read-only"
