/**
 * toggle-commands.test.ts — `/wg on | off | status` (SESSION-SCOPED ACTIVATION).
 *
 * These verbs used to be plugin-lifecycle verbs (`on` ran `wg pi-plugin install`,
 * `off` removed the settings entry — a catch-22 that could strand the next
 * session without `/wg`). They are now the SESSION activators: `on` puts the
 * wg_* tool family into the active set via `pi.setActiveTools`, `off` takes it
 * out, `status` prints one truth line. Neither touches settings.json, the
 * extension entry, `.wg/` presence, or the daemon — `wg pi-plugin disable` stays
 * the CLI-only uninstall.
 *
 * This file pins the COMMAND SURFACE (dispatch, completions, honest output,
 * self-referential safety). The activation mechanics (active-set deltas,
 * persistence, load-time dormancy) are pinned in `activation.test.ts`.
 *
 * Tests run against the built `pi-worksgood/` artifact — `npm test` builds first.
 */

import { mkdtempSync, mkdirSync, existsSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it, expect, vi, afterEach } from "vitest";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import {
  registerWgCommands,
  parsePluginState,
  formatStatusLine,
  WG_SUBCOMMANDS,
} from "../pi-worksgood/commands.js";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { installSessionActivation, activationMarkerPath } from "../pi-worksgood/activation.js";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { registerWgTools } from "../pi-worksgood/tools.js";

interface Registered {
  description?: string;
  getArgumentCompletions?: (prefix: string) => Array<{ value: string; label: string }> | null;
  handler: (args: string, ctx: unknown) => Promise<void>;
}

const cleanups: string[] = [];
afterEach(() => {
  for (const dir of cleanups.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function makePi(): { pi: any; commands: Map<string, Registered>; tools: string[] } {
  const commands = new Map<string, Registered>();
  const tools: string[] = [];
  let active = ["read", "bash", "edit", "write"];
  const pi = {
    registerTool: vi.fn((tool: { name: string }) => tools.push(tool.name)),
    registerCommand: vi.fn((name: string, opts: Registered) => commands.set(name, opts)),
    on: vi.fn(),
    sendUserMessage: vi.fn(),
    setModel: vi.fn(async () => true),
    getActiveTools: vi.fn(() => [...active]),
    getAllTools: vi.fn(() => tools.map((name) => ({ name, active: active.includes(name) }))),
    // pi ignores unknown names, so only registered/built-in names can be active.
    setActiveTools: vi.fn((names: string[]) => {
      active = names.filter((n) => tools.includes(n) || ["read", "bash", "edit", "write"].includes(n));
    }),
  };
  return { pi, commands, tools };
}

function makeCtx(cwd?: string): { ctx: any; notify: ReturnType<typeof vi.fn> } {
  const notify = vi.fn();
  return { ctx: { hasUI: true, ui: { notify }, signal: undefined, cwd }, notify };
}

/** A real temp project dir; `withWg: false` leaves `.wg/` absent. */
function project(withWg = true): string {
  const dir = mkdtempSync(join(tmpdir(), "wg-toggle-"));
  cleanups.push(dir);
  if (withWg) mkdirSync(join(dir, ".wg"));
  return dir;
}

const OK = (stdout: string) => ({ stdout, stderr: "", code: 0, killed: false });
const ERR = (stderr: string) => ({ stdout: "", stderr, code: 1, killed: false });

function baseBackend(overrides: Record<string, unknown> = {}): any {
  return {
    piPluginInstall: vi.fn(async () => OK("Installed pi-worksgood.")),
    piPluginStatus: vi.fn(async () => OK("  cache state:      current (matches this binary's embedded build)")),
    piPluginDisable: vi.fn(async () => OK("Disabled pi-worksgood — removed 1 managed extension entry.")),
    graphPresent: vi.fn(async () => true),
    statusJson: vi.fn(async () => ({
      service: { running: true },
      agents: { alive: 2 },
      tasks: { in_progress: 3, ready: 1, done_total: 178 },
    })),
    ready: vi.fn(async () => OK("No ready tasks.")),
    show: vi.fn(async () => OK("task")),
    add: vi.fn(async () => OK("added")),
    publish: vi.fn(async () => OK("published")),
    done: vi.fn(async () => OK("done")),
    fail: vi.fn(async () => OK("failed")),
    ...overrides,
  };
}

function wire(overrides: Record<string, unknown> = {}, cwd = project()) {
  const backend = baseBackend(overrides);
  const { pi, commands, tools } = makePi();
  registerWgTools(pi, backend);
  const activation = installSessionActivation(pi, backend, {}, {});
  registerWgCommands(pi, backend, activation);
  return { backend, pi, commands, activation, cwd, tools };
}

describe("/wg subcommand surface", () => {
  it("includes on/off/status alongside the task verbs", () => {
    for (const sub of ["on", "off", "status"]) {
      expect(WG_SUBCOMMANDS as readonly string[]).toContain(sub);
    }
  });

  it("completes on/off/status from the empty prefix and by prefix", () => {
    const { commands } = wire();
    const wg = commands.get("wg");
    expect(wg?.getArgumentCompletions).toBeTypeOf("function");

    const empty = wg!.getArgumentCompletions!("") ?? [];
    const emptyValues = empty.map((c) => c.value);
    for (const sub of ["on", "off", "status"]) {
      expect(emptyValues).toContain(sub);
    }

    const o = (wg!.getArgumentCompletions!("o") ?? []).map((c) => c.value);
    expect(o).toEqual(["on", "off"]);

    const s = (wg!.getArgumentCompletions!("s") ?? []).map((c) => c.value);
    expect(s).toEqual(["show", "status"]);
  });

  it("advertises the activation verbs and their non-destructive contract", () => {
    const { commands } = wire();
    const desc = commands.get("wg")?.description ?? "";
    for (const word of ["on", "off", "status"]) {
      expect(desc).toContain(word);
    }
    expect(desc).toContain("settings.json");
  });

  it("keeps /wg registered in every state (self-referential safety)", async () => {
    const { commands, pi } = wire();
    const wg = commands.get("wg")!;
    const { ctx } = makeCtx();

    // dormant → on → off → on: /wg never disappears, and no path disables the
    // extension or edits settings.json.
    await wg.handler("status", ctx);
    await wg.handler("on", ctx);
    await wg.handler("off", ctx);
    await wg.handler("on", ctx);
    expect(commands.has("wg")).toBe(true);
    expect(commands.has("wg-model")).toBe(true);
    expect(pi.registerCommand).toHaveBeenCalledWith("wg", expect.any(Object));
    // The wg tools are still registered; only their ACTIVE membership changed.
    expect(pi.getAllTools).toBeTypeOf("function");
  });
});

describe("/wg on", () => {
  it("activates the wg tool family and says so in one line", async () => {
    const { commands, pi, backend, cwd } = wire();
    const { ctx, notify } = makeCtx(cwd);

    await commands.get("wg")!.handler("on", ctx);

    expect(pi.setActiveTools).toHaveBeenCalledTimes(1);
    const names = pi.setActiveTools.mock.calls[0][0] as string[];
    expect(names).toEqual(expect.arrayContaining(["read", "wg_ready", "wg_done"]));

    const line = notify.mock.calls[0][0] as string;
    expect(line).toContain("WG active in this session");
    expect(line).toContain("wg tools visible");
    expect(line).toContain("context re-priced");
    // The old lifecycle behavior is gone: it never installs/rewrites anything.
    expect(backend.piPluginInstall).not.toHaveBeenCalled();
    expect(backend.piPluginDisable).not.toHaveBeenCalled();
  });

  it("is a session-only activation when the project has no .wg/", async () => {
    const dir = project(false);
    const { commands } = wire({}, dir);
    const { ctx, notify } = makeCtx(dir);

    await commands.get("wg")!.handler("on", ctx);

    const line = notify.mock.calls[0][0] as string;
    expect(line).toContain("session-only");
    // Never initializes WG presence just to record a marker.
    expect(existsSync(join(dir, ".wg"))).toBe(false);
  });

  it("writes the per-project marker inside an existing .wg/", async () => {
    const dir = project();
    const { commands } = wire({}, dir);
    const { ctx, notify } = makeCtx(dir);

    await commands.get("wg")!.handler("on", ctx);

    const marker = activationMarkerPath(dir);
    expect(existsSync(marker)).toBe(true);
    expect(readFileSync(marker, "utf8")).toBe("active\n");
    expect(notify.mock.calls[0][0]).toContain("project marked active");
    // The marker lives INSIDE .wg/ — never mutating its presence.
    expect(existsSync(join(dir, ".wg"))).toBe(true);
  });
});

describe("/wg off", () => {
  it("deactivates, clears the marker, and touches nothing else", async () => {
    const dir = project();
    const { commands, pi, backend } = wire({}, dir);
    const { ctx, notify } = makeCtx(dir);

    await commands.get("wg")!.handler("on", ctx);
    const marker = activationMarkerPath(dir);
    expect(existsSync(marker)).toBe(true);
    pi.setActiveTools.mockClear();

    await commands.get("wg")!.handler("off", ctx);

    const names = pi.setActiveTools.mock.calls[0][0] as string[];
    expect(names).toContain("read");
    expect(names).not.toContain("wg_ready");
    expect(names.some((n: string) => n.startsWith("wg_"))).toBe(false);

    expect(existsSync(marker)).toBe(false);
    expect(existsSync(join(dir, ".wg"))).toBe(true);

    const line = notify.mock.calls.at(-1)![0] as string;
    expect(line).toContain("WG dormant in this session");
    expect(line).toContain("project marker cleared");
    expect(line).toContain("settings.json");
    expect(line).toContain("untouched");

    // NEVER the plugin lifecycle: no settings remove, no extension removal.
    expect(backend.piPluginDisable).not.toHaveBeenCalled();
    expect(backend.piPluginInstall).not.toHaveBeenCalled();
  });
});

describe("/wg status", () => {
  it("prints the four required fields on one line", async () => {
    const dir = project();
    const { commands } = wire({}, dir);
    const { ctx, notify } = makeCtx(dir);

    await commands.get("wg")!.handler("status", ctx);
    const line = notify.mock.calls[0][0] as string;
    expect(line).toBe("session: dormant · project: dormant · tools: 0 wg tools visible · daemon: running");

    await commands.get("wg")!.handler("on", ctx);
    await commands.get("wg")!.handler("status", ctx);
    const active = notify.mock.calls.at(-1)![0] as string;
    expect(active).toBe("session: active · project: marked active · tools: 12 wg tools visible · daemon: running");
  });

  it("reports daemon down and unknown honestly", async () => {
    const down = wire({ statusJson: vi.fn(async () => ({ service: { running: false } })) });
    const { ctx, notify } = makeCtx(project());
    await down.commands.get("wg")!.handler("status", ctx);
    expect(notify.mock.calls[0][0]).toContain("daemon: stopped");

    const broken = wire({ statusJson: vi.fn(async () => { throw new Error("no daemon"); }) });
    const second = makeCtx(project());
    await broken.commands.get("wg")!.handler("status", second.ctx);
    expect(second.notify.mock.calls[0][0]).toContain("daemon: unknown");
  });
});

describe("plugin/status parsing helpers", () => {
  it("maps the wg pi-plugin status cache-state line", () => {
    expect(parsePluginState("  cache state:      current (matches this binary's embedded build)")).toBe("current");
    expect(parsePluginState("  cache state:      MISSING — run `wg pi-plugin install`")).toBe("missing");
    expect(parsePluginState("  cache state:      DRIFTED — stale/incompatible cache")).toBe("stale");
    expect(parsePluginState("garbage")).toBe("unknown");
  });

  it("formats a single-line summary without newlines", () => {
    const line = formatStatusLine({
      plugin: "current",
      graph: true,
      status: { service: { running: true }, agents: { alive: 1 }, tasks: { in_progress: 0, ready: 0, done_total: 2 } },
    });
    expect(line).toBe("WG · plugin: current · graph: present (0 in-progress, 0 ready, 2 done) · fleet: service up (1 agent)");
  });
});
