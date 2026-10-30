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
 *     "fleetViewHeightPct": 0.6,      // how much of the height the cockpit covers
 *     "fleetViewLayout": {           // the inspector dock: split/full/hidden + %
 *       "mode": "split",
 *       "sizePercent": 50
 *     }
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
 * Reads are pure/read-only. The writes are
 * {@link writeFleetViewHeightPct} (the cockpit's drag/keyboard resize persists
 * the operator's chosen `fleetViewHeightPct` so it sticks across opens — the
 * operator-reported gap "no click-drag or way to modulate the screen fraction
 * covered") and {@link writeFleetViewLayout} (the inspector-dock layout chosen
 * with `1`/`2`/`3`, `=`, `\`, `i`/`v`, the layout row or the divider drag, so
 * the panel reopens exactly as the operator left it). Both go through
 * {@link writeFleetViewSetting}: one key, atomic, never touching any other field.
 */
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { DEFAULT_FLEET_LAYOUT, layoutFromConfig, layoutToConfig, } from "./fleet-layout.js";
/** Smallest cockpit fraction (three rows' worth on a normal terminal). */
export const MIN_FLEET_HEIGHT_PCT = 0.2;
/** Largest cockpit fraction (the whole interactive area). */
export const MAX_FLEET_HEIGHT_PCT = 1;
/** The config key the panel's resize persists. */
export const FLEET_HEIGHT_PCT_KEY = "fleetViewHeightPct";
/** The config key the inspector-dock layout persists. */
export const FLEET_LAYOUT_KEY = "fleetViewLayout";
/** Env override for the cockpit height fraction. */
export const FLEET_HEIGHT_PCT_ENV = "WG_PI_FLEET_HEIGHT_PCT";
/** Safe defaults: visible one-line strip, compact body, below the editor, 5s cadence. */
export const DEFAULT_FLEET_VIEW_CONFIG = {
    enabled: true,
    collapsed: true,
    expanded: false,
    placement: "belowEditor",
    pollMs: 5000,
    heightPct: MAX_FLEET_HEIGHT_PCT,
    layout: { ...DEFAULT_FLEET_LAYOUT },
};
/**
 * Candidate extension-config directories under `<agent-dir>/extensions/`.
 * pi-subagents uses its display name (`subagent`); the WG plugin's display name
 * is `pi-worksgood`, so that is primary. `worksgood`/`wg` are accepted aliases
 * so an operator who guessed either spelling is not surprised by a no-op.
 */
export const FLEET_CONFIG_DIRS = ["pi-worksgood", "worksgood", "wg"];
/** The pi agent dir: `$PI_CODING_AGENT_DIR` (with `~` expansion) or `$HOME/.pi/agent`. */
export function resolveAgentDir(env = process.env) {
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
export function fleetConfigPaths(env = process.env) {
    return FLEET_CONFIG_DIRS.map((dir) => join(resolveAgentDir(env), "extensions", dir, "config.json"));
}
const defaultIO = {
    exists: (path) => {
        try {
            return existsSync(path);
        }
        catch {
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
function asBool(value, fallback) {
    if (value === undefined || value === null || value === "")
        return fallback;
    if (typeof value === "boolean")
        return value;
    if (typeof value === "string") {
        const v = value.trim().toLowerCase();
        if (["off", "0", "false", "no"].includes(v))
            return false;
        if (["on", "1", "true", "yes"].includes(v))
            return true;
    }
    return fallback;
}
function asPlacement(value, fallback) {
    return value === "aboveEditor" || value === "belowEditor" ? value : fallback;
}
function asPollMs(value, fallback) {
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
export function clampHeightFraction(value, fallback = MAX_FLEET_HEIGHT_PCT) {
    const n = typeof value === "number" ? value : Number(value);
    if (!Number.isFinite(n))
        return fallback;
    return Math.min(MAX_FLEET_HEIGHT_PCT, Math.max(MIN_FLEET_HEIGHT_PCT, n));
}
/**
 * Parse a CONFIGURED cockpit height: a fraction (`0.6`) or a percent (`60`).
 * Non-positive / non-numeric values degrade to `fallback` (the default).
 */
export function clampHeightPct(value, fallback = MAX_FLEET_HEIGHT_PCT) {
    const n = typeof value === "number" ? value : Number(value);
    if (!Number.isFinite(n) || n <= 0)
        return fallback;
    const normalized = n > 1 ? n / 100 : n; // tolerate a percent (e.g. 60)
    return clampHeightFraction(normalized, fallback);
}
function asHeightPct(value, fallback) {
    if (value === undefined || value === null || value === "")
        return fallback;
    const n = typeof value === "number" ? value : Number(value);
    if (!Number.isFinite(n) || n <= 0)
        return fallback;
    return clampHeightPct(n, fallback);
}
/** Pure parse of a raw config object (unknown keys ignored, bad types degraded). */
export function parseFleetViewConfig(raw) {
    const obj = raw && typeof raw === "object" && !Array.isArray(raw) ? raw : {};
    return {
        enabled: asBool(obj.fleetView, DEFAULT_FLEET_VIEW_CONFIG.enabled),
        collapsed: asBool(obj.fleetViewCollapsed, DEFAULT_FLEET_VIEW_CONFIG.collapsed),
        expanded: asBool(obj.fleetViewExpanded, DEFAULT_FLEET_VIEW_CONFIG.expanded),
        placement: asPlacement(obj.fleetViewPlacement, DEFAULT_FLEET_VIEW_CONFIG.placement),
        pollMs: asPollMs(obj.fleetViewPollMs, DEFAULT_FLEET_VIEW_CONFIG.pollMs),
        heightPct: asHeightPct(obj[FLEET_HEIGHT_PCT_KEY], DEFAULT_FLEET_VIEW_CONFIG.heightPct),
        layout: layoutFromConfig(obj[FLEET_LAYOUT_KEY] ?? DEFAULT_FLEET_VIEW_CONFIG.layout),
    };
}
/**
 * Resolve the effective FleetView config. Env overrides win over the file; a
 * missing/corrupt file degrades to the safe default rather than throwing (a
 * bad config must never take a pi session down).
 */
export function readFleetViewConfig(env = process.env, io = defaultIO, explicitPath) {
    let config = { ...DEFAULT_FLEET_VIEW_CONFIG };
    const paths = explicitPath ? [explicitPath] : fleetConfigPaths(env);
    for (const path of paths) {
        if (!io.exists(path))
            continue;
        try {
            config = parseFleetViewConfig(JSON.parse(io.readFile(path)));
        }
        catch {
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
        layout: config.layout,
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
export function writeFleetViewHeightPct(pct, env = process.env, io = defaultIO) {
    return writeFleetViewSetting(FLEET_HEIGHT_PCT_KEY, clampHeightPct(pct), env, io);
}
/**
 * Persist the operator's chosen inspector-dock layout.
 *
 * SAME contract as {@link writeFleetViewHeightPct}: one key, atomic temp+rename,
 * every other key preserved verbatim, `null` on no-change/failure (a dock change
 * must never throw into the chat). The panel calls this on every applied layout
 * so the next open restores it.
 */
export function writeFleetViewLayout(layout, env = process.env, io = defaultIO) {
    return writeFleetViewSetting(FLEET_LAYOUT_KEY, layoutToConfig(layout), env, io);
}
/**
 * Write ONE `fleetView*` key atomically, preserving every other key.
 *
 * Returns the path written, or `null` when the value was unchanged or the write
 * failed.
 */
export function writeFleetViewSetting(key, value, env = process.env, io = defaultIO) {
    const paths = fleetConfigPaths(env);
    const target = paths.find((path) => io.exists(path)) ?? paths[0];
    if (!target)
        return null;
    let raw = {};
    try {
        if (io.exists(target)) {
            const parsed = JSON.parse(io.readFile(target));
            if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
                raw = { ...parsed };
            }
        }
    }
    catch {
        // A corrupt config must not block the resize: rewrite it from scratch.
        raw = {};
    }
    const unchanged = key === FLEET_HEIGHT_PCT_KEY
        ? asHeightPct(raw[key], -1) === value
        : JSON.stringify(raw[key] ?? null) === JSON.stringify(value);
    if (unchanged)
        return null;
    raw[key] = value;
    // The writer seams are optional on the IO interface (a read-only test double
    // stays valid); resolve them to the real filesystem when absent.
    const mkdir = io.mkdir ?? defaultIO.mkdir;
    const writeFile = io.writeFile ?? defaultIO.writeFile;
    const rename = io.rename ?? defaultIO.rename;
    try {
        mkdir(dirname(target));
        const tmp = `${target}.tmp`;
        writeFile(tmp, `${JSON.stringify(raw, null, 2)}\n`);
        rename(tmp, target);
        return target;
    }
    catch {
        return null;
    }
}
//# sourceMappingURL=fleet-config.js.map