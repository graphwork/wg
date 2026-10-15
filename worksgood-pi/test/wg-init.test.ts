/**
 * /wg init — the conversational bootstrap command.
 *
 * Pins the shipped contract (wg-init-bootstrap):
 *   - `/wg init` injects ONE short bootstrap prompt via `pi.sendUserMessage`
 *     and runs no `wg` subprocess itself (the agent does the setup);
 *   - `init` is offered by getArgumentCompletions;
 *   - registering the command family adds ZERO load-time behavior (no `wg`
 *     subprocess, no polling) — the plugin stays quiet until summoned;
 *   - the bare-`/wg` nicety hints at `/wg init` in a project with no WG state.
 *
 * Tests run against the built `pi-worksgood/` artifact — `npm test` builds first.
 */

import { mkdtempSync, mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { registerWgCommands, WG_INIT_BOOTSTRAP_PROMPT } from "../pi-worksgood/commands.js";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { WgBackend } from "../pi-worksgood/wg-backend.js";

const tmpDirs: string[] = [];
function scratch(): string {
  const dir = mkdtempSync(join(tmpdir(), "wg-init-"));
  tmpDirs.push(dir);
  return dir;
}
afterEach(() => {
  while (tmpDirs.length) rmSync(tmpDirs.pop()!, { recursive: true, force: true });
});

interface FakePi {
  sendUserMessage: ReturnType<typeof vi.fn>;
  on: ReturnType<typeof vi.fn>;
  registerCommand: ReturnType<typeof vi.fn>;
  commands: Map<string, any>;
  events: string[];
}

function fakePi(): FakePi {
  const commands = new Map<string, any>();
  const events: string[] = [];
  return {
    sendUserMessage: vi.fn(),
    on: vi.fn((event: string) => events.push(event)),
    registerCommand: vi.fn((name: string, options: any) => commands.set(name, options)),
    commands,
    events,
  } as unknown as FakePi;
}

function fakeBackend(dir?: string) {
  const calls: string[][] = [];
  const host = {
    exec: vi.fn(async (_command: string, args: string[]) => {
      calls.push(args);
      return { stdout: "[]", stderr: "", code: 0, killed: false };
    }),
  };
  const backend = new WgBackend(host as never, dir ? { dir } : {});
  return { backend, host, calls };
}

function fakeCtx(cwd: string) {
  return {
    hasUI: true,
    mode: "tui",
    cwd,
    signal: undefined,
    ui: { notify: vi.fn() },
  } as any;
}

function register(pi: FakePi, backend: WgBackend) {
  registerWgCommands(pi as any, backend);
  const wg = pi.commands.get("wg");
  if (!wg) throw new Error("wg command not registered");
  return wg;
}

describe("/wg init", () => {
  it("injects the bootstrap prompt via sendUserMessage and runs no wg subprocess", async () => {
    const project = scratch();
    const pi = fakePi();
    const { backend, calls, host } = fakeBackend(project);
    const wg = register(pi, backend);

    await wg.handler("init", fakeCtx(project));

    expect(pi.sendUserMessage).toHaveBeenCalledTimes(1);
    expect(pi.sendUserMessage).toHaveBeenCalledWith(WG_INIT_BOOTSTRAP_PROMPT);
    // The plugin only hands the job to the agent — it must not shell out.
    expect(host.exec).not.toHaveBeenCalled();
    expect(calls.length).toBe(0);
  });

  it("prompt is short, names the setup verbs, and asks before surprising the operator", () => {
    expect(WG_INIT_BOOTSTRAP_PROMPT.length).toBeLessThan(1400);
    expect(WG_INIT_BOOTSTRAP_PROMPT).not.toContain("\n\n");
    for (const needle of ["wg init", "wg setup", "wg pi-plugin install", "/wg-fleet", "Ask before creating"]) {
      expect(WG_INIT_BOOTSTRAP_PROMPT).toContain(needle);
    }
  });

  it("offers init in argument completions", () => {
    const pi = fakePi();
    const { backend } = fakeBackend(scratch());
    const wg = register(pi, backend);

    const all = wg.getArgumentCompletions("").map((c: any) => c.value);
    expect(all).toContain("init");
    const filtered = wg.getArgumentCompletions("i").map((c: any) => c.value);
    expect(filtered).toEqual(["init"]);
  });
});

describe("/wg init — zero load-time behavior", () => {
  it("registers with no wg subprocess and no polling before it is summoned", () => {
    const pi = fakePi();
    const { backend, host } = fakeBackend(scratch());

    register(pi, backend);

    expect(host.exec).not.toHaveBeenCalled();
    // The only lifecycle hook is the existing (non-polling) model-registry
    // stash on session_start; nothing new was added for the bootstrap.
    expect(pi.events).toEqual(["session_start"]);
  });
});

describe("bare /wg nicety", () => {
  it("hints at /wg init when the project has no WG state", async () => {
    const project = scratch(); // no `.wg/`, no worksgood.toml
    const pi = fakePi();
    const { backend, host } = fakeBackend(project);
    const wg = register(pi, backend);
    const ctx = fakeCtx(project);

    await wg.handler("", ctx);

    expect(ctx.ui.notify).toHaveBeenCalledWith(
      expect.stringContaining("/wg init"),
      "warning",
    );
    expect(host.exec).not.toHaveBeenCalled();
  });

  it("runs the normal ready view when WG state exists", async () => {
    const project = scratch();
    mkdirSync(join(project, ".wg"));
    const pi = fakePi();
    const { backend, host } = fakeBackend(project);
    const wg = register(pi, backend);
    const ctx = fakeCtx(project);

    await wg.handler("", ctx);

    expect(host.exec).toHaveBeenCalledTimes(1);
    expect(host.exec.mock.calls[0][1]).toContain("ready");
  });
});
