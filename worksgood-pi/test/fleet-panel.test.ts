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
  boundTranscriptBody,
  buildFleetTree,
  clampScroll,
  fleetCountsHeader,
  getFleetToVizSnapshot,
  maxScroll,
  pageScroll,
  readAgentStreamTail,
  readTextTail,
  scrollToKeepVisible,
  transcriptLineLimit,
} from "../pi-worksgood/index.js";
// @ts-expect-error — built ESM artifact import (see above)
import { FleetPanelComponent } from "../pi-worksgood/fleet-panel.js";
// @ts-expect-error — built ESM artifact import (see above)
import { installFleetView } from "../pi-worksgood/fleet-view.js";

// ── fixtures ─────────────────────────────────────────────────────────────────

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
      },
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
});

// ── component interactions ───────────────────────────────────────────────────

describe("FleetPanelComponent", () => {
  function makeComponent(snapshot: unknown = bigSnap(), height = 6, liveDir?: string) {
    const tui = { requestRender: vi.fn(), terminal: { rows: height } };
    const closed = vi.fn();
    const component = new FleetPanelComponent(snapshot as never, tui, closed, null, {}, liveDir);
    return { component, tui, closed, render: (w = 100) => component.render(w) };
  }

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
    // Wheel up (negative delta) scrolls toward the top and clamps at 0.
    expect(component.handleMouse({ type: "wheel", wheelDelta: -100 })).toBe(true);
    expect(component.treeScrollOffset).toBe(0);
    // Wheel down (positive delta) scrolls away from the top.
    component.handleMouse({ type: "wheel", wheelDelta: 8 });
    expect(component.treeScrollOffset).toBeGreaterThan(0);
    // End selects the last task and pins the viewport to the bottom.
    component.handleInput("\x1b[F"); // End
    expect(component.selected).toBe("task-039");
    expect(component.treeScrollOffset).toBe(maxScroll(40, component.bodyViewport()));
  });

  it("drills into the selected task's detail and back, showing live activity + tail", () => {
    const root = mkdtempSync(join(tmpdir(), "wg-fleet-compose-"));
    const agentDir = join(root, "agents", "agent-7");
    mkdirSync(agentDir, { recursive: true });
    writeFileSync(join(agentDir, "raw_stream.jsonl"), '{"type":"tool_execution_start","toolName":"bash","args":{}}\n');
    const { component, render } = makeComponent(snap(), 20, root);
    render();
    // Select active-b (nested under done-a: done-a is already selected first).
    expect(component.selected).toBe("done-a");
    component.handleInput("\x1b[B"); // down
    expect(component.selected).toBe("active-b");
    component.handleInput("\r"); // Enter → detail
    expect(component.detailVisible).toBe(true);
    const detail = render();
    expect(detail.some((l) => l.includes("── active-b ──"))).toBe(true);
    expect(detail.some((l) => l.includes("running cargo test --lib"))).toBe(true);
    expect(detail.some((l) => l.includes("Transcript tail"))).toBe(true);
    expect(detail.length).toBeLessThanOrEqual(20);
    component.handleInput("l"); // back to tree
    expect(component.detailVisible).toBe(false);
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
    expect(component.handleMouse({ type: "press", y: hit + 1 })).toBe(true); // +1 for the header line
    expect(component.selected).toBe("task-001");
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
