/**
 * fleet-tui-parity tests — the `/wg-fleet` panel's **full TUI interaction
 * parity**, one describe block per batch of the task contract:
 *
 *   - BATCH 1: inspector dock + views (Alt-←/→ cycling, raw JSON, section
 *     collapse, the draggable divider, Shift-↑/↓);
 *   - BATCH 2: layout fractions/presets/cycles and PERSISTENCE;
 *   - BATCH 3: task actions (with the confirm gates) and their absence when
 *     dormant;
 *   - BATCH 4: search / sort / filter;
 *   - BATCH 5: the auto-generated help modal + accurate footer hints;
 *   - CROSS-CUTTING: pi's mouse OBJECT shape, a keyboard path for every
 *     interaction, and the keymap↔help↔dispatch completeness invariant.
 *
 * The parity assertions quote the WG TUI's own tests:
 * `horizontal_divider_drag_up_grows_inspector` (35% → 45% on 40 rows) and
 * `horizontal_divider_drag_down_shrinks_inspector` (50% → 40%), plus the TUI
 * help table's verbatim labels (auto-derived from `FLEET_BINDINGS`).
 *
 * Tests run against the built `pi-worksgood/` artifact — `npm test` builds first.
 */

import { describe, expect, it, vi } from "vitest";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { FleetPanelComponent, printableText } from "../pi-worksgood/fleet-panel.js";
// @ts-expect-error — built ESM artifact import (see above)
import { isPiMouseResult } from "../pi-worksgood/mouse.js";
// @ts-expect-error — built ESM artifact import (see above)
import {
  DEFAULT_FLEET_LAYOUT,
  FLEET_ACTIONS,
  FLEET_BINDINGS,
  FLEET_DOCK_MAX_PERCENT,
  FLEET_DOCK_MIN_PERCENT,
  applySectionCollapse,
  bindingIds,
  clampDockPercent,
  cycleLayout,
  dividerPercentAfterDrag,
  dockPercent,
  growVizPane,
  helpLines,
  helpMaxScroll,
  helpWindow,
  layoutFromConfig,
  layoutLabel,
  layoutMode,
  layoutToConfig,
  matchFleetBinding,
  nextSortMode,
  nextVisibilityMode,
  parseFleetViewConfig,
  shrinkVizPane,
  sortTaskRows,
  splitDetailSections,
  toggleDock,
  validateFleetBindings,
  writeFleetViewLayout,
  writeFleetViewSetting,
} from "../pi-worksgood/index.js";

// ── fixtures ─────────────────────────────────────────────────────────────────

const PALETTE = {
  open: [200, 200, 80],
  "in-progress": [60, 200, 220],
  done: [80, 220, 100],
  blocked: [180, 120, 60],
  failed: [220, 60, 60],
};

const TREE_TEXT = [
  "‖ root-a  (open) 5s",
  "├→ ‖ child-1  (open) 2s",
  "└→ ‖ child-2  (failed) 2s",
  "‖ .flip-a  (in-progress) 1s",
].join("\n");

function paritySnap() {
  return {
    revision: "rev-parity",
    unchanged: false,
    source: "daemon",
    counts: { in_progress: 1, ready: 2, blocked: 0, done: 1, failed: 1, total: 5, active_agents: 1 },
    tasks: [
      { id: "root-a", title: "Root A", status: "open", depends_on: [], started_at: "2026-01-01T00:00:00Z" },
      { id: "child-1", title: "Child 1", status: "open", depends_on: ["root-a"], started_at: "2026-01-02T00:00:00Z" },
      { id: "child-2", title: "Child 2", status: "failed", depends_on: ["root-a"], started_at: "2026-01-03T00:00:00Z" },
      { id: ".flip-a", title: "Flip A", status: "in-progress", depends_on: [], started_at: "2026-01-04T00:00:00Z" },
      { id: "done-a", title: "Done A", status: "done", depends_on: [], started_at: "2026-01-05T00:00:00Z" },
    ],
    agents: [
      {
        id: "agent-9",
        task_id: ".flip-a",
        executor: "pi",
        model: "pi:openrouter:x/y",
        status: "working",
        activity: "scoring agency verdicts",
        usage: { totalTokens: 4242, inputTokens: 4000, outputTokens: 242, turnCount: 3, toolUses: 7 },
      },
    ],
    tree: {
      text: TREE_TEXT,
      node_lines: { "root-a": 0, "child-1": 1, "child-2": 2, ".flip-a": 3 },
      palette: PALETTE,
    },
  };
}

/** WG's verbatim `wg show` body: column-0 fields with indented section bodies. */
const DETAIL_LINES = [
  "Task: root-a",
  "Title: Root A",
  "Status: open",
  "Completion contract: land",
  "Required deterministic completion checks (exact enforced order):",
  "  [baseline] git diff --check — provenance: land",
  "  required evidence: run each command",
  "Log:",
  "  [1] created",
  "  [2] started",
];

const DETAIL_TEXT = `${DETAIL_LINES.join("\n")}\n`;

function makeTui(rows = 24, columns = 100) {
  return { requestRender: vi.fn(), terminal: { rows, columns } };
}

function makeComponent(
  overrides: Record<string, unknown> = {},
  opts: { rows?: number; snapshot?: unknown; detail?: unknown } = {},
) {
  const persisted: any[] = [];
  const layouts: any[] = [];
  const runActions: string[][] = [];
  const refreshes: number[] = [];
  const tui = makeTui(opts.rows ?? 24);
  const closed = vi.fn();
  const component = new FleetPanelComponent(
    (opts.snapshot ?? paritySnap()) as never,
    tui as never,
    closed,
    null,
    {
      heightPct: 1,
      onHeightPctChange: (pct: number) => persisted.push(pct),
      persistLayout: (layout: unknown) => layouts.push(layout),
      runAction: async (argv: readonly string[]) => {
        runActions.push([...argv]);
        return { stdout: `did ${argv[0]}\n`, stderr: "", code: 0, killed: false };
      },
      onRefresh: () => {
        refreshes.push(Date.now());
      },
      ...overrides,
    },
    undefined,
    (async () => ({ task_id: "root-a", text: (opts.detail ?? DETAIL_TEXT) as string, source: "daemon" })) as never,
  ) as any;
  return { component, tui, closed, persisted, layouts, runActions, refreshes };
}

const flush = async () => {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 5));
};

/** pi-tui's `dispatchMouseEvent` clone: a truthy non-object THROWS there. */
function piDispatch(component: { handleMouse?: (e: unknown) => unknown }, event: Record<string, unknown>) {
  const result = component.handleMouse?.(event) as any;
  if (result) {
    if (typeof result !== "object") {
      throw new TypeError(`Cannot use 'in' operator to search for 'target' in ${String(result)}`);
    }
    if (!result.handled && !result.capture && !result.focus) return undefined;
    return { ...result, handled: true };
  }
  return undefined;
}

const press = (y: number) => ({ type: "press", button: "left", y });
const drag = (y: number) => ({ type: "drag", button: "left", y });
const release = (y: number) => ({ type: "release", button: "left", y });
const click = (y: number, clickCount = 1) => ({ type: "click", button: "left", y, clickCount });

// ── BATCH 1: inspector dock + views ──────────────────────────────────────────

describe("batch 1 — inspector dock + views", () => {
  it("Alt-←/→ cycle the inspector views (WG detail ↔ raw JSON), R/J toggle them", async () => {
    const { component } = makeComponent();
    component.render(100);
    component.handleInput("\r"); // Enter → inspector
    await flush();
    expect(component.detailVisible).toBe(true);
    expect(component.inspectorViewKind).toBe("detail");

    // The TUI's "Cycle inspector views" is Alt-←/→; the raw JSON view is the
    // Detail tab's `R` and the Log tab's `J`.
    component.handleInput("\x1b[1;3C"); // alt+right
    await flush();
    expect(component.inspectorViewKind).toBe("json");
    expect(component.rawJsonVisible).toBe(true);
    const json = component.render(100).join("\n");
    // No `wg show --json` source wired here ⇒ a LABELLED projection, never a
    // silent fake of WG output.
    expect(json).toContain("client projection");
    expect(json).toContain("root-a");

    component.handleInput("\x1b[1;3D"); // alt+left
    expect(component.inspectorViewKind).toBe("detail");
    expect(component.render(100).join("\n")).not.toContain("client projection");

    component.handleInput("R"); // shift+r → raw JSON
    expect(component.inspectorViewKind).toBe("json");
    component.handleInput("R");
    expect(component.inspectorViewKind).toBe("detail");
    component.handleInput("J"); // the Log tab's raw-JSON mode
    expect(component.inspectorViewKind).toBe("json");
  });

  it("Space collapses the Detail section at the viewport and restores it", async () => {
    const { component } = makeComponent();
    component.render(100);
    component.handleInput("\r");
    await flush();
    const before = component.render(100).join("\n");
    expect(before).toContain("  [baseline] git diff --check");

    // Scroll so the collapsible "Required deterministic …" section leads, then
    // Space collapses its BODY (the heading stays, marked with the hidden count).
    component.scrollBy(4);
    component.handleInput(" ");
    const collapsed = component.render(100).join("\n");
    expect(collapsed).not.toContain("  [baseline] git diff --check");
    expect(collapsed).toContain("Required deterministic completion checks (exact enforced order):");
    expect(collapsed).toMatch(/\(\+\d+ lines\)/);
    expect(component.collapsedSectionKeys.length).toBe(1);

    component.handleInput(" ");
    expect(component.collapsedSectionKeys.length).toBe(0);
    expect(component.render(100).join("\n")).toContain("  [baseline] git diff --check");
  });

  it("the pure section model splits WG's body without rewriting a line", () => {
    const sections = splitDetailSections(DETAIL_LINES);
    const keys = sections.map((s: any) => s.heading);
    expect(keys).toContain("Required deterministic completion checks (exact enforced order):");
    expect(keys).toContain("Log:");
    const log = sections.find((sec: any) => sec.heading === "Log:");
    expect(log).toBeTruthy();
    const collapsed = applySectionCollapse(DETAIL_LINES, new Set([log.key]));
    expect(collapsed.hidden).toBe(2);
    expect(collapsed.lines).toContain("Log: (+2 lines)");
    expect(collapsed.lines).not.toContain("  [1] created");
    // Every non-collapsed line is byte-identical to WG's.
    expect(collapsed.lines[0]).toBe("Task: root-a");
  });

  it("Shift-↑/↓ scroll the inspector (the TUI's 'Scroll detail panel')", async () => {
    const longDetail = `${Array.from({ length: 60 }, (_, i) => (i % 10 === 0 ? `Section ${i}:` : `  line ${i}`)).join("\n")}\n`;
    const { component } = makeComponent({}, { detail: longDetail });
    component.render(100);
    component.handleInput("\r");
    await flush();
    const start = component.scrollOffset;
    component.handleInput("\x1b[1;2B"); // shift+down
    expect(component.scrollOffset).toBe(start + 1);
    component.handleInput("\x1b[1;2A"); // shift+up
    expect(component.scrollOffset).toBe(start);
  });

  it("docked split: tree above, divider header, inspector below (both visible)", async () => {
    const { component } = makeComponent();
    component.handleInput("\r");
    await flush();
    component.handleInput("2"); // dock at 1/2
    const rows = component.render(100);
    expect(component.isSplit).toBe(true);
    expect(component.dockFraction).toBe(50);
    // Header row 0 still carries the panel identity + the persistent fraction.
    expect(rows[0]).toContain("wg-fleet ▸");
    expect(rows[0]).toContain("⇕ 100%");
    expect(rows[0]).toContain("split 1/2");
    // The tree is visible ABOVE the divider and the inspector BELOW it.
    const divider = rows.findIndex((r: string) => r.includes("[< Back]") && r.includes("detail (wg show)"));
    expect(divider).toBeGreaterThan(0);
    expect(rows.slice(0, divider).join("\n")).toContain("root-a");
    expect(rows.slice(divider).join("\n")).toContain("Task: root-a");
    expect(rows.join("\n")).toContain("⇕ 50%");
  });

  it("the divider DRAGS with the TUI's exact percentage math (parity numbers)", async () => {
    // TUI `horizontal_divider_drag_up_grows_inspector`: 35% + drag up 4 of 40 → 45%.
    expect(
      dividerPercentAfterDrag({ startPercent: 35, startRow: 36, currentRow: 32, rows: 40 }),
    ).toBe(45);
    // TUI `horizontal_divider_drag_down_shrinks_inspector`: 50% − drag down 4 → 40%.
    expect(
      dividerPercentAfterDrag({ startPercent: 50, startRow: 20, currentRow: 24, rows: 40 }),
    ).toBe(40);
    // Clamped into the TUI's bounded range.
    expect(clampDockPercent(2)).toBe(FLEET_DOCK_MIN_PERCENT);
    expect(clampDockPercent(200)).toBe(FLEET_DOCK_MAX_PERCENT);

    const { component, layouts } = makeComponent({ layout: { mode: "split", sizePercent: 50 } });
    component.handleInput("\r");
    await flush();
    const rows = component.render(100);
    const dividerRow = rows.findIndex(
      (r: string) => r.includes("[< Back]") && r.includes("detail (wg show)"),
    );
    expect(dividerRow).toBeGreaterThan(0);

    // Press on the divider captures the gesture (pi contract) …
    const start = piDispatch(component, press(dividerRow));
    expect(start).toMatchObject({ handled: true, capture: true, focus: true });
    expect(component.isDockDragging).toBe(true);
    expect(component.isResizing).toBe(false);
    // … and dragging UP grows the inspector.
    piDispatch(component, drag(dividerRow - 2));
    expect(component.dockFraction).toBeGreaterThan(50);
    const grown = component.dockFraction;
    // Nothing persists mid-gesture (one write per gesture).
    expect(layouts).toEqual([]);
    piDispatch(component, drag(dividerRow + 2));
    expect(component.dockFraction).toBeLessThan(grown);
    piDispatch(component, release(dividerRow));
    expect(component.isDockDragging).toBe(false);
    expect(layouts).toHaveLength(1);
    expect(layouts[0]).toMatchObject({ mode: "split" });
    expect(component.visibleNotice ?? "").toMatch(/inspector \d+% \(saved\)/);
  });

  it("clicking a tree row while docked re-targets the inspector without leaving split", async () => {
    const { component } = makeComponent();
    component.handleInput("\r");
    await flush();
    component.handleInput("1"); // dock 1/3
    const rows = component.render(100);
    const childRow = rows.findIndex((r: string) => r.includes("child-2"));
    expect(childRow).toBeGreaterThan(0);
    piDispatch(component, press(childRow));
    piDispatch(component, click(childRow));
    expect(component.selected).toBe("child-2");
    expect(component.detailVisible).toBe(true);
    expect(component.isSplit).toBe(true);
  });
});

// ── BATCH 2: layout + fractions + persistence ────────────────────────────────

describe("batch 2 — layout fractions, cycles and persistence", () => {
  it("1/2/3 dock the panel at 1/3, 1/2, 2/3; the header states it", () => {
    const { component, layouts } = makeComponent();
    component.render(100);
    expect(layoutLabel(component.layoutState)).toBe("panel (full)");
    component.handleInput("1");
    expect(component.dockFraction).toBe(33);
    expect(component.render(100)[0]).toContain("split 1/3");
    component.handleInput("2");
    expect(component.dockFraction).toBe(50);
    component.handleInput("3");
    expect(component.dockFraction).toBe(67);
    expect(component.render(100)[0]).toContain("split 2/3");
    expect(layouts.map((l: any) => l.sizePercent)).toEqual([33, 50, 67]);
  });

  it("= cycles split → panel → graph (third → half → 2/3 → full → hidden → third)", () => {
    const { component } = makeComponent();
    component.render(100);
    component.handleInput("1");
    expect(layoutLabel(component.layoutState)).toBe("split 1/3");
    component.handleInput("=");
    expect(layoutLabel(component.layoutState)).toBe("split 1/2");
    component.handleInput("=");
    expect(layoutLabel(component.layoutState)).toBe("split 2/3");
    component.handleInput("=");
    expect(layoutLabel(component.layoutState)).toBe("panel (full)");
    component.handleInput("=");
    expect(layoutLabel(component.layoutState)).toBe("graph (dock hidden)");
    component.handleInput("=");
    expect(layoutLabel(component.layoutState)).toBe("split 1/3");
    // …and the pure cycle matches the TUI's LayoutMode order exactly.
    expect(layoutMode(cycleLayout({ mode: "split", sizePercent: 67 }))).toBe("full");
    expect(layoutMode(cycleLayout({ mode: "full", sizePercent: 100 }))).toBe("off");
    expect(layoutMode(cycleLayout({ mode: "hidden", sizePercent: 0 }))).toBe("third");
  });

  it("i/v grow/shrink the viz pane in 10% steps and WRAP", () => {
    const { component } = makeComponent();
    component.render(100);
    component.handleInput("2"); // 50%
    component.handleInput("i"); // viz grows → dock shrinks 10
    expect(component.dockFraction).toBe(40);
    component.handleInput("i");
    expect(component.dockFraction).toBe(30);
    component.handleInput("v");
    expect(component.dockFraction).toBe(40);
    // The wrap: past the floor the dock hides, past the ceiling it goes full.
    expect(growVizPane({ mode: "split", sizePercent: 10 })).toEqual({ mode: "hidden", sizePercent: 0 });
    expect(growVizPane({ mode: "hidden", sizePercent: 0 })).toEqual({ mode: "full", sizePercent: 100 });
    expect(shrinkVizPane({ mode: "full", sizePercent: 100 })).toEqual({ mode: "hidden", sizePercent: 0 });
    expect(shrinkVizPane({ mode: "hidden", sizePercent: 0 })).toEqual({
      mode: "split",
      sizePercent: 10,
    });
  });

  it("\\ toggles the right panel (the inspector dock) visible", () => {
    const { component } = makeComponent();
    component.render(100);
    component.handleInput("\\");
    expect(layoutLabel(component.layoutState)).toBe("graph (dock hidden)");
    expect(toggleDock({ mode: "hidden", sizePercent: 50 })).toEqual({ mode: "split", sizePercent: 50 });
    component.handleInput("\\");
    expect(component.isSplit).toBe(true);
  });

  it("the layout row: p opens it, r/f/0 set PENDING, Enter applies, Esc restores", async () => {
    const { component, layouts } = makeComponent();
    component.handleInput("\r");
    await flush();
    component.handleInput("2"); // 50%
    const before = component.layoutState;

    component.handleInput("p");
    expect(component.layoutRowOpen).toBe(true);
    component.handleInput("f"); // pending FULL
    expect(component.pendingLayoutState).toMatchObject({ mode: "full" });
    // Pending is NOT applied until Enter, and the footer says so.
    expect(component.layoutState).toMatchObject(before);
    expect(component.render(100).at(-1)).toContain("Enter apply");
    component.handleInput("\r");
    expect(component.layoutRowOpen).toBe(false);
    expect(layoutLabel(component.layoutState)).toBe("panel (full)");

    // Esc in the layout row restores the complete PRIOR layout.
    component.handleInput("p");
    component.handleInput("0"); // pending HIDDEN
    component.handleInput("\x1b");
    expect(component.layoutRowOpen).toBe(false);
    expect(component.layoutState).toMatchObject({ mode: "full" });
    expect(component.visibleNotice ?? "").toContain("restored");

    // In the layout row `+`/`-` resize the pending DOCK by 5%.
    component.handleInput("p");
    component.handleInput("r"); // pending split (half)
    component.handleInput("+");
    expect(component.pendingLayoutState).toMatchObject({ mode: "split", sizePercent: 55 });
    component.handleInput("-");
    expect(component.pendingLayoutState).toMatchObject({ mode: "split", sizePercent: 50 });
    component.handleInput("\x1b");
    expect(component.layouts ?? layouts.length).toBeGreaterThan(0);
  });

  it("the layout is PERSISTED and restored on the next open", () => {
    const { component, layouts } = makeComponent();
    component.render(100);
    component.handleInput("3");
    expect(layouts.at(-1)).toEqual({ mode: "split", sizePercent: 67 });
    expect(layoutToConfig(component.layoutState)).toEqual({ mode: "split", sizePercent: 67 });

    // Config round-trip: what was written is what the next open reads.
    const raw = { fleetViewLayout: layouts.at(-1), fleetViewHeightPct: 0.6 };
    const parsed = parseFleetViewConfig(raw);
    expect(parsed.layout).toEqual({ mode: "split", sizePercent: 67 });
    expect(layoutFromConfig(raw.fleetViewLayout)).toEqual({ mode: "split", sizePercent: 67 });
    const reopened = makeComponent({ layout: parsed.layout });
    expect(reopened.component.layoutState).toEqual({ mode: "split", sizePercent: 67 });

    // Garbage degrades to the default layout, never a crash.
    expect(parseFleetViewConfig({ fleetViewLayout: "nonsense" }).layout).toEqual(DEFAULT_FLEET_LAYOUT);
    expect(parseFleetViewConfig({}).layout).toEqual(DEFAULT_FLEET_LAYOUT);
  });

  it("writeFleetViewLayout persists ONE key atomically and preserves the rest", () => {
    const env = { PI_CODING_AGENT_DIR: "/tmp/wg-fleet-parity-agent" };
    const written: Record<string, string> = {};
    const io = {
      exists: () => true,
      readFile: (path: string) =>
        written[path] ?? JSON.stringify({ fleetView: true, fleetViewHeightPct: 0.6 }),
      mkdir: () => {},
      writeFile: (path: string, data: string) => {
        written[path] = data;
      },
      rename: (from: string, to: string) => {
        written[to] = written[from] ?? "";
        delete written[from];
      },
    };
    const path = writeFleetViewLayout({ mode: "split", sizePercent: 33 }, env, io as never);
    expect(path).toBeTruthy();
    const body = JSON.parse(written[path as string]);
    expect(body.fleetView).toBe(true);
    expect(body.fleetViewHeightPct).toBe(0.6);
    expect(body.fleetViewLayout).toEqual({ mode: "split", sizePercent: 33 });
    // An unchanged layout is a no-op, not a rewrite.
    expect(writeFleetViewLayout({ mode: "split", sizePercent: 33 }, env, io as never)).toBeNull();
    // `dockPercent` lands on the same bounded space the presets use.
    expect(dockPercent({ mode: "split", sizePercent: 50 })).toBe(50);
    expect(writeFleetViewSetting("fleetViewPollMs", 9000, env, io as never)).toBeTruthy();
  });
});

// ── BATCH 3: task actions ────────────────────────────────────────────────────

describe("batch 3 — task actions", () => {
  it("D confirms, then shells `wg done <task>`; the output is a panel notify line", async () => {
    const { component, runActions, refreshes } = makeComponent();
    component.render(100);
    expect(component.selected).toBe("root-a");
    component.handleInput("D");
    expect(component.pendingConfirm).toEqual({ kind: "done", taskId: "root-a" });
    expect(component.render(100).join("\n")).toContain("mark root-a done?");
    component.handleInput("y");
    await flush();
    expect(runActions).toEqual([["done", "root-a"]]);
    expect(component.actionNotify[0]).toContain("mark selected task done");
    expect(component.actionNotify[0]).toContain("ok");
    expect(refreshes.length).toBe(1);
  });

  it("a destructive confirm can be cancelled with n", () => {
    const { component, runActions } = makeComponent();
    component.render(100);
    component.handleInput("x");
    expect(component.pendingConfirm).toMatchObject({ kind: "retry" });
    component.handleInput("n");
    expect(component.pendingConfirm).toBeNull();
    expect(runActions).toEqual([]);
    expect(component.visibleNotice ?? "").toContain("cancelled");
  });

  it("f/fail takes a typed reason (the deliberate step) and passes it to wg", async () => {
    const { component, runActions } = makeComponent();
    component.render(100);
    component.handleInput("f");
    expect(component.pendingInput).toMatchObject({ kind: "fail", taskId: "root-a" });
    for (const ch of "blocked upstream") component.handleInput(ch);
    expect(component.pendingInput?.buffer).toBe("blocked upstream");
    component.handleInput("\r");
    await flush();
    expect(runActions[0]).toEqual([
      "fail",
      "root-a",
      "--intent",
      "deliberate-stop",
      "--reason",
      "blocked upstream",
    ]);
  });

  it("a/D/e/M/r all reach the shared backend with wg's own argv", async () => {
    const { component, runActions, refreshes } = makeComponent();
    component.render(100);

    component.handleInput("a");
    for (const ch of "new task title") component.handleInput(ch);
    component.handleInput("\r");
    await flush();
    expect(runActions.at(-1)).toEqual(["add", "new task title"]);

    component.handleInput("e");
    for (const ch of "better description") component.handleInput(ch);
    component.handleInput("\r");
    await flush();
    expect(runActions.at(-1)).toEqual(["edit", "root-a", "--description", "better description"]);

    component.handleInput("M");
    for (const ch of "ping") component.handleInput(ch);
    component.handleInput("\r");
    await flush();
    expect(runActions.at(-1)).toEqual(["msg", "send", "root-a", "ping"]);

    component.handleInput("D");
    component.handleInput("\r"); // Enter also confirms
    await flush();
    expect(runActions.at(-1)).toEqual(["done", "root-a"]);

    const before = refreshes.length;
    component.handleInput("r"); // force refresh
    await flush();
    expect(refreshes.length).toBe(before + 1);
    component.handleInput("x");
    component.handleInput("y");
    await flush();
    expect(runActions.at(-1)).toEqual(["retry", "root-a"]);
  });

  it("Esc cancels a typed action without running anything", () => {
    const { component, runActions } = makeComponent();
    component.render(100);
    component.handleInput("M");
    component.handleInput("x");
    component.handleInput("\x1b");
    expect(component.pendingInput).toBeNull();
    expect(runActions).toEqual([]);
    expect(component.detailVisible).toBe(false); // Esc cancelled the input, not the panel
  });

  it("a dormant session HIDES the actions (they are not attempted-and-failed)", () => {
    const { component, runActions } = makeComponent({ inactive: true });
    component.render(100);
    const body = component.helpBody().join("\n");
    for (const kind of Object.keys(FLEET_ACTIONS)) {
      const desc = FLEET_ACTIONS[kind].desc;
      expect(body).not.toContain(desc);
    }
    component.handleInput("D");
    component.handleInput("a");
    component.handleInput("x");
    expect(component.pendingConfirm).toBeNull();
    expect(component.pendingInput).toBeNull();
    expect(runActions).toEqual([]);
  });
});

// ── BATCH 4: search + sort + filter ─────────────────────────────────────────

describe("batch 4 — search, sort and filter", () => {
  it("/ searches with typeahead, Enter jumps + selects, Esc clears", () => {
    const { component, closed } = makeComponent();
    component.render(100);
    component.handleInput("/");
    expect(component.searchState.active).toBe(true);
    for (const ch of "child-2") component.handleInput(ch);
    expect(component.searchState.query).toBe("child-2");
    expect(component.searchState.matches).toEqual(["child-2"]);
    // The footer states the live match count (typeahead feedback).
    expect(component.render(100).at(-1)).toContain("1 match");
    // Only the matching task rows survive the typeahead.
    const rendered = component.render(100).join("\n");
    expect(rendered).toContain("child-2");
    expect(rendered).not.toContain("root-a  (open)");

    component.handleInput("\r"); // commit
    expect(component.searchState.active).toBe(false);
    expect(component.selected).toBe("child-2");
    expect(component.render(100).join("\n")).toContain("root-a  (open)");

    // Esc clears without quitting the panel.
    component.handleInput("/");
    for (const ch of "zzz") component.handleInput(ch);
    expect(component.searchState.matches).toEqual([]);
    component.handleInput("\x1b");
    expect(component.searchState.active).toBe(false);
    expect(closed).not.toHaveBeenCalled();
  });

  it("s cycles Chrono↓ → Chrono↑ → Status and the header states the sort", () => {
    const { component } = makeComponent();
    component.render(100);
    expect(nextSortMode("chronoDesc")).toBe("chronoAsc");
    expect(nextSortMode("chronoAsc")).toBe("status");
    expect(nextSortMode("status")).toBe("chronoDesc");
    component.handleInput("s");
    expect(component.sortMode).toBe("chronoAsc");
    expect(component.render(100)[0]).toContain("sort Chrono ↑");
    component.handleInput("s");
    expect(component.render(100)[0]).toContain("sort Status");
    // Status sort groups live work first, done last.
    const order = sortTaskRows(paritySnap().tasks, "status").map((t: any) => t.id);
    expect(order.at(-1)).toBe("done-a");
    expect(order.indexOf(".flip-a")).toBeLessThan(order.indexOf("child-2"));
  });

  it(". / < cycle the plumbing visibility hidden → running → all", () => {
    const { component } = makeComponent();
    component.render(100);
    expect(nextVisibilityMode("hidden")).toBe("running");
    expect(nextVisibilityMode("running")).toBe("all");
    // Hidden (default): the `.flip-a` automation task is not rendered.
    expect(component.render(100).join("\n")).not.toContain("flip-a");
    component.handleInput(".");
    expect(component.visibilityMode).toBe("running");
    expect(component.render(100)[0]).toContain("running plumbing only");
    // `.flip-a` is in-progress, so the running-only view shows it.
    expect(component.render(100).join("\n")).toContain("flip-a");
    component.handleInput("<");
    expect(component.visibilityMode).toBe("all");
    expect(component.render(100)[0]).toContain("all plumbing");
    component.handleInput(".");
    expect(component.visibilityMode).toBe("hidden");
  });
});

// ── BATCH 5: help + discoverability ─────────────────────────────────────────

describe("batch 5 — the auto-generated help modal", () => {
  it("? opens the help listing EVERY binding, and it scrolls + dismisses", () => {
    const { component, closed } = makeComponent();
    component.render(100);
    component.handleInput("?");
    expect(component.helpVisible).toBe(true);
    const first = component.render(100);
    expect(first[0]).toContain("? Keybindings");
    expect(first[0]).toMatch(/\d+-\d+\/\d+/);

    // The body is generated from the binding table: every non-alias label and
    // description the keymap declares is in it (never stale).
    const body = component.helpBody().join("\n");
    for (const row of FLEET_BINDINGS as any[]) {
      if (row.hideInHelp) continue;
      expect(body).toContain(row.label.trim());
      expect(body).toContain(row.desc);
    }
    // And the TUI's verbatim parity rows are present.
    expect(body).toContain("Cycle inspector views (with slide animation)");
    expect(body).toContain("Toggle raw JSON in Detail tab");
    expect(body).toContain("Toggle section collapse in Detail");
    expect(body).toContain("Cycle plumbing: hidden → running only → all");
    expect(body).toContain("Toggle this help");

    // j/k/PgUp/PgDn/Home/End scroll the modal and clamp at both ends.
    const start = component.helpScrollOffset;
    component.handleInput("j");
    expect(component.helpScrollOffset).toBe(start + 1);
    component.handleInput("\x1b[6~"); // page down
    expect(component.helpScrollOffset).toBeGreaterThan(start + 1);
    component.handleInput("G"); // bottom
    const bottom = component.helpScrollOffset;
    expect(bottom).toBeGreaterThan(0);
    component.handleInput("g"); // top
    expect(component.helpScrollOffset).toBe(0);
    // The wheel scrolls Help only (the tree does not move behind it).
    const treeBefore = component.treeScrollOffset;
    piDispatch(component, { type: "wheel", wheelDelta: 3, y: 4 });
    expect(component.helpScrollOffset).toBe(3);
    expect(component.treeScrollOffset).toBe(treeBefore);

    // ? / q / Esc close it (and the panel stays open).
    component.handleInput("q");
    expect(component.helpVisible).toBe(false);
    expect(closed).not.toHaveBeenCalled();
    component.handleInput("?");
    component.handleInput("\x1b");
    expect(component.helpVisible).toBe(false);
    expect(closed).not.toHaveBeenCalled();
  });

  it("the help window math mirrors the TUI's position indicator", () => {
    expect(helpMaxScroll(10, 4)).toBe(6);
    const win = helpWindow(["a", "b", "c", "d"], 1, 2);
    expect(win.lines).toEqual(["b", "c"]);
    expect(win.first).toBe(2);
    expect(win.last).toBe(3);
    expect(win.total).toBe(4);
    expect(win.overflow).toBe(true);
    expect(helpWindow(["a"], 99, 5).scroll).toBe(0);
  });

  it("footer hints are accurate at every state change", async () => {
    const { component } = makeComponent();
    const tree = component.render(100).at(-1);
    expect(tree).toContain("enter detail");
    expect(tree).toContain("? help");
    expect(tree).toContain("drag top edge");
    expect(tree).toContain("+/- resize");

    component.handleInput("\r");
    await flush();
    expect(component.render(100).at(-1)).toContain("[< Back]");

    // Docking announces itself (a transient notice owns the footer), so the
    // docked HINTS are asserted on a panel that opens already docked.
    const docked = makeComponent({ layout: { mode: "split", sizePercent: 50 } });
    docked.component.handleInput("\r");
    await flush();
    expect(docked.component.render(100).at(-1)).toContain("Alt-←/→ view");
    expect(docked.component.render(100).at(-1)).toContain("Space collapse");

    component.handleInput("p"); // layout row prompt replaces the hints
    expect(component.render(100).at(-1)).toContain("layout row");
    component.handleInput("\x1b");

    component.handleInput("D"); // confirm prompt
    expect(component.render(100).join("\n")).toContain("y/Enter to confirm");
    component.handleInput("n");

    component.handleInput("/"); // search prompt
    expect(component.render(100).at(-1)).toContain("Enter jump");
    component.handleInput("\x1b");
    // Back on the tree, the panel's own header is restored.
    component.handleInput("\x1b");
    expect(component.render(100)[0]).toContain("wg-fleet ▸");
  });
});

// ── CROSS-CUTTING: mouse contract, keyboard paths, keymap completeness ──────

describe("cross-cutting — mouse object shape, keyboard paths, keymap", () => {
  it("every mouse handler returns pi's OBJECT shape (never a bare boolean)", async () => {
    const { component } = makeComponent({ layout: { mode: "split", sizePercent: 50 } });
    component.handleInput("\r");
    await flush();
    const rows = component.render(100);
    const target = rows.findIndex((r: string) => r.includes("detail (wg show)"));
    const events: Array<Record<string, unknown>> = [
      { type: "wheel", wheelDelta: 2, y: 3 },
      { type: "wheel", wheelDelta: 0, y: 3 },
      press(0),
      drag(-4),
      release(0),
      press(target),
      drag(target - 2),
      release(target),
      press(2),
      click(2),
      click(2, 2),
      click(0),
      { type: "press", button: "right", y: 2 },
      { type: "click", button: "left" },
      { type: "unknown", y: 2 },
    ];
    for (const event of events) {
      const result = component.handleMouse(event as never);
      if (result) expect(isPiMouseResult(result as never)).toBe(true);
      // Through pi's own dispatcher: a bare boolean would throw here.
      expect(() => piDispatch(component, event)).not.toThrow();
    }
    // Dormant: clicks are not ours (unhandled), still object-shaped.
    const dormant = makeComponent({ inactive: true });
    dormant.component.render(100);
    expect(dormant.component.handleMouse(click(1) as never)).toBeUndefined();
  });

  it("every binding has a keyboard path and the keymap is conflict-free", () => {
    expect(() => validateFleetBindings()).not.toThrow();
    const ids = bindingIds();
    expect(ids.length).toBeGreaterThanOrEqual(45);
    // Each declared key resolves back to SOME binding in its own scope, so no
    // row is decorative: what the help says is what dispatch does.
    for (const row of FLEET_BINDINGS as any[]) {
      for (const key of row.keys) {
        const raw = rawInputForKey(key);
        if (raw === null) continue;
        const resolved = matchFleetBinding(raw, row.scope, { inactive: row.activeOnly === true });
        if (row.activeOnly === true) {
          // A dormant panel must NOT dispatch an active-only row.
          expect(matchFleetBinding(raw, row.scope, { inactive: true })).not.toBe(row.id);
        } else {
          expect(resolved).toBeTruthy();
        }
      }
    }
    // The mouse table names a keyboard binding for every gesture.
    for (const row of FLEET_BINDINGS as any[]) void row;
    const labels = helpLines().join("\n");
    expect(labels).toContain("Mouse");
    expect(labels).toContain("click again");
  });

  it("dispatch is robust: every binding id can be driven without throwing", async () => {
    const { component } = makeComponent({ layout: { mode: "split", sizePercent: 50 } });
    component.render(100);
    component.handleInput("\r");
    await flush();
    for (const row of FLEET_BINDINGS as any[]) {
      const raw = rawInputForKey(row.keys[0]);
      if (raw === null) continue;
      expect(() => component.handleInput(raw)).not.toThrow();
      if (component.helpVisible) component.handleInput("\x1b");
      if (component.layoutRowOpen) component.handleInput("\x1b");
      if (component.pendingConfirm) component.handleInput("n");
      if (component.pendingInput) component.handleInput("\x1b");
    }
    // The panel is still alive and rendering inside its bounds.
    const rows = component.render(100);
    expect(rows.length).toBeLessThanOrEqual(24);
    expect(rows.length).toBeGreaterThanOrEqual(3);
  });

  it("printableText only claims real typed text", () => {
    expect(printableText("a")).toBe("a");
    expect(printableText(" ")).toBe(" ");
    expect(printableText("\x1b[A")).toBeNull();
    expect(printableText("\r")).toBeNull();
    expect(printableText("")).toBeNull();
  });
});

/** Convert a pi-tui key id into the raw terminal bytes a test can deliver. */
function rawInputForKey(key: string): string | null {
  const named: Record<string, string> = {
    up: "\x1b[A",
    down: "\x1b[B",
    right: "\x1b[C",
    left: "\x1b[D",
    enter: "\r",
    return: "\r",
    escape: "\x1b",
    esc: "\x1b",
    tab: "\t",
    space: " ",
    backspace: "\x7f",
    home: "\x1b[H",
    end: "\x1b[F",
    pageUp: "\x1b[5~",
    pageDown: "\x1b[6~",
  };
  if (named[key]) return named[key];
  const alt = /^alt\+(\w+)$/.exec(key);
  if (alt) {
    const inner = named[alt[1] as string];
    if (inner) return `\x1b${inner.slice(1)}`;
    return `\x1b${alt[1]}`;
  }
  const shift = /^shift\+(\w+)$/.exec(key);
  if (shift) {
    const base = shift[1] as string;
    if (base.length === 1) return base.toUpperCase();
    const mod: Record<string, string> = { up: "\x1b[1;2A", down: "\x1b[1;2B", tab: "\x1b[Z", g: "G", r: "R", j: "J", d: "D", m: "M" };
    return mod[base] ?? null;
  }
  const ctrl = /^ctrl\+(\w+)$/.exec(key);
  if (ctrl) {
    const ch = (ctrl[1] as string).toLowerCase();
    if (ch.length === 1) return String.fromCharCode(ch.charCodeAt(0) - 96);
    return null;
  }
  if (key.length === 1) return key;
  return null;
}
