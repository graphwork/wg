/**
 * fleet-config.ts — the config gate for the FleetView bottom panel.
 *
 * The FleetView panel is **optional**: nothing is mounted below the editor
 * unless the operator opts in. The opt-in follows the same **extension-config
 * file pattern** pi-subagents uses
 * (`~/.pi/agent/extensions/<name>/config.json`, honouring `PI_CODING_AGENT_DIR`
 * and `HOME`), so a WG operator already has a familiar place to flip it on:
 *
 *   ~/.pi/agent/extensions/pi-worksgood/config.json
 *   {
 *     "fleetView": true,           // default: true — the ambient strip is visible
 *                                  // when this session/project is active
 *     "fleetViewCollapsed": true,  // start FOLDED (the one-line live status strip)
 *     "fleetViewExpanded": false,  // unfolded: compact body (a few live agents)
 *     "fleetViewPlacement": "belowEditor",
 *     "fleetViewPollMs": 5000,
 *     "fleetViewHeightPct": 0.6       // how much of the height the cockpit covers
 *   }
 *
 * **The strip is the default surface, not the exception.** It used to default to
 * OFF, so a fresh operator saw nothing clickable and concluded the clickable
 * fleet did not exist. The one-line live status strip is now visible by default
 * whenever WG is active in the session (or the project is marked active), and a
 * dormant-but-WG project still gets a one-line discovery hint. `fleetView:
 * false` / `WG_PI_FLEET_VIEW=0` is the (honoured) opt-out; the FULL scrollable
 * cockpit stays an explicit open (`/wg-fleet`, or a click on the strip).
 *
 * Resolution order (first win):
 *   1. explicit env override (`WG_PI_FLEET_VIEW`, `WG_PI_FLEET_VIEW_COLLAPSED`,
 *      `WG_PI_FLEET_VIEW_EXPANDED`, `WG_PI_FLEET_VIEW_POLL_MS`);
 *   2. the first existing config file among the candidate dirs;
 *   3. the safe default (`enabled: true`, `collapsed: true`).
 *
 * Reads are pure/read-only. The ONE write is
 * {@link writeFleetViewHeightPct} — the cockpit's drag/keyboard resize persists
 * the operator's chosen `fleetViewHeightPct` so it sticks across opens (the
 * operator-reported gap: "no click-drag or way to modulate the screen fraction
 * covered"). It rewrites only that one key, atomically, and never touches any
 * other config field.
 */

import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

/** Where the widget is anchored. Mirrors pi's `WidgetPlacement`. */
export type FleetPlacement = "aboveEditor" | "belowEditor";

export interface FleetViewConfig {
  /**
   * Mount the ambient surface at all. Default **true** — the one-line strip is
   * the visible affordance; `false` (config `fleetView: false` or
   * `WG_PI_FLEET_VIEW=0`) is an explicit opt-out.
   */
  enabled: boolean;
  /**
   * Start FOLDED: the strip renders as ONE line (the live status summary). This
   * is the pi-subagents `asyncWidgetCollapsed` semantics — folding is a click
   * away, persists across updates, and resets when the widget is re-mounted.
   * Default **true** (the strip is the default surface).
   */
  collapsed: boolean;
  /** Unfolded body layout: `true` lists every live agent, `false` shows a few. */
  expanded: boolean;
  /** Widget placement; defaults to `belowEditor` (the FleetView feel). */
  placement: FleetPlacement;
  /** Bounded poll cadence in milliseconds. */
  pollMs: number;
  /**
   * Fraction (`0.2`–`1`) of the terminal height the `/wg-fleet` cockpit covers.
   * Default **1** (the whole interactive area, today's behaviour). Set by
   * dragging the panel's top edge or with `+`/`-` (`]`/`[`), and persisted into
   * the extension config by {@link writeFleetViewHeightPct}.
   */
  heightPct: number;
}

/** Smallest cockpit fraction (three rows' worth on a normal terminal). */
export const MIN_FLEET_HEIGHT_PCT = 0.2;
/** Largest cockpit fraction (the whole interactive area). */
export const MAX_FLEET_HEIGHT_PCT = 1;
/** The config key the panel's resize persists. */
export const FLEET_HEIGHT_PCT_KEY = "fleetViewHeightPct";
/** Env override for the cockpit height fraction. */
export const FLEET_HEIGHT_PCT_ENV = "WG_PI_FLEET_HEIGHT_PCT";

/** Safe defaults: visible one-line strip, compact body, below the editor, 5s cadence. */
export const DEFAULT_FLEET_VIEW_CONFIG: FleetViewConfig = {
  enabled: true,
  collapsed: true,
  expanded: false,
  placement: "belowEditor",
  pollMs: 5000,
  heightPct: MAX_FLEET_HEIGHT_PCT,
};

/**
 * Candidate extension-config directories under `<agent-dir>/extensions/`.
 * pi-subagents uses its display name (`subagent`); the WG plugin's display name
 * is `pi-worksgood`, so that is primary. `worksgood`/`wg` are accepted aliases
 * so an operator who guessed either spelling is not surprised by a no-op.
 */
export const FLEET_CONFIG_DIRS: readonly string[] = ["pi-worksgood", "worksgood", "wg"];

/** The pi agent dir: `$PI_CODING_AGENT_DIR` (with `~` expansion) or `$HOME/.pi/agent`. */
export function resolveAgentDir(env: Record<string, string | undefined> = process.env): string {
  const configured = env.PI_CODING_AGENT_DIR?.trim();
  const home = env.HOME || env.USERPROFILE || homedir();
  if (configured && configured !== "~") {
    if (configured.startsWith("~/") || configured.startsWith("~\\")) {
      return join(home, configured.slice(2));
    }
    return configured;
  }
  return join(home, ".pi", "agent");
}

/** Ordered candidate config paths (the first existing one wins). */
export function fleetConfigPaths(env: Record<string, string | undefined> = process.env): string[] {
  return FLEET_CONFIG_DIRS.map((dir) => join(resolveAgentDir(env), "extensions", dir, "config.json"));
}

/** IO seam so tests can inject a config file without touching `$HOME`. */
export interface FleetConfigIO {
  exists: (path: string) => boolean;
  readFile: (path: string) => string;
  /** Writer seam for {@link writeFleetViewHeightPct} (`fs.mkdirSync -p`). */
  mkdir?: (path: string) => void;
  /** Writer seam for {@link writeFleetViewHeightPct}. */
  writeFile?: (path: string, data: string) => void;
  /** Writer seam for the atomic rename. */
  rename?: (from: string, to: string) => void;
}

const defaultIO: FleetConfigIO = {
  exists: (path) => {
    try {
      return existsSync(path);
    } catch {
      return false;
    }
  },
  readFile: (path) => readFileSync(path, "utf8"),
  mkdir: (path) => {
    mkdirSync(path, { recursive: true });
  },
  writeFile: (path, data) => {
    writeFileSync(path, data, "utf8");
  },
  rename: (from, to) => {
    renameSync(from, to);
  },
};

function asBool(value: unknown, fallback: boolean): boolean {
  if (value === undefined || value === null || value === "") return fallback;
  if (typeof value === "boolean") return value;
  if (typeof value === "string") {
    const v = value.trim().toLowerCase();
    if (["off", "0", "false", "no"].includes(v)) return false;
    if (["on", "1", "true", "yes"].includes(v)) return true;
  }
  return fallback;
}

function asPlacement(value: unknown, fallback: FleetPlacement): FleetPlacement {
  return value === "aboveEditor" || value === "belowEditor" ? value : fallback;
}

function asPollMs(value: unknown, fallback: number): number {
  const n = typeof value === "number" ? value : Number(value);
  return Number.isFinite(n) && n >= 1000 ? Math.floor(n) : fallback;
}

/** Clamp a height fraction into `[MIN, MAX]` (NaN/garbage → `fallback`). */
/**
 * Clamp an ALREADY-FRACTIONAL cockpit height (`0.2`–`1`) into range.
 *
 * This is the runtime clamp used by the panel's drag/keyboard resize, so it must
 * not reinterpret its input as a percentage: `1.05` (one step past the top) is
 * "too tall, clamp to 1", never "1.05 percent, clamp to the floor". Non-finite
 * input degrades to `fallback`.
 */
export function clampHeightFraction(value: unknown, fallback = MAX_FLEET_HEIGHT_PCT): number {
  const n = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(n)) return fallback;
  return Math.min(MAX_FLEET_HEIGHT_PCT, Math.max(MIN_FLEET_HEIGHT_PCT, n));
}

/**
 * Parse a CONFIGURED cockpit height: a fraction (`0.6`) or a percent (`60`).
 * Non-positive / non-numeric values degrade to `fallback` (the default).
 */
export function clampHeightPct(value: unknown, fallback = MAX_FLEET_HEIGHT_PCT): number {
  const n = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(n) || n <= 0) return fallback;
  const normalized = n > 1 ? n / 100 : n; // tolerate a percent (e.g. 60)
  return clampHeightFraction(normalized, fallback);
}

function asHeightPct(value: unknown, fallback: number): number {
  if (value === undefined || value === null || value === "") return fallback;
  const n = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(n) || n <= 0) return fallback;
  return clampHeightPct(n, fallback);
}

/** Pure parse of a raw config object (unknown keys ignored, bad types degraded). */
export function parseFleetViewConfig(raw: unknown): FleetViewConfig {
  const obj = raw && typeof raw === "object" && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {};
  return {
    enabled: asBool(obj.fleetView, DEFAULT_FLEET_VIEW_CONFIG.enabled),
    collapsed: asBool(obj.fleetViewCollapsed, DEFAULT_FLEET_VIEW_CONFIG.collapsed),
    expanded: asBool(obj.fleetViewExpanded, DEFAULT_FLEET_VIEW_CONFIG.expanded),
    placement: asPlacement(obj.fleetViewPlacement, DEFAULT_FLEET_VIEW_CONFIG.placement),
    pollMs: asPollMs(obj.fleetViewPollMs, DEFAULT_FLEET_VIEW_CONFIG.pollMs),
    heightPct: asHeightPct(obj[FLEET_HEIGHT_PCT_KEY], DEFAULT_FLEET_VIEW_CONFIG.heightPct),
  };
}

/**
 * Resolve the effective FleetView config. Env overrides win over the file; a
 * missing/corrupt file degrades to the safe default rather than throwing (a
 * bad config must never take a pi session down).
 */
export function readFleetViewConfig(
  env: Record<string, string | undefined> = process.env,
  io: FleetConfigIO = defaultIO,
  explicitPath?: string,
): FleetViewConfig {
  let config = { ...DEFAULT_FLEET_VIEW_CONFIG };
  const paths = explicitPath ? [explicitPath] : fleetConfigPaths(env);
  for (const path of paths) {
    if (!io.exists(path)) continue;
    try {
      config = parseFleetViewConfig(JSON.parse(io.readFile(path)));
    } catch {
      // Corrupt config: keep the safe defaults.
    }
    break;
  }
  return {
    enabled: asBool(env.WG_PI_FLEET_VIEW, config.enabled),
    collapsed: asBool(env.WG_PI_FLEET_VIEW_COLLAPSED, config.collapsed),
    expanded: asBool(env.WG_PI_FLEET_VIEW_EXPANDED, config.expanded),
    placement: asPlacement(env.WG_PI_FLEET_VIEW_PLACEMENT, config.placement),
    pollMs: asPollMs(env.WG_PI_FLEET_VIEW_POLL_MS, config.pollMs),
    heightPct: asHeightPct(env[FLEET_HEIGHT_PCT_ENV], config.heightPct),
  };
}

/**
 * Persist the operator's chosen cockpit height fraction.
 *
 * Writes `fleetViewHeightPct` into the FIRST existing extension config file (or
 * the primary `pi-worksgood` path when none exists), preserving every other key
 * verbatim. The write is atomic (temp file + rename) so a crash mid-write can
 * never leave a truncated config that would take the next session's config
 * parse down.
 *
 * Returns the path written, or `null` when the value was unchanged or the write
 * failed — a resize must never throw into the chat.
 */
export function writeFleetViewHeightPct(
  pct: number,
  env: Record<string, string | undefined> = process.env,
  io: FleetConfigIO = defaultIO,
): string | null {
  const value = clampHeightPct(pct);
  const paths = fleetConfigPaths(env);
  const target = paths.find((path) => io.exists(path)) ?? paths[0];
  if (!target) return null;
  let raw: Record<string, unknown> = {};
  try {
    if (io.exists(target)) {
      const parsed: unknown = JSON.parse(io.readFile(target));
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        raw = { ...(parsed as Record<string, unknown>) };
      }
    }
  } catch {
    // A corrupt config must not block the resize: rewrite it from scratch.
    raw = {};
  }
  if (asHeightPct(raw[FLEET_HEIGHT_PCT_KEY], -1) === value) return null;
  raw[FLEET_HEIGHT_PCT_KEY] = value;
  // The writer seams are optional on the IO interface (a read-only test double
  // stays valid); resolve them to the real filesystem when absent.
  const mkdir = io.mkdir ?? defaultIO.mkdir!;
  const writeFile = io.writeFile ?? defaultIO.writeFile!;
  const rename = io.rename ?? defaultIO.rename!;
  try {
    mkdir(dirname(target));
    const tmp = `${target}.tmp`;
    writeFile(tmp, `${JSON.stringify(raw, null, 2)}\n`);
    rename(tmp, target);
    return target;
  } catch {
    return null;
  }
}
