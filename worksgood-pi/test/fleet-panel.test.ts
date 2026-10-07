/**
 * fleet-panel tests — the **scrollable** FleetView panel (`/wg-fleet`).
 *
 * Pins, without a live terminal:
 *   - the pure scroll math (selection-follows-scroll + clamping);
 *   - the bounded transcript tail (byte/line budget, tail semantics);
 *   - the glyph/status + counts header mapping and the nested tree projection;
 *   - the component interactions (wheel/PgUp/PgDn/Home/End scroll the tree, the
 *     selection follows, Enter drills into detail, q/Esc close, render stays
 *     inside the panel bounds);
 *   - the config gate (disabled ⇒ no mount + command no-op) and that the
 *     ambient below-editor widget strip is unaffected.
 *
 * Tests run against the built `pi-worksgood/` artifact — `npm test` builds first.
 */

import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import {
  DEFAULT_TRANSCRIPT_LINES,
  MAX_TRANSCRIPT_BODY_BYTES,
  MAX_TRANSCRIPT_LINES,
  TRANSCRIPT_TAIL_BYTES,
  agentActivityLabel,
  agentForTask,
  agentUsageDetailLabel,
  agentUsageLabel,
  boundTranscriptBody,
  buildFleetTree,
  clampScroll,
  fleetCountsHeader,
  getFleetToVizSnapshot,
  maxScroll,
  pageScroll,
  readAgentStreamTail,
  readTextTail,
  renderWgTree,
  scrollToKeepVisible,
  transcriptLineLimit,
  wgTreeLineDepth,
} from "../pi-worksgood/index.js";
// @ts-expect-error — built ESM artifact import (see above)
import { FleetPanelComponent } from "../pi-worksgood/fleet-panel.js";
// @ts-expect-error — built ESM artifact import (see above)
import { installFleetView } from "../pi-worksgood/fleet-view.js";
// @ts-expect-error — built ESM artifact import (see above)
import { isPiMouseResult } from "../pi-worksgood/mouse.js";

// ── fixtures ─────────────────────────────────────────────────────────────────

/**
 * A faithful clone of pi-tui's `dispatchMouseEvent` (bundle
 * `chunk-6FX7UEPL.js`) — the exact function that crashed on the panel's
 * bare-boolean `handleMouse` return:
 *   TypeError: Cannot use 'in' operator to search for 'target' in true
 * Feeding panel results through this MUST NOT throw.
 */
function piDispatchMouseEvent(
  component: { handleMouse?: (event: unknown) => unknown },
  event: Record<string, unknown>,
): unknown {
  const result = component.handleMouse?.(event) as
    | { handled?: boolean; capture?: boolean; focus?: boolean; target?: unknown }
    | undefined
    | false
    | true;
  if (result) {
    if ("target" in (result as object)) return result;
    const r = result as { handled?: boolean; capture?: boolean; focus?: boolean };
    if (!(!r.handled && !r.capture && !r.focus)) {
      return {
        ...r,
        handled: true,
        ...(r.focus ? { focusTarget: component } : {}),
        target: { component, originX: 0, originY: 0, width: 1, height: 1 },
      };
    }
  }
  return undefined;
}

function snap() {
  return {
    revision: "wggraph:v1:test#abc",
    unchanged: false,
    source: "daemon",
    counts: {
      in_progress: 1,
      ready: 1,
      blocked: 1,
      done: 1,
      failed: 0,
      total: 4,
      active_agents: 1,
    },
    tasks: [
      { id: "done-a", title: "Done A", status: "done", depends_on: [] },
      { id: "active-b", title: "Active B", status: "in-progress", depends_on: ["done-a"], assigned: "agent-7" },
      { id: "open-c", title: "Open C", status: "open", depends_on: [] },
      { id: "blocked-d", title: "Blocked D", status: "blocked", depends_on: ["active-b"] },
    ],
    agents: [
      {
        id: "agent-7",
        task_id: "active-b",
        executor: "pi",
        model: "pi:openrouter:anthropic/claude-opus-4-7",
        status: "working",
        activity: "running cargo test --lib",
        usage: {
          inputTokens: 12000,
          outputTokens: 345,
          totalTokens: 12345,
          costUsd: 0.42,
          turnCount: 14,
          toolUses: 31,
        },
      },
    ],
  };
}

/**
 * A snapshot carrying WG's OWN rendered tree (`GetFleet.tree`) — the exact
 * plain-text `wg viz` output for a small fixture graph (captured from the real
 * renderer): a paused root, a nested child/grandchild and a failed child, plus
 * a second component. Structure is WG's, verbatim.
 */
const WG_TREE_TEXT = [
  "‖ root-a  (open) 5s",
  "├→ ‖ child-1  (open) 2s",
  "│ └→ ‖ grand-1  (open) 2s",
  "└→ ‖ child-2  (failed) 2s",
  "",
  "‖ loner  (open) 2s",
].join("\n");

const WG_TREE_NODE_LINES = {
  "root-a": 0,
  "child-1": 1,
  "grand-1": 2,
  "child-2": 3,
  loner: 5,
};

/** WG's exported graph-view palette (mirror of `src/status_palette.rs`). */
const WG_PALETTE: Record<string, [number, number, number]> = {
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

function treeSnap() {
  return {
    ...snap(),
    tree: { text: WG_TREE_TEXT, node_lines: WG_TREE_NODE_LINES, palette: WG_PALETTE },
    tasks: [
      { id: "done-a", title: "Done A", status: "done", depends_on: [] },
      { id: "active-b", title: "Active B", status: "in-progress", depends_on: ["done-a"] },
      { id: "blocked-d", title: "Blocked D", status: "blocked", depends_on: ["active-b"] },
      { id: "open-c", title: "Open C", status: "open", depends_on: [] },
      { id: "gone-e", title: "Gone E", status: "abandoned", depends_on: [] },
      { id: "root-a", title: "Root A", status: "open", depends_on: [] },
      { id: "child-1", title: "Child 1", status: "open", depends_on: ["root-a"] },
      { id: "grand-1", title: "Grand 1", status: "open", depends_on: ["child-1"] },
      { id: "child-2", title: "Child 2", status: "failed", depends_on: ["root-a"] },
      { id: "loner", title: "Loner", status: "open", depends_on: [] },
    ],
  };
}

/** A wide fixture (many roots) so a small viewport actually scrolls. */
function bigSnap(n = 40) {
  const tasks = Array.from({ length: n }, (_, i) => ({
    id: `task-${String(i).padStart(3, "0")}`,
    title: `Task ${i}`,
    status: i % 3 === 0 ? "done" : "open",
    depends_on: [],
  }));
  return {
    revision: "rev-big",
    unchanged: false,
    source: "daemon",
    counts: { in_progress: 0, ready: n, blocked: 0, done: 0, failed: 0, total: n, active_agents: 0 },
    tasks,
    agents: [],
  };
}

// ── scroll math ──────────────────────────────────────────────────────────────

describe("fleet panel scroll math", () => {
  it("computes the max offset and clamps out-of-range values", () => {
    expect(maxScroll(100, 10)).toBe(90);
    expect(maxScroll(5, 10)).toBe(0);
    expect(maxScroll(0, 10)).toBe(0);
    expect(clampScroll(-5, 100, 10)).toBe(0);
    expect(clampScroll(1000, 100, 10)).toBe(90);
    expect(clampScroll(42, 100, 10)).toBe(42);
    expect(clampScroll(Number.NaN, 100, 10)).toBe(0);
    expect(clampScroll(Number.POSITIVE_INFINITY, 100, 10)).toBe(0);
  });

  it("scrolls to keep a selection visible (selection-follows-scroll)", () => {
    // Below the viewport: scroll down so the index is the last visible row.
    expect(scrollToKeepVisible(0, 10, 25, 100)).toBe(16);
    // Above the viewport: scroll up to the index.
    expect(scrollToKeepVisible(20, 10, 5, 100)).toBe(5);
    // Already visible: unchanged.
    expect(scrollToKeepVisible(20, 10, 25, 100)).toBe(20);
    // Clamped at the end of the list.
    expect(scrollToKeepVisible(90, 10, 99, 100)).toBe(90);
  });

  it("pages by a viewport (minus one) and clamps", () => {
    expect(pageScroll(0, 10, 1, 100)).toBe(9);
    expect(pageScroll(50, 10, -1, 100)).toBe(41);
    expect(pageScroll(90, 10, 1, 100)).toBe(90);
    expect(pageScroll(0, 10, -1, 100)).toBe(0);
  });
});

// ── bounded transcript tail ──────────────────────────────────────────────────

describe("fleet panel transcript tail bounds", () => {
  let dir: string;
  const lines = (n: number) => Array.from({ length: n }, (_, i) => `line ${i}`);

  beforeAll(() => {
    dir = mkdtempSync(join(tmpdir(), "wg-fleet-tail-"));
  });
  afterAll(() => {
    /* temp dir left for the OS to reap */
  });

  it("clamps the requested line count like pi-subagents", () => {
    expect(transcriptLineLimit(undefined)).toBe(DEFAULT_TRANSCRIPT_LINES);
    expect(transcriptLineLimit(Number.NaN)).toBe(DEFAULT_TRANSCRIPT_LINES);
    expect(transcriptLineLimit(0)).toBe(1);
    expect(transcriptLineLimit(-5)).toBe(1);
    expect(transcriptLineLimit(10)).toBe(10);
    expect(transcriptLineLimit(10_000)).toBe(MAX_TRANSCRIPT_LINES);
  });

  it("reads only the tail of a large file", () => {
    const path = join(dir, "stream.jsonl");
    writeFileSync(path, `${lines(5000).join("\n")}\n`);
    const tail = readTextTail(path, 10);
    expect(tail.lines).toEqual(lines(5000).slice(-10));
    expect(tail.truncated).toBe(true);
    expect(tail.error).toBeUndefined();
  });

  it("bounds the byte budget at the tail boundary", () => {
    const path = join(dir, "huge.jsonl");
    // > TRANSCRIPT_TAIL_BYTES of content, so the read must seek to the end.
    const big = "x".repeat(2048);
    const content = Array.from({ length: Math.ceil(TRANSCRIPT_TAIL_BYTES / 2049) + 5 }, (_, i) => `${i}:${big}`).join("\n");
    expect(Buffer.byteLength(content)).toBeGreaterThan(TRANSCRIPT_TAIL_BYTES);
    writeFileSync(path, content);
    const tail = readTextTail(path, 5);
    expect(tail.truncated).toBe(true);
    expect(tail.lines.length).toBe(5);
    // The very last line is preserved.
    expect(tail.lines.at(-1)).toContain(big);
  });

  it("degrades to an empty tail (never throws) for a missing file", () => {
    const tail = readTextTail(join(dir, "missing.jsonl"), 10);
    expect(tail.lines).toEqual([]);
    expect(tail.error).toBeTruthy();
  });

  it("bounds the rendered body to MAX_TRANSCRIPT_BODY_BYTES, preserving recent lines", () => {
    const source = Array.from({ length: 4000 }, (_, i) => `chunk ${i} ${"z".repeat(200)}`);
    const body = boundTranscriptBody(source);
    const bytes = body.reduce((sum, line) => sum + Buffer.byteLength(line) + 1, 0);
    expect(bytes).toBeLessThanOrEqual(MAX_TRANSCRIPT_BODY_BYTES);
    expect(body.length).toBeLessThan(source.length);
    // Most-recent lines survive; the omission marker is present at the head.
    expect(body.at(-1)).toContain("chunk 3999");
    expect(body[0]).toContain("earlier lines omitted");
  });

  it("reads an agent stream tail from the WG agents dir", () => {
    const root = mkdtempSync(join(tmpdir(), "wg-fleet-live-"));
    const agentDir = join(root, "agents", "agent-7");
    mkdirSync(agentDir, { recursive: true });
    writeFileSync(join(agentDir, "raw_stream.jsonl"), '{"type":"tool_execution_start","toolName":"bash"}\n');
    const tail = readAgentStreamTail(root, "agent-7", 10);
    expect(tail).not.toBeNull();
    expect(tail!.lines.length).toBe(1);
    expect(readAgentStreamTail(root, "agent-nope", 10)).toBeNull();
  });
});

// ── glyph/status + counts mapping + nested tree ──────────────────────────────

describe("fleet panel read model", () => {
  it("renders a counts header with in-progress/ready/blocked/done", () => {
    const header = fleetCountsHeader(snap().counts);
    expect(header).toContain("wg-fleet ▸");
    expect(header).toContain("1 in-progress");
    expect(header).toContain("1 ready");
    expect(header).toContain("1 blocked");
    expect(header).toContain("1 done");
    expect(header).toContain("1 active");
  });

  it("projects GetFleet rows onto the viz tree (depends_on → after, reverse before)", () => {
    const viz = getFleetToVizSnapshot(snap());
    const active = viz.tasks.find((t) => t.id === "active-b")!;
    expect(active.after).toEqual(["done-a"]);
    const done = viz.tasks.find((t) => t.id === "done-a")!;
    expect(done.before).toContain("active-b");
    const blocked = viz.tasks.find((t) => t.id === "blocked-d")!;
    expect(blocked.after).toEqual(["active-b"]);
  });

  it("renders the dependency tree nested by blockers", () => {
    const tree = buildFleetTree(snap());
    const ids = tree.lines.map((l) => l.taskId);
    expect(ids).toContain("active-b");
    const active = tree.lines.find((l) => l.taskId === "active-b")!;
    const blocked = tree.lines.find((l) => l.taskId === "blocked-d")!;
    expect(blocked.depth).toBeGreaterThan(active.depth);
  });

  it("maps an agent to its task with a compact activity label", () => {
    const agent = agentForTask(snap().agents, "active-b");
    expect(agent?.id).toBe("agent-7");
    expect(agentActivityLabel(agent)).toBe("running cargo test --lib");
    expect(agentActivityLabel(agentForTask([], "x"))).toBeNull();
    // No activity text falls back to the status.
    expect(agentActivityLabel({ id: "a", task_id: "t", status: "idle" })).toBe("idle");
  });

  it("shows a compact usage segment on the row and full counts in detail", () => {
    const agent = agentForTask(snap().agents, "active-b");
    // Row segment is abbreviated, matching the ambient agent rows.
    expect(agentUsageLabel(agent)).toBe("12.3k tok · 14 turns · 31 tools");
    // Detail label is full and un-abbreviated.
    expect(agentUsageDetailLabel(agent)).toBe(
      "12345 tokens · (12000 in / 345 out) · 14 turns · 31 tool uses · $0.42",
    );
    // No usage → no segment (graceful degradation).
    expect(agentUsageLabel({ id: "a", task_id: "t", status: "idle" })).toBeNull();
    expect(agentUsageDetailLabel(null)).toBeNull();
  });

  it("renders the WG status glyphs in the tree (one vocabulary, no drift)", () => {
    const tree = buildFleetTree(snap());
    const byId = new Map(tree.lines.map((l) => [l.taskId, l.text]));
    expect(byId.get("done-a")).toContain("✓"); // done
    expect(byId.get("active-b")).toContain("●"); // in-progress
    expect(byId.get("open-c")).toContain("○"); // open
    expect(byId.get("blocked-d")).toContain("⏸"); // blocked
  });
});

// ── structure parity with `wg viz` (WG's own rendered text) ──────────────────

describe("fleet tree structure parity — renders WG's own `wg viz` text", () => {
  it("returns WG's rendered lines byte-for-byte (verbatim, no re-derivation)", () => {
    const render = renderWgTree({ text: WG_TREE_TEXT, node_lines: WG_TREE_NODE_LINES });
    expect(render.fromWg).toBe(true);
    expect(render.lines.map((l) => l.text)).toEqual(WG_TREE_TEXT.split("\n"));
  });

  it("hits each line to its task id and derives WG's depth from the prefix", () => {
    const render = renderWgTree({ text: WG_TREE_TEXT, node_lines: WG_TREE_NODE_LINES });
    const byTask = new Map(render.lines.filter((l) => l.taskId).map((l) => [l.taskId!, l]));
    // top-level row: column 0, no edge glyph
    expect(byTask.get("root-a")!.depth).toBe(0);
    expect(byTask.get("root-a")!.text.startsWith("├→")).toBe(false);
    expect(byTask.get("root-a")!.text.startsWith("└→")).toBe(false);
    // children: `├→`/`└→` at column 0 → depth 1
    expect(byTask.get("child-1")!.depth).toBe(1);
    expect(byTask.get("child-1")!.text.startsWith("├→ ")).toBe(true);
    expect(byTask.get("child-2")!.depth).toBe(1);
    expect(byTask.get("child-2")!.text.startsWith("└→ ")).toBe(true);
    // grandchildren: WG's 2-space-per-depth prefix (`│ `/`  `) → depth 2
    expect(byTask.get("grand-1")!.depth).toBe(2);
    expect(byTask.get("grand-1")!.text.startsWith("│ └→ ")).toBe(true);
    // A second component's root is still top-level.
    expect(byTask.get("loner")!.depth).toBe(0);
  });

  it("derives line depth from WG's connector/prefix shape", () => {
    expect(wgTreeLineDepth("root-a  (open)")).toBe(0);
    expect(wgTreeLineDepth("└→ child")).toBe(1);
    expect(wgTreeLineDepth("├→ child")).toBe(1);
    expect(wgTreeLineDepth("  └→ grand")).toBe(2);
    expect(wgTreeLineDepth("│ └→ grand")).toBe(2);
    expect(wgTreeLineDepth("  │ └→ great")).toBe(3);
    expect(wgTreeLineDepth("")).toBeNull();
  });

  it("collapse is a display overlay: it hides WG's rendered descendant block only", () => {
    const none = renderWgTree({ text: WG_TREE_TEXT, node_lines: WG_TREE_NODE_LINES });
    expect(none.lines.map((l) => l.text)).toEqual(WG_TREE_TEXT.split("\n"));

    const collapsed = renderWgTree(
      { text: WG_TREE_TEXT, node_lines: WG_TREE_NODE_LINES },
      new Set(["root-a"]),
    );
    const texts = collapsed.lines.map((l) => l.text);
    // The collapsed row stays (with a bounded hidden count)…
    expect(texts[0]).toContain("root-a");
    expect(texts[0]).toContain("(+3)");
    // …its 3 descendants and the blank continuation inside the block are hidden…
    expect(texts.some((t) => t.includes("child-1"))).toBe(false);
    expect(texts.some((t) => t.includes("grand-1"))).toBe(false);
    expect(texts.some((t) => t.includes("child-2"))).toBe(false);
    // …and the sibling component is untouched.
    expect(texts.some((t) => t.includes("loner"))).toBe(true);
  });

  it("has no rendered tree when the payload is absent (fallback only)", () => {
    const render = renderWgTree(undefined);
    expect(render.fromWg).toBe(false);
    expect(render.lines).toEqual([]);
  });
});

// ── component interactions ───────────────────────────────────────────────────

describe("FleetPanelComponent", () => {
  function makeComponent(
    snapshot: unknown = bigSnap(),
    height = 6,
    liveDir?: string,
    detailFetcher?: (taskId: string, columns?: number) => Promise<unknown>,
    enterGuardMs?: number,
  ) {
    const tui = { requestRender: vi.fn(), terminal: { rows: height, columns: 100 } };
    const closed = vi.fn();
    const component = new FleetPanelComponent(
      snapshot as never,
      tui as never,
      closed,
      null,
      enterGuardMs === undefined ? {} : { enterGuardMs },
      liveDir,
      detailFetcher as never,
    );
    return { component, tui, closed, render: (w = 100) => component.render(w) };
  }

  /** Flush the microtask queue so an async detail fetch settles. */
  const flush = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

  it("never renders past the panel bounds (fixed 6-row viewport)", () => {
    const { component, render } = makeComponent(bigSnap(), 6);
    const lines = render();
    expect(lines.length).toBeLessThanOrEqual(6);
    // Header + body + footer.
    expect(lines[0]).toContain("wg-fleet ▸");
    expect(lines.at(-1)).toContain("close");
    void component;
  });

  it("returns a cached render for the same width/height", () => {
    const { render } = makeComponent(bigSnap(), 6);
    const a = render(80);
    const b = render(80);
    expect(a).toBe(b);
  });

  it("moves the selection with arrow keys and scrolls it into view", () => {
    const { component, render } = makeComponent(bigSnap(), 6);
    render();
    expect(component.selected).toBe("task-000");
    for (let i = 0; i < 5; i++) component.handleInput("\x1b[B"); // down
    expect(component.selected).toBe("task-005");
    // The selected row is inside the bounded body viewport.
    render();
    const view = component.bodyViewport();
    const hit = component.hitLines.findIndex((id) => id === component.selected);
    expect(hit).toBeGreaterThanOrEqual(0);
    expect(hit).toBeLessThan(view);
    expect(component.treeScrollOffset).toBeGreaterThan(0);
  });

  it("pages the tree with PgDn/PgUp and drags the selection so it stays visible", () => {
    const { component, render } = makeComponent(bigSnap(), 6);
    render();
    const view = component.bodyViewport();
    component.handleInput("\x1b[6~"); // PageDown
    const offset = component.treeScrollOffset;
    expect(offset).toBeGreaterThan(0);
    render();
    const idx = component.hitLines.findIndex((id) => id === component.selected);
    expect(idx).toBeGreaterThanOrEqual(0);
    expect(idx).toBeLessThan(view);
    component.handleInput("\x1b[5~"); // PageUp
    expect(component.treeScrollOffset).toBeLessThanOrEqual(offset);
  });

  it("supports Home/End and wheel scrolling with clamping", () => {
    const { component } = makeComponent(bigSnap(40), 6);
    component.handleInput("\x1b[6~"); // page down a few times
    component.handleInput("\x1b[6~");
    component.handleInput("\x1b[6~");
    const mid = component.treeScrollOffset;
    expect(mid).toBeGreaterThan(0);
    // Wheel up (negative delta) scrolls toward the top and clamps at 0; the
    // return is pi's consumed-object shape, never a bare boolean.
    const wheelUp = component.handleMouse({ type: "wheel", wheelDelta: -100 });
    expect(wheelUp).toEqual({ handled: true });
    expect(isPiMouseResult(wheelUp)).toBe(true);
    expect(component.treeScrollOffset).toBe(0);
    // Wheel down (positive delta) scrolls away from the top.
    component.handleMouse({ type: "wheel", wheelDelta: 8 });
    expect(component.treeScrollOffset).toBeGreaterThan(0);
    // End selects the last task and pins the viewport to the bottom.
    component.handleInput("\x1b[F"); // End
    expect(component.selected).toBe("task-039");
    expect(component.treeScrollOffset).toBe(maxScroll(40, component.bodyViewport()));
  });

  it("shows the compact usage segment on the live agent's tree row", () => {
    const { render } = makeComponent(snap(), 20);
    const lines = render();
    const row = lines.find((l) => l.includes("active-b")) ?? "";
    expect(row).toContain("running cargo test --lib");
    expect(row).toContain("12.3k tok · 14 turns · 31 tools");
  });

  it("drills into the selected task's detail: WG's own text verbatim, then labelled pi additions", async () => {
    const root = mkdtempSync(join(tmpdir(), "wg-fleet-compose-"));
    const agentDir = join(root, "agents", "agent-7");
    mkdirSync(agentDir, { recursive: true });
    writeFileSync(join(agentDir, "raw_stream.jsonl"), '{"type":"tool_execution_start","toolName":"bash","args":{}}\n');
    // WG's OWN detail body for `active-b` — the exact `wg show active-b` text.
    const WG_DETAIL = [
      "Task: active-b",
      "Title: Active B",
      "Status: in-progress",
      "Completion contract: land",
      "Required deterministic completion checks (exact enforced order):",
      "  [x] cargo test --lib — checked-in policy",
    ].join("\n");
    const detailFetcher = vi.fn().mockResolvedValue({
      task_id: "active-b",
      text: `${WG_DETAIL}\n`,
      source: "daemon",
    });
    const { component, render } = makeComponent(snap(), 20, root, detailFetcher);
    render();
    // Select active-b (nested under done-a: done-a is already selected first).
    expect(component.selected).toBe("done-a");
    component.handleInput("\x1b[B"); // down
    expect(component.selected).toBe("active-b");
    component.handleInput("\r"); // Enter → detail
    expect(component.detailVisible).toBe(true);
    await flush();
    const detail = render();
    // WG's text is rendered verbatim, in order, starting at the first body row.
    expect(detail[1]).toBe("Task: active-b");
    expect(detail[2]).toBe("Title: Active B");
    expect(detail[3]).toBe("Status: in-progress");
    expect(detail[4]).toBe("Completion contract: land");
    expect(detail[5]).toContain("Required deterministic completion checks");
    expect(detail[6]).toContain("cargo test --lib");
    // The pi-side additions are appended AFTER WG's text and labelled.
    const addIdx = detail.findIndex((l) => l.includes("pi-side additions"));
    expect(addIdx).toBeGreaterThan(6);
    expect(detail.some((l) => l.includes("running cargo test --lib"))).toBe(true);
    // The detail view carries the full, un-abbreviated usage counts.
    expect(detail.some((l) => l.includes("usage · 12345 tokens"))).toBe(true);
    expect(detail.some((l) => l.includes("14 turns") && l.includes("31 tool uses"))).toBe(true);
    expect(detail.some((l) => l.includes("Transcript tail"))).toBe(true);
    expect(detail.length).toBeLessThanOrEqual(20);
    // The fetcher was asked for this task (at the panel width).
    expect(detailFetcher).toHaveBeenCalledWith("active-b", 100);
    component.handleInput("l"); // back to tree
    expect(component.detailVisible).toBe(false);
  });

  it("Enter opens the selected task's detail; a doubled Enter does not close it; 'o'/space expand", () => {
    const { component, render } = makeComponent(treeSnap(), 20);
    render();
    // First visible task in WG's own render is the paused root.
    expect(component.selected).toBe("root-a");
    expect(component.detailVisible).toBe(false);
    component.handleInput("\r"); // Enter → detail
    expect(component.detailVisible).toBe(true);
    expect(render().some((l) => l.includes("wg-fleet · root-a · detail"))).toBe(true);
    // Some terminals deliver Enter as `\r\n`; the second event must NOT close it.
    component.handleInput("\r");
    component.handleInput("\n");
    expect(component.detailVisible).toBe(true);
    component.handleInput("l"); // back to the tree
    expect(component.detailVisible).toBe(false);

    // 'o' collapses (WG overlay), space toggles it back; neither enters detail.
    component.handleInput("o");
    expect(component.detailVisible).toBe(false);
    expect(render().some((l) => l.includes("(+3)"))).toBe(true);
    component.handleInput(" ");
    expect(render().some((l) => l.includes("(+3)"))).toBe(false);
  });

  it("full-screen navigable detail: q/Esc/left/Enter return to the tree (never close), scroll keys clamp", async () => {
    const WG = Array.from({ length: 60 }, (_, i) => `WG line ${i}`).join("\n");
    const detailFetcher = vi.fn().mockResolvedValue({
      task_id: "task-000",
      text: `${WG}\n`,
      source: "daemon",
    });
    const { component, render, closed } = makeComponent(bigSnap(), 10, undefined, detailFetcher, 0);
    render();
    component.handleInput("\r"); // open detail
    await flush();
    expect(component.detailVisible).toBe(true);
    // Full-screen: the detail occupies the whole panel, footer carries a
    // visible scroll/position indicator.
    const rows = render();
    expect(rows.length).toBe(10);
    // The visible scroll/position indicator is in the always-visible header.
    expect(rows[0]).toContain("%");
    expect(rows[0]).toContain("lines");
    // The footer carries the key hints.
    expect(rows.at(-1)).toContain("PgUp/PgDn");
    const view = component.bodyViewport();
    // PgDn scrolls into the body.
    component.handleInput("\x1b[6~"); // PageDown
    expect(component.scrollOffset).toBeGreaterThan(0);
    // End clamps to the maximum offset.
    component.handleInput("\x1b[F"); // End
    expect(component.scrollOffset).toBe(maxScroll(60, view));
    // PgUp moves back up.
    component.handleInput("\x1b[5~"); // PageUp
    expect(component.scrollOffset).toBeLessThan(maxScroll(60, view));
    // Home clamps to the top.
    component.handleInput("\x1b[H"); // Home
    expect(component.scrollOffset).toBe(0);

    // q in the detail returns to the tree — it does NOT close the panel.
    component.handleInput("q");
    expect(component.detailVisible).toBe(false);
    expect(closed).not.toHaveBeenCalled();
    // ... and the same for Enter and ←.
    component.handleInput("\r");
    await flush();
    expect(component.detailVisible).toBe(true);
    component.handleInput("\r");
    expect(component.detailVisible).toBe(false);
    component.handleInput("\r");
    await flush();
    component.handleInput("\x1b"); // Esc
    expect(component.detailVisible).toBe(false);
    expect(closed).not.toHaveBeenCalled();
    component.handleInput("\r");
    await flush();
    component.handleInput("\x1b[D"); // left arrow
    expect(component.detailVisible).toBe(false);
    expect(closed).not.toHaveBeenCalled();
    // Back in the tree, q closes the panel as before.
    component.handleInput("q");
    expect(closed).toHaveBeenCalledTimes(1);
  });

  it("shows only a labelled error when no WG detail source is available", async () => {
    const { component, render } = makeComponent(snap(), 20);
    render();
    component.handleInput("\r"); // open detail (no fetcher)
    expect(component.detailVisible).toBe(true);
    const detail = render();
    expect(detail.some((l) => l.includes("WG detail unavailable"))).toBe(true);
    // No approximate WG body is fabricated.
    expect(detail.some((l) => l.includes("Task: done-a"))).toBe(false);
  });

  it("paints every task line with WG's exact RGB palette (no semantic indirection)", () => {
    const theme = { fg: (_color: string, text: string) => `ROLE:${text}`, getColorMode: () => "truecolor" as const };
    const tui = { requestRender: vi.fn(), terminal: { rows: 30 } };
    const component = new FleetPanelComponent(
      treeSnap() as never,
      tui as never,
      vi.fn(),
      theme as never,
      {},
    );
    const lines = component.render(200);
    const lineFor = (task: string) => lines.find((l) => l.includes(`${task}  (`)) ?? "";
    const rgb = (status: string) => {
      const [r, g, b] = WG_PALETTE[status]!;
      return `38;2;${r};${g};${b}`;
    };
    // Per-status exact RGB — never a semantic role on the default path.
    expect(lineFor("root-a")).toContain(rgb("open"));
    expect(lineFor("child-1")).toContain(rgb("open"));
    expect(lineFor("child-2")).toContain(rgb("failed"));
    // No semantic role leaked into any task row (chrome rows may still use one).
    const taskRows = ["root-a", "child-1", "grand-1", "child-2", "loner"].map(lineFor);
    expect(taskRows.filter((l) => l.includes("ROLE:"))).toEqual([]);
    // Every visible task row carries a truecolour escape.
    for (const task of ["root-a", "child-1", "grand-1", "child-2", "loner"]) {
      expect(lineFor(task)).toContain("\x1b[38;2;");
    }
    // With WG's palette present there is no fallback notice.
    expect(lines[0]).not.toContain("fallback");
  });

  it("surfaces a visible theme fallback when WG exports no palette", () => {
    const theme = { fg: (_color: string, text: string) => `ROLE:${text}`, getColorMode: () => "truecolor" as const };
    const tui = { requestRender: vi.fn(), terminal: { rows: 30 } };
    const noPalette = {
      ...treeSnap(),
      tree: { text: WG_TREE_TEXT, node_lines: WG_TREE_NODE_LINES },
    };
    const component = new FleetPanelComponent(noPalette as never, tui as never, vi.fn(), theme as never, {});
    const lines = component.render(200);
    // The notice is visible, never a silent mismatch.
    expect(lines[0]).toContain("fallback");
    // It still paints from the local mirror of WG's palette (exact RGB).
    expect(lines.find((l) => l.includes("root-a  (")) ?? "").toContain("38;2;200;200;80");
  });

  it("closes on q and Esc", () => {
    const one = makeComponent();
    one.component.handleInput("q");
    expect(one.closed).toHaveBeenCalledTimes(1);
    const two = makeComponent();
    two.component.handleInput("\x1b");
    expect(two.closed).toHaveBeenCalledTimes(1);
  });

  it("selects a task by a mouse press hit-test", () => {
    const { component, render } = makeComponent(bigSnap(), 8);
    render();
    const hit = component.hitLines.findIndex((id) => id === "task-001");
    // A press consumes the event, focuses the panel and captures the gesture
    // so a following drag keeps routing here — pi's object shape, not a bool.
    expect(component.handleMouse({ type: "press", y: hit + 1 })).toEqual({
      handled: true,
      capture: true,
      focus: true,
    }); // +1 for the header line
    expect(component.selected).toBe("task-001");
  });

  // ── full mouse parity with the TUI: select / inspect / drag-pan ───────────

  it("press-selects the correct task at a scrolled offset, honoring blank/arc rows", () => {
    // `treeSnap` uses WG's own rendered tree, whose line 4 is a blank separator
    // (taskId === null). Hit-testing must index the *rendered* line list, not
    // the task-only order, or every row after the blank line is mis-mapped.
    const { component, render } = makeComponent(treeSnap(), 5); // body = 3
    render();
    expect(component.selected).toBe("root-a");
    component.handleInput("\x1b[6~"); // PageDown
    render();
    expect(component.treeScrollOffset).toBe(2);
    // The window now shows rendered lines 2,3,4 = grand-1, child-2, <blank>.
    component.handleMouse({ type: "press", y: 1, button: "left" });
    expect(component.selected).toBe("grand-1");
    component.handleMouse({ type: "press", y: 2, button: "left" });
    expect(component.selected).toBe("child-2");
    // y=3 is the blank separator: consumed but selects nothing.
    component.handleMouse({ type: "press", y: 3, button: "left" });
    expect(component.selected).toBe("child-2");
  });

  it("press-selects the correct task after a subtree is collapsed", () => {
    const { component, render } = makeComponent(treeSnap(), 6); // body = 4
    render();
    expect(component.selected).toBe("root-a");
    component.handleInput("o"); // collapse root-a → child-1/grand-1/child-2 hidden
    const collapsed = render();
    expect(collapsed.some((l) => l.includes("loner"))).toBe(true);
    // Rendered lines are now [root-a (+3), loner].
    component.handleMouse({ type: "press", y: 2, button: "left" });
    expect(component.selected).toBe("loner");
    component.handleMouse({ type: "press", y: 1, button: "left" });
    expect(component.selected).toBe("root-a");
  });

  it("press-drag pans the tree; the gesture captures and release ends it", () => {
    const { component, render } = makeComponent(bigSnap(40), 6); // body = 4
    render();
    expect(component.treeScrollOffset).toBe(0);
    const start = component.handleMouse({ type: "press", y: 4, button: "left" });
    expect(start).toEqual({ handled: true, capture: true, focus: true });
    expect(component.isDragging).toBe(true);
    // Natural panning: dragging the pointer UP scrolls the content DOWN by the
    // row delta (y 4 → 1 = +3).
    const drag = component.handleMouse({ type: "drag", y: 1, button: "left" });
    expect(drag).toMatchObject({ handled: true, capture: true });
    expect(component.treeScrollOffset).toBe(3);
    // A second drag step is incremental.
    component.handleMouse({ type: "move", y: 0, button: "left" });
    expect(component.treeScrollOffset).toBe(4);
    // Release ends the gesture.
    expect(component.handleMouse({ type: "release", y: 0, button: "left" })).toMatchObject({ handled: true });
    expect(component.isDragging).toBe(false);
    // After release a stray drag is no longer ours.
    expect(component.handleMouse({ type: "drag", y: 8, button: "left" })).toBeUndefined();
    expect(component.treeScrollOffset).toBe(4);
  });

  it("capture: a drag that leaves the content area keeps panning and stays captured", () => {
    const { component, render } = makeComponent(bigSnap(40), 6); // body = 4
    render();
    component.handleMouse({ type: "press", y: 2, button: "left" }); // inside content
    // Drag far above the panel: content scrolls down, clamped at the end. The
    // pointer is outside the content area but the gesture is captured.
    const out = component.handleMouse({ type: "drag", y: -50, button: "left" });
    expect(out).toMatchObject({ handled: true, capture: true });
    expect(component.treeScrollOffset).toBe(maxScroll(40, component.bodyViewport()));
    expect(component.isDragging).toBe(true);
    // Drag far below: offset clamps back to the top, still captured.
    const back = component.handleMouse({ type: "drag", y: 500, button: "left" });
    expect(back).toMatchObject({ handled: true, capture: true });
    expect(component.treeScrollOffset).toBe(0);
    component.handleMouse({ type: "release", y: 500, button: "left" });
    expect(component.isDragging).toBe(false);
  });

  it("ignores presses outside the content area (header/footer/off-panel/NaN)", () => {
    const { component, render } = makeComponent(bigSnap(40), 6); // rows 0..5, content 1..4
    render();
    const sel = component.selected;
    for (const y of [0, 5, -1, 9999, Number.NaN]) {
      expect(component.handleMouse({ type: "press", y, button: "left" })).toBeUndefined();
    }
    expect(component.selected).toBe(sel);
    expect(component.isDragging).toBe(false);
  });

  it("click selects; clicking the already-selected row (or a double-click) opens detail", () => {
    const { component, render } = makeComponent(bigSnap(40), 8);
    render();
    // First click on an as-yet-unselected row: select only, no detail.
    component.handleMouse({ type: "press", y: 2, button: "left" });
    expect(component.handleMouse({ type: "click", y: 2, button: "left", clickCount: 1 })).toMatchObject({
      handled: true,
    });
    expect(component.detailVisible).toBe(false);
    expect(component.selected).toBe("task-001");
    // Click the SAME (already-selected) row again → open detail (TUI parity:
    // the inspector shows the clicked node).
    component.handleMouse({ type: "press", y: 2, button: "left" });
    component.handleMouse({ type: "click", y: 2, button: "left", clickCount: 1 });
    expect(component.detailVisible).toBe(true);

    // A double-click (clickCount >= 2) opens detail even on a fresh row.
    const other = makeComponent(bigSnap(40), 8);
    other.render();
    other.component.handleMouse({ type: "press", y: 3, button: "left" });
    other.component.handleMouse({ type: "click", y: 3, button: "left", clickCount: 2 });
    expect(other.component.detailVisible).toBe(true);
  });

  it("advertises drag in the footer hint (mouse/keyboard discovery)", () => {
    const { component, render } = makeComponent(bigSnap(40), 6);
    const lines = render();
    expect(lines.at(-1)).toContain("drag");
    // A scrolled view still advertises drag-pan.
    component.handleInput("\x1b[6~");
    const scrolled = component.render(100);
    expect(scrolled.at(-1)).toContain("drag");
  });

  it("CONTRACT: handleMouse never returns a truthy non-object (pi-core crash guard)", () => {
    // Regression for the wheel-scroll crash:
    //   TypeError: Cannot use 'in' operator to search for 'target' in true
    // pi's dispatchMouseEvent does `if (result) { if ("target" in result) ... }`,
    // so any truthy primitive (a bare `true`) crashes pi core. The return must
    // be falsy or a real object over the whole mouse battery.
    const { component, render } = makeComponent(bigSnap(40), 8);
    render();
    const hit = component.hitLines.findIndex((id) => id === "task-001");
    const battery: Array<{ label: string; event: Record<string, unknown> }> = [
      { label: "wheel up", event: { type: "wheel", wheelDelta: -8 } },
      { label: "wheel down", event: { type: "wheel", wheelDelta: 8 } },
      { label: "wheel zero delta", event: { type: "wheel", wheelDelta: 0 } },
      { label: "wheel no delta", event: { type: "wheel" } },
      { label: "legacy mouse.wheel", event: { type: "mouse.wheel", wheelDelta: 5 } },
      { label: "press on a row", event: { type: "press", y: hit + 1 } },
      { label: "press on the header", event: { type: "press", y: 0 } },
      { label: "press outside (negative)", event: { type: "press", y: -5 } },
      { label: "press outside (below)", event: { type: "press", y: 9999 } },
      { label: "click on a row", event: { type: "click", y: hit + 1 } },
      { label: "drag", event: { type: "drag", y: 3 } },
      { label: "release", event: { type: "release", y: 3 } },
      { label: "move", event: { type: "move", y: 3 } },
      { label: "junk: empty event", event: {} },
      { label: "junk: unknown type", event: { type: "nonsense", y: 1 } },
      { label: "junk: non-numeric wheelDelta", event: { type: "wheel", wheelDelta: "nope" } },
      { label: "junk: NaN y", event: { type: "press", y: Number.NaN } },
    ];
    for (const { label, event } of battery) {
      const result = component.handleMouse(event as never);
      // The exact contract assertion (also centralised in isPiMouseResult).
      expect(
        result === undefined || result === false || (typeof result === "object" && result !== null),
        `handleMouse(${label}) returned a truthy non-object: ${String(result)}`,
      ).toBe(true);
      expect(isPiMouseResult(result), `isPiMouseResult(${label})`).toBe(true);
      // The real failure mode: pi's dispatch throws on a truthy primitive.
      expect(
        () => piDispatchMouseEvent(component, event),
        `pi dispatchMouseEvent threw on handleMouse(${label})`,
      ).not.toThrow();
    }

    // Stateful gesture battery: press → drag (in and out of content) → move →
    // release → click. Every return must still be pi's shape, and pi's real
    // dispatch must not throw across the whole gesture.
    component.handleMouse({ type: "press", y: 2, clickCount: 1 });
    const gesture: Array<{ label: string; event: Record<string, unknown> }> = [
      { label: "gesture: drag in content", event: { type: "drag", y: 1 } },
      { label: "gesture: drag outside content", event: { type: "drag", y: -10 } },
      { label: "gesture: move while dragging", event: { type: "move", y: 3 } },
      { label: "gesture: release", event: { type: "release", y: 3 } },
      { label: "gesture: click", event: { type: "click", y: 2, clickCount: 1 } },
      { label: "gesture: double click", event: { type: "click", y: 2, clickCount: 2 } },
      { label: "gesture: drag with no active gesture", event: { type: "drag", y: 5 } },
      { label: "gesture: release with no active gesture", event: { type: "release", y: 5 } },
      { label: "gesture: legacy mouse.drag", event: { type: "mouse.drag", y: 4 } },
      { label: "gesture: legacy mouse.release", event: { type: "mouse.release", y: 4 } },
    ];
    for (const { label, event } of gesture) {
      const result = component.handleMouse(event as never);
      expect(isPiMouseResult(result), `isPiMouseResult(${label})`).toBe(true);
      expect(
        () => piDispatchMouseEvent(component, event),
        `pi dispatchMouseEvent threw on handleMouse(${label})`,
      ).not.toThrow();
    }
  });
});

// ── installation: config gate + ambient widget unaffected ────────────────────

describe("installFleetView — gate + ambient widget", () => {
  function makePi(mode: "tui" | "rpc" | "json" | "print" = "tui") {
    const handlers = new Map<string, (event: unknown, ctx: unknown) => void>();
    const commands = new Map<string, { handler: (args: string, ctx: unknown) => Promise<void> }>();
    const ui = { setWidget: vi.fn(), notify: vi.fn(), custom: vi.fn(), theme: undefined };
    const pi = {
      registerCommand: vi.fn((name: string, def: { handler: (args: string, ctx: unknown) => Promise<void> }) =>
        commands.set(name, def),
      ),
      on: vi.fn((event: string, handler: (event: unknown, ctx: unknown) => void) => handlers.set(event, handler)),
      registerTool: vi.fn(),
      registerProvider: vi.fn(),
    };
    const ctx = { mode, hasUI: mode !== "print" && mode !== "json", ui, signal: undefined };
    return { pi, handlers, commands, ui, ctx };
  }

  const backend = {
    run: vi.fn(async () => ({
      stdout: JSON.stringify([{ id: "task-a", title: "A", status: "in-progress", after: [], before: [] }]),
      stderr: "",
      code: 0,
      killed: false,
    })),
    getFleet: vi.fn(async () => snap()),
  };

  it("does not mount or open anything when disabled", async () => {
    const { pi, handlers, commands, ui, ctx } = makePi("tui");
    installFleetView(pi as never, backend as never, {}, { config: { enabled: false } });
    handlers.get("session_start")!({}, ctx);
    expect(ui.setWidget).not.toHaveBeenCalled();
    await commands.get("wg-fleet")!.handler("", ctx);
    expect(ui.custom).not.toHaveBeenCalled();
    expect(ui.notify).toHaveBeenCalledWith(expect.stringContaining("disabled"), "info");
  });

  it("opens the scrollable view via ctx.ui.custom when enabled", async () => {
    const { pi, commands, ui, ctx } = makePi("tui");
    installFleetView(pi as never, backend as never, {}, { config: { enabled: true } });
    await commands.get("wg-fleet")!.handler("", ctx);
    expect(ui.custom).toHaveBeenCalledTimes(1);
    expect(backend.getFleet).toHaveBeenCalled();
  });

  it("is a no-op in non-TUI modes", async () => {
    const { pi, commands, ui, ctx } = makePi("json");
    installFleetView(pi as never, backend as never, {}, { config: { enabled: true } });
    await commands.get("wg-fleet")!.handler("", ctx);
    expect(ui.custom).not.toHaveBeenCalled();
    expect(ui.setWidget).not.toHaveBeenCalled();
  });

  it("still mounts the ambient below-editor widget strip (no regression)", async () => {
    vi.useFakeTimers();
    try {
      const { pi, handlers, ui, ctx } = makePi("tui");
      installFleetView(pi as never, backend as never, { daemonSocket: "/nonexistent.sock" }, {
        config: { enabled: true },
        pollMs: 10,
      });
      handlers.get("session_start")!({}, ctx);
      await vi.advanceTimersByTimeAsync(30);
      expect(ui.setWidget).toHaveBeenCalled();
      const call = ui.setWidget.mock.calls.at(-1)!;
      expect(call[0]).toBe("wg-fleet");
      expect(call[2]).toEqual({ placement: "belowEditor" });
      handlers.get("session_shutdown")!({}, ctx);
    } finally {
      vi.useRealTimers();
    }
  });
});
