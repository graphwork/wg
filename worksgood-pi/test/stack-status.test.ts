/**
 * stack-status tests — the `/wg status` STACK line (daemon build age +
 * plugin-cache currency).
 *
 * The operator-reported fleet-panel bug had an environment cause first: the
 * session ran an OLD daemon binary and a plugin cache that needed a re-sync, so
 * the panel's palette fell back and nothing inside the session said why. These
 * tests pin that a stale stack is NAMED (with the recovery command) and that a
 * refused probe fails closed to `unknown` instead of reporting fake health.
 *
 * Tests run against the built `pi-worksgood/` artifact — `npm test` builds first.
 */

import { describe, expect, it } from "vitest";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { compareBuildStamps, shortDigest } from "../pi-worksgood/plugin-stamp.js";
// @ts-expect-error — built ESM artifact import (see above)
import {
  formatStackStatus,
  parsePluginCacheStatus,
  probeStackStatus,
} from "../pi-worksgood/stack-status.js";

const OK_STATUS = { service: { running: true, uptime: "2h" } };

function pluginText(opts: { embed?: string; cache?: string; state?: string } = {}): string {
  const embed = opts.embed ?? "b3:aaaa1111bbbb";
  const cache = opts.cache ?? embed;
  return [
    "WorksGood Pi integration status (pi-worksgood / @worksgood/pi)",
    "  compat version:   0.3.0",
    `  embed digest:     ${embed}`,
    `  cache digest:     ${cache}`,
    `  cache state:      ${opts.state ?? "current (matches this binary's embedded build)"}`,
  ].join("\n");
}

describe("parsePluginCacheStatus", () => {
  it("reads the embed/cache digests and the cache state", () => {
    const parsed = parsePluginCacheStatus(pluginText({ embed: "b3:new", cache: "b3:new" }));
    expect(parsed).toEqual({ embedDigest: "b3:new", cacheDigest: "b3:new", cacheState: "current" });
  });

  it("maps a DRIFTED / MISSING cache honestly", () => {
    expect(parsePluginCacheStatus(pluginText({ state: "DRIFTED — stale" }))?.cacheState).toBe("stale");
    expect(parsePluginCacheStatus(pluginText({ state: "MISSING — run install" }))?.cacheState).toBe("missing");
  });

  it("treats <none> as absent and empty output as unavailable", () => {
    expect(parsePluginCacheStatus("  cache digest:     <none>\n  cache state: current")?.cacheDigest).toBeUndefined();
    expect(parsePluginCacheStatus("")).toBeNull();
    expect(parsePluginCacheStatus(null)).toBeNull();
  });
});

describe("compareBuildStamps", () => {
  it("matches identical digests, flags a difference, and never guesses without both", () => {
    expect(compareBuildStamps("b3:a", "b3:a")).toBe("match");
    expect(compareBuildStamps("b3:a", "b3:b")).toBe("skew");
    expect(compareBuildStamps(undefined, "b3:b")).toBe("unknown");
    expect(compareBuildStamps("b3:a", undefined)).toBe("unknown");
  });

  it("shortens a digest for the one-line status", () => {
    expect(shortDigest("b3:0123456789abcdef")).toBe("b3:01234567…");
    expect(shortDigest("b3:abc")).toBe("b3:abc");
    expect(shortDigest(undefined)).toBe("unknown");
  });
});

describe("formatStackStatus", () => {
  const base = { status: OK_STATUS, pluginDigest: "b3:same", workerSession: false };

  it("reports a current stack with the daemon age + cache currency", () => {
    const stack = formatStackStatus({
      ...base,
      pluginStatusText: pluginText({ embed: "b3:same", cache: "b3:same" }),
    });
    expect(stack.verdict).toBe("current");
    expect(stack.text).toContain("daemon up 2h");
    expect(stack.text).toContain("build matches b3:same");
    expect(stack.text).toContain("plugin cache: current");
    expect(stack.text).not.toContain("STALE");
  });

  it("NAMES a stale loaded build (the reported wrong-colour cause) with the fix", () => {
    const stack = formatStackStatus({
      ...base,
      pluginStatusText: pluginText({ embed: "b3:newerbuild", cache: "b3:newerbuild" }),
    });
    expect(stack.verdict).toBe("skew");
    expect(stack.text).toContain("STALE");
    expect(stack.text).toContain("daemon older than plugin");
    expect(stack.text).toContain("wg dev-sync");
    expect(stack.text).toContain("b3:same");
  });

  it("NAMES a drifted/missing plugin cache with the fix", () => {
    const drifted = formatStackStatus({
      ...base,
      pluginStatusText: pluginText({ embed: "b3:same", cache: "b3:old", state: "DRIFTED — stale cache" }),
    });
    expect(drifted.verdict).toBe("drift");
    expect(drifted.text).toContain("DRIFTED");
    expect(drifted.text).toContain("wg dev-sync");

    const missing = formatStackStatus({
      ...base,
      pluginStatusText: pluginText({ embed: "b3:same", state: "MISSING — run install" }),
    });
    expect(missing.verdict).toBe("drift");
    expect(missing.text).toContain("MISSING");
  });

  it("surfaces the worktree-build binary warning", () => {
    const stack = formatStackStatus({
      ...base,
      status: { ...OK_STATUS, build_warning: "build/wg-binary-from-worktree: …" },
      pluginStatusText: pluginText({ embed: "b3:same" }),
    });
    expect(stack.text).toContain("WORKTREE BUILD");
  });

  it("fails CLOSED to unknown when a probe is refused or unavailable", () => {
    const worker = formatStackStatus({ ...base, pluginStatusText: pluginText(), workerSession: true });
    expect(worker.verdict).toBe("unknown");
    expect(worker.text).toContain("worker session");

    const noPluginStatus = formatStackStatus({ ...base, pluginStatusText: null });
    expect(noPluginStatus.verdict).toBe("unknown");
    expect(noPluginStatus.text).toContain("pi-plugin status unavailable");

    const noDaemon = formatStackStatus({
      status: null,
      pluginStatusText: pluginText({ embed: "b3:same" }),
      pluginDigest: "b3:same",
      workerSession: false,
    });
    expect(noDaemon.text).toContain("daemon status unavailable");
  });

  it("reports daemon down and a dev/npm install without a stamp", () => {
    const down = formatStackStatus({
      status: { service: { running: false } },
      pluginStatusText: pluginText({ embed: "b3:same" }),
      pluginDigest: undefined,
      workerSession: false,
    });
    expect(down.text).toContain("daemon down");
    expect(down.text).toContain("no embed stamp");
    expect(down.verdict).toBe("current");
  });
});

describe("probeStackStatus", () => {
  it("runs both read-only probes and combines them", async () => {
    const calls: string[] = [];
    const backend = {
      statusJson: async () => {
        calls.push("status");
        return OK_STATUS;
      },
      piPluginStatus: async () => {
        calls.push("plugin");
        return { stdout: pluginText({ embed: "b3:same" }), code: 0 };
      },
    };
    const stack = await probeStackStatus(backend, { pluginDigest: "b3:same", workerSession: false });
    expect(calls).toEqual(["status", "plugin"]);
    expect(stack.verdict).toBe("current");
  });

  it("never probes an admin verb in a worker session", async () => {
    const calls: string[] = [];
    const backend = {
      statusJson: async () => {
        calls.push("status");
        return OK_STATUS;
      },
      piPluginStatus: async () => {
        calls.push("plugin");
        return { stdout: pluginText(), code: 0 };
      },
    };
    const stack = await probeStackStatus(backend, { pluginDigest: "b3:same", workerSession: true });
    expect(calls).toEqual(["status"]);
    expect(stack.verdict).toBe("unknown");
  });

  it("survives a throwing probe (a status line must always render)", async () => {
    const backend = {
      statusJson: async () => {
        throw new Error("no daemon");
      },
      piPluginStatus: async () => {
        throw new Error("refused");
      },
    };
    const stack = await probeStackStatus(backend, { pluginDigest: undefined, workerSession: false });
    expect(stack.verdict).toBe("unknown");
    expect(stack.text).toContain("daemon status unavailable");
  });
});
