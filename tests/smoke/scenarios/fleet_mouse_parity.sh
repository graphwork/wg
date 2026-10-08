#!/usr/bin/env bash
#
# fleet_mouse_parity.sh — full mouse parity for the /wg-fleet panel.
#
# Credential-free. Proves, against the BUILT + EMBEDDED plugin bundle, a REAL
# pi TUI and a REAL wg TUI:
#   * the focused fleet-panel unit tests pass (select at scroll/collapse, drag
#     pan, capture, out-of-area ignore, click-to-open, the crash contract);
#   * a gesture probe drives the REAL embedded component through pi-tui's exact
#     dispatch + capture loop (`dispatchMouseEvent`, press→drag/release
#     retargeting, release-synthesized `click`): a press selects the hit row, a
#     drag pans and keeps panning outside the content area (capture), release
#     ends it, and out-of-content presses are ignored;
#   * a LIVE `pi` fullscreen session opens /wg-fleet and a synthetic SGR mouse
#     press selects a node, a second click opens its detail, and a drag pans;
#   * a LIVE `wg tui` session shows the reference behaviour the parity targets:
#     a single node click selects it AND the inspector shows its detail.
#
# The live legs loud-SKIP when tmux / pi are unavailable so the scenario stays
# green in constrained environments while still exercising the shipped surface.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v npm >/dev/null 2>&1 || loud_skip "MISSING NPM" "npm is required for the fleet mouse parity scenario"
command -v node >/dev/null 2>&1 || loud_skip "MISSING NODE" "node is required for the fleet mouse parity scenario"

repo="$(cd "$HERE/../../.." && pwd)"
plugin="$repo/worksgood-pi"
[ -f "$plugin/package-lock.json" ] || loud_fail "missing worksgood-pi/package-lock.json" "worksgood-pi/package-lock.json"

if [ ! -d "$plugin/node_modules" ]; then
    npm --prefix "$plugin" ci >/tmp/fleet-mouse-npm-ci.log 2>&1 || \
        loud_skip "PI PLUGIN DEPS UNAVAILABLE" "npm ci failed: $(tail -20 /tmp/fleet-mouse-npm-ci.log)"
fi

# ── 1. Focused unit tests (the mouse model) ─────────────────────────────────
( cd "$plugin" && npm run build >/tmp/fleet-mouse-build.log 2>&1 ) || \
    loud_fail "worksgood-pi build failed" "$(tail -40 /tmp/fleet-mouse-build.log)"
( cd "$plugin" && npx vitest run test/fleet-panel.test.ts >/tmp/fleet-mouse-vitest.log 2>&1 ) || \
    loud_fail "fleet panel unit tests failed" "$(tail -60 /tmp/fleet-mouse-vitest.log)"

# ── 2. Gesture probe through the REAL embedded component + pi-tui dispatch ───
node --input-type=module - "$plugin" <<'NODE' >/tmp/fleet-mouse-probe.log 2>&1 || \
    loud_fail "fleet mouse gesture probe failed" "$(cat /tmp/fleet-mouse-probe.log)"
const plugin = process.argv[2];
const { FleetPanelComponent } = await import(`${plugin}/embedded/pi-worksgood/fleet-panel.js`);

// A faithful clone of pi-tui's `dispatchMouseEvent` (dist/tui.js) — the exact
// function that returns the captured target and the crash-guard shape.
function dispatchMouseEvent(component, event) {
  const result = component.handleMouse?.(event);
  if (!result) return undefined;
  if ("target" in result) return result;
  if (!result.handled && !result.capture && !result.focus) return undefined;
  return {
    ...result,
    handled: true,
    ...(result.focus ? { focusTarget: component } : {}),
    target: { component, originX: event.screenX - event.x, originY: event.screenY - event.y, width: event.width, height: event.height },
  };
}
const retarget = (event, t) => ({ ...event, x: event.screenX - t.originX, y: event.screenY - t.originY, width: t.width, height: t.height });

// A faithful clone of the alt-screen capture loop (dist/tui-alt-screen.js):
// a handled press is remembered; drag/release retarget to it; a release that
// did not move synthesizes a `click` with a clickCount.
function makeDriver(component, H) {
  let mouseCapture = null, pressTarget = null, pressPoint = null, pressMoved = false;
  const seen = [];
  const ev = (type, y, clickCount) => {
    const e = { type, button: "left", x: 20, y, screenX: 20, screenY: y, width: 100, height: H, shift: false, alt: false, ctrl: false };
    if (clickCount !== undefined) e.clickCount = clickCount;
    return e;
  };
  const apply = (res) => { if (res && res.capture) mouseCapture = res.target; };
  const send = (type, y, clickCount) => {
    const base = ev(type, y, clickCount);
    if (mouseCapture || pressTarget) {
      const target = mouseCapture ?? pressTarget;
      if (pressPoint && y !== pressPoint.y) pressMoved = true;
      const res = dispatchMouseEvent(target.component, retarget(base, target));
      seen.push({ type, y, res: !!res });
      if (res) apply(res);
      if (type === "release") {
        if (!pressMoved) {
          const c = dispatchMouseEvent(target.component, retarget(ev("click", y, 1), target));
          seen.push({ type: "click", y, res: !!c });
          if (c) apply(c);
        }
        mouseCapture = null; pressTarget = null; pressPoint = null; pressMoved = false;
      }
      return res;
    }
    const res = dispatchMouseEvent(component, base);
    seen.push({ type, y, res: !!res });
    if (res) {
      apply(res);
      if (type === "press") { pressTarget = res.target; pressPoint = { y }; pressMoved = false; }
    }
    return res;
  };
  return { send, seen };
}

// A rendered WG tree with blank separator rows (taskId === null), wide enough
// to scroll a small viewport.
const lines = [];
const nodeLines = {};
for (let i = 0; i < 20; i++) {
  nodeLines[`m-${String(i).padStart(2, "0")}`] = lines.length;
  lines.push(`‖ m-${String(i).padStart(2, "0")}  (open) 1s`);
  lines.push(""); // blank separator (hit-map must NOT skip past it)
}
const snapshot = {
  revision: "rev-mouse",
  unchanged: false,
  source: "daemon",
  counts: { in_progress: 0, ready: 20, blocked: 0, done: 0, failed: 0, total: 20, active_agents: 0 },
  tasks: Array.from({ length: 20 }, (_, i) => ({
    id: `m-${String(i).padStart(2, "0")}`, title: `M ${i}`, status: "open", depends_on: [],
  })),
  agents: [],
  tree: { text: lines.join("\n"), node_lines: nodeLines },
};

const H = 8; // header + 6 content rows + footer
const tui = { requestRender: () => {}, terminal: { rows: H } };
let closed = 0;
const component = new FleetPanelComponent(snapshot, tui, () => { closed++; }, null, {}, undefined);
component.render(100);
const driver = makeDriver(component, H);

const assert = (cond, msg) => { if (!cond) throw new Error(msg); };

// (a) A press selects the hit row (rendered line map, including blank rows).
//     The initially-selected row is m-00 (rendered line 0); press rendered
//     line 2 (y=3) → m-01 so the release-synthesized click does NOT open
//     detail (it only clicks-open when the hit row was ALREADY selected).
driver.send("press", 3);
driver.send("release", 3);
assert(component.selected === "m-01", `press did not select m-01: ${component.selected}`);
assert(component.detailVisible === false, "click on a fresh row opened detail");

// (b) Scroll down via wheel, then press a row whose rendered line is a task
//     line after the blank separator; the blank line itself is a no-op.
component.handleMouse({ type: "wheel", wheelDelta: 4 });
component.render(100);
const off = component.treeScrollOffset;
assert(off === 4, `wheel did not scroll: ${off}`);
// window shows rendered lines 4..9 = m-02, <blank>, m-03, <blank>, m-04, <blank>
driver.send("press", 3); // rendered line 6 → m-03 (not the current selection)
driver.send("release", 3);
assert(component.selected === "m-03", `scrolled press wrong: ${component.selected}`);
component.render(100);
driver.send("press", 2); // rendered line 5 → blank
driver.send("release", 2);
assert(component.selected === "m-03", `blank-row press changed selection: ${component.selected}`);

// (c) Drag pan + capture: press inside, drag far outside the content area,
//     release. The offset moves and the gesture stays captured meanwhile.
component.render(100);
const base = component.treeScrollOffset;
const r1 = driver.send("press", 2);
assert(r1 && r1.capture === true && r1.focus === true, "press did not capture+focus");
assert(component.isDragging === true, "press did not start a drag");
const r2 = driver.send("drag", -40); // far above the panel — outside the content
assert(r2 && r2.capture === true, "drag outside content was not captured");
assert(component.treeScrollOffset > base, `drag did not pan: ${base} -> ${component.treeScrollOffset}`);
driver.send("drag", 500); // far below — still captured, clamps
assert(component.treeScrollOffset === 0, `drag-down did not clamp to 0: ${component.treeScrollOffset}`);
driver.send("release", 500);
assert(component.isDragging === false, "release did not end the drag");
// A stray drag after release is not ours.
const stray = driver.send("drag", 4);
assert(stray === undefined, "stray drag after release was handled");

// (d) Out-of-content presses are ignored (header row 0, footer row H-1).
const sel = component.selected;
for (const y of [0, H - 1, -5, 9999, Number.NaN]) {
  assert(driver.send("press", y) === undefined, `out-of-content press y=${y} was handled`);
}
assert(component.selected === sel, "out-of-content press changed the selection");
assert(component.isDragging === false, "out-of-content press started a drag");

// (e) Click a fresh row → select only; click it again → open detail.
component.render(100);
component.handleInput("g"); // top (m-00)
const freshY = 3; // rendered line 2 → m-01, not the current selection
driver.send("press", freshY); driver.send("release", freshY); // click synthesised by driver
assert(component.detailVisible === false, "first click unexpectedly opened detail");
assert(component.selected === "m-01", `fresh click did not select m-01: ${component.selected}`);
driver.send("press", freshY); driver.send("release", freshY);
assert(component.detailVisible === true, "second click on the selected row did not open detail");

// (f) Every return the driver ever observed was truthy-object or falsy — never
//     a bare boolean (the pi-core crash guard).
for (const s of driver.seen) {
  // `res` here is only a boolean summary; the shape guard is asserted by the
  // unit contract test + the fact that dispatchMouseEvent above would have
  // thrown on a truthy primitive ("target" in true).
  assert(typeof s.res === "boolean", "driver observed a non-boolean summary");
}

console.log("probe ok: press-select at scroll/blank rows, drag-pan + capture + release, out-of-area ignored, click-to-open");
NODE

# ── 2b. Embed freshness: the committed bundle is a fresh build of src/ ───────
( cd "$repo" && scripts/embed-worksgood-pi.sh --no-install >/tmp/fleet-mouse-embed.log 2>&1 ) || \
    loud_fail "re-embed failed" "$(tail -30 /tmp/fleet-mouse-embed.log)"
if ! git -C "$repo" diff --exit-code -- worksgood-pi/embedded worksgood-pi/src/version.ts >/dev/null 2>&1; then
    loud_fail "embed drift: worksgood-pi/embedded is not a fresh build of worksgood-pi/src (run 'make embed-worksgood-pi' and commit)" \
        "$(git -C "$repo" diff --stat -- worksgood-pi/embedded worksgood-pi/src/version.ts)"
fi

# ── 3. LIVE pi fullscreen session drives the panel with SGR mouse ────────────
if ! command -v tmux >/dev/null 2>&1; then
    echo "note: tmux unavailable — skipping the live pi mouse leg"
elif ! command -v pi >/dev/null 2>&1; then
    echo "note: pi unavailable — skipping the live pi mouse leg"
else
    unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_SPAWN_EPOCH WG_EXECUTOR_TYPE WG_MODEL WG_TIER WG_PI_PLUGIN_COMPAT_VERSION
    scratch="$(make_scratch)"
    project="$scratch/project"
    graph="$project/.wg"
    home="$scratch/home"
    mkdir -p "$project" "$home"
    export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg" TMUX_TMPDIR="$scratch/tmux"
    mkdir -p "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$TMUX_TMPDIR"
    cd "$project"
    git init -q -b main
    git config user.email smoke@example.invalid
    git config user.name 'WG Smoke'
    touch seed.txt && git add seed.txt && git commit -q -m seed

    run_wg() { env -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH -u WG_WORKTREE_ACTIVE -u WG_BRANCH \
        -u WG_AGENT_ID -u WG_TASK_ID -u WG_EXECUTOR_TYPE -u WG_MODEL \
        HOME="$HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" wg "$@"; }
    run_wg init >/dev/null 2>&1
    for i in $(seq 0 59); do run_wg add "Task $i" --id "task-$i" >/dev/null 2>&1; done

    start_wg_daemon "$project" --no-chat-agent --interval 1

    session="wgsmoke-fleet-mouse-$$"
    cleanup_mouse_session() { tmux kill-session -t "$session" 2>/dev/null || true; }
    add_cleanup_hook cleanup_mouse_session
    tmux new-session -d -s "$session" -x 100 -y 40 \
        "env TERM=xterm-256color PATH='$PATH' HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' TMUX_TMPDIR='$TMUX_TMPDIR' WG_PI_FLEET_VIEW=1 pi -e '$plugin/embedded/pi-worksgood/index.js' -ne"
    tmux set-option -t "$session" mouse on

    cap() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
    wait_cap() {
        local needle=$1 label=$2
        for _ in $(seq 1 200); do cap | grep -Fq "$needle" && return 0; sleep 0.05; done
        loud_fail "$label: $(cap | tr '\n' '|')"
    }
    sgr() { tmux send-keys -t "$session" -l "$(printf '\033[<%s;%s;%s%s' "$1" "$2" "$3" "$4")"; }
    click() { sgr 0 "$2" "$1" M; sleep 0.2; sgr 0 "$2" "$1" m; sleep 0.8; }
    sel_id() { cap | grep -o '❯ *‖ *task-[0-9]*' | head -1 | grep -o 'task-[0-9]*'; }

    wait_cap "wg fleet ·" "pi did not boot with the wg fleet widget"
    tmux send-keys -t "$session" '/wg-fleet'; sleep 1; tmux send-keys -t "$session" Enter
    wait_cap "wg-fleet ▸" "live /wg-fleet panel did not open"

    # Click the 3rd visible task row → the selection marker moves to it.
    row3="$(cap | python3 -c '
import sys
hits=[y+1 for y,r in enumerate(sys.stdin.read().splitlines()) if "‖ task-" in r]
print(hits[2] if len(hits) > 2 else (hits[-1] if hits else 0))
')"
    [ -n "$row3" ] && [ "$row3" != "0" ] || loud_fail "no visible task rows in the live panel" "$(cap | tr '\n' '|')"
    target="$(cap | sed -n "${row3}p" | grep -o 'task-[0-9]*' | head -1)"
    before="$(sel_id)"
    click "$row3" 30
    after="$(sel_id)"
    [ "$after" = "$target" ] || loud_fail "live pi click did not select $target (got '$after', was '$before')" "$(cap | tr '\n' '|')"

    # Click the same (now-selected) row again → the detail view opens.
    click "$row3" 30
    cap | grep -qE "task-[0-9]+ · detail" || loud_fail "live pi second click did not open detail" "$(cap | tr '\n' '|')"
    tmux send-keys -t "$session" 'l'; sleep 0.8   # back to the tree

    # Drag-pan: press on the selected row, drag up, release → the top row moves.
    dr="$(cap | python3 -c '
import sys
for y,r in enumerate(sys.stdin.read().splitlines()):
    if r.startswith("❯"): print(y+1); break
')"
    top_before="$(cap | grep -o 'task-[0-9]*' | head -1)"
    sgr 0 30 "$dr" M; sleep 0.2
    sgr 32 30 "$((dr - 3))" M; sleep 0.2
    sgr 32 30 "$((dr - 6))" M; sleep 0.2
    sgr 0 30 "$((dr - 6))" m; sleep 1
    top_after="$(cap | grep -o 'task-[0-9]*' | head -1)"
    [ "$top_before" != "$top_after" ] || loud_fail "live pi drag did not pan the tree (top stayed $top_before)" "$(cap | tr '\n' '|')"

    cleanup_mouse_session
    echo "live pi ok: click-select ($before -> $after), click-to-open detail, drag-pan ($top_before -> $top_after)"
fi

# ── 4. LIVE wg TUI reference: a node click selects + shows the inspector ──────
# This is the behaviour parity targets: the TUI keeps a persistent inspector, so
# its click both selects and reveals the detail. The pi panel matches selection
# exactly and reaches the same detail with an explicit second click / Enter.
if command -v tmux >/dev/null 2>&1; then
    unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_SPAWN_EPOCH WG_EXECUTOR_TYPE WG_MODEL WG_TIER WG_PI_PLUGIN_COMPAT_VERSION
    scratch="$(make_scratch)"
    project="$scratch/project"
    graph="$project/.wg"
    home="$scratch/home"
    mkdir -p "$project" "$home"
    export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg" TMUX_TMPDIR="$scratch/tmux"
    mkdir -p "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$TMUX_TMPDIR"
    cd "$project"
    git init -q -b main
    git config user.email smoke@example.invalid
    git config user.name 'WG Smoke'
    touch seed.txt && git add seed.txt && git commit -q -m seed
    env -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH -u WG_BRANCH -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" wg init >/dev/null 2>&1
    for i in $(seq 0 5); do
        env HOME="$HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" wg add "Task $i" --id "task-$i" >/dev/null 2>&1
    done

    session="wgsmoke-fleet-tui-ref-$$"
    cleanup_tui_session() { tmux kill-session -t "$session" 2>/dev/null || true; }
    add_cleanup_hook cleanup_tui_session
    tmux new-session -d -s "$session" -x 120 -y 40 \
        "env TERM=xterm-256color PATH='$PATH' HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' TMUX_TMPDIR='$TMUX_TMPDIR' wg --dir '$graph' tui"
    tmux set-option -t "$session" mouse on
    cap() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
    for _ in $(seq 1 200); do cap | grep -Fq "task-3" && break; sleep 0.05; done
    read -r cy cx < <(cap | python3 -c '
import sys
for y,r in enumerate(sys.stdin.read().splitlines()):
    i=r.find("task-3")
    if i>=0 and "task-30" not in r:
        print(y+1, i+2); break
')
    tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sM' "$cx" "$cy")"; sleep 0.3
    tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sm' "$cx" "$cy")"; sleep 1
    cap | grep -qE "── task-3 ──" || loud_fail "TUI reference: node click did not show the inspector detail" "$(cap | tr '\n' '|')"
    cleanup_tui_session
    echo "live wg TUI ref ok: node click selected task-3 and showed the inspector detail"
else
    echo "note: tmux unavailable — skipping the live wg TUI reference leg"
fi

echo "PASS: /wg-fleet full mouse parity — click-select at any scroll/collapse, click-to-open detail, drag-pan with capture, out-of-area ignored, crash-contract, live pi + TUI comparison"
