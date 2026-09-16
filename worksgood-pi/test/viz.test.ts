/**
 * viz tests — the embedded VizView panel (widget + /wg-viz custom component).
 *
 * Pins, against a **fixture daemon socket** (a real one-shot UDS server that
 * speaks the daemon's `IpcRequest`/`IpcResponse` JSON-line protocol):
 *   - the panel's only IPC request is the read-only `viz_snapshot`;
 *   - the tree renders and selection moves through the visible order;
 *   - detail lines match the TUI HUD read model (sections + shapes) for the
 *     fixture graph;
 *   - the widget one-line summary derives from the snapshot and silently
 *     clears when the daemon is offline;
 *   - socket-unavailable degrades to the read-only `wg viz` ASCII fallback.
 */

import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { createServer, type Server, type Socket } from "node:net";
import { mkdtempSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import {
  fetchVizSnapshot,
  resolveSocketPath,
  socketCandidates,
  vizSnapshotWithFallback,
  VizPoller,
} from "../pi-worksgood/viz-snapshot.js";
// @ts-expect-error — built artifact import (see above)
import {
  buildTree,
  detailLines,
  lineTaskMap,
  taskCounts,
  treeText,
  widgetLine,
} from "../pi-worksgood/viz-readmodel.js";
// @ts-expect-error — built artifact import (see above)
import { VizPanelComponent, installVizPanel, VIZ_WIDGET_KEY } from "../pi-worksgood/viz-panel.js";

// ── fixture graph ────────────────────────────────────────────────────────────

const FIXTURE_TASKS = [
  {
    id: "smoke-pin",
    title: "Smoke pin completion",
    presentation: "primary",
    status: "done",
    assigned: null,
    after: [],
    before: ["opaque-exec"],
    description_head: "Pin the completion smoke scenario.",
    created_at: "2026-02-01T10:00:00Z",
    started_at: "2026-02-01T10:05:00Z",
    completed_at: "2026-02-01T15:00:00Z",
    token_usage: { input_tokens: 182000, output_tokens: 132000, cost_usd: 45.2 },
    retry_count: 0,
    failure_reason: null,
    log_count: 3,
    log_tail: [
      { timestamp: "2026-02-01T15:00:00Z", actor: "agent-1", message: "landed" },
      { timestamp: "2026-02-01T12:00:00Z", actor: "agent-1", message: "tests green" },
    ],
  },
  {
    id: "opaque-exec",
    title: "Opaque pi execution",
    presentation: "primary",
    status: "in-progress",
    assigned: "agent-7",
    after: ["smoke-pin"],
    before: [],
    description_head: "Make opaque pi execution observable.",
    created_at: "2026-02-01T11:00:00Z",
    started_at: "2026-02-01T11:30:00Z",
    completed_at: null,
    token_usage: { input_tokens: 75000, output_tokens: 46000, cost_usd: 10.1 },
    retry_count: 1,
    failure_reason: null,
    log_count: 5,
    log_tail: [
      { timestamp: "2026-02-02T09:00:00Z", actor: "agent-7", message: "wiring panel" },
      { timestamp: "2026-02-01T20:00:00Z", actor: "agent-7", message: "socket protocol pinned" },
    ],
  },
  {
    id: "demo",
    title: "Demo",
    presentation: "primary",
    status: "open",
    assigned: null,
    after: [],
    before: [],
    created_at: "2026-01-20T00:00:00Z",
    started_at: null,
    completed_at: null,
    token_usage: null,
    retry_count: 0,
    failure_reason: null,
    log_count: 0,
    log_tail: [],
  },
];

function makeSnapshot(tasks = FIXTURE_TASKS) {
  return { tasks };
}

// ── fixture daemon socket ────────────────────────────────────────────────────

let server: Server;
let socketPath: string;
const receivedRequests: Array<Record<string, unknown>> = [];

/** One-shot UDS server mirroring the daemon's one-request-per-connection contract. */
function startFixtureSocket(path: string, response: (req: Record<string, unknown>) => unknown): Promise<void> {
  return new Promise((resolve) => {
    server = createServer((stream: Socket) => {
      let buffer = "";
      stream.on("data", (chunk: Buffer) => {
        buffer += chunk.toString("utf8");
        const idx = buffer.indexOf("\n");
        if (idx < 0) return;
        const line = buffer.slice(0, idx).trim();
        const req = JSON.parse(line) as Record<string, unknown>;
        receivedRequests.push(req);
        stream.end(`${JSON.stringify(response(req))}\n`);
      });
    });
    server.listen(path, () => resolve());
  });
}

beforeAll(async () => {
  socketPath = join(mkdtempSync(join(tmpdir(), "wg-viz-test-")), "daemon.sock");
  await startFixtureSocket(socketPath, (req) => {
    if (req.cmd !== "viz_snapshot") {
      return { ok: false, error: `unexpected cmd ${String(req.cmd)}` };
    }
    return { ok: true, tasks: FIXTURE_TASKS };
  });
});

afterAll(() => {
  server?.close();
});

// ── snapshot client (socket protocol) ────────────────────────────────────────

describe("viz-snapshot daemon client", () => {
  it("round-trips viz_snapshot over the fixture socket and only ever sends the read-only cmd", async () => {
    receivedRequests.length = 0;
    const snapshot = await fetchVizSnapshot(socketPath, { timeoutMs: 2000 });
    expect(snapshot.tasks.map((t: { id: string }) => t.id)).toEqual(["smoke-pin", "opaque-exec", "demo"]);
    // Read-only proof: the only command the panel ever sends is viz_snapshot.
    expect(receivedRequests).toHaveLength(1);
    expect(receivedRequests[0]!.cmd).toBe("viz_snapshot");
    expect(receivedRequests[0]!.log_tail).toBe(20);
  });

  it("rejects on an error response (fail loud to the caller, silent at the UI layer)", async () => {
    const errPath = join(mkdtempSync(join(tmpdir(), "wg-viz-test-")), "err.sock");
    await startFixtureSocket(errPath, () => ({ ok: false, error: "graph unreadable" }));
    const s = createServer; // keep import used
    void s;
    await expect(fetchVizSnapshot(errPath, { timeoutMs: 2000 })).rejects.toThrow(/graph unreadable/);
  });

  it("rejects (bounded) when the socket does not exist", async () => {
    await expect(fetchVizSnapshot(join(tmpdir(), "wg-viz-missing", "daemon.sock"), { timeoutMs: 300 })).rejects.toThrow();
  });

  it("resolves the socket path: env first, then WG_DIR spellings, then cwd", () => {
    expect(resolveSocketPath({ daemonSocket: "/x/daemon.sock", dir: undefined }, "/cwd", () => true)).toEqual({
      socket: "/x/daemon.sock",
      source: "env",
    });
    expect(
      resolveSocketPath({ daemonSocket: undefined, dir: "/proj/.wg" }, "/cwd", (p) => p.endsWith("/proj/.wg/service/daemon.sock")),
    ).toEqual({ socket: "/proj/.wg/service/daemon.sock", source: "wg-dir" });
    expect(
      resolveSocketPath({ daemonSocket: undefined, dir: "/proj" }, "/cwd", (p) => p.endsWith("/proj/.wg/service/daemon.sock")),
    ).toEqual({ socket: "/proj/.wg/service/daemon.sock", source: "wg-dir" });
    expect(resolveSocketPath({ daemonSocket: undefined, dir: undefined }, "/cwd", (p) => p === "/cwd/.wg/service/daemon.sock")).toEqual({
      socket: "/cwd/.wg/service/daemon.sock",
      source: "cwd",
    });
    expect(resolveSocketPath({}, "/cwd", () => false)).toEqual({ socket: null, source: "none" });
    expect(socketCandidates({}, "/cwd").length).toBeGreaterThan(0);
  });

  it("falls back to the read-only `wg viz` ASCII output when the socket is unavailable", async () => {
    const run = vi.fn(async (_args: string[]) => ({
      stdout: "┌→ ✓ parent-a (done)\n",
      stderr: "",
      code: 0,
      killed: false,
    }));
    const result = await vizSnapshotWithFallback({ run } as never, { daemonSocket: undefined, dir: "/nowhere" }, {});
    expect(result.kind).toBe("ascii");
    if (result.kind === "ascii") expect(result.text).toContain("parent-a");
    // Read-only: the fallback verb list contains no mutating command.
    expect(run).toHaveBeenCalledTimes(1);
    expect(run.mock.calls[0]![0]).toEqual(["viz", "--all", "--no-tui"]);
  });

  it("VizPoller is bounded: no overlapping fetches and change-guarded delivery", async () => {
    let fetches = 0;
    const seen: number[] = [];
    const poller = new VizPoller(
      async () => {
        fetches++;
        await new Promise((r) => setTimeout(r, 20));
        return makeSnapshot(fetches === 1 ? FIXTURE_TASKS : FIXTURE_TASKS.slice(1));
      },
      (snapshot) => seen.push(snapshot.tasks.length),
      5,
    );
    poller.start();
    await new Promise((r) => setTimeout(r, 120));
    poller.stop();
    expect(fetches).toBeGreaterThanOrEqual(2);
    // The first snapshot and the (changed) second are delivered; identical
    // snapshots would not be.
    expect(seen).toEqual([3, 2]);
  });
});

// ── read model ───────────────────────────────────────────────────────────────

describe("viz-readmodel", () => {
  it("renders the dependency tree with nesting, glyphs and collapse markers", () => {
    const render = buildTree(makeSnapshot());
    const text = treeText(render, null).join("\n");
    expect(text).toContain("✓ smoke-pin");
    expect(text).toContain("● opaque-exec");
    // opaque-exec depends on smoke-pin → nested one level deeper.
    const parentLine = render.lines.findIndex((l) => l.taskId === "smoke-pin");
    const childLine = render.lines.findIndex((l) => l.taskId === "opaque-exec");
    expect(childLine).toBe(parentLine + 1);
    expect(render.lines[parentLine]!.depth).toBe(0);
    expect(render.lines[childLine]!.depth).toBe(1);
    expect(text).toContain("demo");
    // token display rides the node line like the TUI's →/←/◎ convention.
    expect(text).toContain("→182k");
  });

  it("collapse markers replace hidden children", () => {
    const render = buildTree(makeSnapshot(), new Set(["smoke-pin"]));
    const collapsedLine = render.lines.find((l) => l.taskId === "smoke-pin");
    expect(collapsedLine!.text).toContain("(+1)");
    expect(render.lines.some((l) => l.taskId === "opaque-exec")).toBe(false);
  });

  it("counts drive the one-line widget summary", () => {
    const counts = taskCounts(FIXTURE_TASKS);
    // smoke-pin done, opaque-exec in-progress, demo open with no blockers → ready.
    expect(counts).toEqual({ inProgress: 1, ready: 1, blocked: 0, done: 1, failed: 0 });
    expect(widgetLine(counts)).toBe("wg ▸ 1 in-progress · 1 ready · 0 blocked · 1 done · 0 failed");
  });

  it("blocked counts an open task with an unfinished dependency", () => {
    const tasks = [
      { id: "a", status: "done" },
      { id: "b", status: "open", after: ["a"] },
      { id: "c", status: "open", after: ["b"] },
    ];
    // b's only dep (a) is done → ready; c's dep (b) is open → blocked.
    expect(taskCounts(tasks as never)).toEqual({ inProgress: 0, ready: 1, blocked: 1, done: 1, failed: 0 });
  });

  it("detail lines match the TUI HUD selected-task → detail-lines model for the fixture graph", () => {
    const tasks = makeSnapshot().tasks;
    const detail = detailLines(
      tasks.find((t: { id: string }) => t.id === "opaque-exec")!,
      tasks,
    );
    const text = detail.lines.join("\n");
    // Section order mirrors load_hud_detail_for_task: header → identity →
    // status/agent → dependencies → description → log tail.
    expect(text).toContain("── opaque-exec ──");
    expect(text).toContain("Title: Opaque pi execution");
    expect(text).toContain("Presentation: primary");
    expect(text).toContain("Status: in-progress");
    expect(text).toContain("Agent: agent-7");
    expect(text).toContain("Retries: 1");
    expect(text.indexOf("── Dependencies ──")).toBeGreaterThan(text.indexOf("Agent: agent-7"));
    expect(text).toContain("after: smoke-pin (done)");
    expect(text).toContain("── Description ──");
    expect(text).toContain("Make opaque pi execution observable.");
    expect(text).toContain("── Log (last 2 of 5) ──");
    expect(text).toContain("wiring panel [agent-7]");
    expect(detail.hasLog).toBe(true);
  });
});

// ── panel component (keyboard + mouse) ───────────────────────────────────────

describe("VizPanelComponent", () => {
  function makeComponent(overrides: { snapshot?: unknown; ascii?: string } = {}) {
    const renders: string[][] = [];
    const tui = { requestRender: () => undefined };
    const poller = new VizPoller(async () => makeSnapshot(), () => undefined, 60_000);
    poller.stop();
    const closed = vi.fn();
    const component = new VizPanelComponent(
      ("snapshot" in overrides ? overrides.snapshot : makeSnapshot()) as never,
      tui,
      poller,
      closed,
      null,
      overrides.ascii ?? null,
    );
    const render = () => {
      const lines = component.render(120);
      renders.push(lines);
      return lines;
    };
    return { component, render, closed, poller };
  }

  it("renders the live graph: widget summary line, tree, and hit map", () => {
    const { component, render } = makeComponent();
    const lines = render();
    expect(lines[0]).toContain("wg ▸ 1 in-progress");
    expect(lines.some((l) => l.includes("smoke-pin"))).toBe(true);
    expect(component.hitLines.filter((id) => id !== null).length).toBe(3);
  });

  it("renders the ASCII fallback when the daemon is offline", () => {
    const { component, render } = makeComponent({ snapshot: null, ascii: "┌→ ✓ smoke-pin (done)" });
    const lines = render();
    expect(lines[0]).toContain("daemon offline");
    expect(lines.some((l) => l.includes("smoke-pin"))).toBe(true);
  });

  it("moves the selection with real arrow keys through the visible tree order and clamps at the ends", () => {
    const { component, render } = makeComponent();
    render();
    // Visible order: roots first (demo, smoke-pin), then nested dependents.
    expect(component.selected).toBe("demo");
    component.handleInput("\x1b[B"); // Key.down
    expect(component.selected).toBe("smoke-pin");
    component.handleInput("\x1b[B");
    expect(component.selected).toBe("opaque-exec");
    component.handleInput("\x1b[B"); // clamp
    expect(component.selected).toBe("opaque-exec");
    component.handleInput("\x1b[A"); // Key.up
    expect(component.selected).toBe("smoke-pin");
    component.handleInput("G");
    expect(component.selected).toBe("opaque-exec");
    component.handleInput("g");
    expect(component.selected).toBe("demo");
    expect(render().length).toBeGreaterThan(0);
  });

  it("expands/collapses the selected subtree (enter) and shows detail lines (l)", () => {
    const { component, render } = makeComponent();
    render();
    // Select smoke-pin (second visible line), then collapse its subtree.
    component.handleInput("\x1b[B");
    expect(component.selected).toBe("smoke-pin");
    component.handleInput("\r"); // Key.enter → toggle collapse
    const collapsed = render();
    expect(collapsed.some((l) => l.includes("(+1)"))).toBe(true);

    // opaque-exec is hidden by the collapse; the selection clamps at
    // smoke-pin, which is now the last visible task.
    component.handleInput("\x1b[B");
    expect(component.selected).toBe("smoke-pin");
    component.handleInput("l");
    const detail = render();
    expect(detail.some((l) => l.includes("── smoke-pin ──"))).toBe(true);
    expect(component.detailVisible).toBe(true);
    component.handleInput("l");
    expect(component.detailVisible).toBe(false);
  });

  it("mouse hit-testing selects the task under a rendered line", () => {
    const { component, render } = makeComponent();
    render();
    const line = component.hitLines.findIndex((id: string | null) => id === "opaque-exec");
    expect(component.handleMouse({ type: "mouse.press", y: line })).toBe(true);
    expect(component.selected).toBe("opaque-exec");
  });

  it("closes via q/escape and stops its poller on dispose", async () => {
    const { component, closed, poller } = makeComponent();
    poller.start();
    component.handleInput("q");
    expect(closed).toHaveBeenCalledTimes(1);
    component.dispose();
    expect(component.isDisposed).toBe(true);
    expect(poller["timer"]).toBeNull();
  });
});

// ── installation: command + widget ───────────────────────────────────────────

describe("installVizPanel", () => {
  function makePi(mode: "tui" | "rpc" | "json" | "print" = "tui") {
    const handlers = new Map<string, (event: unknown, ctx: unknown) => void>();
    const commands = new Map<string, { handler: (args: string, ctx: unknown) => Promise<void> }>();
    const ui = { setWidget: vi.fn(), notify: vi.fn() };
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

  it("registers the /wg-viz command and the TUI widget lifecycle", async () => {
    const { pi, commands } = makePi();
    installVizPanel(pi as never, { run: async () => ({ stdout: "", stderr: "", code: 0, killed: false }) } as never, {});
    expect(commands.has("wg-viz")).toBe(true);
    expect(pi.on).toHaveBeenCalledWith("session_start", expect.any(Function));
    expect(pi.on).toHaveBeenCalledWith("session_shutdown", expect.any(Function));
  });

  it("widget shows the one-line summary from the fixture socket and never mutates the graph", async () => {
    vi.useFakeTimers();
    try {
      receivedRequests.length = 0;
      const { pi, handlers, ui, ctx } = makePi("tui");
      installVizPanel(pi as never, { run: async () => ({ stdout: "", stderr: "", code: 0, killed: false }) } as never, {
        daemonSocket: socketPath,
      } as never, { widgetPollMs: 10 });
      handlers.get("session_start")!({}, ctx);
      await vi.advanceTimersByTimeAsync(30);
      expect(ui.setWidget).toHaveBeenCalled();
      const lastCall = ui.setWidget.mock.calls.at(-1)!;
      expect(lastCall[0]).toBe(VIZ_WIDGET_KEY);
      expect(String(lastCall[1]![0])).toContain("wg ▸ 1 in-progress · 1 ready · 0 blocked · 1 done · 0 failed");
      // Read-only proof, widget path: only viz_snapshot requests were sent.
      for (const req of receivedRequests) expect(req.cmd).toBe("viz_snapshot");
      handlers.get("session_shutdown")!({}, ctx);
    } finally {
      vi.useRealTimers();
    }
  });

  it("silently clears the widget when the daemon is offline (no ASCII cruft in the footer)", async () => {
    vi.useFakeTimers();
    try {
      const { pi, handlers, ui, ctx } = makePi("tui");
      const run = vi.fn(async () => ({ stdout: "ascii", stderr: "", code: 0, killed: false }));
      installVizPanel(pi as never, { run } as never, { daemonSocket: undefined, dir: "/nowhere" } as never, {
        widgetPollMs: 10,
      });
      handlers.get("session_start")!({}, ctx);
      await vi.advanceTimersByTimeAsync(30);
      const calls = ui.setWidget.mock.calls.filter((c) => c[0] === VIZ_WIDGET_KEY);
      expect(calls.length).toBeGreaterThan(0);
      expect(calls.at(-1)![1]).toBeUndefined();
    } finally {
      vi.useRealTimers();
    }
  });

  it("degrades silently in non-TUI modes: no widget, no polling, command no-ops", async () => {
    vi.useFakeTimers();
    try {
      const { pi, handlers, ui, ctx, commands } = makePi("json");
      installVizPanel(pi as never, { run: async () => ({ stdout: "", stderr: "", code: 0, killed: false }) } as never, {
        daemonSocket: socketPath,
      } as never, { widgetPollMs: 10 });
      handlers.get("session_start")!({}, ctx);
      await vi.advanceTimersByTimeAsync(30);
      expect(ui.setWidget).not.toHaveBeenCalled();
      // The command itself no-ops outside TUI mode.
      await commands.get("wg-viz")!.handler("", ctx);
      expect(ui.notify).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });
});
