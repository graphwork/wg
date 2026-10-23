/**
 * bash-timeout tests — FIX A of the tool-hang hotfix.
 *
 * Pins that a `bash` tool call **without** a model-provided timeout gets the
 * configured default injected (so it can never wait forever), that an explicit
 * timeout is preserved verbatim, and that env / config-file overrides win.
 *
 * Tests run against the built `pi-worksgood/` artifact — `npm test` builds first.
 */

import { describe, expect, it, vi } from "vitest";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import {
  DEFAULT_BASH_TIMEOUT_SECS,
  applyBashTimeout,
  installBashToolTimeout,
  parseBashTimeoutConfig,
  parseBashTimeoutSecs,
  readBashTimeoutConfig,
} from "../pi-worksgood/index.js";

const NO_FILES = { exists: () => false, readFile: () => "" };

describe("bash timeout default injection (tool_call mutator)", () => {
  it("injects the default when the model omitted a timeout", () => {
    const event = { toolName: "bash", input: { command: "cat <<'EOF'\n" } };
    expect(applyBashTimeout(event, DEFAULT_BASH_TIMEOUT_SECS)).toBe(true);
    expect(event.input.timeout).toBe(600);
    // The command itself is untouched.
    expect(event.input.command).toBe("cat <<'EOF'\n");
  });

  it("preserves an explicit model-provided timeout", () => {
    const event = { toolName: "bash", input: { command: "sleep 5", timeout: 30 } };
    expect(applyBashTimeout(event, 600)).toBe(false);
    expect(event.input.timeout).toBe(30);
  });

  it("preserves an explicit timeout of 0/negative so pi's validator decides", () => {
    for (const explicit of [0, -1]) {
      const event = { toolName: "bash", input: { command: "ls", timeout: explicit } };
      expect(applyBashTimeout(event, 600)).toBe(false);
      expect(event.input.timeout).toBe(explicit);
    }
  });

  it("ignores non-bash tools", () => {
    const event = { toolName: "read", input: { path: "/tmp/x" } };
    expect(applyBashTimeout(event, 600)).toBe(false);
    expect(event.input).toEqual({ path: "/tmp/x" });
  });

  it("is a no-op when the guard is disabled (0)", () => {
    const event = { toolName: "bash", input: { command: "cat" } };
    expect(applyBashTimeout(event, 0)).toBe(false);
    expect(event.input.timeout).toBeUndefined();
  });

  it("registers a tool_call handler that mutates the event in place", async () => {
    let handler: ((event: unknown, ctx: unknown) => unknown) | undefined;
    const pi = {
      on: vi.fn((event: string, fn: (e: unknown, c: unknown) => unknown) => {
        expect(event).toBe("tool_call");
        handler = fn;
        return () => {};
      }),
    };
    installBashToolTimeout(pi as never, 600);
    expect(pi.on).toHaveBeenCalledTimes(1);
    const event = { toolName: "bash", input: { command: "python3 - <<'PY'\n" } };
    await handler?.(event, {});
    expect((event.input as { timeout?: number }).timeout).toBe(600);
  });
});

describe("bash timeout resolution (config file + env)", () => {
  it("defaults to 600s", () => {
    expect(readBashTimeoutConfig({}, NO_FILES)).toBe(600);
  });

  it("reads bashTimeoutSecs from the extension config file", () => {
    const io = {
      exists: (p: string) => p.endsWith("config.json"),
      readFile: () => JSON.stringify({ bashTimeoutSecs: 120 }),
    };
    expect(readBashTimeoutConfig({}, io)).toBe(120);
    expect(parseBashTimeoutConfig({ bashTimeoutSecs: "45" })).toBe(45);
  });

  it("lets WG_PI_BASH_TIMEOUT_SECS override the file", () => {
    const io = {
      exists: () => true,
      readFile: () => JSON.stringify({ bashTimeoutSecs: 120 }),
    };
    expect(readBashTimeoutConfig({ WG_PI_BASH_TIMEOUT_SECS: "42" }, io)).toBe(42);
  });

  it("honors 0 as an explicit disable and rejects junk", () => {
    expect(readBashTimeoutConfig({ WG_PI_BASH_TIMEOUT_SECS: "0" }, NO_FILES)).toBe(0);
    expect(parseBashTimeoutSecs("abc")).toBeUndefined();
    expect(parseBashTimeoutSecs(-5)).toBeUndefined();
    expect(parseBashTimeoutSecs("")).toBeUndefined();
    expect(parseBashTimeoutConfig({ bashTimeoutSecs: "nope" })).toBeUndefined();
  });

  it("degrades a corrupt config file to the safe default", () => {
    const io = { exists: () => true, readFile: () => "{not json" };
    expect(readBashTimeoutConfig({}, io)).toBe(DEFAULT_BASH_TIMEOUT_SECS);
  });
});
