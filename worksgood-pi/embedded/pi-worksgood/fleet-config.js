/**
 * fleet-config.ts — the config gate for the FleetView bottom panel.
 *
 * The FleetView panel is **optional**: unlike the always-on one-line `wg-viz`
 * widget, nothing is mounted below the editor unless the operator opts in. The
 * opt-in follows the same **extension-config file pattern** pi-subagents uses
 * (`~/.pi/agent/extensions/<name>/config.json`, honouring `PI_CODING_AGENT_DIR`
 * and `HOME`), so a WG operator already has a familiar place to flip it on:
 *
 *   ~/.pi/agent/extensions/pi-worksgood/config.json
 *   {
 *     "fleetView": true,          // default: false (no chatter unless asked)
 *     "fleetViewExpanded": false,  // start compact (header + a few live agents)
 *     "fleetViewPlacement": "belowEditor",
 *     "fleetViewPollMs": 5000
 *   }
 *
 * Resolution order (first win):
 *   1. explicit env override (`WG_PI_FLEET_VIEW`, `WG_PI_FLEET_VIEW_POLL_MS`);
 *   2. the first existing config file among the candidate dirs;
 *   3. the safe default (`enabled: false`).
 *
 * Everything here is pure/read-only — it never writes a config file.
 */
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
/** Safe defaults: disabled, compact, below the editor, 5s cadence. */
export const DEFAULT_FLEET_VIEW_CONFIG = {
    enabled: false,
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
/** Pure parse of a raw config object (unknown keys ignored, bad types degraded). */
export function parseFleetViewConfig(raw) {
    const obj = raw && typeof raw === "object" && !Array.isArray(raw) ? raw : {};
    return {
        enabled: asBool(obj.fleetView, DEFAULT_FLEET_VIEW_CONFIG.enabled),
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
        expanded: asBool(env.WG_PI_FLEET_VIEW_EXPANDED, config.expanded),
        placement: asPlacement(env.WG_PI_FLEET_VIEW_PLACEMENT, config.placement),
        pollMs: asPollMs(env.WG_PI_FLEET_VIEW_POLL_MS, config.pollMs),
    };
}
//# sourceMappingURL=fleet-config.js.map