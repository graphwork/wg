/**
 * Verifies the plugin's registration entry: calling wgPlugin(fakePi) must
 * register the full wg tool family and the /wg + /wg-model commands, and
 * subscribe to the documented lifecycle events.
 *
 * Tests run against the built artifact (dist/) — `npm test` builds first.
 */

import { readFile } from "node:fs/promises";
import { describe, it, expect, vi } from "vitest";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import wgPlugin from "../dist/index.js";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { registerWgTools } from "../dist/tools.js";

interface FakePi {
  registerTool: ReturnType<typeof vi.fn>;
  registerCommand: ReturnType<typeof vi.fn>;
  registerProvider: ReturnType<typeof vi.fn>;
  on: ReturnType<typeof vi.fn>;
  setModel: ReturnType<typeof vi.fn>;
  sendUserMessage: ReturnType<typeof vi.fn>;
  exec: ReturnType<typeof vi.fn>;
  events: { on: ReturnType<typeof vi.fn>; emit: ReturnType<typeof vi.fn> };
  toolNames: string[];
  commandNames: string[];
  subscribedEvents: string[];
}

function makeFakePi(): FakePi {
  const toolNames: string[] = [];
  const commandNames: string[] = [];
  const subscribedEvents: string[] = [];
  return {
    registerTool: vi.fn((tool: { name: string }) => toolNames.push(tool.name)),
    registerCommand: vi.fn((name: string) => commandNames.push(name)),
    registerProvider: vi.fn(),
    on: vi.fn((event: string) => subscribedEvents.push(event)),
    setModel: vi.fn(async () => true),
    sendUserMessage: vi.fn(),
    exec: vi.fn(async () => ({ stdout: "", stderr: "", code: 0, killed: false })),
    events: { on: vi.fn(), emit: vi.fn() },
    toolNames,
    commandNames,
    subscribedEvents,
  };
}

const EXPECTED_TOOLS = [
  "wg_ready",
  "wg_show",
  "wg_add",
  "wg_done",
  "wg_fail",
  "wg_msg_send",
  "wg_msg_read",
  "wg_run",
];

describe("wgPlugin registration entry", () => {
  it("registers the full wg tool family", () => {
    const pi = makeFakePi();
    wgPlugin(pi);
    for (const name of EXPECTED_TOOLS) {
      expect(pi.toolNames, `tool ${name} should be registered`).toContain(name);
    }
    expect(pi.registerTool).toHaveBeenCalledTimes(EXPECTED_TOOLS.length);
  });

  it("registers the /wg and /wg-model commands", () => {
    const pi = makeFakePi();
    wgPlugin(pi);
    expect(pi.commandNames).toContain("wg");
    expect(pi.commandNames).toContain("wg-model");
    expect(pi.registerCommand).toHaveBeenCalledWith("wg", expect.any(Object));
    expect(pi.registerCommand).toHaveBeenCalledWith("wg-model", expect.any(Object));
  });

  it("subscribes to the lifecycle + model_select hooks", () => {
    const pi = makeFakePi();
    wgPlugin(pi);
    expect(pi.subscribedEvents).toEqual(
      expect.arrayContaining(["session_start", "turn_end", "model_select", "session_shutdown"]),
    );
  });

  it("does not register a provider when WG exports no endpoint", () => {
    const saved = process.env.WG_PI_BASE_URL;
    delete process.env.WG_PI_BASE_URL;
    try {
      const pi = makeFakePi();
      wgPlugin(pi);
      expect(pi.registerProvider).not.toHaveBeenCalled();
    } finally {
      if (saved !== undefined) process.env.WG_PI_BASE_URL = saved;
    }
  });
});

describe("WG tool result shaping", () => {
  it("truncates large wg output and records fullOutputPath", async () => {
    const tools = new Map<string, any>();
    const pi = {
      registerTool: vi.fn((tool: { name: string }) => tools.set(tool.name, tool)),
    };
    const large = Array.from({ length: 2500 }, (_, i) => `line-${String(i).padStart(4, "0")}`).join("\n");
    const backend = {
      ready: vi.fn(async () => ({ stdout: large, stderr: "", code: 0, killed: false })),
    };

    registerWgTools(pi as any, backend as any);
    const tool = tools.get("wg_ready");
    expect(tool).toBeTruthy();

    const result = await tool.execute("call-1", {}, undefined, undefined, { cwd: process.cwd() });
    const text = result.content[0].text;
    expect(text.length).toBeLessThan(large.length);
    expect(text).toContain("[Output truncated:");
    expect(result.details.fullOutputPath).toBeTruthy();
    expect(result.details.truncation.truncated).toBe(true);
    await expect(readFile(result.details.fullOutputPath, "utf8")).resolves.toBe(large);
  });
});
