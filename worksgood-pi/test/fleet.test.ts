/**
 * fleet tests — the FleetView-style bottom panel (read model, config gate,
 * read-only data path, and the below-editor widget mount).
 *
 * Pins, against a **fixture daemon socket** (the same one-request-per-connection
 * `IpcRequest`/`IpcResponse` protocol the TUI family speaks):
 *   - glyph/colour mapping stays consistent with the WG TUI / viz read-model;
 *   - the fleet line formatting (header + agent rows) is compact/expandable;
 *   - the config gate: default OFF, opt-in via the extension config file;
 *   - the panel issues only the read-only `viz_snapshot` + `agents` requests;
 *   - the widget mounts BELOW the editor only when enabled and in TUI mode.
 *
 * Tests run against the built `pi-worksgood/` artifact — `npm test` builds first.
 */

import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { createServer, type Server, type Socket } from "node:net";
import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import {
  DEFAULT_FLEET_VIEW_CONFIG,
  FLEET_WIDGET_KEY,
  FleetPoller,
  agentColor,
  agentElapsed,
  agentGlyph,
  agentLine,
  agentModel,
  agentUsageCompact,
  agentUsageFull,
  compactCount,
  fetchFleetOverSocket,
  fleetCounts,
  fleetHeaderLine,
  fleetSnapshotWithFallback,
  isAgentAlive,
  normalizeAgent,
  parseFleetViewConfig,
  readFleetViewConfig,
  renderFleetLines,
  taskColor,
  taskGlyph,
} from "../pi-worksgood/index.js";
// @ts-expect-error — built ESM artifact import (see above)
import { installFleetView } from "../pi-worksgood/fleet-view.js";

// ── fixtures ─────────────────────────────────────────────────────────────────

const FIXTURE_TASKS = [
  { id: "smoke-pin", title: "Smoke pin", status: "done", after: [], before: [] },
  { id: "opaque-exec", title: "Opaque exec", status: "in-progress", assigned: "agent-7", after: ["smoke-pin"], before: [] },
  { id: "demo", title: "Demo", status: "open", after: [], before: [] },
  { id: "gated", title: "Gated", status: "open", after: ["opaque-exec"], before: [] },
];

const FIXTURE_AGENTS_RAW = [
  {
    id: "agent-7",
    task_id: "opaque-exec",
    executor: "pi",
    model: "pi:openrouter:anthropic/claude-opus-4-7",
    pid: 4242,
    status: "working",
    uptime: "12m",
    started_at: "2026-02-02T09:00:00Z",
    process_alive: true,
    usage: { input_tokens: 12000, output_tokens: 300, total_tokens: 12300, cost_usd: 0.4, turn_count: 14, tool_uses: 31 },
  },
  {
    id: "agent-1",
    task_id: "smoke-pin",
    executor: "claude",
    model: null,
    pid: 1000,
    status: "dead",
    uptime: "2d",
    started_at: "2026-02-01T10:00:00Z",
    process_alive: false,
  },
  {
    id: "agent-9",
    task_id: "demo",
    executor: "codex",
    pid: 9000,
    status: "idle",
    uptime: "5m",
    started_at: "2026-02-02T09:07:00Z",
    process_alive: true,
  },
];

function snapshot() {
  return {
    agents: FIXTURE_AGENTS_RAW.map(normalizeAgent).filter(Boolean),
    tasks: FIXTURE_TASKS,
  };
}

// ── fixture daemon socket ────────────────────────────────────────────────────

let server: Server;
let socketPath: string;
const receivedRequests: Array<Record<string, unknown>> = [];

function startFixtureSocket(path: string): Promise<void> {
  return new Promise((resolve) => {
    server = createServer((stream: Socket) => {
      let buffer = "";
      stream.on("data", (chunk: Buffer) => {
        buffer += chunk.toString("utf8");
        const idx = buffer.indexOf("\n");
        if (idx < 0) return;
        const line = buffer.slice(0, idx).trim();
        if (!line) return;
        const req = JSON.parse(line) as Record<string, unknown>;
        receivedRequests.push(req);
        let response: unknown;
        if (req.cmd === "viz_snapshot") response = { ok: true, tasks: FIXTURE_TASKS };
        else if (req.cmd === "agents") response = { ok: true, agents: FIXTURE_AGENTS_RAW };
        else response = { ok: false, error: `unexpected cmd ${String(req.cmd)}` };
        stream.end(`${JSON.stringify(response)}\n`);
      });
    });
    server.listen(path, () => resolve());
  });
}

beforeAll(async () => {
  socketPath = join(mkdtempSync(join(tmpdir(), "wg-fleet-test-")), "daemon.sock");
  await startFixtureSocket(socketPath);
});

afterAll(() => {
  server?.close();
});

// ── read model: glyphs / colours ─────────────────────────────────────────────

describe("fleet read model — glyph + colour mapping", () => {
  it("maps agent statuses to WG-TUI-consistent glyphs", () => {
    expect(agentGlyph("working", true)).toBe("●");
    expect(agentGlyph("starting", true)).toBe("◌");
    expect(agentGlyph("idle", true)).toBe("○");
    expect(agentGlyph("parked", true)).toBe("⏸");
    expect(agentGlyph("failed")).toBe("✗");
    expect(agentGlyph("dead")).toBe("⨯");
    // A dead process overrides a nominally-live registry status.
    expect(agentGlyph("working", false)).toBe("⨯");
  });

  it("reuses the viz task glyph vocabulary (no drift)", () => {
    expect(taskGlyph("in-progress")).toBe("●");
    expect(taskGlyph("done")).toBe("✓");
    expect(taskGlyph("blocked")).toBe("⏸");
    expect(taskGlyph("failed")).toBe("✗");
    expect(taskGlyph("open")).toBe("○");
  });

  it("maps agent statuses to the WG TUI colours (green ok, red bad, yellow busy)", () => {
    expect(agentColor("working")).toBe("success");
    expect(agentColor("failed")).toBe("error");
    expect(agentColor("dead")).toBe("error");
    expect(agentColor("working", false)).toBe("error");
    expect(agentColor("parked")).toBe("warning");
  });

  // Colour-role parity with the TUI status palette
  // (`src/tui/viz_viewer/state.rs::flash_color_for_status`, mirrored by
  // `src/html.rs::status_color`): done=green, in-progress=cyan/blue,
  // failed=red, abandoned=purple, open/blocked yellow-orange.
  it("maps every task status to its WG TUI palette role", () => {
    expect(taskColor("done")).toBe("success"); // green 80,220,100
    expect(taskColor("in-progress")).toBe("borderAccent"); // cyan 60,200,220
    expect(taskColor("failed")).toBe("error"); // red 220,60,60
    expect(taskColor("abandoned")).toBe("customMessageLabel"); // purple 140,100,160
    expect(taskColor("open")).toBe("warning"); // yellow 200,200,80
    expect(taskColor("blocked")).toBe("warning"); // orange 180,120,60
    expect(taskColor("waiting")).toBe("border"); // blue 60,160,220
    expect(taskColor("pending-validation")).toBe("border");
    expect(taskColor("pending-eval")).toBe("success"); // chartreuse 140,230,80
    expect(taskColor("failed-pending-eval")).toBe("error"); // coral 210,130,70
    expect(taskColor("incomplete")).toBe("warning");
    expect(taskColor("something-unknown")).toBe("dim");
  });

  it("formats an agent row with id, task, model and elapsed", () => {
    const line = agentLine({
      id: "agent-7",
      taskId: "opaque-exec",
      model: "pi:openrouter:anthropic/claude-opus-4-7",
      status: "working",
      uptime: "12m",
    });
    expect(line.text).toBe("● agent-7 · opaque-exec · pi:openrouter:anthropic/claude-opus-4-7 · 12m");
    expect(line.color).toBe("success");
  });

  it("appends an abbreviated usage segment when usage is present", () => {
    const line = agentLine({
      id: "agent-7",
      taskId: "opaque-exec",
      model: "pi:openrouter:anthropic/claude-opus-4-7",
      status: "working",
      uptime: "12m",
      usage: { totalTokens: 12345, turnCount: 14, toolUses: 31 },
    });
    expect(line.text).toBe(
      "● agent-7 · opaque-exec · pi:openrouter:anthropic/claude-opus-4-7 · 12m · 12.3k tok · 14 turns · 31 tools",
    );
  });

  it("degrades to route + elapsed when usage is missing or all-zero", () => {
    const base = { id: "agent-7", taskId: "t", model: "m", status: "working", uptime: "12m" };
    // No usage field.
    expect(agentLine(base).text).toBe("● agent-7 · t · m · 12m");
    // Explicit null.
    expect(agentLine({ ...base, usage: null }).text).toBe("● agent-7 · t · m · 12m");
    // A zero-ish usage object yields no segment (never a fake `0 tok`).
    expect(agentLine({ ...base, usage: { totalTokens: 0, turnCount: 0, toolUses: 0 } }).text).toBe(
      "● agent-7 · t · m · 12m",
    );
  });

  it("abbreviates counts (k/M) and pluralizes turns/tools", () => {
    expect(compactCount(950)).toBe("950");
    expect(compactCount(12345)).toBe("12.3k");
    expect(compactCount(1_250_000)).toBe("1.3M");
    expect(compactCount(0)).toBeNull();
    expect(compactCount(undefined)).toBeNull();
    expect(agentUsageCompact({ totalTokens: 1, turnCount: 1, toolUses: 1 })).toBe("1 tok · 1 turn · 1 tool");
    expect(agentUsageCompact(null)).toBeNull();
  });

  it("renders full, un-abbreviated counts for the detail view", () => {
    expect(agentUsageFull({ totalTokens: 1234567, inputTokens: 1200000, outputTokens: 34567, turnCount: 14, toolUses: 31, costUsd: 0.42 })).toBe(
      "1234567 tokens · (1200000 in / 34567 out) · 14 turns · 31 tool uses · $0.42",
    );
    expect(agentUsageFull(null)).toBeNull();
    expect(agentUsageFull({ totalTokens: 0 })).toBeNull();
  });

  it("falls back to executor, then '?', and ages from startedAt", () => {
    expect(agentModel({ id: "a", taskId: "t", status: "idle", executor: "codex" })).toBe("codex");
    expect(agentModel({ id: "a", taskId: "t", status: "idle" })).toBe("?");
    const now = Date.parse("2026-02-02T09:12:00Z");
    expect(agentElapsed({ id: "a", taskId: "t", status: "working", startedAt: "2026-02-02T09:00:00Z" }, now)).toBe("12m");
  });

  it("counts live agents and task buckets", () => {
    const counts = fleetCounts(snapshot().agents, FIXTURE_TASKS);
    // agent-7 working + agent-9 idle are live; agent-1 is dead.
    expect(counts.active).toBe(2);
    expect(counts.inProgress).toBe(1);
    expect(counts.ready).toBe(1); // demo (no blockers)
    expect(counts.blocked).toBe(1); // gated (opaque-exec not terminal)
    expect(counts.done).toBe(1);
    expect(fleetHeaderLine(counts)).toContain("wg fleet · 2 active");
    expect(fleetHeaderLine(counts)).toContain("1 in-progress");
  });

  it("isAgentAlive prefers the explicit flag over the status", () => {
    expect(isAgentAlive({ id: "a", taskId: "t", status: "working", alive: false })).toBe(false);
    expect(isAgentAlive({ id: "a", taskId: "t", status: "dead", alive: true })).toBe(true);
  });
});

// ── render: compact by default, expandable ──────────────────────────────────

describe("renderFleetLines — compact/expand", () => {
  it("compact mode shows the header, up to N live rows and a hint", () => {
    const lines = renderFleetLines(snapshot(), { maxCompactAgents: 1 });
    expect(lines[0].text).toContain("wg fleet ·");
    expect(lines[1].text).toContain("agent-7");
    expect(lines.at(-1)!.text).toContain("/wg-fleet to expand");
    // Expanded rows beyond the cap are summarised, not dropped silently.
    expect(lines.some((l) => l.text.includes("+1 more"))).toBe(true);
  });

  it("expanded mode lists every live agent", () => {
    const lines = renderFleetLines(snapshot(), { expanded: true });
    expect(lines.some((l) => l.text.includes("agent-7"))).toBe(true);
    expect(lines.some((l) => l.text.includes("agent-9"))).toBe(true);
    expect(lines.some((l) => l.text.includes("agent-1"))).toBe(false); // dead
    expect(lines.at(-1)!.text).toContain("/wg-fleet to collapse");
  });
});

// ── config gate ──────────────────────────────────────────────────────────────

describe("fleet config gate", () => {
  it("is disabled by default", () => {
    expect(DEFAULT_FLEET_VIEW_CONFIG.enabled).toBe(false);
    expect(DEFAULT_FLEET_VIEW_CONFIG.placement).toBe("belowEditor");
    expect(parseFleetViewConfig({}).enabled).toBe(false);
    expect(parseFleetViewConfig({ fleetView: true }).enabled).toBe(true);
    expect(parseFleetViewConfig({ fleetView: "yes" }).enabled).toBe(true);
    expect(parseFleetViewConfig(null).enabled).toBe(false);
  });

  it("reads the pi-subagents-style extension config file (opt-in)", () => {
    const home = mkdtempSync(join(tmpdir(), "wg-fleet-cfg-"));
    const cfgDir = join(home, "extensions", "pi-worksgood");
    mkdirSync(cfgDir, { recursive: true });
    writeFileSync(join(cfgDir, "config.json"), JSON.stringify({ fleetView: true, fleetViewExpanded: true }));
    const config = readFleetViewConfig({ PI_CODING_AGENT_DIR: home });
    expect(config.enabled).toBe(true);
    expect(config.expanded).toBe(true);
    expect(config.placement).toBe("belowEditor");
  });

  it("degrades to disabled when no config file exists and honours the env override", () => {
    const home = mkdtempSync(join(tmpdir(), "wg-fleet-cfg-empty-"));
    expect(readFleetViewConfig({ PI_CODING_AGENT_DIR: home }).enabled).toBe(false);
    expect(readFleetViewConfig({ PI_CODING_AGENT_DIR: home, WG_PI_FLEET_VIEW: "1" }).enabled).toBe(true);
  });
});

// ── read-only data path (fixture daemon socket) ─────────────────────────────

describe("fleet snapshot — read-only daemon path", () => {
  it("issues only the read-only viz_snapshot + agents requests", async () => {
    receivedRequests.length = 0;
    const snap = await fetchFleetOverSocket(socketPath, { timeoutMs: 2000 });
    expect(snap.agents.map((a: { id: string }) => a.id).sort()).toEqual(["agent-1", "agent-7", "agent-9"]);
    expect(snap.tasks.length).toBe(FIXTURE_TASKS.length);
    // Usage flows end-to-end from the `agents` lane into the rendered row.
    const working = snap.agents.find((a) => a.id === "agent-7")!;
    expect(working.usage?.totalTokens).toBe(12300);
    expect(agentLine(working, Date.parse("2026-02-02T09:12:00Z")).text).toContain("12.3k tok · 14 turns · 31 tools");
    const cmds = receivedRequests.map((r) => r.cmd).sort();
    expect(cmds).toEqual(["agents", "viz_snapshot"]);
  });

  it("normalizes a registry row, keeping model, liveness and usage", () => {
    const agent = normalizeAgent(FIXTURE_AGENTS_RAW[0])!;
    expect(agent.model).toBe("pi:openrouter:anthropic/claude-opus-4-7");
    expect(agent.status).toBe("working");
    expect(agent.alive).toBe(true);
    expect(agent.usage).toEqual({ inputTokens: 12000, outputTokens: 300, totalTokens: 12300, costUsd: 0.4, turnCount: 14, toolUses: 31 });
    // A row with no usage normalizes to null (no fake segment).
    expect(normalizeAgent(FIXTURE_AGENTS_RAW[1])!.usage).toBeNull();
    expect(normalizeAgent({ nope: true })).toBeNull();
  });

  it("falls back to the read-only CLI when the socket is unavailable", async () => {
    const run = vi.fn(async (args: string[]) => {
      if (args.includes("agents")) {
        return { stdout: JSON.stringify(FIXTURE_AGENTS_RAW), stderr: "", code: 0, killed: false };
      }
      return { stdout: JSON.stringify(FIXTURE_TASKS), stderr: "", code: 0, killed: false };
    });
    const snap = await fleetSnapshotWithFallback({ run } as never, { daemonSocket: "/nonexistent/fleet.sock", dir: "/nowhere" }, {});
    expect(snap).not.toBeNull();
    expect(snap!.agents.length).toBe(3);
    expect(run.mock.calls.map((c) => (c[0] as string[])[0]).sort()).toEqual(["agents", "list"]);
  });

  it("FleetPoller is bounded and change-guarded", async () => {
    let fetches = 0;
    const seen: number[] = [];
    const poller = new FleetPoller(
      async () => {
        fetches++;
        await new Promise((r) => setTimeout(r, 20));
        return snapshot();
      },
      (s) => seen.push(s ? s.agents.length : -1),
      5,
    );
    poller.start();
    await new Promise((r) => setTimeout(r, 60));
    poller.stop();
    // Change guard: identical snapshots are delivered exactly once.
    expect(seen).toEqual([3]);
  });
});

// ── installation: config-gated below-editor widget ──────────────────────────

describe("installFleetView", () => {
  function makePi(mode: "tui" | "rpc" | "json" | "print" = "tui") {
    const handlers = new Map<string, (event: unknown, ctx: unknown) => void>();
    const commands = new Map<string, { handler: (args: string, ctx: unknown) => Promise<void> }>();
    const ui = { setWidget: vi.fn(), notify: vi.fn(), theme: undefined };
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
    run: vi.fn(async (args: string[]) => {
      if (args.includes("agents")) {
        return { stdout: JSON.stringify(FIXTURE_AGENTS_RAW), stderr: "", code: 0, killed: false };
      }
      return { stdout: JSON.stringify(FIXTURE_TASKS), stderr: "", code: 0, killed: false };
    }),
  };

  it("registers the /wg-fleet command and the session lifecycle", () => {
    const { pi, commands } = makePi();
    installFleetView(pi as never, backend as never, {}, { config: { enabled: true } });
    expect(commands.has("wg-fleet")).toBe(true);
    expect(pi.on).toHaveBeenCalledWith("session_start", expect.any(Function));
    expect(pi.on).toHaveBeenCalledWith("session_shutdown", expect.any(Function));
  });

  it("mounts NOTHING when disabled", async () => {
    vi.useFakeTimers();
    try {
      const { pi, handlers, ui, ctx } = makePi("tui");
      installFleetView(pi as never, backend as never, { daemonSocket: "/nonexistent/fleet.sock", dir: "/nowhere" }, {
        config: { enabled: false },
        pollMs: 10,
      });
      handlers.get("session_start")!({}, ctx);
      await vi.advanceTimersByTimeAsync(30);
      expect(ui.setWidget).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("mounts a multi-line widget BELOW the editor when enabled in TUI mode", async () => {
    vi.useFakeTimers();
    try {
      const { pi, handlers, ui, ctx } = makePi("tui");
      const controller = installFleetView(
        pi as never,
        backend as never,
        { daemonSocket: "/nonexistent/fleet.sock", dir: "/nowhere" },
        { config: { enabled: true }, pollMs: 10 },
      );
      handlers.get("session_start")!({}, ctx);
      await vi.advanceTimersByTimeAsync(30);
      expect(ui.setWidget).toHaveBeenCalled();
      const call = ui.setWidget.mock.calls.at(-1)!;
      expect(call[0]).toBe(FLEET_WIDGET_KEY);
      expect(call[2]).toEqual({ placement: "belowEditor" });
      expect(Array.isArray(call[1])).toBe(true);
      expect(call[1]!.length).toBeGreaterThan(1); // multi-line
      expect(String(call[1]![0])).toContain("wg fleet ·");
      expect(controller.isEnabled()).toBe(true);
      handlers.get("session_shutdown")!({}, ctx);
    } finally {
      vi.useRealTimers();
    }
  });

  it("degrades silently in non-TUI modes", async () => {
    vi.useFakeTimers();
    try {
      const { pi, handlers, ui, ctx } = makePi("json");
      installFleetView(pi as never, backend as never, { daemonSocket: socketPath }, {
        config: { enabled: true },
        pollMs: 10,
      });
      handlers.get("session_start")!({}, ctx);
      await vi.advanceTimersByTimeAsync(30);
      expect(ui.setWidget).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("/wg-fleet toggles expand/collapse and explains when disabled", async () => {
    const { pi, commands, ctx } = makePi("tui");
    const controller = installFleetView(pi as never, backend as never, {}, { config: { enabled: false } });
    await commands.get("wg-fleet")!.handler("", ctx);
    expect(ctx.ui.notify).toHaveBeenCalledWith(expect.stringContaining("disabled"), "info");
    expect(controller.isExpanded()).toBe(false);

    const enabled = makePi("tui");
    const on = installFleetView(enabled.pi as never, backend as never, {}, { config: { enabled: true } });
    await enabled.commands.get("wg-fleet")!.handler("expand", enabled.ctx);
    expect(on.isExpanded()).toBe(true);
    await enabled.commands.get("wg-fleet")!.handler("collapse", enabled.ctx);
    expect(on.isExpanded()).toBe(false);
    await enabled.commands.get("wg-fleet")!.handler("toggle", enabled.ctx);
    expect(on.isExpanded()).toBe(true);
  });
});
