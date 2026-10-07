#!/usr/bin/env bash
#
# fleet_scrollable_view_contract.sh — pins the **scrollable** /wg-fleet panel.
#
# Credential-free. Proves, against the BUILT + EMBEDDED plugin bundle and a real
# daemon:
#   * the focused fleet-panel unit tests pass (scroll math, tail bounding,
#     component interactions, config gate);
#   * the embedded bundle carries the scrollable panel (`fleet-panel.js`,
#     `fleet-panel-model.js`) and `/wg-fleet` opens it (`openFleetPanel` wiring);
#   * a scripted human-flow probe drives the REAL embedded component through
#     wheel / PgUp / PgDn / Home / End keystrokes and mouse-wheel events: the
#     tree scrolls, the selection follows, the render never exceeds the panel
#     bounds, Enter drills into detail, q closes;
#   * the read-only daemon `GetFleet` surface returns the fleet snapshot the
#     panel renders (counts + task rows + revision delta) and never mutates the
#     graph;
#   * the ambient below-editor widget strip is unaffected (default-off; mounts
#     below the editor when enabled, non-TUI silent).
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v npm >/dev/null 2>&1 || loud_skip "MISSING NPM" "npm is required for the fleet scrollable panel contract"
command -v node >/dev/null 2>&1 || loud_skip "MISSING NODE" "node is required for the fleet scrollable panel contract"

repo="$(cd "$HERE/../../.." && pwd)"
plugin="$repo/worksgood-pi"
[ -f "$plugin/package-lock.json" ] || loud_fail "missing worksgood-pi/package-lock.json" \
    "worksgood-pi/package-lock.json"

if [ ! -d "$plugin/node_modules" ]; then
    npm --prefix "$plugin" ci >/tmp/fleet-scroll-npm-ci.log 2>&1 || \
        loud_skip "PI PLUGIN DEPS UNAVAILABLE" "npm ci failed: $(tail -20 /tmp/fleet-scroll-npm-ci.log)"
fi

# ── 1. Build + focused unit tests (scroll math, tail bounds, component) ──────
( cd "$plugin" && npm run build >/tmp/fleet-scroll-build.log 2>&1 ) || \
    loud_fail "worksgood-pi build failed" "$(tail -40 /tmp/fleet-scroll-build.log)"
( cd "$plugin" && npx vitest run test/fleet-panel.test.ts test/fleet.test.ts >/tmp/fleet-scroll-vitest.log 2>&1 ) || \
    loud_fail "fleet panel unit tests failed" "$(tail -60 /tmp/fleet-scroll-vitest.log)"

# ── 2. The embedded bundle carries the scrollable panel ──────────────────────
for module in fleet-panel fleet-panel-model; do
    [ -f "$plugin/embedded/pi-worksgood/$module.js" ] || \
        loud_fail "embed missing $module.js" "worksgood-pi/embedded/pi-worksgood/$module.js (run 'make embed-worksgood-pi')"
done
grep -q "openFleetPanel" "$plugin/embedded/pi-worksgood/fleet-view.js" || \
    loud_fail "embedded /wg-fleet does not open the scrollable panel" "fleet-view.js must reference openFleetPanel"
grep -q "FLEET_WIDGET_KEY" "$plugin/embedded/pi-worksgood/fleet-view.js" || \
    loud_fail "embed missing the ambient widget key" "FLEET_WIDGET_KEY"
# Read-only proof at the artifact level: the panel never sends a mutating IPC command.
if grep -Eq '"cmd"[^,]*(:|, )"(spawn|graph_changed|add_task|shutdown|worker|kill|done|fail)"' \
    "$plugin/embedded/pi-worksgood/fleet-panel.js" \
    "$plugin/embedded/pi-worksgood/fleet-panel-model.js"; then
    loud_fail "embedded fleet panel artifact references a mutating IPC command"
fi

# ── 3. Scripted human-flow probe against the EMBEDDED component ──────────────
node --input-type=module - "$plugin" <<'NODE' >/tmp/fleet-scroll-probe.log 2>&1 || \
    loud_fail "fleet scrollable embedded probe failed" "$(cat /tmp/fleet-scroll-probe.log)"
const plugin = process.argv[2];
const { FleetPanelComponent } = await import(`${plugin}/embedded/pi-worksgood/fleet-panel.js`);
const { maxScroll } = await import(`${plugin}/embedded/pi-worksgood/fleet-panel-model.js`);

const N = 40;
const snapshot = {
  revision: "rev-probe",
  unchanged: false,
  source: "daemon",
  counts: { in_progress: 1, ready: N - 1, blocked: 0, done: 0, failed: 0, total: N, active_agents: 1 },
  tasks: Array.from({ length: N }, (_, i) => ({
    id: `task-${String(i).padStart(3, "0")}`,
    title: `Task ${i}`,
    status: i === 0 ? "in-progress" : "open",
    depends_on: i === 0 ? [] : [`task-000`],
    assigned: i === 0 ? "agent-7" : null,
  })),
  agents: [{ id: "agent-7", task_id: "task-000", status: "working", activity: "running cargo test --lib" }],
};

const HEIGHT = 6;
const renders = [];
const tui = { requestRender: () => {}, terminal: { rows: HEIGHT } };
let closed = 0;
const component = new FleetPanelComponent(snapshot, tui, () => { closed++; }, null, {}, undefined);
const render = (w = 100) => { const lines = component.render(w); renders.push(lines); return lines; };

// (a) never render past the panel bounds.
let lines = render();
if (lines.length > HEIGHT) throw new Error(`render exceeded bounds: ${lines.length} > ${HEIGHT}`);
if (!lines[0].includes("wg-fleet ▸")) throw new Error(`counts header missing: ${lines[0]}`);

// (b) arrow selection scrolls the tree into view and stays visible.
for (let i = 0; i < N - 1; i++) component.handleInput("\x1b[B");
if (component.selected !== "task-039") throw new Error(`selection did not follow arrows: ${component.selected}`);
if (component.treeScrollOffset !== maxScroll(N, component.bodyViewport())) {
  throw new Error(`selection not scrolled to bottom: ${component.treeScrollOffset}`);
}
lines = render();
const hitBottom = component.hitLines.indexOf(component.selected);
if (hitBottom < 0 || hitBottom >= component.bodyViewport()) throw new Error("selected row not visible after scroll");
if (lines.length > HEIGHT) throw new Error(`render exceeded bounds after scroll: ${lines.length}`);

// (c) Home + PageDown + wheel with clamping.
component.handleInput("\x1b[H"); // Home
if (component.selected !== "task-000" || component.treeScrollOffset !== 0) {
  throw new Error(`Home did not reset to the top: ${component.selected} @ ${component.treeScrollOffset}`);
}
component.handleInput("\x1b[6~"); // PageDown
if (component.treeScrollOffset <= 0) throw new Error("PageDown did not scroll the tree");
if (!component.handleMouse({ type: "wheel", wheelDelta: -100 })) throw new Error("wheel event not handled");
if (component.treeScrollOffset !== 0) throw new Error(`wheel-up did not clamp to top: ${component.treeScrollOffset}`);
component.handleMouse({ type: "wheel", wheelDelta: 4 });
if (component.treeScrollOffset <= 0) throw new Error("wheel-down did not scroll");

// (d) Enter drills into detail; WG's own text is fetched and rendered, the
// live activity is appended as a labelled addition; q returns to the tree.
component.handleInput("\x1b[H");
component.handleInput("\x1b[B"); // select task-001 (nested under task-000)
const WG_DETAIL = "Task: task-001\nTitle: Task 1\nCompletion contract: land";
const detailFetcher = async (taskId) => ({ task_id: taskId, text: `${WG_DETAIL}\n`, source: "daemon" });
const detailTui = { requestRender: () => {}, terminal: { rows: 30, columns: 100 } };
const detailComponent = new FleetPanelComponent(
  snapshot,
  detailTui,
  () => { closed++; },
  null,
  {},
  undefined,
  detailFetcher,
);
const detailRender = (w = 100) => detailComponent.render(w);
detailComponent.handleInput("\r"); // open detail on task-000 (the agent's task)
component.handleInput("\r"); // Enter
if (!component.detailVisible) throw new Error("Enter did not open the task detail");
const detail = render();
if (!detail.some((l) => l.includes("wg-fleet · task-001 · detail"))) throw new Error("detail header missing");
if (detail.length > HEIGHT) throw new Error(`detail render exceeded bounds: ${detail.length}`);
// q in the detail returns to the TREE — it must NOT close the panel.
component.handleInput("q");
if (component.detailVisible) throw new Error("q did not return to the tree from detail");
if (closed !== 0) throw new Error(`q in detail closed the whole panel (closed=${closed})`);
// The detail component renders WG's text verbatim, then a labelled addition.
detailComponent.handleInput("\r");
await new Promise((r) => setTimeout(r, 0));
const dlines = detailRender();
if (dlines[1] !== "Task: task-001") throw new Error(`detail body is not WG's text: ${dlines[1]}`);
if (dlines[2] !== "Title: Task 1") throw new Error(`WG detail line drifted: ${dlines[2]}`);
const addIdx = dlines.findIndex((l) => l.includes("pi-side additions"));
if (addIdx < 3) throw new Error("pi-side additions not appended after WG's text");

// (e) q closes (from the tree).
component.handleInput("q");
if (closed !== 1) throw new Error(`q did not close the panel (closed=${closed})`);

console.log("probe ok: bounds, selection-follows-scroll, wheel/PgDn/Home, WG detail verbatim + additions, q returns to tree then closes");
NODE

# ── 4. Ambient widget strip: default-off + below-editor mount, non-TUI silent ─
node --input-type=module - "$plugin" <<'NODE' >/tmp/fleet-scroll-widget.log 2>&1 || \
    loud_fail "fleet ambient widget probe failed" "$(cat /tmp/fleet-scroll-widget.log)"
const plugin = process.argv[2];
const { mkdtempSync } = await import("node:fs");
const { tmpdir } = await import("node:os");
const { join } = await import("node:path");
const mod = await import(`${plugin}/embedded/pi-worksgood/index.js`);

process.env.PI_CODING_AGENT_DIR = mkdtempSync(join(tmpdir(), "wg-fleet-scroll-"));
process.env.WG_DAEMON_SOCKET = join(process.env.PI_CODING_AGENT_DIR, "none.sock");

const AGENTS = JSON.stringify([{ id: "agent-7", task_id: "task-a", executor: "pi", status: "working", process_alive: true }]);
const TASKS = JSON.stringify([{ id: "task-a", title: "A", status: "in-progress", after: [], before: [] }]);

function harness(mode, enabled) {
  const calls = [];
  const handlers = {};
  const ui = { setWidget: (...a) => calls.push(a), notify: () => {}, custom: () => {}, theme: undefined };
  const pi = {
    registerTool: () => {}, registerProvider: () => {},
    registerCommand: () => {},
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

const off = harness("tui", false);
fireStart(off); await settle();
if (fleetCalls(off).length !== 0) throw new Error("disabled ambient widget mounted something");

const on = harness("tui", true);
fireStart(on); await settle();
const last = fleetCalls(on).at(-1);
if (!last) throw new Error("enabled ambient widget did not mount");
if (last[0] !== "wg-fleet" || !last[2] || last[2].placement !== "belowEditor") {
  throw new Error(`expected belowEditor widget, got ${JSON.stringify(last)}`);
}
if (!Array.isArray(last[1]) || last[1].length < 2) throw new Error("ambient widget should stay multi-line");

const json = harness("json", true);
fireStart(json); await settle();
if (fleetCalls(json).length !== 0) throw new Error("non-TUI mode mounted a widget");

console.log("probe ok: ambient widget default-off, belowEditor mount, non-TUI silent");
for (const h of [off, on, json]) for (const handler of h.handlers.session_shutdown ?? []) handler({}, h.ctx);
process.exit(0);
NODE

# ── 5. Live read-only GetFleet daemon round trip ────────────────────────────
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
run_wg add 'Fleet parent' --id fleet-parent >/dev/null
run_wg add 'Fleet child' --id fleet-child --after fleet-parent >/dev/null

start_wg_daemon "$project" --no-chat-agent --interval 1
socket="$project/.wg/service/daemon.sock"

read -r -d '' PY_FLEET <<'PY'
import json, socket, sys

socket_path = sys.argv[1]

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

resp = request({"cmd": "get_fleet", "max_rows": 50})
if not resp.get("ok"):
    raise SystemExit(f"get_fleet failed: {resp}")
tasks = {t["id"]: t for t in resp["tasks"]}
for wanted in ("fleet-parent", "fleet-child"):
    if wanted not in tasks:
        raise SystemExit(f"get_fleet missing task {wanted}: {sorted(tasks)}")
if tasks["fleet-child"]["status"] != "open":
    raise SystemExit(f"fleet-child should be open, got {tasks['fleet-child']['status']}")
if list(tasks["fleet-child"]["depends_on"]) != ["fleet-parent"]:
    raise SystemExit(f"fleet-child depends_on wrong: {tasks['fleet-child']['depends_on']}")
counts = resp["counts"]
if counts.get("total") != 2:
    raise SystemExit(f"get_fleet counts.total wrong: {counts}")
if "description" in json.dumps(resp):
    raise SystemExit("get_fleet leaked an unbounded description field")
revision = resp["revision"]
if not revision:
    raise SystemExit("get_fleet missing revision token")

# Bounded delta: echoing the revision returns counts only, no rows.
delta = request({"cmd": "get_fleet", "since_revision": revision})
if not delta.get("ok") or delta.get("unchanged") is not True:
    raise SystemExit(f"get_fleet delta not unchanged: {delta}")
if delta.get("tasks"):
    raise SystemExit("get_fleet delta should carry no task rows")

print(f"get_fleet ok: total={counts['total']} revision={revision[:24]}… delta=unchanged")
PY
python3 -c "$PY_FLEET" "$socket" || loud_fail "live read-only GetFleet round trip failed"

# Read-only proof: the snapshot poll never mutated the graph.
before="$(run_wg list --json 2>/dev/null | python3 -c 'import json,sys;print(len(json.load(sys.stdin)))')"
python3 -c "$PY_FLEET" "$socket" >/dev/null || loud_fail "second GetFleet round trip failed"
after="$(run_wg list --json 2>/dev/null | python3 -c 'import json,sys;print(len(json.load(sys.stdin)))')"
[ "$before" = "$after" ] || loud_fail "GetFleet mutated the graph: ${before} -> ${after}"

embedded_compat="$(python3 -c "import json;print(json.load(open('$plugin/embedded/version.json'))['compat'])")"
installed_compat="$(run_wg pi-plugin compat-version 2>/dev/null || echo unknown)"
[ "$embedded_compat" = "$installed_compat" ] \
    || loud_fail "compat drift: embedded=$embedded_compat installed wg=$installed_compat"

# ── 6. Embed == cache (the shipped surface is the built one) ─────────────────
status_out="$(run_wg pi-plugin status 2>/dev/null || true)"
embed_digest="$(printf '%s' "$status_out" | sed -n 's/.*embed digest:[[:space:]]*\(\S*\).*/\1/p' | head -n1)"
cache_digest="$(printf '%s' "$status_out" | sed -n 's/.*cache digest:[[:space:]]*\(\S*\).*/\1/p' | head -n1)"
if [ -n "$embed_digest" ] && [ -n "$cache_digest" ] && [ "$embed_digest" != "$cache_digest" ]; then
    loud_fail "wg pi-plugin status reports embed != cache digests" "embed=$embed_digest cache=$cache_digest"
fi
if ! printf '%s' "$status_out" | grep -q .; then
    echo "note: wg pi-plugin status produced no output (non-blocking)"
fi

echo "PASS: /wg-fleet scrollable panel — bounded tree scroll, selection-follows-scroll, detail drill-down, read-only GetFleet, ambient widget unaffected"
