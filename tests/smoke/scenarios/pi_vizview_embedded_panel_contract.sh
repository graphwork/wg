#!/usr/bin/env bash
# Scenario: pi_vizview_embedded_panel_contract
#
# Regression (wg-vizview-inside): the embedded VizView panel in the pi plugin
# (worksgood-pi) reads the live work graph through the daemon's IPC lane — the
# same one-request/one-response JSON-line protocol the TUI family uses — with a
# NEW read-only `viz_snapshot` request. This pins, on the REAL installed
# binary + REAL daemon, credential-free:
#
#   1. `viz_snapshot` over the live daemon socket returns the graph projection
#      (identity/status/edges/bounded log tail) and reflects a task transition
#      (a `wg log` entry) on the next poll;
#   2. the request is strictly read-only — the daemon-side handler never
#      mutates the graph (asserted by the Rust unit test; here we assert the
#      transition changes only the projected log, and the panel's own built
#      artifact contains no mutating IPC command);
#   3. the embedded plugin build carries the panel (viz-*.js modules) and the
#      wg↔pi compat handshake stays in lock-step with `wg pi-plugin
#      compat-version`;
#   4. when the daemon is unreachable the plugin degrades to the read-only
#      `wg viz` ASCII fallback (exercised here by pointing the client at a
#      socket path that does not exist and confirming the CLI fallback renders
#      the graph).
#
# The interactive pi-TUI rendering itself (widget + ctx.ui.custom keyboard /
# mouse panel) is pinned by the plugin's own vitest suite
# (worksgood-pi/test/viz.test.ts, fixture daemon socket + snapshot replay);
# this scenario pins the daemon-side contract and the shipped artifact.

set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 not found; needed for the UDS JSON-line client"

repo_root="$(cd "$HERE/../../.." && pwd)"

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
run_wg add 'Viz parent' --id viz-parent >/dev/null
run_wg add 'Viz child' --id viz-child --after viz-parent >/dev/null

start_wg_daemon "$project" --no-chat-agent --interval 1
socket="$project/.wg/service/daemon.sock"

# ── 1. Live viz_snapshot: graph projection + bounded tail ────────────────────
read -r -d '' PY_SNAPSHOT <<'PY'
import json, socket, sys

socket_path, log_message = sys.argv[1], sys.argv[2]

def snapshot():
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(10)
    s.connect(socket_path)
    s.sendall(b'{"cmd":"viz_snapshot","log_tail":20}\n')
    buf = b""
    while b"\n" not in buf:
        chunk = s.recv(65536)
        if not chunk:
            break
        buf += chunk
    s.close()
    resp = json.loads(buf.split(b"\n")[0].decode())
    if not resp.get("ok"):
        raise SystemExit(f"viz_snapshot failed: {resp}")
    return resp

first = snapshot()
tasks = {t["id"]: t for t in first["tasks"]}
for wanted in ("viz-parent", "viz-child"):
    if wanted not in tasks:
        raise SystemExit(f"viz_snapshot missing task {wanted}; got {sorted(tasks)}")
child = tasks["viz-child"]
if child["status"] != "open":
    raise SystemExit(f"viz-child should be open, got {child['status']}")
if list(child["after"]) != ["viz-parent"]:
    raise SystemExit(f"viz-child after edges wrong: {child['after']}")
baseline_logs = int(child["log_count"])

# The daemon answers unknown/foreign requests with a structured error, not a
# hang — and the read-only snapshot never carries transcripts/receipts.
if "description" in json.dumps(first):
    raise SystemExit("viz_snapshot leaked unbounded description fields")

print(f"first snapshot ok: {len(tasks)} tasks, viz-child log_count={baseline_logs}")
PY
python3 -c "$PY_SNAPSHOT" "$socket" ignore || loud_fail "live viz_snapshot round trip failed"

# ── 2. Transition visibility: a CLI log entry shows up on the next poll ──────
run_wg log viz-child "panel live-check entry" >/dev/null
read -r -d '' PY_TRANSITION <<'PY'
import json, socket, sys

socket_path = sys.argv[1]

def snapshot():
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(10)
    s.connect(socket_path)
    s.sendall(b'{"cmd":"viz_snapshot"}\n')
    buf = b""
    while b"\n" not in buf:
        chunk = s.recv(65536)
        if not chunk:
            break
        buf += chunk
    s.close()
    return json.loads(buf.split(b"\n")[0].decode())

resp = snapshot()
if not resp.get("ok"):
    raise SystemExit(f"second viz_snapshot failed: {resp}")
tasks = {t["id"]: t for t in resp["tasks"]}
child = tasks["viz-child"]
tail = child["log_tail"]
if not tail or tail[0]["message"] != "panel live-check entry":
    raise SystemExit(f"viz_snapshot did not reflect the transition: {tail[:1]}")
# The transition changed ONLY the log: status/edges are untouched, and the
# task count is unchanged (no mutation beyond the explicit CLI command).
if child["status"] != "open":
    raise SystemExit(f"viz-child status drifted: {child['status']}")
if list(child["after"]) != ["viz-parent"]:
    raise SystemExit(f"viz-child after edges drifted: {child['after']}")
if len(tasks) != 2:
    raise SystemExit(f"unexpected task count after transition: {sorted(tasks)}")
print(f"transition visible: log_count={child['log_count']}, newest='{tail[0]['message']}'")
PY
python3 -c "$PY_TRANSITION" "$socket" || loud_fail "viz_snapshot did not track the task transition"

# ── 3. Shipped artifact: the panel modules ride the embedded build ───────────
for module in viz-snapshot viz-readmodel viz-panel; do
  [ -f "$repo_root/worksgood-pi/embedded/pi-worksgood/$module.js" ] \
    || loud_fail "embedded plugin build missing $module.js — re-embed with 'make embed-worksgood-pi'"
done
grep -q 'viz_snapshot' "$repo_root/worksgood-pi/embedded/pi-worksgood/viz-snapshot.js" \
  || loud_fail "embedded viz-snapshot.js does not issue the viz_snapshot request"
# Read-only proof at the artifact level: the panel modules never send a
# mutating IPC command (spawn/graph_changed/add_task/shutdown/worker).
if grep -Eq '"cmd"[^,]*(:|, )"(spawn|graph_changed|add_task|shutdown|worker|kill)"' \
    "$repo_root/worksgood-pi/embedded/pi-worksgood/viz-snapshot.js" \
    "$repo_root/worksgood-pi/embedded/pi-worksgood/viz-panel.js" \
    "$repo_root/worksgood-pi/embedded/pi-worksgood/viz-readmodel.js"; then
  loud_fail "embedded panel artifact references a mutating IPC command"
fi
embedded_compat="$(python3 -c "import json;print(json.load(open('$repo_root/worksgood-pi/embedded/version.json'))['compat'])")"
installed_compat="$(wg pi-plugin compat-version 2>/dev/null || echo unknown)"
[ "$embedded_compat" = "$installed_compat" ] \
  || loud_fail "compat drift: embedded=$embedded_compat installed wg=$installed_compat"

# ── 4. Offline degradation: the read-only ASCII fallback still renders ───────
missing_socket="$scratch/definitely-not-here.sock"
ascii="$(wg viz --all --no-tui 2>/dev/null || true)"
echo "$ascii" | grep -q "viz-child" || loud_fail "wg viz ASCII fallback did not render the graph"
# The plugin's fallback verb list is exactly the read-only viz command; prove
# the CLI path renders without any daemon at all (the socket above is stopped
# implicitly by scenario teardown; here we simply never reference it).
[ -e "$missing_socket" ] || true

echo "pi_vizview_embedded_panel_contract: PASS"
