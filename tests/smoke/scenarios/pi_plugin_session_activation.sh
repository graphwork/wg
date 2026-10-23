#!/usr/bin/env bash
# Scenario: pi_plugin_session_activation
#
# Pins SESSION-SCOPED WG ACTIVATION (wg-session-activation): the accumulated
# tangle this replaces was that the plugin registered ~13 model-visible wg_*
# tools in EVERY pi session, in EVERY directory, unconditionally — so the agent
# permanently "perceived" WG whether invited or not — while the earlier
# off-switches were destructive (removing the settings entry cost the NEXT
# session its `/wg` command).
#
# Credential-free human-flow proof: the REAL plugin factory (`worksgoodPi`) is
# loaded with a fake pi that models pi's actual contracts —
#   * `registerTool` honours `exposure` / `defaultActive` (pi's
#     `_isActivatedOnRegistration`), so `defaultActive: false` tools are
#     registered but NOT active;
#   * `setActiveTools(names)` REPLACES the active set by name;
#   * `getActiveTools()` is the set declared to the model;
#   * commands are separate from tools and always registered.
#
# Asserted:
#   1. LOAD TIME in any directory: ZERO wg tools active, all 12 REGISTERED, and
#      `/wg` + `/wg-model` still available (commands are the affordance).
#   2. `/wg on` activates exactly the wg set mid-session; `/wg off` removes
#      exactly it; the built-in tools survive both.
#   3. NEITHER verb touches settings.json, removes the extension, removes `.wg/`,
#      or shells the plugin lifecycle / daemon (`wg pi-plugin disable` stays the
#      CLI-only uninstall).
#   4. The per-project marker lives at `.wg/session-active` ('active\n'): a
#      booted project starts ACTIVE in a NEW session; a never-booted project
#      stays dormant and never gets a `.wg/` created for it.
#   5. Surfaces follow the session state: with the fleetView config gate ON, the
#      below-editor panel mounts on `/wg on` and is cleared by `/wg off`.
#   6. `/wg status` prints the four required fields on one line.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v node >/dev/null 2>&1 || loud_skip "MISSING NODE" "node is required for the session-activation contract"

REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null || echo "$HERE/../..")"
plugin="$REPO_ROOT/worksgood-pi"

if [ ! -f "$plugin/pi-worksgood/index.js" ] || [ ! -f "$plugin/pi-worksgood/activation.js" ]; then
    command -v npm >/dev/null 2>&1 || loud_skip "MISSING NPM" "npm needed to build worksgood-pi/pi-worksgood"
    if [ ! -d "$plugin/node_modules" ]; then
        npm --prefix "$plugin" ci >/tmp/pi-worksgood-activation-npm-ci.log 2>&1 || \
            loud_skip "PI PLUGIN DEPS UNAVAILABLE" "npm ci failed: $(tail -20 /tmp/pi-worksgood-activation-npm-ci.log)"
    fi
    npm --prefix "$plugin" run build >/tmp/pi-worksgood-activation-build.log 2>&1 || \
        loud_fail "pi-worksgood build failed: $(tail -40 /tmp/pi-worksgood-activation-build.log)"
fi

scratch="$(make_scratch)"
agent_dir="$scratch/agent"
booted="$scratch/booted-project"
fresh="$scratch/fresh-project"
mkdir -p "$agent_dir" "$booted/.wg" "$fresh"

log="$scratch/activation.log"
if ! env -u WG_AGENT_ID -u WG_WORKER_CAPABILITY -u WG_PI_FLEET_VIEW \
        -u WG_PI_COMPLETION_WAKES -u WG_DIR -u WG_PROJECT_DIR -u WG_PROJECT_ROOT \
        PI_CODING_AGENT_DIR="$agent_dir" \
        HOME="$scratch/home" \
        node --input-type=module - "$plugin" "$agent_dir" "$booted" "$fresh" \
        >"$log" 2>&1 <<'NODE'
const [plugin, agentDir, bootedProject, freshProject] = process.argv.slice(2);

const assert = (cond, msg) => { if (!cond) throw new Error(msg); };
const fs = await import("node:fs/promises");
const exists = async (p) => { try { await fs.access(p); return true; } catch { return false; } };
const read = async (p) => fs.readFile(p, "utf8");

/** A fake pi that models pi's real tool-active-set contract. */
function makePi({ cwd, mode = "tui", enabledTypes = ["read", "bash", "edit", "write"] }) {
  let active = [...enabledTypes];
  const tools = new Map();
  const commands = new Map();
  const handlers = new Map();
  const widgets = [];
  const notes = [];
  const wgCalls = [];
  const pi = {
    registerTool: (tool) => {
      const exposure = tool.exposure ?? "direct";
      const defaultActive = tool.defaultActive !== false;
      tools.set(tool.name, { exposure, defaultActive });
      // pi activates direct/model-only tools on registration unless
      // defaultActive is false.
      if (defaultActive && (exposure === "direct" || exposure === "model-only") && !active.includes(tool.name)) {
        active.push(tool.name);
      }
    },
    registerCommand: (name, def) => commands.set(name, def),
    registerProvider: () => {},
    on: (event, handler) => handlers.set(event, [...(handlers.get(event) ?? []), handler]),
    sendUserMessage: () => {},
    setModel: async () => true,
    getActiveTools: () => [...active],
    getAllTools: () => [...tools.entries()].map(([name, t]) => ({ name, ...t, active: active.includes(name) })),
    setActiveTools: (names) => {
      active = names.filter((n) => tools.has(n) || enabledTypes.includes(n));
    },
    exec: async (command, args) => {
      wgCalls.push([command, ...args].join(" "));
      return { stdout: "", stderr: "no wg in this contract", code: 1, killed: false };
    },
    events: { on: () => {}, emit: () => {} },
  };
  const ctx = {
    mode,
    hasUI: mode !== "json" && mode !== "print",
    cwd,
    signal: undefined,
    ui: {
      notify: (text, type) => notes.push({ text, type }),
      setWidget: (key, payload, options) => widgets.push({ key, payload, options }),
      theme: undefined,
    },
  };
  const wgVisible = () => active.filter((n) => n.startsWith("wg_"));
  const lastNote = () => notes[notes.length - 1]?.text ?? "";
  const sessionStart = async () => {
    for (const handler of handlers.get("session_start") ?? []) await handler({}, ctx);
    // Let deferred work (surfaces, IPC probes) settle.
    await new Promise((r) => setTimeout(r, 25));
  };
  return { pi, commands, handlers, ctx, tools, wgVisible, lastNote, notes, widgets, wgCalls, sessionStart, active: () => [...active] };
}

const settingsJson = `${agentDir}/settings.json`;
const { default: worksgoodPi } = await import(`${plugin}/pi-worksgood/index.js`);
const { WG_TOOL_NAMES } = await import(`${plugin}/pi-worksgood/tools.js`);
const { activationMarkerPath, ACTIVATION_MARKER_CONTENT, resolveProjectRoot } = await import(`${plugin}/pi-worksgood/activation.js`);

// WG_DIR is the STATE dir (`<project>/.wg`), so an explicit --dir must resolve to
// the owning project root, not to `<project>/.wg/.wg`.
assert(resolveProjectRoot({ dir: `${bootedProject}/.wg` }, "/elsewhere") === bootedProject,
  "WG_DIR=<project>/.wg must resolve to the project root");
assert(resolveProjectRoot({ dir: bootedProject }, "/elsewhere") === bootedProject,
  "an explicit project --dir must be used as-is");

// ── 1. LOAD TIME: dormant in every directory ───────────────────────────────
const fresh = makePi({ cwd: freshProject });
worksgoodPi(fresh.pi);
assert(fresh.tools.size === WG_TOOL_NAMES.length, `expected ${WG_TOOL_NAMES.length} registered wg tools, got ${fresh.tools.size}`);
assert(fresh.wgVisible().length === 0, `a fresh load must show ZERO wg tools, saw ${fresh.wgVisible()}`);
for (const name of WG_TOOL_NAMES) {
  const tool = fresh.tools.get(name);
  assert(tool.exposure === "direct" && tool.defaultActive === false,
    `${name} must register as direct + defaultActive:false (hidden would be unactivatable), got ${JSON.stringify(tool)}`);
}
for (const command of ["wg", "wg-model"]) {
  assert(fresh.commands.has(command), `/${command} must stay registered in the dormant state`);
}

await fresh.sessionStart();
assert(fresh.wgVisible().length === 0, `a never-booted project must start dormant, saw ${fresh.wgVisible()}`);
assert(!(await exists(`${freshProject}/.wg`)), "dormancy must never create .wg/ in a project that has none");
assert(!(await exists(activationMarkerPath(freshProject))), "a never-booted project must have no marker");
assert(!(await exists(settingsJson)), "load must not write settings.json");

// ── 2./3./4. /wg on then /wg off in a real WG project ──────────────────────
const booted = makePi({ cwd: bootedProject });
worksgoodPi(booted.pi);
await booted.sessionStart();
const wg = booted.commands.get("wg");
assert(wg && wg.handler, "/wg must be registered");

await wg.handler("on", booted.ctx);
const afterOn = booted.wgVisible();
assert(afterOn.length === WG_TOOL_NAMES.length, `on must activate the whole wg family, got ${afterOn.length}`);
for (const name of WG_TOOL_NAMES) assert(booted.active().includes(name), `on did not activate ${name}`);
for (const name of ["read", "bash", "edit", "write"]) assert(booted.active().includes(name), `on must keep ${name} active`);
assert(/WG active in this session/.test(booted.lastNote()), `on must confirm: ${booted.lastNote()}`);
const marker = activationMarkerPath(bootedProject);
assert(await exists(marker), `on must write ${marker}`);
assert((await read(marker)) === ACTIVATION_MARKER_CONTENT, "the marker body must stay 'active\\n'");
assert(!(await exists(settingsJson)), "on must never create/rewrite settings.json");
assert(!(await exists(`${agentDir}/extensions`)), "on must never install/uninstall the extension");
assert(!booted.wgCalls.some((c) => /pi-plugin (install|disable)/.test(c)),
  `on must not run the plugin lifecycle: ${booted.wgCalls.join(", ")}`);

// A NEW session of the booted project starts ACTIVE (persistence across sessions).
const second = makePi({ cwd: bootedProject });
worksgoodPi(second.pi);
await second.sessionStart();
assert(second.wgVisible().length === WG_TOOL_NAMES.length,
  `a booted project must start active in a new session, got ${second.wgVisible().length}`);

// /wg off: teardown of this session AND of the persistence marker.
const callsBeforeOff = booted.wgCalls.length;
await wg.handler("off", booted.ctx);
assert(booted.wgVisible().length === 0, `off must deactivate the wg family, saw ${booted.wgVisible()}`);
for (const name of ["read", "bash", "edit", "write"]) assert(booted.active().includes(name), `off must keep ${name} active`);
assert(!(await exists(marker)), "off must clear the project marker");
assert(await exists(`${bootedProject}/.wg`), "off must never remove .wg/");
assert(!(await exists(settingsJson)), "off must never touch settings.json");
assert(!booted.wgCalls.some((c) => /pi-plugin (disable|install)/.test(c)),
  `off must not run the plugin lifecycle: ${booted.wgCalls.join(", ")}`);
const offCalls = booted.wgCalls.slice(callsBeforeOff);
assert(!offCalls.some((c) => /\b(wg status|status)\b/.test(c) && !/compat-version|digest/.test(c)),
  `off must not touch the daemon: ${offCalls.join(", ")}`);
assert(/WG dormant in this session/.test(booted.lastNote()), `off must confirm: ${booted.lastNote()}`);
assert(/settings\.json/.test(booted.lastNote()) && /untouched/.test(booted.lastNote()),
  `off must state what it left alone: ${booted.lastNote()}`);
assert(/running agents are separate processes/.test(booted.lastNote()),
  `off must state that running agents are unaffected: ${booted.lastNote()}`);

// /wg survives every toggle (no catch-22), and on-after-off re-activates.
assert(booted.commands.has("wg") && booted.commands.has("wg-model"), "commands must survive off");
await wg.handler("on", booted.ctx);
assert(booted.wgVisible().length === WG_TOOL_NAMES.length, "on after off must re-activate");

// ── 6. /wg status shape ────────────────────────────────────────────────────
await wg.handler("status", booted.ctx);
const statusLine = booted.lastNote();
assert(!statusLine.includes("\n"), `status must be one line: ${statusLine}`);
assert(/^session: (active|dormant) · project: (marked active|dormant) · tools: \d+ wg tools visible · daemon: (running|stopped|unknown)$/.test(statusLine),
  `status shape wrong: ${statusLine}`);
assert(/^session: active/.test(statusLine), `status must report the live session state: ${statusLine}`);
assert(/project: marked active/.test(statusLine), `status must report the marker: ${statusLine}`);
assert(/tools: 12 wg tools visible/.test(statusLine), `status must report the visible count: ${statusLine}`);

// A dormant session reports dormant/dormant/0.
await wg.handler("off", booted.ctx);
await wg.handler("status", booted.ctx);
const dormantLine = booted.lastNote();
assert(/^session: dormant · project: dormant · tools: 0 wg tools visible · daemon: /.test(dormantLine),
  `dormant status wrong: ${dormantLine}`);

// ── 5. surfaces follow the session state (fleetView gate ON) ───────────────
process.env.WG_PI_FLEET_VIEW = "1";
const surfaced = makePi({ cwd: bootedProject });
worksgoodPi(surfaced.pi);
await surfaced.sessionStart();
assert(surfaced.widgets.length === 0, "a dormant session must mount NO ambient surface");
const wg2 = surfaced.commands.get("wg");
await wg2.handler("on", surfaced.ctx);
// With the gate ON and no daemon the payload clears immediately, but the mount
// itself is observable: the strip is written/refreshed at least once.
const mounted = surfaced.widgets.filter((w) => w.key === "wg-fleet");
assert(mounted.length > 0, "with the fleetView gate ON, /wg on must mount the fleet panel");
await wg2.handler("off", surfaced.ctx);
const cleared = surfaced.widgets.filter((w) => w.key === "wg-fleet").at(-1);
assert(cleared && cleared.payload === undefined, "/wg off must unmount the surfaces it mounted");

// ── settings.json was never created anywhere in this whole flow ────────────
assert(!(await exists(settingsJson)), "the entire activation flow must never write settings.json");

console.log("pi_plugin_session_activation: flow OK");
// The fleet strip / completion watcher run on setInterval, so an activated
// session owns live timers. Exit explicitly instead of waiting for an idle loop.
process.exit(0);
NODE
then
    loud_fail "pi session-activation human-flow contract failed: $(tail -80 "$log")"
fi

grep -q "flow OK" "$log" || loud_fail "session-activation flow did not complete: $(tail -40 "$log")"

echo "pi_plugin_session_activation: PASS"
exit 0
