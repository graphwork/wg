/**
 * Console freshness self-heal (fix-console-plugin).
 *
 * Pins the pi → wg console direction: a live `pi` session must never silently
 * run a plugin cache materialized by an OLDER `wg` binary. The console plugin
 * compares its own `.wg-embed-digest` stamp against `wg pi-plugin digest` and,
 * on mismatch, warns loudly AND runs `wg pi-plugin install` so the next launch
 * is fixed. A current build is a silent, write-free no-op.
 *
 * Tests run against the built `pi-worksgood/` artifact — `npm test` builds first.
 */

import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, it, expect, vi, afterEach } from "vitest";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { assertConsolePluginCurrent, isWorkerSession, readEmbedDigestAt } from "../pi-worksgood/index.js";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { WgBackend } from "../pi-worksgood/wg-backend.js";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { WG_PI_PLUGIN_COMPAT_VERSION as COMPAT } from "../pi-worksgood/version.js";

const tmpDirs: string[] = [];
function scratch(): string {
  const dir = mkdtempSync(path.join(tmpdir(), "wg-console-fresh-"));
  tmpDirs.push(dir);
  return dir;
}
afterEach(() => {
  while (tmpDirs.length) rmSync(tmpDirs.pop()!, { recursive: true, force: true });
  delete process.env.WG_PI_PLUGIN_COMPAT_VERSION;
  delete process.env.WG_AGENT_ID;
  delete process.env.WG_WORKER_CAPABILITY;
});

interface FakeBackend {
  backend: InstanceType<typeof WgBackend>;
  calls: string[][];
}

function fakeBackend(opts: { compat?: string; digest?: string; stderr?: string }): FakeBackend {
  const calls: string[][] = [];
  const host = {
    exec: async (_command: string, args: string[]) => {
      calls.push(args);
      if (args.includes("compat-version")) {
        return { stdout: `${opts.compat ?? COMPAT}\n`, stderr: opts.stderr ?? "", code: 0, killed: false };
      }
      if (args.includes("digest")) {
        return { stdout: `${opts.digest ?? "b3:current"}\n`, stderr: "", code: 0, killed: false };
      }
      if (args.includes("install")) {
        return { stdout: "installed", stderr: "", code: 0, killed: false };
      }
      return { stdout: "", stderr: "", code: 0, killed: false };
    },
  };
  return { backend: new WgBackend(host as never, {}), calls };
}

describe("readEmbedDigestAt", () => {
  it("reads the materialization stamp when present", () => {
    const dir = scratch();
    mkdirSync(path.join(dir, "pi-worksgood"));
    writeFileSync(path.join(dir, ".wg-embed-digest"), "b3:abc123\n");
    expect(readEmbedDigestAt(dir)).toBe("b3:abc123");
  });

  it("returns undefined for a dev / npm install with no companion stamp", () => {
    expect(readEmbedDigestAt(scratch())).toBeUndefined();
  });
});

describe("isWorkerSession", () => {
  it("is true when WG_AGENT_ID identifies a spawned agent", () => {
    expect(isWorkerSession({ WG_AGENT_ID: "agent-231" })).toBe(true);
  });

  it("is true for an attempt-scoped sandbox holding WG_WORKER_CAPABILITY", () => {
    expect(isWorkerSession({ WG_WORKER_CAPABILITY: "tok" })).toBe(true);
  });

  it("is false for a human console (no worker env)", () => {
    expect(isWorkerSession({})).toBe(false);
    expect(isWorkerSession({ WG_AGENT_ID: "   " })).toBe(false);
  });
});

describe("assertConsolePluginCurrent", () => {
  it("skips ENTIRELY in a worker session (WG_AGENT_ID): zero wg subprocesses at load", async () => {
    process.env.WG_AGENT_ID = "agent-231";
    const { backend, calls } = fakeBackend({ digest: "b3:new-binary" });
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    await assertConsolePluginCurrent(backend, "b3:old-binary");
    expect(calls.length).toBe(0);
    expect(err).not.toHaveBeenCalled();
    err.mockRestore();
  });

  it("skips ENTIRELY in a capability sandbox (WG_WORKER_CAPABILITY)", async () => {
    process.env.WG_WORKER_CAPABILITY = "attempt-token";
    const { backend, calls } = fakeBackend({ compat: "9.9.9", digest: "b3:new-binary" });
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    await assertConsolePluginCurrent(backend, "b3:old-binary");
    expect(calls.length).toBe(0);
    expect(err).not.toHaveBeenCalled();
    err.mockRestore();
  });

  it("still runs the digest check in a console session (no worker env)", async () => {
    const { backend, calls } = fakeBackend({ digest: "b3:new-binary" });
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    await assertConsolePluginCurrent(backend, "b3:old-binary");
    expect(calls.some((c) => c.includes("digest"))).toBe(true);
    expect(err.mock.calls.map((c) => String(c[0])).join("\n")).toContain("stale plugin cache");
    err.mockRestore();
  });

  it("is a silent no-op when the loaded stamp matches this wg's embed", async () => {
    const { backend, calls } = fakeBackend({ digest: "b3:same" });
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    await assertConsolePluginCurrent(backend, "b3:same");
    expect(err).not.toHaveBeenCalled();
    expect(calls.some((c) => c.includes("install"))).toBe(false);
    err.mockRestore();
  });

  it("warns loudly and self-heals on a stale cache (digest mismatch)", async () => {
    const { backend, calls } = fakeBackend({ digest: "b3:new-binary" });
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    await assertConsolePluginCurrent(backend, "b3:old-binary");
    const text = err.mock.calls.map((c) => String(c[0])).join("\n");
    expect(text).toContain("stale plugin cache");
    expect(text).toContain("restart pi");
    expect(calls.some((c) => c.includes("install"))).toBe(true);
    err.mockRestore();
  });

  it("warns on a compat mismatch even without a stamp (dev install)", async () => {
    const { backend, calls } = fakeBackend({ compat: "9.9.9" });
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    await assertConsolePluginCurrent(backend, "");
    const text = err.mock.calls.map((c) => String(c[0])).join("\n");
    expect(text).toContain("compat mismatch");
    // A stamp-less dev/npm build must not trigger a cache repair.
    expect(calls.some((c) => c.includes("install"))).toBe(false);
    err.mockRestore();
  });

  it("does nothing when wg injected the compat env (hermetic spawn)", async () => {
    process.env.WG_PI_PLUGIN_COMPAT_VERSION = COMPAT;
    const { backend, calls } = fakeBackend({});
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    await assertConsolePluginCurrent(backend, "b3:old");
    expect(err).not.toHaveBeenCalled();
    expect(calls.length).toBe(0);
    err.mockRestore();
  });

  it("forwards the Rust self-heal warning from compat-version stderr", async () => {
    const { backend } = fakeBackend({ stderr: "console plugin was stale — refreshed\n" });
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    await assertConsolePluginCurrent(backend, "b3:same");
    const text = err.mock.calls.map((c) => String(c[0])).join("\n");
    expect(text).toContain("console plugin was stale");
    err.mockRestore();
  });
});
