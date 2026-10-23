/**
 * bash-timeout.ts — FIX A of the tool-hang hotfix.
 *
 * Pi's built-in `bash` tool has **no default timeout**:
 * `resolveTimeoutMs(undefined) => undefined`, so a model-generated command that
 * reads stdin (a half-open heredoc, a `cat` with no redirect, an interactive
 * prompt) waits forever at 0% CPU. That is exactly how a WG worker hung for 21
 * hours on 2026-10-23 while its observer spun.
 *
 * Pi documents that a `tool_call` handler **can mutate `event.input` in place**
 * before the tool runs (docs/extensions.md, `tool_call`), so this module
 * injects a bounded default `timeout` (seconds) into a `bash` call **only when
 * the model omitted one**. An explicit model-provided timeout is always
 * honored — we never shrink a caller's deliberate long-running step.
 *
 * On expiry pi returns an ordinary error result to the model ("timeout"), which
 * is the desired recovery: the model sees the failure and retries with
 * corrected quoting instead of waiting forever.
 *
 * Resolution order (first win):
 *   1. `$WG_PI_BASH_TIMEOUT_SECS` (env — per-spawn / per-worker override);
 *   2. `bashTimeoutSecs` in the extension config file
 *      (`~/.pi/agent/extensions/pi-worksgood/config.json`, honouring
 *      `PI_CODING_AGENT_DIR`, same file the FleetView gate reads);
 *   3. {@link DEFAULT_BASH_TIMEOUT_SECS} (600s = 10 min).
 *
 * 600s is deliberately chosen: long enough for any legitimate inline
 * build/test step a worker runs, short enough that a wedged stdin read dies in
 * minutes. `0` (env or file) explicitly disables the injection.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { existsSync, readFileSync } from "node:fs";
import { fleetConfigPaths } from "./fleet-config.js";

/** Injected default when neither the env nor the config file says otherwise. */
export const DEFAULT_BASH_TIMEOUT_SECS = 600;

/** Env override name (documented in the plugin README / hotfix notes). */
export const BASH_TIMEOUT_ENV = "WG_PI_BASH_TIMEOUT_SECS";

/** The config-file key inside the extension config JSON. */
export const BASH_TIMEOUT_CONFIG_KEY = "bashTimeoutSecs";

/** Only this tool carries the hazardous unbounded-stdin shape. */
export const BASH_TOOL_NAME = "bash";

/** Minimal IO seam so tests can inject a config file without touching `$HOME`. */
export interface BashTimeoutIO {
  exists: (path: string) => boolean;
  readFile: (path: string) => string;
}

const defaultIO: BashTimeoutIO = {
  exists: (path) => {
    try {
      return existsSync(path);
    } catch {
      return false;
    }
  },
  readFile: (path) => readFileSync(path, "utf8"),
};

/**
 * Coerce a raw config/env value into a usable timeout in seconds.
 *
 * Returns `undefined` for "not usable" so the caller can fall through to the
 * next source. `0` is preserved (an explicit disable), negative/NaN/absurd
 * values are rejected rather than silently clamped — a broken knob must not
 * quietly re-enable the hang.
 */
export function parseBashTimeoutSecs(value: unknown): number | undefined {
  if (value === undefined || value === null || value === "") return undefined;
  const n = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(n) || !Number.isInteger(n) || n < 0) return undefined;
  // pi itself caps at 2_147_483_647s; anything beyond a day is certainly a typo.
  if (n > 86_400) return undefined;
  return n;
}

/** Pure parse of a raw extension-config object (unknown keys ignored). */
export function parseBashTimeoutConfig(raw: unknown): number | undefined {
  const obj =
    raw && typeof raw === "object" && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {};
  return parseBashTimeoutSecs(obj[BASH_TIMEOUT_CONFIG_KEY]);
}

/**
 * Resolve the effective injected timeout (seconds). `0` means "do not inject".
 * Env wins over the file; a missing/corrupt file degrades to the default.
 */
export function readBashTimeoutConfig(
  env: Record<string, string | undefined> = process.env,
  io: BashTimeoutIO = defaultIO,
  explicitPath?: string,
): number {
  const fromEnv = parseBashTimeoutSecs(env[BASH_TIMEOUT_ENV]);
  if (fromEnv !== undefined) return fromEnv;
  const paths = explicitPath ? [explicitPath] : fleetConfigPaths(env);
  for (const path of paths) {
    if (!io.exists(path)) continue;
    try {
      const parsed = parseBashTimeoutConfig(JSON.parse(io.readFile(path)));
      if (parsed !== undefined) return parsed;
    } catch {
      // Corrupt config: fall through to the safe default.
    }
    break;
  }
  return DEFAULT_BASH_TIMEOUT_SECS;
}

/** The shape of the slice of pi's `tool_call` event we touch. */
export interface MutableToolCallEvent {
  toolName: string;
  /** `unknown` so the whole pi `ToolCallEvent` union is assignable here. */
  input?: unknown;
}

/**
 * Inject the default timeout into a `bash` tool call **iff** the model omitted
 * one. Mutates `event.input` in place (the documented pi contract) and returns
 * `true` when it injected, `false` otherwise.
 */
export function applyBashTimeout(event: MutableToolCallEvent, timeoutSecs: number): boolean {
  if (timeoutSecs <= 0) return false;
  if (event.toolName !== BASH_TOOL_NAME) return false;
  const input = event.input as Record<string, unknown> | undefined;
  if (!input || typeof input !== "object") return false;
  // Honor an explicit model-provided value, including an out-of-range one:
  // pi's own validator should reject `-1`/`Infinity` rather than us silently
  // rewriting the caller's intent.
  if (input.timeout !== undefined && input.timeout !== null) return false;
  input.timeout = timeoutSecs;
  return true;
}

/**
 * Register the `tool_call` mutator (session-scoped; pi drops it at shutdown).
 *
 * Handler failures are reported by pi and block the tool (fail-safe), so this
 * handler is intentionally total — it never throws on a malformed event.
 */
export function installBashToolTimeout(
  pi: Pick<ExtensionAPI, "on">,
  timeoutSecs: number,
): void {
  pi.on("tool_call", (event) => {
    try {
      applyBashTimeout(event, timeoutSecs);
    } catch {
      // Never break a tool call because the guard could not inspect it.
    }
  });
}
