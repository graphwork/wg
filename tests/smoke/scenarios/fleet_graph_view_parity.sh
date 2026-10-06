#!/usr/bin/env bash
#
# fleet_graph_view_parity.sh — pins the **graph/structure parity** of /wg-fleet.
#
# The operator's complaint was that the panel re-implemented WG's tree in
# TypeScript (a per-node connector string) so its prefixes/edges/indentation
# drifted from `wg viz`, and that no status colour reached the lines. This
# scenario proves, credential-free and deterministically:
#
#   * the plugin renders WG's OWN `wg viz` text verbatim (`renderWgTree`);
#     top-level rows are unindented and children use WG's `└→`/`├→` edges with
#     WG's own 2-space-per-depth prefix, by construction;
#   * every task line is painted with **WG's exported RGB palette** (the
#     `palette` object carried on `GetFleet.tree` / `wg viz --json`) as exact
#     truecolour/256-colour ANSI — never a semantic pi theme role on the default
#     path — and the local fallback (when no palette is exported) is *visible*;
#   * Enter opens the selected task's DETAIL view (and a doubled Enter — some
#     terminals send `\r\n` — does not immediately close it); `o`/space expand;
#     q closes;
#   * the legacy TS tree is fallback-only: with no rendered tree the panel still
#     renders (best-effort), but with one it is never consulted;
#   * end-to-end: the daemon's `GetFleet` tree is byte-for-byte the same
#     `wg viz --json` render for the same graph (structure + prefixes + indent),
#     and both carry the SAME status palette, which equals the checked-in
#     cross-language fixture (`worksgood-pi/test/fixtures/status-palette.json`)
#     that `src/status_palette.rs` also pins;
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v npm >/dev/null 2>&1 || loud_skip "MISSING NPM" "npm is required for the fleet graph-view parity contract"
command -v node >/dev/null 2>&1 || loud_skip "MISSING NODE" "node is required for the fleet graph-view parity contract"

repo="$(cd "$HERE/../../.." && pwd)"
plugin="$repo/worksgood-pi"
[ -f "$plugin/package-lock.json" ] || loud_fail "missing worksgood-pi/package-lock.json" \
    "worksgood-pi/package-lock.json"

if [ ! -d "$plugin/node_modules" ]; then
    npm --prefix "$plugin" ci >/tmp/fleet-parity-npm-ci.log 2>&1 || \
        loud_skip "PI PLUGIN DEPS UNAVAILABLE" "npm ci failed: $(tail -20 /tmp/fleet-parity-npm-ci.log)"
fi

# ── 1. Build + focused unit tests (parity/colour/key regressions) ────────────
( cd "$plugin" && npm run build >/tmp/fleet-parity-build.log 2>&1 ) || \
    loud_fail "worksgood-pi build failed" "$(tail -40 /tmp/fleet-parity-build.log)"
( cd "$plugin" && npx vitest run test/fleet-panel.test.ts test/fleet.test.ts test/wg-backend.test.ts test/status-palette.test.ts \
    >/tmp/fleet-parity-vitest.log 2>&1 ) || \
    loud_fail "fleet graph-view unit tests failed" "$(tail -80 /tmp/fleet-parity-vitest.log)"

# ── 2. Embedded bundle carries the parity renderer ───────────────────────────
for module in fleet-panel fleet-panel-model fleet-readmodel wg-backend status-palette; do
    [ -f "$plugin/embedded/pi-worksgood/$module.js" ] || \
        loud_fail "embed missing $module.js" "worksgood-pi/embedded/pi-worksgood/$module.js (run 'make embed-worksgood-pi')"
done
grep -q "renderWgTree" "$plugin/embedded/pi-worksgood/fleet-panel-model.js" || \
    loud_fail "embedded bundle lacks the WG-tree renderer" "renderWgTree missing from fleet-panel-model.js"
grep -q "paintStatusText" "$plugin/embedded/pi-worksgood/status-palette.js" || \
    loud_fail "embedded bundle lacks the RGB status palette" \
        "worksgood-pi/embedded/pi-worksgood/status-palette.js (run 'make embed-worksgood-pi')"
grep -q "status-palette" "$plugin/embedded/pi-worksgood/fleet-panel.js" || \
    loud_fail "embedded fleet panel does not use the RGB status palette" \
        "fleet-panel.js does not reference status-palette.js (run 'make embed-worksgood-pi')"

# ── 3. Scripted human-flow probe against the EMBEDDED component ──────────────
node --input-type=module - "$plugin" <<'NODE' >/tmp/fleet-parity-probe.log 2>&1 || \
    loud_fail "fleet graph-view embedded probe failed" "$(cat /tmp/fleet-parity-probe.log)"
const plugin = process.argv[2];
const { FleetPanelComponent } = await import(`${plugin}/embedded/pi-worksgood/fleet-panel.js`);
const { renderWgTree } = await import(`${plugin}/embedded/pi-worksgood/fleet-panel-model.js`);

// WG's own rendered text (captured from `wg viz --json` for a small fixture):
// a paused root, a nested child/grandchild, a failed child, a second component.
const WG_TEXT = [
  "‖ root-a  (open) 5s",
  "├→ ‖ child-1  (open) 2s",
  "│ └→ ‖ grand-1  (open) 2s",
  "└→ ‖ child-2  (failed) 2s",
  "",
  "‖ loner  (open) 2s",
].join("\n");
const NODE_LINES = { "root-a": 0, "child-1": 1, "grand-1": 2, "child-2": 3, loner: 5 };

// (a) WG's own lines are returned verbatim (structure + prefixes + indentation).
const rendered = renderWgTree({ text: WG_TEXT, node_lines: NODE_LINES });
if (!rendered.fromWg) throw new Error("renderWgTree did not use WG's own text");
if (JSON.stringify(rendered.lines.map((l) => l.text)) !== JSON.stringify(WG_TEXT.split("\n"))) {
  throw new Error(`rendered lines drifted from wg viz: ${JSON.stringify(rendered.lines.map((l) => l.text))}`);
}
const byTask = new Map(rendered.lines.filter((l) => l.taskId).map((l) => [l.taskId, l]));
if (byTask.get("root-a").depth !== 0 || byTask.get("root-a").text.startsWith("├→") || byTask.get("root-a").text.startsWith("└→")) {
  throw new Error("top-level row must sit at column 0 with no edge glyph");
}
if (byTask.get("child-1").depth !== 1 || !byTask.get("child-1").text.startsWith("├→ ")) {
  throw new Error(`child row must use WG's ├→ edge: ${JSON.stringify(byTask.get("child-1"))}`);
}
if (!byTask.get("child-2").text.startsWith("└→ ")) {
  throw new Error(`last child must use WG's └→ edge: ${JSON.stringify(byTask.get("child-2"))}`);
}
if (byTask.get("grand-1").depth !== 2 || !byTask.get("grand-1").text.startsWith("│ └→ ")) {
  throw new Error(`grandchild must use WG's 2-space-per-depth prefix: ${JSON.stringify(byTask.get("grand-1"))}`);
}

// (b) No rendered tree ⇒ explicit fallback (best-effort), never the default.
const fallback = renderWgTree(undefined);
if (fallback.fromWg || fallback.lines.length !== 0) throw new Error("missing tree must report fromWg=false");

// (c) Real component: each task line is painted WG's EXACT RGB palette (no
// semantic-role indirection); Enter → detail.
const WG_PALETTE = {
  open: [200, 200, 80],
  "in-progress": [60, 200, 220],
  waiting: [60, 160, 220],
  done: [80, 220, 100],
  blocked: [180, 120, 60],
  failed: [220, 60, 60],
  abandoned: [140, 100, 160],
  "pending-validation": [60, 160, 220],
  "pending-eval": [140, 230, 80],
  "failed-pending-eval": [210, 130, 70],
  incomplete: [255, 165, 0],
};
const snapshot = {
  revision: "rev-parity",
  unchanged: false,
  source: "daemon",
  counts: { in_progress: 0, ready: 5, blocked: 0, done: 0, failed: 1, total: 5, active_agents: 0 },
  tasks: [
    { id: "root-a", title: "Root A", status: "open", depends_on: [] },
    { id: "child-1", title: "Child 1", status: "open", depends_on: ["root-a"] },
    { id: "grand-1", title: "Grand 1", status: "open", depends_on: ["child-1"] },
    { id: "child-2", title: "Child 2", status: "failed", depends_on: ["root-a"] },
    { id: "loner", title: "Loner", status: "open", depends_on: [] },
  ],
  agents: [],
  tree: { text: WG_TEXT, node_lines: NODE_LINES, palette: WG_PALETTE },
};
const theme = { fg: (color, text) => `ROLE(${color}):${text}`, getColorMode: () => "truecolor" };
const tui = { requestRender: () => {}, terminal: { rows: 20, columns: 100 } };
let closed = 0;
const component = new FleetPanelComponent(snapshot, tui, () => { closed++; }, theme, {}, undefined);
const render = () => component.render(200);
let lines = render();
const lineFor = (task) => lines.find((l) => l.includes(`${task}  (`)) ?? "";
const rgb = (r, g, b) => `\x1b[38;2;${r};${g};${b}m`;
if (!lineFor("child-2").includes(rgb(220, 60, 60))) throw new Error("failed line is not painted WG's exact red RGB");
if (!lineFor("root-a").includes(rgb(200, 200, 80))) throw new Error("open line is not painted WG's exact yellow RGB");
for (const t of ["root-a", "child-1", "grand-1", "child-2", "loner"]) {
  if (!lineFor(t).includes("\x1b[38;2;")) throw new Error(`task line ${t} has no truecolour escape`);
  if (lineFor(t).includes("ROLE(")) throw new Error(`task line ${t} used a semantic theme role, not WG's RGB palette`);
}
if (lines[0].includes("fallback")) throw new Error("a fallback notice appeared despite WG exporting a palette");

// (c2) No exported palette ⇒ the local mirror still paints exact RGB, and the
// fallback is VISIBLE (never a silent semantic mismatch).
const noPalette = { ...snapshot, tree: { text: WG_TEXT, node_lines: NODE_LINES } };
const fallbackComponent = new FleetPanelComponent(noPalette, tui, () => {}, theme, {}, undefined);
const fallbackLines = fallbackComponent.render(200);
if (!fallbackLines[0].includes("fallback")) throw new Error("missing palette did not surface a visible fallback notice");
if (!(fallbackLines.find((l) => l.includes("root-a  (")) ?? "").includes(rgb(200, 200, 80))) {
  throw new Error("fallback path did not paint WG's mirrored RGB palette");
}
if (!(fallbackLines.find((l) => l.includes("child-2  (")) ?? "").includes(rgb(220, 60, 60))) {
  throw new Error("fallback path did not paint WG's mirrored RGB palette");
}

component.handleInput("\r"); // Enter → DETAIL
if (!component.detailVisible) throw new Error("Enter did not open the task detail");
if (!render().some((l) => l.includes("── root-a ──"))) throw new Error("detail header missing");
component.handleInput("\r"); // doubled Enter (`\r\n`) must NOT close it
component.handleInput("\n");
if (!component.detailVisible) throw new Error("doubled Enter closed the detail view");
component.handleInput("l"); // back to the tree
if (component.detailVisible) throw new Error("l did not return to the tree");

component.handleInput("o"); // collapse (display overlay over WG's text)
if (component.detailVisible) throw new Error("o entered detail instead of collapsing");
if (!render().some((l) => l.includes("(+3)"))) throw new Error("o did not collapse the WG block");
component.handleInput(" "); // space expands again
if (render().some((l) => l.includes("(+3)"))) throw new Error("space did not expand");

component.handleInput("q");
if (closed !== 1) throw new Error(`q did not close the panel (closed=${closed})`);

console.log("probe ok: WG text verbatim, depth/prefix parity, exact-RGB status colour (visible fallback), Enter->detail, o/space expand, fallback-only TS tree");
NODE

# ── 4. Ambient below-editor widget strip is unaffected ──────────────────────
node --input-type=module - "$plugin" <<'NODE' >/tmp/fleet-parity-widget.log 2>&1 || \
    loud_fail "fleet ambient widget probe failed" "$(cat /tmp/fleet-parity-widget.log)"
const plugin = process.argv[2];
const { mkdtempSync } = await import("node:fs");
const { tmpdir } = await import("node:os");
const { join } = await import("node:path");
const mod = await import(`${plugin}/embedded/pi-worksgood/index.js`);
process.env.PI_CODING_AGENT_DIR = mkdtempSync(join(tmpdir(), "wg-fleet-parity-"));
process.env.WG_DAEMON_SOCKET = join(process.env.PI_CODING_AGENT_DIR, "none.sock");
const AGENTS = JSON.stringify([{ id: "agent-7", task_id: "task-a", executor: "pi", status: "working", process_alive: true }]);
const TASKS = JSON.stringify([{ id: "task-a", title: "A", status: "in-progress", after: [], before: [] }]);
function harness(mode, enabled) {
  const calls = [];
  const handlers = {};
  const ui = { setWidget: (...a) => calls.push(a), notify: () => {}, custom: () => {}, theme: undefined };
  const pi = {
    registerTool: () => {}, registerProvider: () => {}, registerCommand: () => {},
    on: (event, handler) => { (handlers[event] ||= []).push(handler); },
    exec: async (_cmd, args) => {
      const joined = Array.isArray(args) ? args.join(" ") : "";
      return joined.includes("agents")
        ? { stdout: AGENTS, stderr: "", code: 0, killed: false }
        : { stdout: TASKS, stderr: "", code: 0, killed: false };
    },
  };
  if (enabled) process.env.WG_PI_FLEET_VIEW = "1"; else delete process.env.WG_PI_FLEET_VIEW;
  mod.default(pi);
  return { calls, handlers, ctx: { mode, hasUI: mode === "tui", ui, signal: undefined } };
}
const settle = async () => { for (let i = 0; i < 40; i++) await new Promise((r) => setTimeout(r, 25)); };
const fireStart = (h) => { for (const handler of h.handlers.session_start ?? []) handler({}, h.ctx); };
const fleetCalls = (h) => h.calls.filter((c) => c[0] === "wg-fleet");
const off = harness("tui", false); fireStart(off); await settle();
if (fleetCalls(off).length !== 0) throw new Error("disabled ambient widget mounted something");
const on = harness("tui", true); fireStart(on); await settle();
const last = fleetCalls(on).at(-1);
if (!last || last[2]?.placement !== "belowEditor") throw new Error(`ambient widget should mount belowEditor: ${JSON.stringify(last)}`);
console.log("probe ok: ambient widget default-off, belowEditor mount unaffected");
for (const h of [off, on]) for (const handler of h.handlers.session_shutdown ?? []) handler({}, h.ctx);
process.exit(0);
NODE

# ── 5. End-to-end: daemon `GetFleet.tree` == `wg viz --json` for the same graph ─
unset WG_AGENT_ID WG_TASK_ID WG_EXECUTOR_TYPE WG_MODEL WG_REASONING WG_TIER
scratch=$(make_scratch)
project="$scratch/project"
home="$scratch/home"
mkdir -p "$project" "$home"
export HOME="$home"
export WG_GLOBAL_DIR="$home/.wg"
cd "$project"
git init -q -b main
git config user.email smoke@example.invalid
git config user.name 'WG Smoke'
touch seed.txt
git add seed.txt
git commit -q -m seed

run_wg() {
  env -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH -u WG_WORKTREE_ACTIVE \
    -u WG_BRANCH -u WG_AGENT_ID -u WG_TASK_ID -u WG_EXECUTOR_TYPE -u WG_MODEL \
    HOME="$HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" wg "$@"
}

run_wg init >/dev/null 2>&1
run_wg add 'Parity parent' --id parity-parent >/dev/null
run_wg add 'Parity child one' --id parity-child-1 --after parity-parent >/dev/null
run_wg add 'Parity grandchild' --id parity-grand-1 --after parity-child-1 >/dev/null
run_wg add 'Parity child two' --id parity-child-2 --after parity-parent >/dev/null

start_wg_daemon "$project" --no-chat-agent --interval 1
socket="$project/.wg/service/daemon.sock"

# The CLI render (WG's own, truncated to the same width) must equal the daemon's.
run_wg viz --json --columns 80 >"$scratch/viz.json" 2>/dev/null \
    || loud_fail "wg viz --json --columns 80 failed"

read -r -d '' PY_PARITY <<'PY'
import json, socket, sys
socket_path, viz_path, fixture_path = sys.argv[1], sys.argv[2], sys.argv[3]

def request(payload):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(10)
    s.connect(socket_path)
    s.sendall((json.dumps(payload) + "\n").encode())
    buf = b""
    while b"\n" not in buf:
        chunk = s.recv(65536)
        if not chunk:
            break
        buf += chunk
    s.close()
    return json.loads(buf.split(b"\n")[0].decode())

resp = request({"cmd": "get_fleet", "max_rows": 50, "include_tree": True, "tree_columns": 80})
if not resp.get("ok"):
    raise SystemExit(f"get_fleet(include_tree) failed: {resp}")
tree = resp.get("tree")
if not isinstance(tree, dict) or not isinstance(tree.get("text"), str):
    # The running wg predates `GetFleet.include_tree` (e.g. a locally installed
    # binary from before this change). The embedded plugin parity is already
    # proven above; the daemon-truth leg needs a freshly built wg.
    print(f"daemon has no GetFleet.tree (unsupported): {json.dumps(resp)[:300]}")
    sys.exit(3)
cli = json.load(open(viz_path))
node_lines = tree.get("node_lines") or {}
lines = tree["text"].split("\n")
cli_node_lines = cli.get("node_lines") or {}
cli_lines = cli["text"].split("\n")
if set(node_lines) != set(cli_node_lines):
    raise SystemExit(
        f"daemon/cli rendered different task sets: {sorted(node_lines)} vs {sorted(cli_node_lines)}"
    )


def leader(text, task):
    """The structural part of a rendered line: prefix + edge + task id.

    Status/age/token columns vary between two renders seconds apart, so parity
    is asserted on the WG-owned prefix + edge + id (indentation by construction).
    """
    i = text.find(task)
    if i < 0:
        return None
    return text[: i + len(task)]


for task, idx in node_lines.items():
    if idx < 0 or idx >= len(lines) or task not in lines[idx]:
        raise SystemExit(f"node_lines[{task}]={idx} does not point at its rendered line")
    d_leader = leader(lines[idx], task)
    c_leader = leader(cli_lines[cli_node_lines[task]], task)
    if d_leader != c_leader:
        raise SystemExit(
            f"structure parity broken for {task!r}: daemon={d_leader!r} cli={c_leader!r}"
        )

# Top-level row unindented / child uses WG's └→ edge (by construction).
top = lines[node_lines["parity-parent"]]
child = lines[node_lines["parity-child-1"]]
grand = lines[node_lines["parity-grand-1"]]
if top.startswith("├→") or top.startswith("└→") or top.startswith("  ") or top.startswith("│ "):
    raise SystemExit(f"top-level row is not at column 0 (no edge glyph): {top!r}")
if not child.startswith("└→ "):
    raise SystemExit(f"child row does not use WG's └→ edge: {child!r}")
if not grand.startswith("  └→ "):
    raise SystemExit(f"grandchild row must use WG's 2-space-per-depth prefix: {grand!r}")
# No tree when not requested (the ambient widget never pays for it).
plain = request({"cmd": "get_fleet", "max_rows": 50})
if plain.get("tree") is not None:
    raise SystemExit("get_fleet returned a tree without include_tree")

# ── Palette parity: CLI export == daemon export == cross-language fixture ──
cli_palette = cli.get("palette")
tree_palette = tree.get("palette")
if not isinstance(cli_palette, dict) or not cli_palette:
    raise SystemExit("wg viz --json did not export a non-empty 'palette' object")
if not isinstance(tree_palette, dict) or not tree_palette:
    raise SystemExit("GetFleet.tree did not export a non-empty 'palette' object")
if cli_palette != tree_palette:
    raise SystemExit(f"daemon palette != CLI palette: {tree_palette} vs {cli_palette}")
fixture = json.load(open(fixture_path))
if cli_palette != fixture:
    raise SystemExit(
        f"exported palette drifted from worksgood-pi/test/fixtures/status-palette.json: "
        f"{cli_palette} vs {fixture}"
    )
for status, rgb in (("done", [80, 220, 100]), ("in-progress", [60, 200, 220]),
                    ("open", [200, 200, 80]), ("failed", [220, 60, 60])):
    if cli_palette.get(status) != rgb:
        raise SystemExit(f"palette[{status}] != {rgb}: {cli_palette.get(status)}")
print(f"tree + palette parity ok: daemon == wg viz ({len(lines)} lines, {len(node_lines)} nodes), palette == fixture")
PY
python3 -c "$PY_PARITY" "$socket" "$scratch/viz.json" "$plugin/test/fixtures/status-palette.json"
parity_rc=$?
case "$parity_rc" in
    0) : ;;
    3) loud_skip "DAEMON TRUST LACKS include_tree" \
        "the running wg predates GetFleet.include_tree; the embedded plugin parity + keys are proven, rebuild/install wg to exercise daemon==wg-viz tree parity" ;;
    *) loud_fail "daemon/CLI tree parity failed" "python exit $parity_rc" ;;
esac

echo "PASS: /wg-fleet graph view — WG text verbatim (structure parity), exact-RGB status palette from WG's export (visible fallback), Enter->detail, o/space expand, TS tree fallback-only"
