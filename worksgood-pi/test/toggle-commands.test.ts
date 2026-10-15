/**
 * toggle-commands.test.ts — `/wg on | off | status` (PLUGIN ON/OFF AS COMMANDS).
 *
 * The plugin lifecycle is driven entirely through the `wg` binary: `/wg on`
 * runs the ensure path (`wg pi-plugin install`), `/wg off` removes this
 * plugin's settings entry (`wg pi-plugin disable`), and `/wg status` prints one
 * compact truth line. These tests pin the *command surface* (parsing,
 * completions, honest output, error surfacing) against a mocked backend; the
 * settings.json rewrite itself is pinned by the Rust unit tests in
 * `src/pi_plugin/mod.rs` (`wg pi-plugin disable`).
 *
 * Tests run against the built `pi-worksgood/` artifact — `npm test` builds first.
 */

import { describe, it, expect, vi } from "vitest";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import {
  registerWgCommands,
  parsePluginState,
  formatStatusLine,
  WG_SUBCOMMANDS,
} from "../pi-worksgood/commands.js";

interface Registered {
  description?: string;
  getArgumentCompletions?: (prefix: string) => Array<{ value: string; label: string }> | null;
  handler: (args: string, ctx: unknown) => Promise<void>;
}

function makePi(): { pi: any; commands: Map<string, Registered> } {
  const commands = new Map<string, Registered>();
  const pi = {
    registerCommand: vi.fn((name: string, opts: Registered) => commands.set(name, opts)),
    on: vi.fn(),
    sendUserMessage: vi.fn(),
    setModel: vi.fn(async () => true),
  };
  return { pi, commands };
}

function makeCtx(): { ctx: any; notify: ReturnType<typeof vi.fn> } {
  const notify = vi.fn();
  return { ctx: { hasUI: true, ui: { notify }, signal: undefined }, notify };
}

const OK = (stdout: string) => ({ stdout, stderr: "", code: 0, killed: false });
const ERR = (stderr: string) => ({ stdout: "", stderr, code: 1, killed: false });

function baseBackend(overrides: Record<string, unknown> = {}): any {
  return {
    piPluginInstall: vi.fn(async () => OK("Installed pi-worksgood (npm: @worksgood/pi, compat 0.3.0).")),
    piPluginStatus: vi.fn(async () => OK("  cache state:      current (matches this binary's embedded build)")),
    piPluginDisable: vi.fn(async () =>
      OK(
        "Disabled pi-worksgood — removed 1 managed extension entry from /home/u/.pi/agent/settings.json.",
      ),
    ),
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

describe("/wg subcommand surface", () => {
  it("includes on/off/status alongside the task verbs", () => {
    for (const sub of ["on", "off", "status"]) {
      expect(WG_SUBCOMMANDS as readonly string[]).toContain(sub);
    }
  });

  it("completes on/off/status from the empty prefix and by prefix", () => {
    const { pi, commands } = makePi();
    registerWgCommands(pi, baseBackend());
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

  it("advertises the lifecycle verbs in the command description", () => {
    const { pi, commands } = makePi();
    registerWgCommands(pi, baseBackend());
    const desc = commands.get("wg")?.description ?? "";
    for (const word of ["on", "off", "status"]) {
      expect(desc).toContain(word);
    }
  });
});

describe("/wg on", () => {
  it("reports a real install and asks for a restart", async () => {
    const backend = baseBackend();
    const { pi, commands } = makePi();
    registerWgCommands(pi, backend);
    const { ctx, notify } = makeCtx();

    await commands.get("wg")!.handler("on", ctx);

    expect(backend.piPluginInstall).toHaveBeenCalledTimes(1);
    const texts = notify.mock.calls.map((c) => c[0]);
    expect(texts[0]).toContain("Installed pi-worksgood");
    expect(texts[1]).toBe("restart pi to load the updated plugin.");
  });

  it("is an honest no-op when the plugin is already current", async () => {
    const backend = baseBackend({
      piPluginInstall: vi.fn(async () =>
        OK("pi-worksgood is already up to date — nothing changed (compat 0.3.0)."),
      ),
    });
    const { pi, commands } = makePi();
    registerWgCommands(pi, backend);
    const { ctx, notify } = makeCtx();

    // Twice: idempotence is the honest no-op, not a second mutation.
    await commands.get("wg")!.handler("on", ctx);
    await commands.get("wg")!.handler("on", ctx);

    expect(backend.piPluginInstall).toHaveBeenCalledTimes(2);
    const texts = notify.mock.calls.map((c) => c[0]);
    expect(texts.filter((t) => t.includes("already up to date"))).toHaveLength(2);
    expect(texts.filter((t) => t === "pi-worksgood is already current — no restart needed.")).toHaveLength(2);
  });

  it("surfaces a failure instead of silently succeeding", async () => {
    const backend = baseBackend({
      piPluginInstall: vi.fn(async () => ERR("worker_control.admin_operation_refused: outside trusted local graph coordination")),
    });
    const { pi, commands } = makePi();
    registerWgCommands(pi, backend);
    const { ctx, notify } = makeCtx();

    await commands.get("wg")!.handler("on", ctx);
    expect(notify).toHaveBeenCalledTimes(1);
    expect(notify.mock.calls[0][0]).toContain("admin_operation_refused");
    expect(notify.mock.calls[0][1]).toBe("error");
  });
});

describe("/wg off", () => {
  it("removes via the wg verb and reports the outcome (next-launch semantics)", async () => {
    const backend = baseBackend();
    const { pi, commands } = makePi();
    registerWgCommands(pi, backend);
    const { ctx, notify } = makeCtx();

    await commands.get("wg")!.handler("off", ctx);
    expect(backend.piPluginDisable).toHaveBeenCalledTimes(1);
    const text = notify.mock.calls[0][0];
    expect(text).toContain("Disabled pi-worksgood");
    expect(text).toContain("removed 1 managed extension entry");
  });

  it("surfaces a permissions failure instead of silently succeeding", async () => {
    const backend = baseBackend({
      piPluginDisable: vi.fn(async () =>
        ERR('write pi settings "/home/u/.pi/agent/settings.json" (check permissions on "/home/u/.pi/agent")'),
      ),
    });
    const { pi, commands } = makePi();
    registerWgCommands(pi, backend);
    const { ctx, notify } = makeCtx();

    await commands.get("wg")!.handler("off", ctx);
    expect(notify).toHaveBeenCalledTimes(1);
    expect(notify.mock.calls[0][0]).toContain("check permissions");
    expect(notify.mock.calls[0][1]).toBe("error");
  });
});

describe("/wg status", () => {
  it("prints one compact truthful line when everything is healthy", async () => {
    const backend = baseBackend();
    const { pi, commands } = makePi();
    registerWgCommands(pi, backend);
    const { ctx, notify } = makeCtx();

    await commands.get("wg")!.handler("status", ctx);

    expect(notify).toHaveBeenCalledTimes(1);
    const line = notify.mock.calls[0][0] as string;
    expect(line.startsWith("WG · plugin: current · graph: present")).toBe(true);
    expect(line).toContain("3 in-progress, 1 ready, 178 done");
    expect(line).toContain("fleet: service up (2 agents)");
    expect(line.split("\n")).toHaveLength(1);
  });

  it("reports missing plugin + absent graph + daemon down", async () => {
    const backend = baseBackend({
      piPluginStatus: vi.fn(async () => OK("  cache state:      MISSING — run `wg pi-plugin install`")),
      graphPresent: vi.fn(async () => false),
      statusJson: vi.fn(async () => ({ service: { running: false }, agents: { alive: 0 }, tasks: {} })),
    });
    const { pi, commands } = makePi();
    registerWgCommands(pi, backend);
    const { ctx, notify } = makeCtx();

    await commands.get("wg")!.handler("status", ctx);

    const line = notify.mock.calls[0][0] as string;
    expect(line).toContain("plugin: missing");
    expect(line).toContain("graph: absent");
    expect(line).toContain("fleet: daemon down");
  });

  it("degrades to plugin: unknown when wg pi-plugin status fails", async () => {
    const backend = baseBackend({ piPluginStatus: vi.fn(async () => ERR("boom")) });
    const { pi, commands } = makePi();
    registerWgCommands(pi, backend);
    const { ctx, notify } = makeCtx();

    await commands.get("wg")!.handler("status", ctx);
    expect((notify.mock.calls[0][0] as string)).toContain("plugin: unknown");
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
