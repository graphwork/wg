#!/usr/bin/env bash
#
# fleet_panel_ux_fixes.sh — the operator-reported /wg-fleet UX gaps, end to end.
#
# Credential-free. Proves, against the BUILT + EMBEDDED plugin bundle and a REAL
# fullscreen `pi` session, the five gaps from the live-use report:
#
#   1. CLOSE IS DELIBERATE AND ANNOUNCED — `q`/`Esc` from the tree close the
#      panel and the session says `fleet closed (q)` / `fleet closed (esc)`;
#      Ctrl+C (pi's global clear/interrupt) is REFUSED with a visible
#      `ctrl+c ignored` notice instead of silently destroying the view.
#   2. CLICK-TASK-INFO — clicks reliably select the hit row and a second click
#      opens its detail, at any scroll offset and across collapsed subtrees
#      (rendered-line hit map), driven through pi-tui's EXACT dispatch loop.
#   3. CLICK-HEADER-TO-BACK — the detail header carries a visible `[< Back]` and
#      clicking row 0 returns to the tree, in a live pi session too.
#   4. DRAG-RESIZE + FRACTION CONTROL — the top edge is a captured resize grip
#      (pi mouse contract `{handled, capture}`), `+`/`-` (and `]`/`[`) resize
#      from the keyboard, and the chosen fraction is PERSISTED as
#      `fleetViewHeightPct` in the extension config so it sticks across opens.
#   5. PALETTE — WG-RGB stays the only default path, and the fallback is VISIBLE
#      and explained (`palette: fallback (daemon older than plugin)`), with the
#      palette source stated unconditionally (`palette: wg-rgb`); `/wg status`
#      gains the STACK line (daemon build age + plugin-cache currency) so a stale
#      daemon/plugin pair is visible from INSIDE the session.
#
# The live legs loud-SKIP when tmux / pi are unavailable so the scenario stays
# green in constrained environments while still exercising the shipped surface.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v npm >/dev/null 2>&1 || loud_skip "MISSING NPM" "npm is required for the fleet panel UX scenario"
command -v node >/dev/null 2>&1 || loud_skip "MISSING NODE" "node is required for the fleet panel UX scenario"

repo="$(cd "$HERE/../../.." && pwd)"
plugin="$repo/worksgood-pi"
[ -f "$plugin/package-lock.json" ] || loud_fail "missing worksgood-pi/package-lock.json" "worksgood-pi/package-lock.json"

if [ ! -d "$plugin/node_modules" ]; then
    npm --prefix "$plugin" ci >/tmp/fleet-ux-npm-ci.log 2>&1 || \
        loud_skip "PI PLUGIN DEPS UNAVAILABLE" "npm ci failed: $(tail -20 /tmp/fleet-ux-npm-ci.log)"
fi

# ── 1. Focused unit tests (close notify, Ctrl+C, resize, palette, hit map) ───
( cd "$plugin" && npm run build >/tmp/fleet-ux-build.log 2>&1 ) || \
    loud_fail "worksgood-pi build failed" "$(tail -40 /tmp/fleet-ux-build.log)"
( cd "$plugin" && npx vitest run test/fleet-panel.test.ts test/fleet.test.ts \
    test/stack-status.test.ts test/toggle-commands.test.ts >/tmp/fleet-ux-vitest.log 2>&1 ) || \
    loud_fail "fleet panel UX unit tests failed" "$(tail -60 /tmp/fleet-ux-vitest.log)"

# ── 2. The embedded bundle carries every new surface ─────────────────────────
[ -f "$plugin/embedded/pi-worksgood/stack-status.js" ] || \
    loud_fail "embed missing stack-status.js" "worksgood-pi/embedded/pi-worksgood/stack-status.js"
[ -f "$plugin/embedded/pi-worksgood/plugin-stamp.js" ] || \
    loud_fail "embed missing plugin-stamp.js" "worksgood-pi/embedded/pi-worksgood/plugin-stamp.js"
grep -q "daemon older than plugin" "$plugin/embedded/pi-worksgood/status-palette.js" || \
    loud_fail "embedded palette notice lost its explanation" "status-palette.js: daemon older than plugin"
grep -q "palette: wg-rgb" "$plugin/embedded/pi-worksgood/status-palette.js" || \
    loud_fail "embedded palette status lost its explicit wg-rgb state" "status-palette.js: palette: wg-rgb"
grep -q "ctrl+c ignored" "$plugin/embedded/pi-worksgood/fleet-panel.js" || \
    loud_fail "embedded panel has no Ctrl+C refusal" "fleet-panel.js: ctrl+c ignored"
grep -q "\[< Back\]" "$plugin/embedded/pi-worksgood/fleet-panel.js" || \
    loud_fail "embedded detail header has no visible [< Back]" "fleet-panel.js: [< Back]"
grep -q "onHeightPctChange" "$plugin/embedded/pi-worksgood/fleet-panel.js" || \
    loud_fail "embedded panel cannot persist its height" "fleet-panel.js: onHeightPctChange"
grep -q "writeFleetViewHeightPct" "$plugin/embedded/pi-worksgood/fleet-view.js" || \
    loud_fail "embedded /wg-fleet does not persist the height fraction" "fleet-view.js: writeFleetViewHeightPct"
grep -q "fleetViewHeightPct" "$plugin/embedded/pi-worksgood/fleet-config.js" || \
    loud_fail "embedded config has no fleetViewHeightPct key" "fleet-config.js: fleetViewHeightPct"
grep -q "stack:" "$plugin/embedded/pi-worksgood/stack-status.js" || \
    loud_fail "embedded stack-status has no status segment" "stack-status.js: stack:"

# ── 3. Behavioural probe: the REAL embedded component + pi-tui dispatch ──────
node --input-type=module - "$plugin" <<'NODE' >/tmp/fleet-ux-probe.log 2>&1 || \
    loud_fail "fleet panel UX embedded probe failed" "$(cat /tmp/fleet-ux-probe.log)"
const plugin = process.argv[2];
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync } = await import("node:fs");
const { tmpdir } = await import("node:os");
const { join } = await import("node:path");
const { FleetPanelComponent } = await import(`${plugin}/embedded/pi-worksgood/fleet-panel.js`);
const { writeFleetViewHeightPct, readFleetViewConfig } = await import(
  `${plugin}/embedded/pi-worksgood/fleet-config.js`
);

const assert = (cond, msg) => {
  if (!cond) throw new Error(msg);
};

// A faithful clone of pi-tui's `dispatchMouseEvent` + alt-screen capture loop:
// a handled press is remembered, drag/release retarget to it, and a release that
// did not move synthesizes a `click` — exactly how real pi delivers the events.
function dispatchMouseEvent(component, event) {
  const result = component.handleMouse?.(event);
  if (!result) return undefined;
  if ("target" in result) return result;
  if (!result.handled && !result.capture && !result.focus) return undefined;
  return {
    ...result,
    handled: true,
    ...(result.focus ? { focusTarget: component } : {}),
    target: {
      component,
      originX: event.screenX - event.x,
      originY: event.screenY - event.y,
      width: 100,
      height: event.height,
    },
  };
}
const retarget = (event, t) => ({
  ...event,
  x: event.screenX - t.originX,
  y: event.screenY - t.originY,
  width: t.width,
  height: t.height,
});

function makeDriver(component, H) {
  let mouseCapture = null;
  let pressTarget = null;
  let pressPoint = null;
  let pressMoved = false;
  const ev = (type, y, clickCount) => {
    const e = {
      type,
      button: "left",
      x: 20,
      y,
      screenX: 20,
      screenY: y,
      width: 100,
      height: H,
      shift: false,
      alt: false,
      ctrl: false,
    };
    if (clickCount !== undefined) e.clickCount = clickCount;
    return e;
  };
  const apply = (res) => {
    if (res && res.capture) mouseCapture = res.target;
  };
  const send = (type, y) => {
    const base = ev(type, y);
    if (mouseCapture || pressTarget) {
      const target = mouseCapture ?? pressTarget;
      if (pressPoint && y !== pressPoint.y) pressMoved = true;
      const res = dispatchMouseEvent(target.component, retarget(base, target));
      if (res) apply(res);
      if (type === "release") {
        if (!pressMoved) {
          const c = dispatchMouseEvent(target.component, retarget(ev("click", y, 1), target));
          if (c) apply(c);
        }
        mouseCapture = null;
        pressTarget = null;
        pressPoint = null;
        pressMoved = false;
      }
      return res;
    }
    const res = dispatchMouseEvent(component, base);
    if (res) {
      apply(res);
      if (type === "press") {
        pressTarget = res.target;
        pressPoint = { y };
        pressMoved = false;
      }
    }
    return res;
  };
  return { send };
}

// A WG-rendered tree with blank separators (hit-map must not skip past them).
const lines = [];
const nodeLines = {};
for (let i = 0; i < 20; i++) {
  nodeLines[`m-${String(i).padStart(2, "0")}`] = lines.length;
  lines.push(`‖ m-${String(i).padStart(2, "0")}  (open) 1s`);
  lines.push("");
}
const PALETTE = { open: [200, 200, 80] };
const snapshot = (withPalette) => ({
  revision: "rev-ux",
  unchanged: false,
  source: "daemon",
  counts: { in_progress: 0, ready: 20, blocked: 0, done: 0, failed: 0, total: 20, active_agents: 0 },
  tasks: Array.from({ length: 20 }, (_, i) => ({
    id: `m-${String(i).padStart(2, "0")}`,
    title: `M ${i}`,
    status: "open",
    depends_on: [],
  })),
  agents: [],
  tree: {
    text: lines.join("\n"),
    node_lines: nodeLines,
    ...(withPalette ? { palette: PALETTE } : {}),
  },
});

const H = 20; // 40-row terminal at 50% → 20 rows
const make = (opts = {}) => {
  const tui = { requestRender: () => {}, terminal: { rows: 40, columns: 100 } };
  const persisted = [];
  const closed = [];
  const component = new FleetPanelComponent(
    snapshot(opts.withPalette !== false),
    tui,
    () => closed.push(component.closedWith ?? "unknown"),
    opts.theme ?? null,
    { heightPct: opts.heightPct ?? 1, onHeightPctChange: (p) => persisted.push(p) },
    undefined,
    async () => ({ text: "wg show body line 1\nline 2" }),
  );
  return { component, persisted, closed };
};

// (1) CLOSE: q/Esc name their trigger; Ctrl+C never closes and says so.
{
  const q = make();
  q.component.render(100);
  q.component.handleInput("q");
  assert(q.closed.length === 1 && q.closed[0] === "q", `q did not close with trigger: ${q.closed}`);
  const esc = make();
  esc.component.render(100);
  esc.component.handleInput("\x1b");
  assert(esc.closed[0] === "esc", `Esc did not close with trigger: ${esc.closed}`);
  const cc = make();
  cc.component.render(100);
  cc.component.handleInput("\x03");
  assert(cc.closed.length === 0, "Ctrl+C closed the panel");
  assert(cc.component.visibleNotice.includes("ctrl+c ignored"), "Ctrl+C was not explained in-panel");
  assert(cc.component.render(100).at(-1).includes("ctrl+c ignored"), "the Ctrl+C notice is not rendered");
  // ... and it is never a silent no-op in the detail view either.
  cc.component.openDetail();
  cc.component.handleInput("\x03");
  assert(cc.closed.length === 0, "Ctrl+C closed the panel from the detail view");
}

// (2) CLICK-TASK-INFO across scroll offsets and blank rows.
{
  const { component } = make();
  component.render(100);
  const driver = makeDriver(component, 20);
  // press → release synthesizes a click; a fresh row only SELECTS.
  driver.send("press", 3);
  driver.send("release", 3);
  assert(component.selected === "m-01", `click did not select m-01: ${component.selected}`);
  assert(component.detailVisible === false, "a first click opened detail");
  // Click the SAME row again → detail.
  driver.send("press", 3);
  driver.send("release", 3);
  assert(component.detailVisible === true, "a second click on the selected row did not open detail");
  component.handleInput("q"); // back to the tree
  // Scroll, then press every rendered row (blank/arc rows included): the hit map
  // must address the RENDERED line list, not the task-only order.
  component.handleMouse({ type: "wheel", wheelDelta: 5 });
  component.render(100);
  const rows = [...component.hitLines];
  assert(rows.some((id) => id === null), "the fixture has no blank/arc rows to exercise");
  for (let r = 0; r < rows.length; r++) {
    const expected = rows[r];
    const res = component.handleMouse({ type: "press", y: r + 1, button: "left" });
    component.handleMouse({ type: "release", y: r + 1, button: "left" });
    if (expected) assert(component.selected === expected, `row ${r} mapped to ${component.selected}, want ${expected}`);
    assert(res === undefined || typeof res === "object", "press returned a bare primitive");
    assert(component.detailVisible === false, "a press opened the detail view");
  }
}

// (3) CLICK-HEADER-TO-BACK with a visible [< Back].
{
  const { component } = make();
  component.render(100);
  component.openDetail();
  const detail = component.render(100);
  assert(detail[0].includes("[< Back]"), `detail header has no [< Back]: ${detail[0]}`);
  const back = component.handleMouse({ type: "click", y: 0, button: "left", clickCount: 1 });
  assert(back && typeof back === "object", "header click did not return an object result");
  assert(component.detailVisible === false, "header click did not return to the tree");
}

// (4) DRAG-RESIZE + keyboard + PERSISTENCE.
{
  const { component, persisted } = make({ heightPct: 0.5 });
  assert(component.render(100).length === 20, `expected 20 rows at 50%, got ${component.render(100).length}`);
  const press = component.handleMouse({ type: "press", y: 0, button: "left" });
  assert(press.capture === true && press.handled === true && press.focus === true, "top-edge press is not a capture");
  assert(component.isResizing === true, "top-edge press did not start a resize");
  component.handleMouse({ type: "drag", y: -8, button: "left" });
  assert(Math.abs(component.heightFraction - 0.7) < 1e-9, `drag up did not grow: ${component.heightFraction}`);
  assert(component.render(100).length === 28, "the render did not grow with the fraction");
  assert(persisted.length === 0, "a drag step persisted (should persist once on release)");
  component.handleMouse({ type: "release", y: -8, button: "left" });
  assert(persisted.length === 1 && Math.abs(persisted[0] - 0.7) < 1e-9, `release persisted: ${persisted}`);
  assert(component.isResizing === false, "release did not end the resize");

  // Keyboard path (pi's regular mode has no mouse at all).
  component.handleInput("-");
  assert(Math.abs(component.heightFraction - 0.65) < 1e-9, `'-' did not shrink: ${component.heightFraction}`);
  component.handleInput("+");
  assert(Math.abs(component.heightFraction - 0.7) < 1e-9, `'+' did not grow: ${component.heightFraction}`);
  assert(persisted.at(-1) === component.heightFraction, "keyboard resize did not persist");

  // The persisted fraction round-trips through the EXTENSION CONFIG.
  const home = mkdtempSync(join(tmpdir(), "wg-fleet-ux-"));
  mkdirSync(join(home, "extensions", "pi-worksgood"), { recursive: true });
  const file = join(home, "extensions", "pi-worksgood", "config.json");
  writeFileSync(file, JSON.stringify({ fleetView: true, fleetViewCollapsed: false }));
  const written = writeFleetViewHeightPct(component.heightFraction, { PI_CODING_AGENT_DIR: home });
  assert(written === file, `height not written: ${written}`);
  const raw = JSON.parse(readFileSync(file, "utf8"));
  assert(Math.abs(raw.fleetViewHeightPct - component.heightFraction) < 1e-9, "fleetViewHeightPct not persisted");
  assert(raw.fleetView === true && raw.fleetViewCollapsed === false, "persisting the height clobbered other keys");
  const reopened = readFleetViewConfig({ PI_CODING_AGENT_DIR: home });
  assert(
    Math.abs(reopened.heightPct - component.heightFraction) < 1e-9,
    `the persisted height does not round-trip: ${reopened.heightPct}`,
  );
  const second = make({ heightPct: reopened.heightPct });
  assert(
    second.component.render(100).length === component.render(100).length,
    "a reopened panel did not honour the persisted height",
  );
}

// (5) PALETTE: WG-RGB on the default path, an explained fallback otherwise.
{
  const theme = { fg: (_c, t) => `ROLE:${t}`, getColorMode: () => "truecolor" };
  const withWg = make({ theme });
  const good = withWg.component.render(200);
  assert(good[0].includes("palette: wg-rgb"), `no explicit wg-rgb state: ${good[0]}`);
  assert(!good[0].includes("fallback"), "a fallback notice appeared on the RGB path");
  assert(
    good.some((l) => l.includes("38;2;200;200;80")),
    "WG's exported RGB was not used for the task lines",
  );
  const noPalette = make({ theme, withPalette: false });
  const fallback = noPalette.component.render(200);
  assert(fallback[0].includes("palette: fallback"), `fallback not surfaced: ${fallback[0]}`);
  // The explanation gets its OWN warning row (so a long counts header can never
  // truncate it away) and names both the cause and the recovery command.
  assert(fallback[1].includes("palette: fallback (daemon older than plugin)"), `no cause row: ${fallback[1]}`);
  assert(fallback[1].includes("wg dev-sync"), "the fallback does not name the recovery command");
  // ... and the fallback panel still paints WG's mirrored RGB (not theme roles).
  assert(fallback.some((l) => l.includes("38;2;200;200;80")), "the fallback stopped painting RGB");
}

// (6) THE /wg status STACK line (daemon build age + plugin-cache currency).
{
  const { formatStackStatus } = await import(`${plugin}/embedded/pi-worksgood/stack-status.js`);
  const pluginText = (embed) =>
    [
      "WorksGood Pi integration status (pi-worksgood / @worksgood/pi)",
      `  embed digest:     ${embed}`,
      `  cache digest:     ${embed}`,
      "  cache state:      current (matches this binary's embedded build)",
    ].join("\n");
  const current = formatStackStatus({
    status: { service: { running: true, uptime: "3h" } },
    pluginStatusText: pluginText("b3:same"),
    pluginDigest: "b3:same",
    workerSession: false,
  });
  assert(current.verdict === "current", `healthy stack misreported: ${current.text}`);
  assert(current.text.includes("daemon up 3h"), "the stack line omits the daemon's age");
  assert(current.text.includes("plugin cache: current"), "the stack line omits cache currency");
  const stale = formatStackStatus({
    status: { service: { running: true, uptime: "3h" } },
    pluginStatusText: pluginText("b3:newer"),
    pluginDigest: "b3:older",
    workerSession: false,
  });
  assert(stale.verdict === "skew", `stale build not flagged: ${stale.text}`);
  assert(stale.text.includes("STALE") && stale.text.includes("wg dev-sync"), "stale build has no fix");
  const worker = formatStackStatus({
    status: { service: { running: true } },
    pluginStatusText: null,
    pluginDigest: undefined,
    workerSession: true,
  });
  assert(worker.verdict === "unknown", "a refused probe was reported as healthy");
}

console.log("probe ok: deliberate close + notify trigger, Ctrl+C refused, click->select->detail at every offset, [< Back] header click, drag/keyboard resize + persisted fleetViewHeightPct, palette stated/explained, /wg status stack line");
NODE

# ── 4. Embed freshness (the committed bundle IS a build of src/) ─────────────
( cd "$repo" && scripts/embed-worksgood-pi.sh --no-install >/tmp/fleet-ux-embed.log 2>&1 ) || \
    loud_fail "re-embed failed" "$(tail -30 /tmp/fleet-ux-embed.log)"
if ! git -C "$repo" diff --exit-code -- worksgood-pi/embedded worksgood-pi/src/version.ts >/dev/null 2>&1; then
    loud_fail "embed drift: worksgood-pi/embedded is not a fresh build of worksgood-pi/src (run 'make embed-worksgood-pi' and commit)" \
        "$(git -C "$repo" diff --stat -- worksgood-pi/embedded worksgood-pi/src/version.ts)"
fi

# ── 5. Embed == cache (when the installed wg can answer) ────────────────────
embed_compat="$(python3 -c "import json;print(json.load(open('$plugin/embedded/version.json'))['compat'])")"
installed_compat="$(wg pi-plugin compat-version 2>/dev/null || true)"
if [ -n "$installed_compat" ]; then
    [ "$embed_compat" = "$installed_compat" ] || \
        loud_fail "compat drift: embedded=$embed_compat installed wg=$installed_compat"
    status_out="$(wg pi-plugin status 2>/dev/null || true)"
    embed_digest="$(printf '%s' "$status_out" | sed -n 's/.*embed digest:[[:space:]]*\(\S*\).*/\1/p' | head -n1)"
    cache_digest="$(printf '%s' "$status_out" | sed -n 's/.*cache digest:[[:space:]]*\(\S*\).*/\1/p' | head -n1)"
    if [ -n "$embed_digest" ] && [ -n "$cache_digest" ] && [ "$embed_digest" != "$cache_digest" ]; then
        loud_fail "wg pi-plugin status reports embed != cache digests" "embed=$embed_digest cache=$cache_digest"
    fi
else
    echo "note: 'wg pi-plugin status' unavailable here (worker/admin gate) — embed==cache is asserted by the operator-side dev-sync"
fi

# ── 6. LIVE fullscreen `pi` session: the real human flow, real SGR mouse ────
if ! command -v tmux >/dev/null 2>&1; then
    echo "note: tmux unavailable — skipping the LIVE pi fleet-panel UX leg"
elif ! command -v pi >/dev/null 2>&1; then
    echo "note: pi unavailable — skipping the LIVE pi fleet-panel UX leg"
else
    unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_SPAWN_EPOCH WG_EXECUTOR_TYPE WG_MODEL WG_TIER \
        WG_PI_PLUGIN_COMPAT_VERSION WG_WORKER_CAPABILITY WG_WORKER_IPC
    scratch="$(make_scratch)"
    project="$scratch/project"
    home="$scratch/home"
    mkdir -p "$project" "$home"
    export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg" TMUX_TMPDIR="$scratch/tmux"
    mkdir -p "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$TMUX_TMPDIR"
    # Seed the PERSISTED height fraction the live session must honour.
    cfg_dir="$HOME/.pi/agent/extensions/pi-worksgood"
    mkdir -p "$cfg_dir"
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
    for i in $(seq 0 39); do run_wg add "Task $i" --id "task-$i" >/dev/null 2>&1; done

    session="wgsmoke-fleet-ux-$$"
    cleanup_ux_session() { tmux kill-session -t "$session" 2>/dev/null || true; }
    add_cleanup_hook cleanup_ux_session
    tmux new-session -d -s "$session" -x 100 -y 40 \
        "env TERM=xterm-256color PATH='$PATH' HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' TMUX_TMPDIR='$TMUX_TMPDIR' WG_PI_FLEET_VIEW=1 pi -e '$plugin/embedded/pi-worksgood/index.js' -ne"
    tmux set-option -t "$session" mouse on

    cap() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
    wait_cap() {
        local needle=$1 label=$2
        for _ in $(seq 1 240); do cap | grep -Fq "$needle" && return 0; sleep 0.05; done
        loud_fail "$label: $(cap | tr '\n' '|')"
    }
    wait_gone() {
        local needle=$1 label=$2
        for _ in $(seq 1 240); do cap | grep -Fq "$needle" || return 0; sleep 0.05; done
        loud_fail "$label: $(cap | tr '\n' '|')"
    }
    sgr() { tmux send-keys -t "$session" -l "$(printf '\033[<%s;%s;%s%s' "$1" "$2" "$3" "$4")"; }
    click() { sgr 0 "$2" "$1" M; sleep 0.2; sgr 0 "$2" "$1" m; sleep 0.7; }
    # The panel's TOP row on screen: the tree header, or the detail header (which
    # leads with `[< Back]`). Mouse coordinates are screen coordinates, so the
    # clickable header must be located from the capture, never assumed.
    header_row() {
        cap | python3 -c '
import sys
for y, row in enumerate(sys.stdin.read().splitlines(), 1):
    if "wg-fleet ▸" in row or "[< Back]" in row:
        print(y); raise SystemExit(0)
print(0)
'
    }
    pct_shown() { cap | grep -o '⇕ [0-9]*%' | head -1; }

    wait_cap "wg fleet" "pi did not boot with the wg fleet strip"
    tmux send-keys -t "$session" '/wg-fleet'; sleep 1; tmux send-keys -t "$session" Enter
    wait_cap "wg-fleet ▸" "live /wg-fleet panel did not open"

    # (5a) The palette source is stated in the live panel (never silent).
    cap | grep -qE "palette: (wg-rgb|fallback)" || \
        loud_fail "the live panel does not state its palette source" "$(cap | tr '\n' '|')"

    # (4a) The PERSISTED height fraction is honoured on open (config 0.5 → 50%).
    pct="$(pct_shown)"
    [ "$pct" = "⇕ 50%" ] || loud_fail "the persisted fleetViewHeightPct was not honoured on open (header: '$pct')" "$(cap | tr '\n' '|')"

    # (1a) Ctrl+C must NOT destroy the panel, and must explain itself.
    tmux send-keys -t "$session" C-c; sleep 0.8
    cap | grep -Fq "wg-fleet ▸" || loud_fail "Ctrl+C silently closed the live fleet panel" "$(cap | tr '\n' '|')"
    cap | grep -Fq "ctrl+c ignored" || loud_fail "the live panel did not explain the ignored Ctrl+C" "$(cap | tr '\n' '|')"

    # (4b) DRAG the top edge DOWN to shrink the panel: the header percentage falls.
    hr="$(header_row)"
    [ "$hr" != "0" ] || loud_fail "no live panel header row for the resize drag" "$(cap | tr '\n' '|')"
    sgr 0 40 "$hr" M; sleep 0.2
    sgr 32 40 "$((hr + 6))" M; sleep 0.3
    sgr 32 40 "$((hr + 10))" M; sleep 0.3
    sgr 0 40 "$((hr + 10))" m; sleep 1
    newpct="$(pct_shown)"
    [ "$newpct" != "⇕ 50%" ] || loud_fail "the live drag did not resize the panel (still '$newpct')" "$(cap | tr '\n' '|')"
    cap | grep -Fq "wg-fleet ▸" || loud_fail "the panel vanished during the resize drag" "$(cap | tr '\n' '|')"

    # (4c) Keyboard resize works too (there is a keyboard path for every mouse
    # gesture), and the chosen fraction lands in the plugin config on disk.
    tmux send-keys -t "$session" -l '+'; sleep 0.8
    kbpct="$(pct_shown)"
    [ "$kbpct" != "$newpct" ] || loud_fail "the live keyboard resize did nothing (still '$kbpct')" "$(cap | tr '\n' '|')"
    python3 - "$cfg_dir/config.json" "$kbpct" <<'PY_CHECK' || loud_fail "the live resize did not persist fleetViewHeightPct" "config=$(cat "$cfg_dir/config.json" 2>/dev/null)"
import json, re, sys
path, shown = sys.argv[1], sys.argv[2]
want = int(re.sub(r"[^0-9]", "", shown)) / 100.0
raw = json.load(open(path))
got = float(raw["fleetViewHeightPct"])
if abs(got - want) > 0.001:
    raise SystemExit(f"persisted height {got} != shown {want}")
if raw.get("fleetView") is not True:
    raise SystemExit("the resize clobbered unrelated config keys")
PY_CHECK

    # (2) CLICK a task row → selects it; click again → its detail opens.
    row="$(cap | python3 -c '
import sys
hits=[y+1 for y,r in enumerate(sys.stdin.read().splitlines()) if "‖ task-" in r]
print(hits[2] if len(hits) > 2 else (hits[-1] if hits else 0))
')"
    [ "$row" != "0" ] || loud_fail "no visible task rows in the live panel" "$(cap | tr '\n' '|')"
    target="$(cap | sed -n "${row}p" | grep -o 'task-[0-9]*' | head -1)"
    click "$row" 40; click "$row" 40
    wait_cap "detail (wg show)" "a second live click did not open the task detail"

    # (3) The detail header carries a visible [< Back]; CLICKING it returns.
    wait_cap "[< Back]" "the live detail header has no [< Back] affordance"
    dhr="$(header_row)"
    [ "$dhr" != "0" ] || loud_fail "no live detail header row to click" "$(cap | tr '\n' '|')"
    click "$dhr" 40
    for _ in $(seq 1 100); do cap | grep -Fq "detail (wg show)" || break; sleep 0.05; done
    cap | grep -Fq "detail (wg show)" && loud_fail "the header click did not leave the detail view" "$(cap | tr '\n' '|')"
    wait_cap "⇕ " "clicking the live detail header did not return to the tree"

    # (1b) An explicit close is announced with its trigger.
    tmux send-keys -t "$session" Escape; sleep 1
    wait_gone "wg-fleet ▸" "Esc did not close the live fleet panel"
    cap | grep -Fq "fleet closed (esc)" || loud_fail "the live close was not announced with its trigger" "$(cap | tr '\n' '|')"

    # (5b) `/wg status` surfaces the STACK line from inside the session.
    tmux send-keys -t "$session" '/wg status'; sleep 1
    tmux send-keys -t "$session" Enter; sleep 1
    # pi's slash-command completion may swallow the first Enter (it accepts the
    # completion); a second Enter runs it.
    cap | grep -Fq "stack:" || { tmux send-keys -t "$session" Enter; sleep 1; }
    for _ in $(seq 1 120); do cap | grep -Fq "stack:" && break; sleep 0.05; done
    cap | grep -Fq "stack:" || loud_fail "/wg status does not surface a stack line" "$(cap | tr '\n' '|')"
    cap | grep -qE "stack:.*(current|STALE|unknown|worker session)" || \
        loud_fail "/wg status stack line has no honest verdict" "$(cap | tr '\n' '|')"

    cleanup_ux_session
    echo "live pi ok: palette stated, persisted 50% honoured, Ctrl+C refused + explained, drag-resize ($pct -> $newpct), keyboard resize ($kbpct) saved, click-select/open, [< Back] click, 'fleet closed (esc)', /wg status stack line"
fi

echo "PASS: /wg-fleet operator-report fixes — deliberate+announced close (Ctrl+C refused), click-select/detail at any offset, [< Back] header click, top-edge drag + keyboard resize persisted as fleetViewHeightPct, palette fallback visible+explained (WG-RGB default), /wg status stack line (daemon build age + plugin-cache currency), embed fresh"
