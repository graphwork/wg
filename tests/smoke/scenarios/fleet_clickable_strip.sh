#!/usr/bin/env bash
#
# fleet_clickable_strip.sh — the CLICKABLE ambient fleet strip.
#
# Credential-free. Proves, against the BUILT + EMBEDDED plugin bundle, the
# pi-subagents-style mouse navigation the operator asked for ("clickable … to
# open it, to navigate, to investigate"):
#
#   * the focused strip + panel unit tests pass (fold/unfold by click, row-click
#     opens the cockpit on that task, detail-header click goes back, the dormant
#     read-only view, and the pi-mouse OBJECT-shape contract);
#   * the embedded bundle carries `fleet-strip.js` and the click wiring;
#   * a behavioural probe against the EMBEDDED bundle mounts the strip in the
#     COMPONENT form (pi only dispatches mouse to a mounted component), folds /
#     unfolds it on a header-row left click, opens the cockpit on an agent-row
#     click, leaves modifier/drag/wheel/blank-row events UNHANDLED, and — in a
#     project that has `.wg/` but is NOT booted — mounts the one-line dormant
#     hint (`/wg on to activate`) whose click opens the READ-ONLY inactive view
#     (activation gating: dormant vs active strip content).
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v npm >/dev/null 2>&1 || loud_skip "MISSING NPM" "npm is required for the fleet clickable-strip scenario"
command -v node >/dev/null 2>&1 || loud_skip "MISSING NODE" "node is required for the fleet clickable-strip scenario"

repo="$(cd "$HERE/../../.." && pwd)"
plugin="$repo/worksgood-pi"
[ -f "$plugin/package-lock.json" ] || loud_fail "missing worksgood-pi/package-lock.json" "worksgood-pi/package-lock.json"

if [ ! -d "$plugin/node_modules" ]; then
    npm --prefix "$plugin" ci >/tmp/fleet-clickable-npm-ci.log 2>&1 || \
        loud_skip "PI PLUGIN DEPS UNAVAILABLE" "npm ci failed: $(tail -20 /tmp/fleet-clickable-npm-ci.log)"
fi

# ── 1. Focused unit tests (the click model + the panel navigation) ───────────
( cd "$plugin" && npm run build >/tmp/fleet-clickable-build.log 2>&1 ) || \
    loud_fail "worksgood-pi build failed" "$(tail -40 /tmp/fleet-clickable-build.log)"
( cd "$plugin" && npx vitest run test/fleet-strip.test.ts test/fleet-panel.test.ts test/fleet.test.ts \
    >/tmp/fleet-clickable-vitest.log 2>&1 ) || \
    loud_fail "fleet clickable-strip unit tests failed" "$(tail -60 /tmp/fleet-clickable-vitest.log)"

# ── 2. The embedded bundle carries the clickable strip + its wiring ──────────
[ -f "$plugin/embedded/pi-worksgood/fleet-strip.js" ] || \
    loud_fail "embed missing fleet-strip.js" "worksgood-pi/embedded/pi-worksgood/fleet-strip.js"
grep -q "FleetStripComponent" "$plugin/embedded/pi-worksgood/fleet-strip.js" || \
    loud_fail "embed missing FleetStripComponent" "embedded/pi-worksgood/fleet-strip.js"
grep -q "handleMouse" "$plugin/embedded/pi-worksgood/fleet-strip.js" || \
    loud_fail "embedded strip has no handleMouse (not clickable)" "embedded/pi-worksgood/fleet-strip.js"
grep -q "installFleetView" "$plugin/embedded/pi-worksgood/index.js" || \
    loud_fail "embed missing installFleetView wiring" "embedded/pi-worksgood/index.js"

# ── 3. Behavioural probe against the EMBEDDED bundle ────────────────────────
node --input-type=module - "$plugin" <<'NODE' >/tmp/fleet-clickable-probe.log 2>&1 || \
    loud_fail "fleet clickable-strip embedded probe failed" "$(cat /tmp/fleet-clickable-probe.log)"
const plugin = process.argv[2];
const { mkdtempSync, mkdirSync, writeFileSync } = await import("node:fs");
const { tmpdir } = await import("node:os");
const { join } = await import("node:path");
const mod = await import(`${plugin}/embedded/pi-worksgood/index.js`);

const scratch = mkdtempSync(join(tmpdir(), "wg-fleet-click-"));
// Hermetic config gate: no config file → the strip's DEFAULT (visible).
process.env.PI_CODING_AGENT_DIR = join(scratch, "agent");
mkdirSync(process.env.PI_CODING_AGENT_DIR, { recursive: true });
process.env.WG_DAEMON_SOCKET = join(scratch, "none.sock");
delete process.env.WG_PI_FLEET_VIEW;
// Drop ANY inherited WG project binding: the activation state under test is the
// synthetic project below (`ctx.cwd`), never the smoke worktree that spawned us.
for (const key of ["WG_DIR", "WG_PROJECT_DIR", "WG_PROJECT_ROOT", "WG_STATE_DIR"]) {
  delete process.env[key];
}

const AGENTS = JSON.stringify([
  { id: "agent-7", task_id: "task-a", executor: "pi", model: "pi:openrouter:x/y", status: "working", uptime: "3m", process_alive: true },
]);
const TASKS = JSON.stringify([
  { id: "task-a", title: "Task A", status: "in-progress", after: [], before: [] },
  { id: "task-b", title: "Task B", status: "open", after: [], before: [] },
]);

const FAKE_TUI = { requestRender: () => {}, terminal: { rows: 24, columns: 100 } };

function harness(projectDir) {
  const calls = [];
  const handlers = {};
  const customs = [];
  const ui = {
    setWidget: (...a) => calls.push(a),
    notify: () => {},
    theme: undefined,
    custom: async (factory) => {
      const component = await factory(FAKE_TUI, undefined, undefined, () => {});
      customs.push(component);
      return undefined;
    },
  };
  const pi = {
    registerTool: () => {},
    registerProvider: () => {},
    registerCommand: () => {},
    on: (event, handler) => { (handlers[event] ||= []).push(handler); },
    exec: async (_cmd, args) => {
      const joined = Array.isArray(args) ? args.join(" ") : "";
      return joined.includes("agents")
        ? { stdout: AGENTS, stderr: "", code: 0, killed: false }
        : { stdout: TASKS, stderr: "", code: 0, killed: false };
    },
  };
  mod.default(pi);
  return { calls, handlers, customs, ctx: { mode: "tui", hasUI: true, ui, signal: undefined, cwd: projectDir } };
}

const settle = async () => { for (let i = 0; i < 40; i++) await new Promise((r) => setTimeout(r, 25)); };
const fireStart = (h) => { for (const handler of h.handlers.session_start ?? []) handler({}, h.ctx); };
const fleetCalls = (h) => h.calls.filter((c) => c[0] === "wg-fleet");
const mounted = (h) => {
  const last = fleetCalls(h).at(-1);
  if (!last) throw new Error("no wg-fleet mount");
  if (typeof last[1] !== "function") throw new Error(`mount is not the clickable component form: ${typeof last[1]}`);
  return last[1](FAKE_TUI, undefined);
};
const assertObjectResult = (label, result) => {
  if (result && typeof result !== "object") throw new Error(`${label} returned a truthy non-object (pi-core crash): ${String(result)}`);
};
const click = (y, extra = {}) => ({ type: "click", button: "left", y, shift: false, alt: false, ctrl: false, ...extra });

// ── 3a. ACTIVE project (`.wg/session-active` present) → the LIVE strip ──
const activeProject = join(scratch, "active");
mkdirSync(join(activeProject, ".wg"), { recursive: true });
writeFileSync(join(activeProject, ".wg", "session-active"), "active\n");
const live = harness(activeProject);
fireStart(live);
await settle();
const strip = mounted(live);
if (lastPlacement(live) !== "belowEditor") throw new Error("the strip must mount belowEditor");
function lastPlacement(h) { return fleetCalls(h).at(-1)?.[2]?.placement; }
if (typeof strip.handleMouse !== "function") throw new Error("the mounted strip has no handleMouse (not clickable)");

// Folded by default: ONE line carrying the click affordance + the live counts.
let lines = strip.render(100);
if (lines.length !== 1) throw new Error(`expected the folded one-line strip, got ${JSON.stringify(lines)}`);
if (!lines[0].includes("wg fleet (click)")) throw new Error(`fold affordance missing: ${lines[0]}`);
if (!lines[0].includes("in-progress")) throw new Error(`live counts missing: ${lines[0]}`);

// LEFT CLICK the header row → unfolds to the usual layout.
let res = strip.handleMouse(click(0));
assertObjectResult("strip header click", res);
if (!res || res.handled !== true) throw new Error(`header click not handled: ${JSON.stringify(res)}`);
lines = strip.render(100);
if (lines.length < 2) throw new Error(`header click did not unfold: ${JSON.stringify(lines)}`);
if (!lines.join("\n").includes("agent-7")) throw new Error("unfolded strip is missing the live agent row");

// Click an agent row → the full cockpit opens (explicit open, on that task).
const agentRow = lines.findIndex((l) => l.includes("agent-7"));
if (agentRow < 1) throw new Error("no agent row to click");
res = strip.handleMouse(click(agentRow));
assertObjectResult("strip agent-row click", res);
await settle();
if (live.customs.length !== 1) throw new Error(`agent-row click did not open the cockpit (${live.customs.length})`);
const cockpit = live.customs[0].render(100).join("\n");
if (!cockpit.includes("wg-fleet ▸")) throw new Error(`cockpit did not render the live panel: ${cockpit.slice(0, 120)}`);
if (cockpit.includes("inactive")) throw new Error("a live cockpit must not open the dormant view");

// Click the header again → folds back to one line.
res = strip.handleMouse(click(0));
assertObjectResult("strip header click (fold)", res);
if (strip.render(100).length !== 1) throw new Error("second header click did not fold back to one line");

// Modifier clicks / other buttons / drag / wheel / blank rows are NOT ours.
for (const [label, event] of [
  ["shift+click", click(0, { shift: true })],
  ["alt+click", click(0, { alt: true })],
  ["ctrl+click", click(0, { ctrl: true })],
  ["right click", click(0, { button: "right" })],
  ["drag", { type: "drag", button: "left", y: 0, shift: false, alt: false, ctrl: false }],
  ["wheel", { type: "wheel", button: "none", y: 0, shift: false, alt: false, ctrl: false, wheelDelta: 3 }],
  ["press", { type: "press", button: "left", y: 0, shift: false, alt: false, ctrl: false }],
  ["no y", { type: "click", button: "left" }],
]) {
  const r = strip.handleMouse(event);
  assertObjectResult(label, r);
  if (r) throw new Error(`${label} must be left unhandled, got ${JSON.stringify(r)}`);
}

// ── 3b. DORMANT project (has `.wg/`, never booted) → the HINT strip ──
const dormantProject = join(scratch, "dormant");
mkdirSync(join(dormantProject, ".wg"), { recursive: true });
const dormant = harness(dormantProject);
fireStart(dormant);
await settle();
const hint = mounted(dormant);
const hintLines = hint.render(100);
if (hintLines.length !== 1) throw new Error(`the dormant strip must be ONE line, got ${JSON.stringify(hintLines)}`);
if (!hintLines[0].includes("dormant")) throw new Error(`dormant state not stated: ${hintLines[0]}`);
if (!hintLines[0].includes("/wg on to activate")) throw new Error(`activation hint missing: ${hintLines[0]}`);

// Clicking it opens the READ-ONLY inactive view — never the live cockpit.
res = hint.handleMouse(click(0));
assertObjectResult("dormant strip click", res);
await settle();
if (dormant.customs.length !== 1) throw new Error("dormant strip click did not open the inactive view");
const inactive = dormant.customs[0].render(100).join("\n");
if (!inactive.includes("inactive")) throw new Error(`dormant view missing the inactive label: ${inactive.slice(0, 120)}`);
if (!inactive.includes("/wg on to activate")) throw new Error("dormant view missing the activation hint");
if (!inactive.includes("read-only")) throw new Error("dormant view must be explicitly read-only");
if (inactive.includes("❯")) throw new Error("dormant view must not be the live cockpit (selection marker present)");

// ── 3c. A project with no `.wg/` at all → nothing mounts ──
const absentProject = join(scratch, "absent");
mkdirSync(absentProject, { recursive: true });
const absent = harness(absentProject);
fireStart(absent);
await settle();
if (fleetCalls(absent).length !== 0) throw new Error("a non-WG project must mount nothing");

console.log("probe ok: active→live clickable strip (fold/unfold/row-open), dormant→hint→inactive view, absent→nothing");
for (const h of [live, dormant, absent]) {
  for (const handler of h.handlers.session_shutdown ?? []) handler({}, h.ctx);
}
process.exit(0);
NODE

# ── 4. The committed embed matches a fresh build of src (anti-drift) ────────
# `npm run build` above wrote worksgood-pi/pi-worksgood/; the committed bundle
# the wg binary embeds must be byte-identical, so a source edit without a
# re-embed fails HERE rather than silently shipping an old strip.
for built in "$plugin"/pi-worksgood/*.js; do
    base="$(basename "$built")"
    [ -f "$plugin/embedded/pi-worksgood/$base" ] || \
        loud_fail "embed is missing $base" "worksgood-pi/embedded/pi-worksgood/$base"
    diff -q "$built" "$plugin/embedded/pi-worksgood/$base" >/dev/null || \
        loud_fail "embed is STALE for $base" \
            "worksgood-pi/embedded/pi-worksgood/$base differs from a fresh build — run \`make embed-worksgood-pi\`"
done
echo "embed ok: committed bundle == fresh build ($(ls "$plugin"/pi-worksgood/*.js | wc -l) files)"

# The operator's materialized plugin CACHE is owned by the installed binary, so
# a mismatch here is a note (it refreshes on the next `wg pi-plugin install` /
# `wg dev-sync`), never a failure of this task's candidate bytes.
cache="${XDG_CACHE_HOME:-$HOME/.cache}/wg/worksgood-pi"
compat="$(sed -n 's/.*WG_PI_PLUGIN_COMPAT_VERSION: &str = "\([^"]*\)".*/\1/p' "$repo/src/pi_plugin/mod.rs" | head -n1)"
if [ -n "$compat" ] && [ -f "$cache/$compat/pi-worksgood/fleet-strip.js" ]; then
    if diff -q "$plugin/embedded/pi-worksgood/fleet-strip.js" \
        "$cache/$compat/pi-worksgood/fleet-strip.js" >/dev/null; then
        echo "cache ok: embedded fleet-strip.js == plugin cache ($compat)"
    else
        echo "note: plugin cache ($compat) predates this build — run \`wg pi-plugin install\` (or \`wg dev-sync\`) to refresh it"
    fi
else
    echo "note: plugin cache not materialized for compat $compat — skipping the cache comparison"
fi

echo "fleet clickable strip OK"
