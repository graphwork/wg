/**
 * fleet-strip tests — the CLICKABLE ambient fleet strip.
 *
 * Pins the pi-subagents `asyncWidgetCollapsed` interaction the operator asked
 * for ("clickable … to open it, to navigate, to investigate"):
 *
 *   - left-click on the header row folds to ONE line, click again restores the
 *     usual layout, and folding PERSISTS across progress updates (the mounted
 *     component is mutated, never re-registered);
 *   - a click on an agent row opens the cockpit selected on that task;
 *   - a DORMANT strip (`WG` present via `.wg/`, not booted) is a one-line hint
 *     whose click opens the read-only `inactive` view — never the live cockpit;
 *   - modifier clicks / drag / wheel / press / blank rows are LEFT UNHANDLED;
 *   - every mouse return is pi's `PiMouseResult` OBJECT shape (never the bare
 *     boolean that crashes pi core);
 *   - the mount is activation-gated: live strip when active, hint strip when
 *     dormant-but-`.wg/`, nothing at all otherwise.
 *
 * Tests run against the built `pi-worksgood/` artifact — `npm test` builds first.
 */

import { describe, expect, it, vi } from "vitest";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { FleetStripComponent, STRIP_DORMANT_HINT, dormantStripLine, stripSummaryLine } from "../pi-worksgood/index.js";
// @ts-expect-error — built ESM artifact import (see above)
import { installFleetView } from "../pi-worksgood/fleet-view.js";
// @ts-expect-error — built ESM artifact import (see above)
import { isPiMouseResult } from "../pi-worksgood/mouse.js";

// ── fixtures ─────────────────────────────────────────────────────────────────

const TASKS = [
  { id: "task-a", title: "Task A", status: "in-progress", after: [], before: [] },
  { id: "task-b", title: "Task B", status: "open", after: [], before: [] },
  { id: "task-c", title: "Task C", status: "blocked", after: ["task-a"], before: [] },
  { id: "task-d", title: "Task D", status: "done", after: [], before: [] },
];

const AGENTS_RAW = [
  { id: "agent-7", task_id: "task-a", executor: "pi", model: "pi:openrouter:x/y", status: "working", uptime: "3m", process_alive: true },
  { id: "agent-9", task_id: "task-b", executor: "codex", status: "idle", uptime: "1m", process_alive: true },
];

function snapshot() {
  return {
    agents: AGENTS_RAW.map((a) => ({
      id: a.id,
      taskId: a.task_id,
      executor: a.executor,
      model: a.model ?? null,
      status: a.status,
      alive: a.process_alive,
      uptime: a.uptime,
      usage: null,
    })),
    tasks: TASKS,
  };
}

/** A left click on `y` (pi's normalized event shape). */
function click(y: number, extra: Record<string, unknown> = {}) {
  return { type: "click", button: "left", y, ...extra };
}

// ── pure helpers ─────────────────────────────────────────────────────────────

describe("strip summary lines", () => {
  it("puts the click affordance early so a narrow terminal cannot hide it", () => {
    const folded = stripSummaryLine(snapshot());
    expect(folded.startsWith("▸ wg fleet (click)")).toBe(true);
    expect(folded).toContain("2 active");
    expect(folded).toContain("1 in-progress");
    expect(folded).toContain("1 ready");
    expect(folded).toContain("1 blocked");
    // Offline: still one honest line, never a fabricated count.
    expect(stripSummaryLine(null)).toContain("awaiting the WG daemon");
  });

  it("states the dormant hint AND the activation command", () => {
    const line = dormantStripLine();
    expect(line).toContain("dormant");
    expect(line).toContain(STRIP_DORMANT_HINT);
    expect(line).toContain("/wg on to activate");
    expect(line).toContain("click to inspect");
  });
});

// ── click model: fold ⇄ unfold ───────────────────────────────────────────────

describe("click fold/unfold (pi-subagents asyncWidgetCollapsed semantics)", () => {
  it("folds to one line and restores the usual layout on the next click", () => {
    const strip = new FleetStripComponent({ collapsed: false, mode: "live", maxCompactAgents: 3 });
    strip.setSnapshot(snapshot());
    const unfolded = strip.render(120);
    expect(unfolded.length).toBeGreaterThan(1);
    expect(unfolded[0]).toContain("click to fold");
    expect(unfolded[0]).toContain("wg fleet");
    expect(unfolded.join("\n")).toContain("agent-7");

    const result = strip.handleMouse(click(0));
    expect(result).toEqual({ handled: true }); // object shape, never a bare boolean
    const folded = strip.render(120);
    expect(folded).toHaveLength(1);
    expect(folded[0]).toContain("(click)");
    expect(folded[0]).toContain("in-progress");
    expect(strip.isCollapsed).toBe(true);

    expect(strip.handleMouse(click(0))).toEqual({ handled: true });
    expect(strip.isCollapsed).toBe(false);
    expect(strip.render(120).length).toBeGreaterThan(1);
  });

  it("starts folded from config and stays folded across progress updates", () => {
    const strip = new FleetStripComponent({ collapsed: true, mode: "live" });
    expect(strip.render(120)).toHaveLength(1);
    // A poll arrives: the strip must NOT silently unfold.
    strip.setSnapshot(snapshot());
    expect(strip.render(120)).toHaveLength(1);
    strip.setSnapshot(null);
    expect(strip.render(120)).toHaveLength(1);
    // Unfold, then another poll: the operator's unfold must stick too.
    strip.setSnapshot(snapshot());
    strip.handleMouse(click(0));
    expect(strip.render(120).length).toBeGreaterThan(1);
    strip.setSnapshot(snapshot());
    expect(strip.render(120).length).toBeGreaterThan(1);
  });

  it("reports the toggle through onToggle", () => {
    const onToggle = vi.fn();
    const strip = new FleetStripComponent({ collapsed: true, mode: "live", onToggle });
    strip.setSnapshot(snapshot());
    strip.handleMouse(click(0));
    expect(onToggle).toHaveBeenCalledWith(false);
    strip.handleMouse(click(0));
    expect(onToggle).toHaveBeenCalledWith(true);
  });
});

// ── click model: open the cockpit on a row ───────────────────────────────────

describe("click a row to investigate", () => {
  it("opens the panel selected on the clicked agent's task", () => {
    const onOpenPanel = vi.fn();
    const strip = new FleetStripComponent({ collapsed: false, mode: "live", onOpenPanel });
    strip.setSnapshot(snapshot());
    const rows = strip.rows();
    const agentRow = rows.findIndex((row) => row.kind === "agent" && row.taskId === "task-a");
    expect(agentRow).toBeGreaterThan(0);
    expect(strip.handleMouse(click(agentRow))).toEqual({ handled: true });
    expect(onOpenPanel).toHaveBeenCalledWith("task-a");
  });

  it("leaves chrome rows (the `+N more` / empty captions) unhandled", () => {
    const onOpenPanel = vi.fn();
    // Four live agents in a 1-row compact body → a truncation caption row.
    const many = {
      agents: ["a1", "a2", "a3"].map((id) => ({
        id,
        taskId: `t-${id}`,
        status: "working",
        alive: true,
        uptime: "1m",
        model: null,
        executor: null,
        usage: null,
      })),
      tasks: TASKS,
    };
    const strip = new FleetStripComponent({
      collapsed: false,
      mode: "live",
      maxCompactAgents: 1,
      onOpenPanel,
    });
    strip.setSnapshot(many);
    const rows = strip.rows();
    expect(rows.length).toBeGreaterThan(2);
    const chromeRow = rows.findIndex((row) => row.kind === "chrome");
    expect(chromeRow).toBeGreaterThan(0);
    expect(strip.handleMouse(click(chromeRow))).toBeUndefined();
    expect(onOpenPanel).not.toHaveBeenCalled();
  });
});

// ── click model: everything else is NOT ours ─────────────────────────────────

describe("unhandled events stay unhandled", () => {
  it("ignores drag/wheel/press/release/move (the host keeps its behaviour)", () => {
    const strip = new FleetStripComponent({ collapsed: true, mode: "live" });
    strip.setSnapshot(snapshot());
    for (const type of ["press", "release", "drag", "move", "wheel", "mouse.press", "mouse.wheel"]) {
      expect(strip.handleMouse({ type, button: "left", y: 0 })).toBeUndefined();
    }
    expect(strip.isCollapsed).toBe(true); // no accidental fold
  });

  it("ignores modifier clicks, non-left buttons and out-of-range rows", () => {
    const onOpenPanel = vi.fn();
    const strip = new FleetStripComponent({ collapsed: false, mode: "live", onOpenPanel });
    strip.setSnapshot(snapshot());
    expect(strip.handleMouse(click(0, { shift: true }))).toBeUndefined();
    expect(strip.handleMouse(click(0, { alt: true }))).toBeUndefined();
    expect(strip.handleMouse(click(0, { ctrl: true }))).toBeUndefined();
    expect(strip.handleMouse(click(0, { button: "right" }))).toBeUndefined();
    expect(strip.handleMouse({ type: "click", button: "left" })).toBeUndefined(); // no y
    expect(strip.handleMouse(click(999))).toBeUndefined(); // past the last row
    expect(onOpenPanel).not.toHaveBeenCalled();

    // Folded: there is nothing below the header to click.
    strip.handleMouse(click(0));
    expect(strip.isCollapsed).toBe(true);
    expect(strip.handleMouse(click(1))).toBeUndefined();
  });
});

// ── dormant strip ────────────────────────────────────────────────────────────

describe("dormant strip (WG present, not booted)", () => {
  it("is ONE hint line whose click opens the read-only view", () => {
    const onOpenInactive = vi.fn();
    const strip = new FleetStripComponent({ mode: "inactive", onOpenInactive });
    const lines = strip.render(120);
    expect(lines).toHaveLength(1);
    expect(lines[0]).toContain("dormant");
    expect(lines[0]).toContain("/wg on to activate");
    // A snapshot must NOT leak live cockpit content into the dormant strip.
    strip.setSnapshot(snapshot());
    expect(strip.render(120)).toHaveLength(1);
    expect(strip.render(120)[0]).not.toContain("agent-7");

    expect(strip.handleMouse(click(0))).toEqual({ handled: true });
    expect(onOpenInactive).toHaveBeenCalledTimes(1);
    // Only the one line is a target.
    expect(strip.handleMouse(click(1))).toBeUndefined();
    expect(onOpenInactive).toHaveBeenCalledTimes(1);
  });
});

// ── mouse contract: object shape, never a bare boolean ───────────────────────

describe("mouse contract", () => {
  it("every return is pi's object shape or falsy (never a truthy primitive)", () => {
    const strip = new FleetStripComponent({
      collapsed: false,
      mode: "live",
      onOpenPanel: () => {},
    });
    strip.setSnapshot(snapshot());
    const events: Array<Record<string, unknown>> = [
      { type: "click", button: "left", y: 0 },
      { type: "click", button: "left", y: 1 },
      { type: "click", button: "left", y: 999 },
      { type: "click", button: "right", y: 0 },
      { type: "click", button: "left", y: 0, shift: true },
      { type: "press", button: "left", y: 0 },
      { type: "release", button: "left", y: 0 },
      { type: "drag", button: "left", y: 3 },
      { type: "wheel", button: "none", y: 2, wheelDelta: 3 },
      { type: "move", button: "none", y: 2 },
      {},
    ];
    for (const event of events) {
      const result = strip.handleMouse(event);
      expect(isPiMouseResult(result)).toBe(true);
      if (result) expect(typeof result).toBe("object");
    }
    // The dormant strip obeys the same contract.
    const dormant = new FleetStripComponent({ mode: "inactive" });
    for (const event of events) {
      const result = dormant.handleMouse(event);
      expect(isPiMouseResult(result)).toBe(true);
      if (result) expect(typeof result).toBe("object");
    }
  });

  /**
   * The exact pi-tui `dispatchMouseEvent` shape check that crashed pi core on a
   * bare `true`: feeding every strip return through it must never throw
   * `TypeError: Cannot use 'in' operator to search for 'target' in true`.
   */
  it("survives pi's real dispatchMouseEvent `in`-operator guard", () => {
    const strip = new FleetStripComponent({ mode: "live" });
    strip.setSnapshot(snapshot());
    const dispatch = (component: { handleMouse?: (e: unknown) => unknown }, event: Record<string, unknown>) => {
      const result = component.handleMouse?.(event) as Record<string, unknown> | undefined;
      if (!result) return undefined;
      if ("target" in result) return result;
      if (!result.handled && !result.capture && !result.focus) return undefined;
      return { ...result, handled: true, target: { component } };
    };
    expect(() => dispatch(strip, click(0))).not.toThrow();
    expect(() => dispatch(strip, { type: "press", button: "left", y: 4 })).not.toThrow();
  });
});

// ── installation: activation-gated strip ────────────────────────────────────

const FAKE_TUI = { requestRender: vi.fn(), terminal: { rows: 24, columns: 100 } };

const BACKEND = {
  run: vi.fn(async (args: string[]) => {
    const joined = Array.isArray(args) ? args.join(" ") : "";
    if (joined.includes("agents")) {
      return { stdout: JSON.stringify(AGENTS_RAW), stderr: "", code: 0, killed: false };
    }
    return { stdout: JSON.stringify(TASKS), stderr: "", code: 0, killed: false };
  }),
};

function makePi(mode: "tui" | "json" = "tui") {
  const handlers = new Map<string, (event: unknown, ctx: unknown) => void>();
  const commands = new Map<string, { handler: (args: string, ctx: unknown) => Promise<void> }>();
  let customRendered: string[] | null = null;
  let customFactory: ((tui: unknown, theme: unknown, kb: unknown, done: () => void) => unknown) | null = null;
  const ui = {
    setWidget: vi.fn(),
    notify: vi.fn(),
    theme: undefined,
    custom: vi.fn(async (factory: typeof customFactory) => {
      customFactory = factory;
      const component = (await factory!(FAKE_TUI, undefined, undefined, () => {})) as {
        render: (width: number) => string[];
        handleInput?: (data: string) => void;
      };
      customRendered = component.render(100);
      return undefined;
    }),
  };
  const pi = {
    registerCommand: vi.fn((name: string, def: { handler: (args: string, ctx: unknown) => Promise<void> }) =>
      commands.set(name, def),
    ),
    on: vi.fn((event: string, handler: (event: unknown, ctx: unknown) => void) => handlers.set(event, handler)),
    registerTool: vi.fn(),
    registerProvider: vi.fn(),
  };
  const ctx = { mode, hasUI: mode === "tui", ui, signal: undefined, cwd: "/tmp/proj" };
  return {
    pi,
    handlers,
    commands,
    ui,
    ctx,
    cockpit: () => customRendered,
    factory: () => customFactory,
  };
}

/** The mounted widget factory's rendered lines (component form). */
function mountedLines(ui: { setWidget: ReturnType<typeof vi.fn> }): string[] {
  const call = ui.setWidget.mock.calls.at(-1);
  expect(call).toBeTruthy();
  expect(typeof call![1]).toBe("function");
  const component = (call![1] as (tui: unknown, theme: unknown) => { render: (w: number) => string[] })(
    FAKE_TUI,
    undefined,
  );
  return component.render(100);
}

function mountedComponent(ui: { setWidget: ReturnType<typeof vi.fn> }): {
  render: (w: number) => string[];
  handleMouse: (event: unknown) => unknown;
  isCollapsed: boolean;
} {
  const call = ui.setWidget.mock.calls.at(-1);
  return (call![1] as (tui: unknown, theme: unknown) => never)(FAKE_TUI, undefined) as never;
}

const settle = async () => {
  for (let i = 0; i < 20; i++) await new Promise((r) => setTimeout(r, 5));
};

describe("installFleetView — activation-gated clickable strip", () => {
  it("mounts the LIVE one-line strip when the session is active", async () => {
    const h = makePi();
    const controller = installFleetView(h.pi as never, BACKEND as never, { daemonSocket: "/nonexistent.sock" }, {
      config: { enabled: true },
      pollMs: 1000,
      isActive: () => true,
      hasWgDir: () => false,
    });
    h.handlers.get("session_start")!({}, h.ctx);
    await settle();
    expect(controller.mode()).toBe("live");
    const lines = mountedLines(h.ui);
    expect(lines).toHaveLength(1);
    expect(lines[0]).toContain("wg fleet (click)");
    expect(lines[0]).toContain("in-progress");
    h.handlers.get("session_shutdown")!({}, h.ctx);
  });

  it("mounts the DORMANT hint strip when WG is not booted but the project has .wg/", async () => {
    BACKEND.run.mockClear();
    const h = makePi();
    const controller = installFleetView(h.pi as never, BACKEND as never, {}, {
      config: { enabled: true },
      pollMs: 1000,
      isActive: () => false,
      isProjectActive: () => false,
      projectRoot: () => "/tmp/proj",
      hasWgDir: () => true,
    });
    h.handlers.get("session_start")!({}, h.ctx);
    await settle();
    expect(controller.mode()).toBe("inactive");
    const lines = mountedLines(h.ui);
    expect(lines).toHaveLength(1);
    expect(lines[0]).toContain("dormant");
    expect(lines[0]).toContain("/wg on to activate");
    // Dormant means NO daemon traffic at all.
    expect(BACKEND.run).not.toHaveBeenCalled();
    h.handlers.get("session_shutdown")!({}, h.ctx);
  });

  it("mounts NOTHING when WG is absent from the project", async () => {
    const h = makePi();
    const controller = installFleetView(h.pi as never, BACKEND as never, {}, {
      config: { enabled: true },
      pollMs: 1000,
      isActive: () => false,
      isProjectActive: () => false,
      hasWgDir: () => false,
    });
    h.handlers.get("session_start")!({}, h.ctx);
    await settle();
    expect(controller.mode()).toBe("off");
    expect(h.ui.setWidget).not.toHaveBeenCalled();
  });

  it("keeps ONE mounted component per surface so a fold survives every refresh", async () => {
    const h = makePi();
    installFleetView(h.pi as never, BACKEND as never, { daemonSocket: "/nonexistent.sock" }, {
      config: { enabled: true },
      pollMs: 5,
      isActive: () => true,
    });
    h.handlers.get("session_start")!({}, h.ctx);
    await settle();
    const mounts = h.ui.setWidget.mock.calls.length;
    const strip = mountedComponent(h.ui);
    expect(strip.render(100)).toHaveLength(1);
    // Click the header row of the MOUNTED component: it unfolds.
    expect(strip.handleMouse(click(0))).toEqual({ handled: true });
    expect(strip.render(100).length).toBeGreaterThan(1);
    // Several poll cycles later the widget is still the same component (no key
    // re-registration) and the unfold is still in effect.
    await settle();
    expect(h.ui.setWidget.mock.calls.length).toBe(mounts);
    expect(mountedComponent(h.ui).isCollapsed).toBe(false);
    expect(mountedComponent(h.ui).render(100).length).toBeGreaterThan(1);
    h.handlers.get("session_shutdown")!({}, h.ctx);
  });

  it("opens the read-only inactive view when the dormant strip is clicked", async () => {
    const h = makePi();
    const controller = installFleetView(h.pi as never, BACKEND as never, { daemonSocket: "/nonexistent.sock" }, {
      config: { enabled: true },
      pollMs: 1000,
      isActive: () => false,
      isProjectActive: () => false,
      projectRoot: () => "/tmp/proj",
      hasWgDir: () => true,
    });
    h.handlers.get("session_start")!({}, h.ctx);
    await settle();
    expect(controller.mode()).toBe("inactive");
    const strip = mountedComponent(h.ui);
    expect(strip.handleMouse(click(0))).toEqual({ handled: true });
    await settle();
    expect(h.ui.custom).toHaveBeenCalledTimes(1);
    const cockpit = h.cockpit();
    expect(cockpit).toBeTruthy();
    const text = cockpit!.join("\n");
    expect(text).toContain("inactive");
    expect(text).toContain("/wg on to activate");
    // Read-only: NOT the live cockpit (no selection marker, no agent activity).
    expect(text).not.toContain("❯");
    expect(text).not.toContain("agent-7");
    h.handlers.get("session_shutdown")!({}, h.ctx);
  });

  it("/wg on re-syncs to the live strip; /wg off leaves the dormant hint", async () => {
    let active = false;
    const h = makePi();
    const controller = installFleetView(h.pi as never, BACKEND as never, { daemonSocket: "/nonexistent.sock" }, {
      config: { enabled: true },
      pollMs: 1000,
      isActive: () => active,
      isProjectActive: () => false,
      projectRoot: () => "/tmp/proj",
      hasWgDir: () => true,
    });
    h.handlers.get("session_start")!({}, h.ctx);
    await settle();
    expect(controller.mode()).toBe("inactive");
    // /wg on
    active = true;
    await controller.sync(h.ctx);
    await settle();
    expect(controller.mode()).toBe("live");
    expect(mountedLines(h.ui)[0]).toContain("(click)");
    expect(mountedLines(h.ui)[0]).toContain("in-progress");
    // /wg off → still discoverable via the dormant hint.
    active = false;
    await controller.sync(h.ctx);
    await settle();
    expect(controller.mode()).toBe("inactive");
    expect(mountedLines(h.ui)[0]).toContain("/wg on to activate");
    h.handlers.get("session_shutdown")!({}, h.ctx);
  });

  it("honours the fleetView opt-out in every state", async () => {
    const h = makePi();
    const controller = installFleetView(h.pi as never, BACKEND as never, {}, {
      config: { enabled: false },
      isActive: () => true,
      hasWgDir: () => true,
    });
    h.handlers.get("session_start")!({}, h.ctx);
    await settle();
    expect(controller.mode()).toBe("off");
    expect(h.ui.setWidget).not.toHaveBeenCalled();
  });

  it("starts folded from config (fleetViewCollapsed false starts unfolded)", async () => {
    const h = makePi();
    installFleetView(h.pi as never, BACKEND as never, { daemonSocket: "/nonexistent.sock" }, {
      config: { enabled: true, collapsed: false },
      pollMs: 1000,
      isActive: () => true,
    });
    h.handlers.get("session_start")!({}, h.ctx);
    await settle();
    expect(mountedLines(h.ui).length).toBeGreaterThan(1);
    h.handlers.get("session_shutdown")!({}, h.ctx);
  });
});
