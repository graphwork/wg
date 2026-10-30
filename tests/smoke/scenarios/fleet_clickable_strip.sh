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

# ── 3d. FULL TUI INTERACTION PARITY probe against the EMBEDDED bundle ───────
# The operator standard for fleet-tui-parity: full functionality, accessibility,
# interactivity and explorability, converging on the TUI viz_viewer's
# interaction model. This drives the REAL embedded component with REAL key
# sequences and REAL mouse events — every batch of the parity contract.
node --input-type=module - "$plugin" <<'NODE' >/tmp/fleet-parity-probe.log 2>&1 || \
    loud_fail "fleet TUI-parity embedded probe failed" "$(cat /tmp/fleet-parity-probe.log)"
const plugin = process.argv[2];
const {
  FleetPanelComponent,
  FLEET_BINDINGS,
  FLEET_ACTIONS,
  dividerPercentAfterDrag,
  helpLines,
  matchFleetBinding,
  validateFleetBindings,
} = await import(`${plugin}/embedded/pi-worksgood/index.js`);

const assert = (cond, msg) => { if (!cond) throw new Error(msg); };
const PALETTE = { open: [200, 200, 80], "in-progress": [60, 200, 220], done: [80, 220, 100], failed: [220, 60, 60] };
const TREE = [
  "‖ root-a  (open) 5s",
  "├→ ‖ child-1  (open) 2s",
  "└→ ‖ child-2  (failed) 2s",
  "‖ .flip-a  (in-progress) 1s",
].join("\n");
const DETAIL = [
  "Task: root-a",
  "Title: Root A",
  "Status: open",
  "Required deterministic completion checks (exact enforced order):",
  "  [baseline] git diff --check — provenance: land",
  "  required evidence: run each command",
  "Log:",
  "  [1] created",
  "  [2] started",
].join("\n") + "\n";

function snap() {
  return {
    revision: "rev-parity",
    counts: { in_progress: 1, ready: 2, blocked: 0, done: 1, failed: 1, total: 5, active_agents: 1 },
    tasks: [
      { id: "root-a", title: "Root A", status: "open", depends_on: [], started_at: "2026-01-01T00:00:00Z" },
      { id: "child-1", title: "Child 1", status: "open", depends_on: ["root-a"], started_at: "2026-01-02T00:00:00Z" },
      { id: "child-2", title: "Child 2", status: "failed", depends_on: ["root-a"], started_at: "2026-01-03T00:00:00Z" },
      { id: ".flip-a", title: "Flip A", status: "in-progress", depends_on: [], started_at: "2026-01-04T00:00:00Z" },
      { id: "done-a", title: "Done A", status: "done", depends_on: [], started_at: "2026-01-05T00:00:00Z" },
    ],
    agents: [{ id: "agent-9", task_id: ".flip-a", status: "working", activity: "scoring verdicts" }],
    tree: { text: TREE, node_lines: { "root-a": 0, "child-1": 1, "child-2": 2, ".flip-a": 3 }, palette: PALETTE },
  };
}
const flush = async () => { for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 5)); };

function mount(overrides = {}) {
  const tui = { requestRender() {}, terminal: { rows: 24, columns: 100 } };
  const layouts = [];
  const actions = [];
  const refreshes = [];
  const closed = [];
  const component = new FleetPanelComponent(
    snap(),
    tui,
    () => closed.push("closed"),
    null,
    {
      heightPct: 1,
      onHeightPctChange: () => {},
      persistLayout: (layout) => layouts.push(layout),
      runAction: async (argv) => {
        actions.push([...argv]);
        return { stdout: `ok ${argv[0]}\n`, stderr: "", code: 0, killed: false };
      },
      onRefresh: () => refreshes.push(1),
      ...overrides,
    },
    undefined,
    async () => ({ task_id: "root-a", text: DETAIL, source: "daemon" }),
  );
  return { component, layouts, actions, refreshes, closed, tui };
}

// ── Batch 1: inspector dock + views ──
{
  const { component } = mount();
  component.render(100);
  component.handleInput("\r");
  await flush();
  assert(component.detailVisible, "Enter did not open the inspector");
  component.handleInput("\x1b[1;3C"); // Alt-→
  await flush();
  assert(component.inspectorViewKind === "json", "Alt-→ did not cycle to raw JSON");
  assert(component.render(100).join("\n").includes("client projection"), "raw JSON view is not labelled");
  component.handleInput("\x1b[1;3D"); // Alt-←
  assert(component.inspectorViewKind === "detail", "Alt-← did not cycle back");
  // Section collapse (Detail-tab Space).
  component.scrollBy(3);
  component.handleInput(" ");
  const collapsed = component.render(100).join("\n");
  assert(!collapsed.includes("  [baseline] git diff --check"), "Space did not collapse the section body");
  assert(/\(\+\d+ lines\)/.test(collapsed), "collapsed section carries no hidden-count marker");
  component.handleInput(" ");
  assert(component.render(100).join("\n").includes("  [baseline] git diff --check"), "Space did not restore the section");
  // Dock + divider drag with the TUI's EXACT percentage math.
  component.handleInput("2");
  const rows = component.render(100);
  const divider = rows.findIndex((r) => r.includes("[< Back]") && r.includes("detail (wg show)"));
  assert(divider > 0, `docked divider row missing: ${JSON.stringify(rows.slice(0, 3))}`);
  assert(rows.slice(0, divider).join("\n").includes("root-a"), "the tree is not visible above the divider");
  assert(rows.slice(divider).join("\n").includes("Task: root-a"), "the inspector is not visible below the divider");
  const start = component.handleMouse({ type: "press", button: "left", y: divider });
  assert(start && start.handled === true && start.capture === true, "divider press did not capture");
  component.handleMouse({ type: "drag", button: "left", y: divider - 2 });
  assert(component.dockFraction > 50, `drag up did not grow the inspector: ${component.dockFraction}`);
  component.handleMouse({ type: "release", button: "left", y: divider });
  assert(component.dockFraction > 50, "dock fraction lost on release");
}
// The parity constant the TUI's own tests pin (event.rs:9223-9293).
assert(dividerPercentAfterDrag({ startPercent: 35, startRow: 36, currentRow: 32, rows: 40 }) === 45,
  "divider drag parity: 35% + up-4-of-40 must be 45% (TUI horizontal_divider_drag_up_grows_inspector)");
assert(dividerPercentAfterDrag({ startPercent: 50, startRow: 20, currentRow: 24, rows: 40 }) === 40,
  "divider drag parity: 50% + down-4-of-40 must be 40% (TUI horizontal_divider_drag_shrinks_inspector)");

// ── Batch 2: layout fractions + persistence ──
{
  const { component, layouts } = mount();
  component.render(100);
  component.handleInput("1"); assert(component.dockFraction === 33, "1 did not dock at 1/3");
  component.handleInput("2"); assert(component.dockFraction === 50, "2 did not dock at 1/2");
  component.handleInput("3"); assert(component.dockFraction === 67, "3 did not dock at 2/3");
  component.handleInput("i"); assert(component.dockFraction === 57, `i did not shrink the dock 10%: ${component.dockFraction}`);
  component.handleInput("v"); assert(component.dockFraction === 67, "v did not grow the dock back 10%");
  component.handleInput("="); assert(component.layoutState.mode === "full", "= did not cycle to the full panel");
  component.handleInput("="); assert(component.layoutState.mode === "hidden", "= did not cycle to the graph-only layout");
  component.handleInput("\\"); assert(component.layoutState.mode === "split", "\\ did not restore the dock");
  component.handleInput("p");
  component.handleInput("f");
  assert(component.pendingLayoutState && component.pendingLayoutState.mode === "full", "f did not set a pending FULL layout");
  component.handleInput("\r");
  assert(component.layoutState.mode === "full", "Enter did not apply the pending layout");
  component.handleInput("p"); component.handleInput("0"); component.handleInput("\x1b");
  assert(component.layoutState.mode === "full", "Esc did not restore the prior layout");
  assert(layouts.length > 0, "the dock layout was never persisted");
  assert(layouts.every((l) => typeof l.mode === "string" && typeof l.sizePercent === "number"),
    "a persisted layout is malformed");
}

// ── Batch 3: task actions (confirm gates) ──
{
  const { component, actions, refreshes } = mount();
  component.render(100);
  component.handleInput("D");
  assert(component.pendingConfirm, "D ran without a confirm step");
  component.handleInput("n");
  assert(actions.length === 0, "a cancelled confirm still ran wg");
  component.handleInput("D");
  component.handleInput("y");
  await flush();
  assert(JSON.stringify(actions.at(-1)) === JSON.stringify(["done", "root-a"]), `wrong argv: ${JSON.stringify(actions.at(-1))}`);
  assert(component.actionNotify[0].includes("ok"), "the action output was not surfaced as a notify line");
  assert(refreshes.length > 0, "a graph-mutating action did not refresh the fleet");
  component.handleInput("f");
  for (const ch of "blocked") component.handleInput(ch);
  component.handleInput("\r");
  await flush();
  assert(actions.at(-1)[0] === "fail" && actions.at(-1).includes("blocked"), `fail argv wrong: ${JSON.stringify(actions.at(-1))}`);
  component.handleInput("a");
  for (const ch of "brand new") component.handleInput(ch);
  component.handleInput("\r");
  await flush();
  assert(JSON.stringify(actions.at(-1)) === JSON.stringify(["add", "brand new"]), "create argv wrong");
}
{
  const { component, actions } = mount({ inactive: true });
  component.render(100);
  const body = component.helpBody().join("\n");
  for (const kind of Object.keys(FLEET_ACTIONS)) {
    assert(!body.includes(FLEET_ACTIONS[kind].desc), `dormant help still lists the ${kind} action`);
  }
  component.handleInput("D"); component.handleInput("a"); component.handleInput("x");
  assert(actions.length === 0, "a dormant session ran a task action");
}

// ── Batch 4: search + sort + filter ──
{
  const { component, closed } = mount();
  component.render(100);
  component.handleInput("/");
  for (const ch of "child-2") component.handleInput(ch);
  assert(component.searchState.matches.length === 1, "typeahead did not narrow to one match");
  assert(component.render(100).at(-1).includes("1 match"), "the footer does not state the live match count");
  assert(!component.render(100).join("\n").includes("root-a  (open)"), "the typeahead did not filter the tree");
  component.handleInput("\r");
  assert(component.selected === "child-2", "Enter did not commit the search selection");
  assert(!component.searchState.active, "commit did not clear the search state");
  component.handleInput("/");
  for (const ch of "zz") component.handleInput(ch);
  component.handleInput("\x1b");
  assert(!component.searchState.active, "Esc did not clear the search");
  assert(closed.length === 0, "Esc in search closed the panel");
  component.handleInput("s");
  assert(component.render(100)[0].includes("sort Chrono ↑"), "s did not state the new sort");
  component.handleInput(".");
  assert(component.render(100)[0].includes("running plumbing only"), ". did not cycle the plumbing filter");
  assert(component.render(100).join("\n").includes("flip-a"), "running-only plumbing hid a running automation task");
}

// ── Batch 5: help modal ──
{
  const { component, closed } = mount();
  component.render(100);
  component.handleInput("?");
  const head = component.render(100)[0];
  assert(head.includes("Keybindings"), `? did not open the help modal: ${head}`);
  assert(/\d+-\d+\/\d+/.test(head), "the help modal has no position indicator");
  const body = component.helpBody().join("\n");
  for (const row of FLEET_BINDINGS) {
    if (row.hideInHelp) continue;
    assert(body.includes(row.desc), `help is missing a binding: ${row.id} (${row.desc})`);
  }
  component.handleInput("j");
  assert(component.helpScrollOffset === 1, "j did not scroll the help");
  component.handleInput("G");
  assert(component.helpScrollOffset > 0, "G did not jump to the help bottom");
  component.handleInput("g");
  assert(component.helpScrollOffset === 0, "g did not jump to the help top");
  component.handleInput("q");
  assert(!component.helpVisible, "q did not close the help");
  assert(closed.length === 0, "q in help closed the panel");
}
validateFleetBindings();
assert(FLEET_BINDINGS.length >= 45, `the keymap lost bindings: ${FLEET_BINDINGS.length}`);
for (const row of FLEET_BINDINGS) {
  for (const key of row.keys) {
    if (key.length !== 1) continue;
    const hit = matchFleetBinding(key, row.scope, { inactive: row.activeOnly === true });
    if (row.activeOnly === true) {
      assert(matchFleetBinding(key, row.scope, { inactive: true }) !== row.id,
        `a dormant panel dispatched an active-only binding: ${row.id}`);
    } else {
      assert(hit, `binding ${row.id} is not dispatchable from its own scope`);
    }
  }
}
console.log(`probe ok: batch 1 dock/views/divider (TUI parity 35->45, 50->40), batch 2 fractions/cycles/persistence, batch 3 actions+confirms+dormant-hide, batch 4 search/sort/filter, batch 5 auto-generated help; ${FLEET_BINDINGS.length} bindings`);
NODE

# ── 3e. CROSS-LANGUAGE help parity: the panel's help IS the TUI's help ──────
# The spec for this task is the TUI's own help table. Re-read it from the Rust
# source and require the SAME key label + description in the plugin's generated
# help, so the two surfaces cannot drift.
WG_TUI_HELP_SRC="$repo/src/tui/viz_viewer/render.rs"
[ -f "$WG_TUI_HELP_SRC" ] || loud_fail "missing the TUI help table source" "$WG_TUI_HELP_SRC"
grep -q "fn draw_help_overlay" "$WG_TUI_HELP_SRC" || \
    loud_fail "the TUI help overlay vanished from render.rs" "$WG_TUI_HELP_SRC"
export WG_TUI_HELP_FILE="$WG_TUI_HELP_SRC"
node --input-type=module - "$plugin" <<'NODE' >/tmp/fleet-parity-help.log 2>&1 || \
    loud_fail "the panel's help has drifted from the TUI's help table" "$(cat /tmp/fleet-parity-help.log)"
const plugin = process.argv[2];
const { readFileSync } = await import("node:fs");
const { helpLines } = await import(`${plugin}/embedded/pi-worksgood/index.js`);
const rust = readFileSync(process.env.WG_TUI_HELP_FILE, "utf8");
const body = helpLines().join("\n");
// (TUI key label, TUI description) pairs the panel MUST mirror verbatim.
const pairs = [
  ["Alt-←/→", "Cycle inspector views (with slide animation)"],
  ["1 / 2 / 3", "Set panel to 1/3 / 1/2 / 2/3"],
  ["r / f / 0", "Pending Split / Full / Hidden"],
  ["\\", "Toggle right panel visible"],
  ["i", "Grow viz pane (10% per press, wraps)"],
  ["v", "Shrink viz pane (10% per press, wraps)"],
  ["=", "Cycle layout: split/panel/graph"],
  ["R", "Toggle raw JSON in Detail tab"],
  ["Space", "Toggle section collapse in Detail"],
  ["J", "Toggle raw JSON mode"],
  ["Shift-↑/↓", "Scroll detail panel"],
  ["a", "Create new task"],
  ["D", "Mark selected task done"],
  ["f", "Mark selected task failed"],
  ["x", "Retry selected task"],
  ["e", "Edit task description"],
  ["M", "Send message to task agent"],
  ["r", "Force refresh"],
  ["/", "Start exact-task search in command mode"],
  ["Enter/tap", "Commit stable task ID; clear all match state"],
  ["Esc", "Clear totally without quitting"],
  ["s", "Cycle sort: Chrono↓/↑/Status"],
  [". / <", "Cycle plumbing: hidden → running only → all"],
  ["?", "Toggle this help"],
];
const missingInTui = [];
const missingInPanel = [];
for (const [label, desc] of pairs) {
  const tuiHasDesc = rust.includes(desc);
  if (!tuiHasDesc) { missingInTui.push(`${label} → ${desc}`); continue; }
  if (!body.includes(desc)) missingInPanel.push(`${label} → ${desc}`);
}
if (missingInTui.length > 0) {
  throw new Error(`the TUI help table no longer carries: ${missingInTui.join(" | ")} (update the parity list)`);
}
if (missingInPanel.length > 0) {
  throw new Error(`the panel's help is missing TUI bindings: ${missingInPanel.join(" | ")}`);
}
console.log(`help parity ok: ${pairs.length} TUI bindings mirrored verbatim (key label + description)`);
NODE
echo "parity ok: TUI help table ↔ panel help"

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


# ── 5. LIVE fullscreen `pi` session: real keys + real SGR mouse, fixture daemon ─
# The parity contract is a HUMAN-FLOW contract, so the last leg drives an actual
# fullscreen pi TUI against a real (fixture) WG graph and a real wg daemon/CLI,
# with synthetic SGR mouse events and real key sequences.
if ! command -v tmux >/dev/null 2>&1; then
    echo "note: tmux unavailable — skipping the LIVE pi fleet TUI-parity leg"
elif ! command -v pi >/dev/null 2>&1; then
    echo "note: pi unavailable — skipping the LIVE pi fleet TUI-parity leg"
else
    unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_SPAWN_EPOCH WG_EXECUTOR_TYPE WG_MODEL WG_TIER \
        WG_PI_PLUGIN_COMPAT_VERSION WG_WORKER_CAPABILITY WG_WORKER_IPC
    scratch="$(make_scratch)"
    project="$scratch/project"
    home="$scratch/home"
    mkdir -p "$project" "$home"
    export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg" TMUX_TMPDIR="$scratch/tmux"
    mkdir -p "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$TMUX_TMPDIR"
    cfg_dir="$HOME/.pi/agent/extensions/pi-worksgood"
    mkdir -p "$cfg_dir"
    # The persisted panel height is honoured on open (unchanged contract); the
    # LAYOUT starts at its default (panel/full) so this leg can prove the dock.
    printf '{"fleetView": true, "fleetViewCollapsed": true, "fleetViewHeightPct": 0.5}\n' >"$cfg_dir/config.json"
    cd "$project"
    git init -q -b main
    git config user.email smoke@example.invalid
    git config user.name 'WG Smoke'
    touch seed.txt && git add seed.txt && git commit -q -m seed

    run_wg() { env -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH -u WG_WORKTREE_ACTIVE -u WG_BRANCH \
        -u WG_AGENT_ID -u WG_TASK_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_WORKER_CAPABILITY \
        HOME="$HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" wg "$@"; }
    run_wg init >/dev/null 2>&1
    for i in $(seq 0 11); do run_wg add "Task $i" --id "task-$i" >/dev/null 2>&1; done
    run_wg add "Child task" --id "task-child" --after task-0 >/dev/null 2>&1

    session="wgsmoke-fleet-parity-$$"
    cleanup_parity_session() { tmux kill-session -t "$session" 2>/dev/null || true; }
    add_cleanup_hook cleanup_parity_session
    tmux new-session -d -s "$session" -x 110 -y 40 \
        "env TERM=xterm-256color PATH='$PATH' HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' TMUX_TMPDIR='$TMUX_TMPDIR' WG_PI_FLEET_VIEW=1 pi -e '$plugin/embedded/pi-worksgood/index.js' -ne"
    tmux set-option -t "$session" mouse on

    cap() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
    wait_cap() {
        local needle=$1 label=$2
        for _ in $(seq 1 300); do cap | grep -Fq "$needle" && return 0; sleep 0.05; done
        loud_fail "$label: $(cap | tr '\n' '|')"
    }
    wait_cap_re() {
        local needle=$1 label=$2
        for _ in $(seq 1 300); do cap | grep -Eq "$needle" && return 0; sleep 0.05; done
        loud_fail "$label: $(cap | tr '\n' '|')"
    }
    key() { tmux send-keys -t "$session" -l "$1"; sleep 0.35; }
    # Type a string ONE KEY AT A TIME (a single literal write can outrun the
    # panel's per-key search handling).
    type_str() {
        local text="$1" i
        for ((i = 0; i < ${#text}; i++)); do
            tmux send-keys -t "$session" -l "${text:$i:1}"; sleep 0.12
        done
        sleep 0.5
    }
    raw() { tmux send-keys -t "$session" -l "$(printf "$1")"; sleep 0.4; }
    sgr() { tmux send-keys -t "$session" -l "$(printf '\033[<%s;%s;%s%s' "$1" "$2" "$3" "$4")"; }
    click() { sgr 0 "$2" "$1" M; sleep 0.2; sgr 0 "$2" "$1" m; sleep 0.6; }
    row_of() {
        cap | python3 -c '
import sys, re
pat = sys.argv[1]
for y, row in enumerate(sys.stdin.read().splitlines(), 1):
    if re.search(pat, row):
        print(y); raise SystemExit(0)
print(0)
' "$1"
    }

    wait_cap "wg fleet" "pi did not boot with the wg fleet strip"
    key '/wg-fleet'; sleep 0.8; tmux send-keys -t "$session" Enter
    wait_cap "wg-fleet ▸" "live /wg-fleet panel did not open"

    # BATCH 2 (live): `2` docks the inspector at 1/2 and the header SAYS so.
    key '2'
    wait_cap "split 1/2" "the live panel did not dock at 1/2 on '2'"

    # BATCH 1 (live): Enter inspects; the tree stays visible above the divider.
    tmux send-keys -t "$session" Enter; sleep 0.8
    wait_cap "detail (wg show)" "Enter did not open the live inspector"
    cap | grep -Fq "Task: task-" || loud_fail "the live inspector does not show WG's own detail text" "$(cap | tr '\n' '|')"

    # BATCH 1 (live): DRAG the horizontal divider UP with a real SGR mouse drag —
    # the inspector grows (the TUI's horizontal_divider_drag semantics).
    divider="$(row_of '\[< Back\].*(detail \(wg show\)|raw json)')"
    [ "$divider" != "0" ] || loud_fail "no live divider row to drag" "$(cap | tr '\n' '|')"
    sgr 0 50 "$divider" M; sleep 0.3
    sgr 32 50 "$((divider - 4))" M; sleep 0.3
    sgr 0 50 "$((divider - 4))" m; sleep 0.8
    wait_cap_re "⇕ (5[5-9]|6[0-9]|7[0-9]|8[0-9]|90)%" "the live divider drag did not grow the inspector"
    python3 - "$cfg_dir/config.json" <<'PY_LAYOUT' || loud_fail "the live dock layout did not persist fleetViewLayout" "config=$(cat "$cfg_dir/config.json" 2>/dev/null)"
import json, sys
raw = json.load(open(sys.argv[1]))
layout = raw.get("fleetViewLayout")
if not isinstance(layout, dict) or layout.get("mode") != "split":
    raise SystemExit(f"fleetViewLayout not persisted as a split layout: {layout}")
if not isinstance(layout.get("sizePercent"), int) or not (50 <= layout["sizePercent"] <= 90):
    raise SystemExit(f"persisted dock percentage out of range: {layout}")
if raw.get("fleetView") is not True or abs(float(raw.get("fleetViewHeightPct", 0)) - 0.5) > 1e-6:
    raise SystemExit(f"the dock drag clobbered unrelated config keys: {raw}")
PY_LAYOUT

    # BATCH 1 (live): Alt-→ cycles the inspector to the RAW JSON view.
    raw '\033[1;3C'
    wait_cap "raw json" "Alt-→ did not switch the live inspector to raw JSON"
    raw '\033[1;3D'
    wait_cap "detail (wg show)" "Alt-← did not switch the live inspector back"

    # BATCH 5 (live): `?` opens the auto-generated help, `q` closes it.
    key '?'
    wait_cap "Keybindings" "? did not open the live help modal"
    cap | grep -Eq "Keybindings +1-[0-9]+/[0-9]+" || loud_fail "the live help has no position indicator" "$(cap | tr '\n' '|')"
    cap | grep -Fq "Cycle inspector views" || loud_fail "the live help does not list the TUI's inspector bindings" "$(cap | tr '\n' '|')"
    # The help SCROLLS (handle_help_key): `j` walks down to the Detail bindings
    # (pi's own viewport owns PgUp/PgDn, so the live leg uses the TUI's other
    # line-scroll keys — both are bound, and both are listed in the modal).
    help_before="$(cap | grep -o 'Keybindings *[0-9]*-[0-9]*/[0-9]*' | head -1)"
    reached=""
    for _ in $(seq 1 30); do
        cap | grep -Fq "Toggle section collapse" && { reached=1; break; }
        tmux send-keys -t "$session" -l 'j'; sleep 0.15
    done
    [ -n "$reached" ] || loud_fail "the live help does not scroll to the TUI's Detail bindings" "$(cap | tr '\n' '|')"
    help_after="$(cap | grep -o 'Keybindings *[0-9]*-[0-9]*/[0-9]*' | head -1)"
    [ "$help_before" != "$help_after" ] || \
        loud_fail "the live help position indicator did not move ($help_before)" "$(cap | tr '\n' '|')"
    key 'q'
    for _ in $(seq 1 120); do cap | grep -Fq "Keybindings" || break; sleep 0.05; done
    cap | grep -Fq "Keybindings" && loud_fail "q did not close the live help modal" "$(cap | tr '\n' '|')"

    # BATCH 4 (live): `/` searches the real graph (typeahead), Enter commits.
    key '/'
    type_str 'child'
    wait_cap "1 match" "the live typeahead did not match the real graph task"
    tmux send-keys -t "$session" Enter; sleep 0.9
    cap | grep -Fq "1 match" && loud_fail "Enter did not clear the live search state" "$(cap | tr '\n' '|')"
    # The commit JUMPED to + SELECTED the matching task: inspecting it now shows
    # that task's own WG detail.
    tmux send-keys -t "$session" Enter; sleep 0.9
    wait_cap "task-child" "the committed search selection is not the matched task"
    tmux send-keys -t "$session" Escape; sleep 0.6

    # BATCH 2 (live): `=` cycles the layout and the header states each mode.
    key '='
    wait_cap "panel (full)" "= did not cycle the live layout to the full panel"
    key '='
    wait_cap "graph (dock hidden)" "= did not cycle the live layout to graph-only"

    # Close deliberately (announced), exactly as the operator expects.
    tmux send-keys -t "$session" Escape; sleep 0.8
    tmux send-keys -t "$session" 'q'; sleep 1
    for _ in $(seq 1 200); do cap | grep -Fq "wg-fleet ▸" || break; sleep 0.05; done
    cap | grep -Fq "wg-fleet ▸" && loud_fail "the live panel did not close on q/Esc" "$(cap | tr '\n' '|')"

    cleanup_parity_session
    echo "live pi ok: dock at 1/2 ('2'), Enter→inspector with WG detail, SGR divider drag grows the dock + persists fleetViewLayout, Alt-←/→ raw JSON, '?' auto-generated help (TUI bindings) + q, '/' typeahead+commit on a real graph, '=' layout cycle"
fi

echo "fleet clickable strip OK"
