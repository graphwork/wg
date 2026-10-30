/**
 * stack-status.ts — the `/wg status` STACK line: is the session running a
 * current build, or a stale daemon/plugin pair?
 *
 * The operator report that opened this work had an environment cause before it
 * had a UI cause: "the operator's daemon was running an OLD binary (dev-check
 * WARN) and the plugin cache needed re-sync — a stale daemon payload lacks the
 * RGB palette => the panel's FALLBACK palette renders (wrong colours)". That is
 * a stack fact, and it was only visible OUTSIDE the session (`wg dev-check`,
 * `wg pi-plugin status`).
 *
 * This module makes it visible INSIDE the session, from two read-only probes the
 * plugin already had reason to make:
 *
 *  - `wg status --json` → the daemon's liveness + uptime, and the binary
 *    provenance warning (`build/wg-binary-from-worktree`);
 *  - `wg pi-plugin status` → the RUNNING BINARY's embed digest and the on-disk
 *    plugin cache state (`current` / `DRIFTED` / `MISSING`);
 *  - this build's own `.wg-embed-digest` stamp ({@link ownBuildDigest}).
 *
 * The honest verdict is one of `current` / `skew` / `drift` / `unknown`, and it
 * FAILS CLOSED to `unknown` (never a confident "current") when a probe is
 * refused — notably in a WG worker session, where `wg pi-plugin …` is an admin
 * verb the worker gate refuses by design (`worker_control.admin_operation_refused`).
 */

import { compareBuildStamps, shortDigest, type BuildSkew } from "./plugin-stamp.js";
import type { WgRunOptions } from "./wg-backend.js";

/** The read-only backend slice the probes need (structural: tests pass stubs). */
export interface StackProbeBackend {
  statusJson?: <T = unknown>(opts?: WgRunOptions) => Promise<T | null>;
  piPluginStatus?: (opts?: WgRunOptions) => Promise<{ stdout?: string; code?: number }>;
}

/** The slice of `wg status --json` the stack line reads. */
export interface StackStatusJson {
  service?: { running?: boolean; pid?: number; uptime?: string };
  /** Set when the running `wg` binary was built from a WG worktree. */
  build_warning?: string;
}

/** The `wg pi-plugin status` fields the stack line reads. */
export interface PluginCacheStatus {
  /** The running binary's embedded build digest (`embed digest:`). */
  embedDigest?: string;
  /** The on-disk cache's digest (`cache digest:`), `<none>` when absent. */
  cacheDigest?: string;
  /** `current` when the cache matches this binary's embed, else drifted/missing/unknown. */
  cacheState: "current" | "stale" | "missing" | "unknown";
}

/** One-word stack verdict. */
export type StackVerdict = "current" | "skew" | "drift" | "unknown";

export interface StackStatusInput {
  /** `wg status --json`, or `null` when the probe failed/refused. */
  status: StackStatusJson | null;
  /** `wg pi-plugin status` stdout, or `null` when refused/unavailable. */
  pluginStatusText: string | null;
  /** This plugin build's own materialization stamp (undefined for dev/npm). */
  pluginDigest: string | undefined;
  /** True in a WG-managed worker/agent session (admin probes are refused). */
  workerSession: boolean;
}

export interface StackStatus {
  verdict: StackVerdict;
  /** One compact line, ready to append to the `/wg status` output. */
  text: string;
}

/** Parse the `embed digest:` / `cache digest:` / `cache state:` lines. */
export function parsePluginCacheStatus(stdout: string | null): PluginCacheStatus | null {
  if (!stdout || !stdout.trim()) return null;
  const pick = (label: string): string | undefined => {
    const match = new RegExp(`${label}:\\s*(.+)`, "i").exec(stdout);
    const value = match?.[1]?.trim();
    return value && value !== "<none>" ? value : undefined;
  };
  const stateRaw = pick("cache state")?.toLowerCase() ?? "";
  const cacheState: PluginCacheStatus["cacheState"] = stateRaw.startsWith("current")
    ? "current"
    : stateRaw.startsWith("drift")
      ? "stale"
      : stateRaw.startsWith("missing")
        ? "missing"
        : "unknown";
  return {
    embedDigest: pick("embed digest"),
    cacheDigest: pick("cache digest"),
    cacheState,
  };
}

/**
 * Build the `/wg status` stack segment.
 *
 * The line names the DAEMON-BUILD AGE (a running daemon whose binary embeds a
 * different plugin build than the loaded one is the reported bad state) and the
 * PLUGIN-CACHE CURRENCY, and says exactly what to run when they disagree.
 */
export function formatStackStatus(input: StackStatusInput): StackStatus {
  const service = input.status?.service;
  const running = service?.running === true;
  const up = service?.uptime ? ` up ${service.uptime}` : "";
  const daemon = input.status ? (running ? `daemon${up}` : "daemon down") : "daemon status unavailable";
  const worktree = input.status?.build_warning ? " · WORKTREE BUILD (wg dev-sync is refused here)" : "";

  // A worker session cannot probe the plugin cache (admin verb, refused by the
  // worker gate). Say that plainly rather than reporting a guess as health.
  if (input.workerSession) {
    return {
      verdict: "unknown",
      text: `stack: ${daemon} · plugin cache: not probed (worker session)${worktree}`,
    };
  }

  const plugin = parsePluginCacheStatus(input.pluginStatusText);
  if (!plugin) {
    return {
      verdict: "unknown",
      text: `stack: ${daemon} · plugin cache: unknown (wg pi-plugin status unavailable)${worktree}`,
    };
  }

  const skew: BuildSkew = compareBuildStamps(input.pluginDigest, plugin.embedDigest);
  const mine = shortDigest(input.pluginDigest);
  const bin = shortDigest(plugin.embedDigest);

  // The cache on disk drifts from the binary that materialized it: the binary
  // was upgraded (or replaced) without a re-ensure.
  if (plugin.cacheState !== "current") {
    const label =
      plugin.cacheState === "missing"
        ? "MISSING"
        : plugin.cacheState === "stale"
          ? "DRIFTED"
          : "UNKNOWN";
    return {
      verdict: "drift",
      text:
        `stack: STALE — plugin cache ${label} vs this wg binary (${bin}); ` +
        `run wg dev-sync (or wg pi-plugin install) then restart pi · ${daemon}${worktree}`,
    };
  }

  // The LOADED build is not the binary's build: an old daemon/session payload.
  // This is the reported wrong-colour case, now nameable from inside.
  if (skew === "skew") {
    return {
      verdict: "skew",
      text:
        `stack: STALE — this plugin build ${mine} ≠ wg binary embed ${bin} ` +
        `(daemon older than plugin); run wg dev-sync then restart pi · ${daemon}${worktree}`,
    };
  }

  const cache = `plugin cache: current (${bin})`;
  const build = skew === "match" ? `build matches ${mine}` : "build: no embed stamp (dev/npm install)";
  return {
    verdict: "current",
    text: `stack: ${daemon} · ${build} · ${cache}${worktree}`,
  };
}

/**
 * Run the read-only probes and format the segment. Never throws: a `/wg status`
 * line must always render, even when every probe is refused or the daemon is
 * gone.
 */
export async function probeStackStatus(
  backend: StackProbeBackend,
  opts: { pluginDigest: string | undefined; workerSession: boolean },
): Promise<StackStatus> {
  let status: StackStatusJson | null = null;
  try {
    status = (await backend.statusJson?.<StackStatusJson>()) ?? null;
  } catch {
    status = null;
  }
  let pluginStatusText: string | null = null;
  if (!opts.workerSession) {
    try {
      const result = await backend.piPluginStatus?.();
      if (result && (result.code === 0 || result.code === undefined) && result.stdout) {
        pluginStatusText = result.stdout;
      }
    } catch {
      pluginStatusText = null;
    }
  }
  return formatStackStatus({
    status,
    pluginStatusText,
    pluginDigest: opts.pluginDigest,
    workerSession: opts.workerSession,
  });
}
