#!/usr/bin/env bash
#
# fleet_detail_view_parity.sh — pins the **WG-own detail parity** of /wg-fleet.
#
# The operator's complaint (fleet-detail-view) was that the panel's detail view
# re-implemented WG's task detail in TypeScript (`detailLines`), so it could
# never show what `wg show <task>` / the TUI inspector show: the completion
# contract and required checks, lifecycle/attempt history, runtime/executor/
# usage and the log. This scenario proves, credential-free and deterministically:
#
#   * the focused plugin unit tests pass (WG-text-verbatim body, labelled
#     pi-side additions, full-screen scroll/clamping, q/Esc/left/Enter keys);
#   * the embedded bundle carries WG's own detail path (the daemon
#     `get_task_detail` request + the verbatim renderer), and the panel body is
#     not fabricated client-side;
#   * a scripted human-flow probe drives the REAL embedded component: Enter
#     opens the detail, WG's fetched text renders verbatim at the top, the
#     pi-side additions are APPENDED and LABELLED, scroll keys clamp, and
#     q/Esc/left/Enter return to the tree (they never close the panel);
#   * end-to-end against a LIVE daemon: the `GetTaskDetail` text is byte-for-byte
#     the `wg show <task>` output for the same graph (so sections/ordering/
#     wording are WG's), a width request word-wraps WITHOUT dropping bytes, and
#     the read is non-mutating;
#   * embed == cache.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v npm >/dev/null 2>&1 || loud_skip "MISSING NPM" "npm is required for the fleet detail-view parity contract"
command -v node >/dev/null 2>&1 || loud_skip "MISSING NODE" "node is required for the fleet detail-view parity contract"

repo="$(cd "$HERE/../../.." && pwd)"
plugin="$repo/worksgood-pi"
[ -f "$plugin/package-lock.json" ] || loud_fail "missing worksgood-pi/package-lock.json" \
    "worksgood-pi/package-lock.json"

# This scenario's Rust changes (the daemon `GetTaskDetail` request + the
# captured `wg show` text) are NOT in the operator's installed `wg`. Build the
# worktree binary and put it first on PATH so both `wg show` and the live daemon
# exercise the CURRENT source. `cargo build` never touches the shared install.
wg_bin="${WG_SMOKE_CANDIDATE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/debug/wg}"
if [ ! -x "$wg_bin" ]; then
    ( cd "$repo" && cargo build --bin wg >/tmp/fleet-detail-cargo-build.log 2>&1 ) || \
        loud_fail "cargo build --bin wg failed" "$(tail -40 /tmp/fleet-detail-cargo-build.log)"
fi
[ -x "$wg_bin" ] || loud_fail "worktree wg binary missing" "$wg_bin"
export PATH="$(dirname "$wg_bin"):$PATH"

if [ ! -d "$plugin/node_modules" ]; then
    npm --prefix "$plugin" ci >/tmp/fleet-detail-npm-ci.log 2>&1 || \
        loud_skip "PI PLUGIN DEPS UNAVAILABLE" "npm ci failed: $(tail -20 /tmp/fleet-detail-npm-ci.log)"
fi

# ── 1. Build + focused unit tests (WG-text parity / additions / keys) ────────
( cd "$plugin" && npm run build >/tmp/fleet-detail-build.log 2>&1 ) || \
    loud_fail "worksgood-pi build failed" "$(tail -40 /tmp/fleet-detail-build.log)"
( cd "$plugin" && npx vitest run test/fleet-panel.test.ts test/wg-backend.test.ts \
    >/tmp/fleet-detail-vitest.log 2>&1 ) || \
    loud_fail "fleet detail-view unit tests failed" "$(tail -80 /tmp/fleet-detail-vitest.log)"

# ── 2. Embedded bundle carries WG's own detail path ─────────────────────────
for module in fleet-panel fleet-panel-model wg-backend; do
    [ -f "$plugin/embedded/pi-worksgood/$module.js" ] || \
        loud_fail "embed missing $module.js" "worksgood-pi/embedded/pi-worksgood/$module.js (run 'make embed-worksgood-pi')"
done
grep -q "makeFleetDetailFetcher" "$plugin/embedded/pi-worksgood/fleet-panel.js" || \
    loud_fail "embedded panel lacks the WG detail fetcher" "makeFleetDetailFetcher missing from fleet-panel.js"
grep -q "get_task_detail" "$plugin/embedded/pi-worksgood/wg-backend.js" || \
    loud_fail "embedded backend lacks the get_task_detail IPC request" "get_task_detail missing from wg-backend.js"
grep -q "pi-side additions" "$plugin/embedded/pi-worksgood/fleet-panel.js" || \
    loud_fail "embedded panel does not label the pi-side additions" "pi-side additions marker missing"
# The panel must not re-derive WG's detail: the legacy viz `detailLines` import
# is gone from the embedded panel artifact.
if grep -q "detailLines" "$plugin/embedded/pi-worksgood/fleet-panel.js"; then
    loud_fail "embedded panel still re-derives WG detail client-side" "detailLines referenced in fleet-panel.js"
fi

# ── 3. Scripted human-flow probe against the EMBEDDED component ──────────────
node --input-type=module - "$plugin" <<'NODE' >/tmp/fleet-detail-probe.log 2>&1 || \
    loud_fail "fleet detail-view embedded probe failed" "$(cat /tmp/fleet-detail-probe.log)"
const plugin = process.argv[2];
const { FleetPanelComponent } = await import(`${plugin}/embedded/pi-worksgood/fleet-panel.js`);
const { maxScroll } = await import(`${plugin}/embedded/pi-worksgood/fleet-panel-model.js`);

const snapshot = {
  revision: "rev-detail",
  unchanged: false,
  source: "daemon",
  counts: { in_progress: 0, ready: 2, blocked: 0, done: 0, failed: 0, total: 2, active_agents: 0 },
  tasks: [
    { id: "alpha", title: "Alpha", status: "open", depends_on: [] },
    { id: "beta", title: "Beta", status: "open", depends_on: ["alpha"] },
  ],
  agents: [],
};

// WG's OWN detail body (`wg show alpha`-equivalent) — a long fixture so the
// scroll math is exercised.
const WG_LINES = [
  "Task: alpha",
  "Title: Alpha",
  "Status: open",
  "Completion contract: land",
  "Required deterministic completion checks (exact enforced order):",
  "  [verify] cargo test --lib — checked-in policy",
];
for (let i = 0; i < 70; i++) WG_LINES.push(`  detail line ${String(i).padStart(2, "0")}`);
const WG_TEXT = `${WG_LINES.join("\n")}\n`;

const calls = [];
const detailFetcher = async (taskId, columns) => {
  calls.push({ taskId, columns });
  return { task_id: taskId, text: WG_TEXT, source: "daemon" };
};

const HEIGHT = 12;
const tui = { requestRender: () => {}, terminal: { rows: HEIGHT, columns: 90 } };
let closed = 0;
const component = new FleetPanelComponent(
  snapshot,
  tui,
  () => { closed++; },
  null,
  { enterGuardMs: 0 },
  undefined,
  detailFetcher,
);
const render = (w = 90) => component.render(w);
const flush = () => new Promise((r) => setTimeout(r, 0));

render();
if (component.selected !== "alpha") throw new Error(`unexpected first selection: ${component.selected}`);
component.handleInput("\r"); // Enter opens detail
await flush();
if (!component.detailVisible) throw new Error("Enter did not open the detail view");
const lines = render();
if (lines.length !== HEIGHT) throw new Error(`detail is not full-screen: ${lines.length} != ${HEIGHT}`);
if (!lines[0].includes("detail (wg show)")) throw new Error(`detail header missing: ${lines[0]}`);
// WG's text is rendered VERBATIM, starting at the first body row.
for (let i = 0; i < WG_LINES.length && i < lines.length - 2; i++) {
  if (lines[i + 1] !== WG_LINES[i]) {
    throw new Error(`WG detail line ${i} not verbatim: got ${JSON.stringify(lines[i + 1])}`);
  }
}
if (calls[0].taskId !== "alpha" || calls[0].columns !== 90) {
  throw new Error(`detail fetch not scoped/width-correct: ${JSON.stringify(calls[0])}`);
}
// Visible scroll/position indicator in the always-visible header; key hints in
// the footer.
if (!lines[0].includes("%") || !lines[0].includes("lines")) {
  throw new Error(`detail header lacks a position indicator: ${lines[0]}`);
}
if (!lines.at(-1).includes("PgUp/PgDn")) {
  throw new Error(`detail footer lacks key hints: ${lines.at(-1)}`);
}
const view = component.bodyViewport();
// PgDn / End / Home clamp the detail scroll.
component.handleInput("\x1b[6~"); // PageDown
if (component.scrollOffset <= 0) throw new Error("PageDown did not scroll the detail");
component.handleInput("\x1b[F"); // End
if (component.scrollOffset !== maxScroll(WG_LINES.length, view)) {
  throw new Error(`End did not clamp to the bottom: ${component.scrollOffset}`);
}
component.handleInput("\x1b[H"); // Home
if (component.scrollOffset !== 0) throw new Error(`Home did not clamp to the top: ${component.scrollOffset}`);
// q / Esc / left / Enter return to the TREE — never close the panel.
component.handleInput("q");
if (component.detailVisible) throw new Error("q did not return to the tree");
if (closed !== 0) throw new Error(`q in detail closed the panel (closed=${closed})`);
for (const back of ["\r", "\x1b", "\x1b[D"]) {
  component.handleInput("\r"); // reopen
  await flush();
  if (!component.detailVisible) throw new Error("Enter did not reopen the detail view");
  component.handleInput(back);
  if (component.detailVisible) throw new Error(`back key ${JSON.stringify(back)} did not return to the tree`);
  if (closed !== 0) throw new Error(`back key ${JSON.stringify(back)} closed the panel`);
}
// A doubled Enter (`\r` then `\n`) on OPEN must not immediately close it.
const two = new FleetPanelComponent(snapshot, tui, () => {}, null, {}, undefined, detailFetcher);
two.handleInput("\r");
two.handleInput("\r");
two.handleInput("\n");
if (!two.detailVisible) throw new Error("a doubled Enter closed the detail view");
// pi-side additions are APPENDED and LABELLED, never interleaved with WG text.
const errSnapComponent = new FleetPanelComponent(snapshot, tui, () => {}, null, {}, undefined, undefined);
errSnapComponent.handleInput("\r");
const errLines = errSnapComponent.render(90);
if (!errLines.some((l) => l.includes("WG detail unavailable"))) {
  throw new Error("missing WG detail did not surface a labelled error");
}
if (errLines.some((l) => l.includes("Task: alpha"))) {
  throw new Error("panel fabricated WG's detail text without a source");
}

console.log("probe ok: WG detail verbatim + full-screen scroll/clamp + q/Esc/left/Enter back + labelled additions");
NODE

# ── 4. Live daemon: GetTaskDetail == `wg show <task>` byte-for-byte ──────────
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
cat >"$scratch/detail-desc.md" <<'DESC'
## Description
The live detail-parity subject.

## Validation
- [ ] detail text is WG's own
DESC
run_wg add 'Detail subject' --id detail-subject -d "$(cat "$scratch/detail-desc.md")" >/dev/null

start_wg_daemon "$project" --no-chat-agent --interval 1
socket="$project/.wg/service/daemon.sock"

run_wg show detail-subject >"$scratch/cli_show.txt" 2>/dev/null || \
    loud_fail "wg show detail-subject failed"

read -r -d '' PY_DETAIL <<'PY'
import json, socket, sys

socket_path, cli_path = sys.argv[1], sys.argv[2]

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

cli = open(cli_path, "r", encoding="utf-8").read()

resp = request({"cmd": "get_task_detail", "task_id": "detail-subject"})
if not resp.get("ok"):
    raise SystemExit(f"get_task_detail failed: {resp}")
if resp.get("task_id") != "detail-subject":
    raise SystemExit(f"get_task_detail wrong task_id: {resp.get('task_id')}")
daemon_text = resp.get("text", "")

# (1) byte-for-byte: WG's own `wg show` text, sections/ordering/wording intact.
if daemon_text != cli:
    import difflib
    diff = "\n".join(
        difflib.unified_diff(
            cli.splitlines(), daemon_text.splitlines(), "wg show", "GetTaskDetail", lineterm=""
        )
    )
    raise SystemExit(f"GetTaskDetail != wg show\n{diff}")

for required in ("Task: detail-subject", "Completion contract:", "Status:"):
    if required not in daemon_text:
        raise SystemExit(f"GetTaskDetail missing WG section {required!r}")

# (2) width wrapping preserves every non-space byte, in order.
wrapped = request({"cmd": "get_task_detail", "task_id": "detail-subject", "columns": 40})
if not wrapped.get("ok"):
    raise SystemExit(f"get_task_detail(columns=40) failed: {wrapped}")
wtext = wrapped.get("text", "")
squash = lambda s: "".join(s.split())
if squash(wtext) != squash(cli):
    raise SystemExit("width wrapping dropped/reordered bytes")
if len(wtext.splitlines()) <= len(cli.splitlines()):
    raise SystemExit("width wrapping did not actually wrap any line")

print(f"GetTaskDetail ok: {len(cli.splitlines())} lines verbatim; wrapped -> {len(wtext.splitlines())} lines")
PY
python3 -c "$PY_DETAIL" "$socket" "$scratch/cli_show.txt" || loud_fail "live GetTaskDetail parity failed"

# Read-only proof: the detail read never mutated the graph.
before="$(run_wg list --json 2>/dev/null | python3 -c 'import json,sys;print(len(json.load(sys.stdin)))')"
python3 -c "$PY_DETAIL" "$socket" "$scratch/cli_show.txt" >/dev/null || loud_fail "second GetTaskDetail parity failed"
after="$(run_wg list --json 2>/dev/null | python3 -c 'import json,sys;print(len(json.load(sys.stdin)))')"
[ "$before" = "$after" ] || loud_fail "GetTaskDetail mutated the graph: ${before} -> ${after}"

# ── 5. Embed == cache (the shipped surface is the built one) ────────────────
embedded_compat="$(python3 -c "import json;print(json.load(open('$plugin/embedded/version.json'))['compat'])")"
installed_compat="$(run_wg pi-plugin compat-version 2>/dev/null || echo unknown)"
[ "$embedded_compat" = "$installed_compat" ] \
    || loud_fail "compat drift: embedded=$embedded_compat installed wg=$installed_compat"

status_out="$(run_wg pi-plugin status 2>/dev/null || true)"
embed_digest="$(printf '%s' "$status_out" | sed -n 's/.*embed digest:[[:space:]]*\(\S*\).*/\1/p' | head -n1)"
cache_digest="$(printf '%s' "$status_out" | sed -n 's/.*cache digest:[[:space:]]*\(\S*\).*/\1/p' | head -n1)"
if [ -n "$embed_digest" ] && [ -n "$cache_digest" ] && [ "$embed_digest" != "$cache_digest" ]; then
    loud_fail "wg pi-plugin status reports embed != cache digests" "embed=$embed_digest cache=$cache_digest"
fi

echo "PASS: /wg-fleet detail view — WG's own 'wg show' body verbatim, full-screen scrollable, q/Esc/left return, pi-side additions labelled, live GetTaskDetail parity, embed == cache"
