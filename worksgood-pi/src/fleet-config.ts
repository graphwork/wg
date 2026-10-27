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
 *     "fleetViewPollMs": 5000
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
 * Everything here is pure/read-only — it never writes a config file.
 */

import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

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
}

/** Safe defaults: visible one-line strip, compact body, below the editor, 5s cadence. */
export const DEFAULT_FLEET_VIEW_CONFIG: FleetViewConfig = {
  enabled: true,
  collapsed: true,
  expanded: false,
  placement: "belowEditor",
  pollMs: 5000,
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

/** Pure parse of a raw config object (unknown keys ignored, bad types degraded). */
export function parseFleetViewConfig(raw: unknown): FleetViewConfig {
  const obj = raw && typeof raw === "object" && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {};
  return {
    enabled: asBool(obj.fleetView, DEFAULT_FLEET_VIEW_CONFIG.enabled),
    collapsed: asBool(obj.fleetViewCollapsed, DEFAULT_FLEET_VIEW_CONFIG.collapsed),
    expanded: asBool(obj.fleetViewExpanded, DEFAULT_FLEET_VIEW_CONFIG.expanded),
    placement: asPlacement(obj.fleetViewPlacement, DEFAULT_FLEET_VIEW_CONFIG.placement),
    pollMs: asPollMs(obj.fleetViewPollMs, DEFAULT_FLEET_VIEW_CONFIG.pollMs),
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
  };
}
