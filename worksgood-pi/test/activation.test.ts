/**
 * activation.test.ts — SESSION-SCOPED WG ACTIVATION (dormant by default).
 *
 * The operator problem this pins: the plugin used to register ~13 model-visible
 * `wg_*` tools in EVERY pi session, in EVERY directory, unconditionally, so the
 * agent permanently "perceived" WG whether invited or not — and the earlier
 * off-switches were destructive (they removed the settings entry, which cost the
 * NEXT session its `/wg` command).
 *
 * Everything here runs against the BUILT `pi-worksgood/` artifact (`npm test`
 * builds first) with a fake pi that models the real contract:
 *   - `registerTool` honours `defaultActive` / `exposure` (pi core's
 *     `_isActivatedOnRegistration`);
 *   - `setActiveTools(names)` REPLACES the active set by name;
 *   - `getActiveTools()` is what the model is declared;
 *   - `getAllTools()` is every registered tool with its exposure.
 */

import { mkdtempSync, mkdirSync, existsSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it, expect, vi, afterEach } from "vitest";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import worksgoodPi from "../pi-worksgood/index.js";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import {
  WG_TOOL_NAMES,
  registerWgTools,
} from "../pi-worksgood/tools.js";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import {
  ACTIVATION_MARKER_CONTENT,
  activationMarkerPath,
  activeToolSet,
  dormantToolSet,
  canPersistProjectMarker,
  formatActivationStatus,
  installSessionActivation,
  isProjectMarkedActive,
  partitionToolNames,
  resolveProjectRoot,
  visibleWgToolCount,
  WG_TOOL_NAME_SET,
} from "../pi-worksgood/activation.js";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { formatOnLine, formatOffLine } from "../pi-worksgood/commands.js";

const BASE_TOOLS = ["read", "bash", "edit", "write"];

const cleanups: string[] = [];
afterEach(() => {
  for (const dir of cleanups.splice(0)) rmSync(dir, { recursive: true, force: true });
});

/** A real temp project dir; `withWg: false` leaves `.wg/` absent. */
function project(withWg = true): string {
  const dir = mkdtempSync(join(tmpdir(), "wg-activation-"));
  cleanups.push(dir);
  if (withWg) mkdirSync(join(dir, ".wg"));
  return dir;
}

interface ToolInfo {
  name: string;
  exposure: string;
  active: boolean;
}

/**
 * Fake pi that mirrors the three pi behaviors activation depends on:
 * default-active-at-registration, a name-replacing setActiveTools, and
 * multi-handler events (handlers accumulate per event, in load order).
 */
function makePi(initialTools: string[] = BASE_TOOLS) {
  let active = [...initialTools];
  const tools = new Map<string, { definition: any; exposure: string; defaultActive: boolean }>();
  const handlers = new Map<string, Array<(event: unknown, ctx: unknown) => unknown>>();
  const commands = new Map<string, any>();
  const registered: string[] = [];

  const pi = {
    registerTool: vi.fn((tool: any) => {
      const exposure = tool.exposure ?? "direct";
      const defaultActive = tool.defaultActive !== false;
      tools.set(tool.name, { definition: tool, exposure, defaultActive });
      registered.push(tool.name);
      // pi activates direct/model-only tools on registration unless
      // defaultActive is explicitly false.
      if (defaultActive && (exposure === "direct" || exposure === "model-only") && !active.includes(tool.name)) {
        active.push(tool.name);
      }
    }),
    registerCommand: vi.fn((name: string, def: any) => commands.set(name, def)),
    registerProvider: vi.fn(),
    on: vi.fn((event: string, handler: (e: unknown, c: unknown) => unknown) => {
      const list = handlers.get(event) ?? [];
      list.push(handler);
      handlers.set(event, list);
      return () => {};
    }),
    getActiveTools: vi.fn(() => [...active]),
    getAllTools: vi.fn((): ToolInfo[] =>
      [...tools.entries()].map(([name, t]) => ({
        name,
        exposure: t.exposure,
        active: active.includes(name),
      })),
    ),
    setActiveTools: vi.fn((names: string[]) => {
      // pi ignores unknown / hidden names; every wg tool here is registered.
      active = names.filter((n) => tools.has(n) || BASE_TOOLS.includes(n));
    }),
    setModel: vi.fn(async () => true),
    sendUserMessage: vi.fn(),
    exec: vi.fn(async () => ({ stdout: "", stderr: "", code: 1, killed: false })),
    events: { on: vi.fn(), emit: vi.fn() },
  };

  const emit = async (event: string, ctx: unknown = { mode: "tui", hasUI: true, ui: { notify: vi.fn(), setWidget: vi.fn() }, signal: undefined }) => {
    for (const handler of handlers.get(event) ?? []) await handler({}, ctx);
  };

  return { pi, tools, commands, registered, emit, active: () => [...active] };
}

/** Fire every session_start handler, then let the async work settle. */
async function startSession(
  harness: ReturnType<typeof makePi>,
  ctx?: any,
): Promise<void> {
  await harness.emit("session_start", ctx);
  await new Promise((resolve) => setTimeout(resolve, 0));
}

function wgVisible(harness: ReturnType<typeof makePi>): string[] {
  return harness.active().filter((n) => WG_TOOL_NAME_SET.has(n));
}

const OK = (stdout: string) => ({ stdout, stderr: "", code: 0, killed: false });

function fakeBackend(overrides: Record<string, unknown> = {}): any {
  return {
    statusJson: vi.fn(async () => ({ service: { running: true } })),
    runJson: vi.fn(async () => []),
    piPluginInstall: vi.fn(async () => OK("")),
    piPluginStatus: vi.fn(async () => OK("cache state: current")),
    piPluginDisable: vi.fn(async () => OK("")),
    graphPresent: vi.fn(async () => true),
    ready: vi.fn(async () => OK("")),
    show: vi.fn(async () => OK("")),
    add: vi.fn(async () => OK("")),
    publish: vi.fn(async () => OK("")),
    done: vi.fn(async () => OK("")),
    fail: vi.fn(async () => OK("")),
    ...overrides,
  };
}

/** A harness with the wg tools registered + session activation installed. */
function wired(
  overrides: Record<string, unknown> = {},
  initialTools: string[] = BASE_TOOLS,
) {
  const harness = makePi(initialTools);
  const backend = fakeBackend(overrides);
  registerWgTools(harness.pi as never, backend);
  const activation = installSessionActivation(harness.pi as never, backend, {}, {});
  return { harness, backend, activation };
}

// ── the registration choice ────────────────────────────────────────────────

describe("tool registration: registered but inactive", () => {
  it("registers every wg tool with exposure direct + defaultActive false", () => {
    const harness = makePi();
    registerWgTools(harness.pi as never, fakeBackend());

    expect(harness.registered).toEqual([...WG_TOOL_NAMES]);
    for (const name of WG_TOOL_NAMES) {
      const tool = harness.tools.get(name)!;
      // `hidden` would be unactivatable, so the choice is direct + explicitly
      // not-active-on-registration.
      expect(tool.exposure).toBe("direct");
      expect(tool.defaultActive).toBe(false);
      expect(tool.definition.exposure).toBe("direct");
      expect(tool.definition.defaultActive).toBe(false);
    }
  });

  it("leaves ZERO wg tools in the active set at load", () => {
    const harness = makePi();
    registerWgTools(harness.pi as never, fakeBackend());
    expect(wgVisible(harness)).toEqual([]);
    expect(harness.active()).toEqual(BASE_TOOLS);
    // …but they are all still REGISTERED (reachable again by name).
    expect(harness.pi.getAllTools().filter((t: { name: string }) => t.name.startsWith("wg_"))).toHaveLength(
      WG_TOOL_NAMES.length,
    );
  });
});

// ── a fresh session in any directory: dormant ──────────────────────────────

describe("load time: dormant by default in every directory", () => {
  it("a fresh session in an unbooted project shows ZERO wg tools and keeps /wg available", async () => {
    const dir = project();
    const harness = makePi();
    worksgoodPi(harness.pi as never);
    expect(harness.registered).toHaveLength(WG_TOOL_NAMES.length);
    expect(wgVisible(harness)).toEqual([]);

    await startSession(harness, {
      mode: "tui",
      hasUI: true,
      cwd: dir,
      ui: { notify: vi.fn(), setWidget: vi.fn() },
      signal: undefined,
    });

    expect(wgVisible(harness)).toEqual([]);
    // The affordances are commands, not tools: they exist in every state.
    expect(harness.commands.has("wg")).toBe(true);
    expect(harness.commands.has("wg-model")).toBe(true);
    expect(harness.commands.has("wg-fleet")).toBe(true);
    // Nothing was persisted for a project we never booted.
    expect(existsSync(activationMarkerPath(dir))).toBe(false);
  });

  it("strips wg tools inherited from an --tools/defaultTools style active set", async () => {
    const dir = project();
    const { harness, activation } = wired({}, [...BASE_TOOLS, "wg_ready", "wg_done"]);
    expect(wgVisible(harness)).toEqual(["wg_ready", "wg_done"]);

    await activation.applyForSession({ cwd: dir } as never);
    expect(wgVisible(harness)).toEqual([]);
    expect(activation.isActive()).toBe(false);
  });
});

// ── /wg on: activate exactly the wg set ────────────────────────────────────

describe("/wg on", () => {
  it("activates exactly the wg tool family mid-session", async () => {
    const dir = project();
    const { harness, activation } = wired();
    await activation.applyForSession({ cwd: dir } as never);

    const outcome = await activation.activate({ cwd: dir } as never);

    expect(activation.isActive()).toBe(true);
    expect(wgVisible(harness).sort()).toEqual([...WG_TOOL_NAMES].sort());
    // Exactly the wg set was ADDED — the built-in tools survive the toggle.
    expect(harness.active()).toEqual(expect.arrayContaining(BASE_TOOLS));
    expect(harness.active()).toHaveLength(BASE_TOOLS.length + WG_TOOL_NAMES.length);
    expect(outcome.visibleTools).toBe(WG_TOOL_NAMES.length);
    // Step 1 set the dormant baseline, step 2 added the wg family.
    expect(harness.pi.setActiveTools).toHaveBeenCalledTimes(2);
  });

  it("writes the per-project marker and confirms in one line", async () => {
    const dir = project();
    const { harness, activation } = wired();
    const outcome = await activation.activate({ cwd: dir } as never);

    const marker = activationMarkerPath(dir);
    expect(existsSync(marker)).toBe(true);
    expect(readFileSync(marker, "utf8")).toBe(ACTIVATION_MARKER_CONTENT);
    expect(outcome.project).toBe(true);
    expect(outcome.markerPath).toBe(marker);

    const line = formatOnLine(outcome);
    expect(line).toContain("WG active in this session");
    expect(line).toContain("12 wg tools visible");
    expect(line).toContain("project marked active");
    expect(line).toContain("context re-priced");
  });

  it("is session-only (no marker, no .wg creation) outside a WG project", async () => {
    const dir = project(false);
    const { activation } = wired();
    const outcome = await activation.activate({ cwd: dir } as never);

    expect(outcome.sessionOnly).toBe(true);
    expect(outcome.project).toBe(false);
    expect(existsSync(join(dir, ".wg"))).toBe(false);
    expect(formatOnLine(outcome)).toContain("session-only");
  });

  it("mounts the session surfaces", async () => {
    const dir = project();
    const { activation } = wired();
    const mount = vi.fn();
    const unmount = vi.fn();
    activation.setHooks({ mount, unmount });

    await activation.activate({ cwd: dir } as never);
    expect(mount).toHaveBeenCalledTimes(1);
    await activation.deactivate({ cwd: dir } as never);
    expect(unmount).toHaveBeenCalledTimes(1);
  });
});

// ── /wg off: teardown without collateral damage ────────────────────────────

describe("/wg off", () => {
  it("deactivates exactly the wg set and leaves settings/.wg/daemon alone", async () => {
    const dir = project();
    const { harness, backend, activation } = wired();
    await activation.activate({ cwd: dir } as never);
    backend.statusJson.mockClear();

    const outcome = await activation.deactivate({ cwd: dir } as never);

    expect(activation.isActive()).toBe(false);
    expect(wgVisible(harness)).toEqual([]);
    expect(harness.active()).toEqual(BASE_TOOLS);
    expect(outcome.visibleTools).toBe(0);

    // The marker is cleared, `.wg/` itself is untouched.
    expect(existsSync(activationMarkerPath(dir))).toBe(false);
    expect(existsSync(join(dir, ".wg"))).toBe(true);

    // Never the plugin lifecycle: no settings.json surgery, no extension
    // removal, and no daemon call at all.
    expect(backend.piPluginDisable).not.toHaveBeenCalled();
    expect(backend.piPluginInstall).not.toHaveBeenCalled();
    expect(backend.statusJson).not.toHaveBeenCalled();

    const line = formatOffLine(outcome);
    expect(line).toContain("WG dormant in this session");
    expect(line).toContain("project marker cleared");
    expect(line).toContain("running agents are separate processes");
    expect(line).toContain("untouched");
  });

  it("keeps /wg on|off working afterwards (no catch-22)", async () => {
    const dir = project();
    const { harness, backend, activation } = wired();

    await activation.activate({ cwd: dir } as never);
    await activation.deactivate({ cwd: dir } as never);
    await activation.activate({ cwd: dir } as never);

    expect(wgVisible(harness)).toHaveLength(WG_TOOL_NAMES.length);
    // The extension entry was never touched, so /wg survives every toggle.
    expect(backend.piPluginDisable).not.toHaveBeenCalled();
  });
});

// ── persistence across sessions of the same project ────────────────────────

describe("per-project persistence", () => {
  it("a booted project starts ACTIVE in a new session", async () => {
    const dir = project();

    // Session 1: boot it.
    const first = wired();
    await first.activation.applyForSession({ cwd: dir } as never);
    expect(wgVisible(first.harness)).toEqual([]);
    await first.activation.activate({ cwd: dir } as never);

    // Session 2: a brand-new process/extension load in the same project.
    const second = wired();
    await second.activation.applyForSession({ cwd: dir } as never);

    expect(second.activation.isActive()).toBe(true);
    expect(wgVisible(second.harness)).toHaveLength(WG_TOOL_NAMES.length);
  });

  it("a never-booted project stays dormant while a booted one does not leak", async () => {
    const booted = project();
    const untouched = project();

    const first = wired();
    await first.activation.activate({ cwd: booted } as never);

    const second = wired();
    await second.activation.applyForSession({ cwd: untouched } as never);

    expect(second.activation.isActive()).toBe(false);
    expect(wgVisible(second.harness)).toEqual([]);
    expect(isProjectMarkedActive(untouched)).toBe(false);
  });

  it("/wg off clears the marker so the project is dormant for FUTURE sessions", async () => {
    const dir = project();
    const first = wired();
    await first.activation.activate({ cwd: dir } as never);
    await first.activation.deactivate({ cwd: dir } as never);
    expect(existsSync(activationMarkerPath(dir))).toBe(false);

    const second = wired();
    await second.activation.applyForSession({ cwd: dir } as never);
    expect(second.activation.isActive()).toBe(false);
    expect(wgVisible(second.harness)).toEqual([]);
  });

  it("honours the daemon dir (WG_DIR) over the session cwd", async () => {
    const projectDir = project();
    const elsewhere = project(false);
    const harness = makePi();
    registerWgTools(harness.pi as never, fakeBackend());
    const activation = installSessionActivation(harness.pi as never, fakeBackend(), { dir: projectDir }, {});
    await activation.activate({ cwd: elsewhere } as never);
    expect(existsSync(activationMarkerPath(projectDir))).toBe(true);
    expect(resolveProjectRoot({ dir: projectDir }, elsewhere)).toBe(projectDir);
    expect(resolveProjectRoot({ dir: undefined }, elsewhere)).toBe(elsewhere);
  });
});

// ── /wg status ─────────────────────────────────────────────────────────────

describe("/wg status shape", () => {
  it("reports session · project · tools · daemon truthfully", async () => {
    const dir = project();
    const { harness, activation } = wired({
      statusJson: vi.fn(async () => ({ service: { running: false } })),
    });

    const dormant = await activation.status({ cwd: dir } as never);
    expect(formatActivationStatus(dormant)).toBe(
      "session: dormant · project: dormant · tools: 0 wg tools visible · daemon: stopped",
    );

    await activation.activate({ cwd: dir } as never);
    const active = await activation.status({ cwd: dir } as never);
    expect(formatActivationStatus(active)).toContain("session: active");
    expect(formatActivationStatus(active)).toContain("project: marked active");
    expect(formatActivationStatus(active)).toContain("tools: 12 wg tools visible");
    expect(active.projectRoot).toBe(dir);
  });

  it("reports daemon unknown when the probe fails", async () => {
    const { activation } = wired({
      statusJson: vi.fn(async () => { throw new Error("down"); }),
    });
    const state = await activation.status({ cwd: project() } as never);
    expect(formatActivationStatus(state)).toContain("daemon: unknown");
  });
});

// ── pure helpers ───────────────────────────────────────────────────────────

describe("activation helpers", () => {
  it("partitions and rebuilds tool sets without disturbing the rest", () => {
    const current = ["read", "wg_ready", "bash", "wg_done"];
    expect(partitionToolNames(current)).toEqual({
      wg: ["wg_ready", "wg_done"],
      other: ["read", "bash"],
    });
    expect(activeToolSet(current)).toEqual(["read", "bash", ...WG_TOOL_NAMES]);
    expect(dormantToolSet(current)).toEqual(["read", "bash"]);
    expect(visibleWgToolCount(() => current)).toBe(2);
    expect(visibleWgToolCount(undefined)).toBe(0);
  });

  it("only persists into an existing .wg/", () => {
    const withWg = project(true);
    const without = project(false);
    expect(canPersistProjectMarker(withWg)).toBe(true);
    expect(canPersistProjectMarker(without)).toBe(false);
    expect(canPersistProjectMarker(undefined)).toBe(false);
  });

  it("treats a marker's existence as the signal", () => {
    const dir = project();
    expect(isProjectMarkedActive(dir)).toBe(false);
    mkdirSync(join(dir, ".wg"), { recursive: true });
    expect(isProjectMarkedActive(dir)).toBe(false);
  });
});
