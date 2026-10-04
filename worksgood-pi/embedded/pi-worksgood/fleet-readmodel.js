/**
 * fleet-readmodel.ts — the pure read-model for the FleetView bottom panel.
 *
 * Mirrors the status vocabulary the WG TUI / viz panel already use (the
 * `statusGlyph` map in `viz-readmodel.ts`) and adds an agent-status glyph map
 * for the runtime worker registry, so the panel speaks one visual language
 * instead of forking a second one.
 *
 * Pure: snapshot in, renderable (text, colour) lines out. No I/O, no mutation —
 * the panel is strictly read-only. Colours are semantic names so the caller can
 * paint per line through the active theme (and tests can pin the mapping
 * without a theme).
 */
import { ageOf, statusGlyph, taskCounts } from "./viz-readmodel.js";
const TERMINAL_AGENT_STATUSES = new Set(["done", "failed", "dead"]);
/** Is a worker live? Prefer the explicit `alive` flag, else derive from status. */
export function isAgentAlive(agent) {
    if (typeof agent.alive === "boolean")
        return agent.alive;
    return !TERMINAL_AGENT_STATUSES.has(agent.status);
}
/**
 * Agent status glyph. Chosen to stay consistent with the task glyph vocabulary
 * (`●` active, `✓` done, `✗` failed, `◌`/`○` idle-ish, `⏸` parked, `⨯` dead).
 */
export function agentGlyph(status, alive) {
    if (alive === false)
        return "⨯";
    switch (status) {
        case "working":
            return "●";
        case "starting":
            return "◌";
        case "idle":
            return "○";
        case "stopping":
        case "frozen":
            return "◔";
        case "parked":
            return "⏸";
        case "done":
            return "✓";
        case "failed":
            return "✗";
        case "dead":
            return "⨯";
        default:
            return "·";
    }
}
/** Semantic colour for an agent status (WG TUI palette: green ok, red bad, yellow busy). */
export function agentColor(status, alive) {
    if (alive === false)
        return "error";
    switch (status) {
        case "working":
            return "success";
        case "starting":
        case "stopping":
        case "frozen":
        case "parked":
            return "warning";
        case "idle":
            return "muted";
        case "done":
            return "success";
        case "failed":
        case "dead":
            return "error";
        default:
            return "dim";
    }
}
/**
 * Semantic colour for a task status. Mirrors WG's TUI status palette
 * (`src/tui/viz_viewer/state.rs::flash_color_for_status`, the same RGB set
 * `src/html.rs::status_color` renders), mapped onto the nearest pi theme token:
 *
 * | status                     | TUI RGB / role     | pi token             |
 * |----------------------------|--------------------|----------------------|
 * | done                       | green  80,220,100  | `success`            |
 * | failed                     | red   220,60,60    | `error`              |
 * | in-progress                | cyan   60,200,220  | `borderAccent`       |
 * | open                       | yellow 200,200,80  | `warning`            |
 * | blocked                    | orange 180,120,60  | `warning`            |
 * | abandoned                  | purple 140,100,160 | `customMessageLabel` |
 * | waiting/pending-validation | blue    60,160,220 | `border`             |
 * | pending-eval               | chartreuse 140,230,80 | `success`         |
 * | failed-pending-eval        | coral  210,130,70  | `error`              |
 * | unknown                    | grey               | `dim`                |
 *
 * (Previously `abandoned` wrongly mapped to `error`/red and every active
 * status to `warning`; the operator saw no colour at all, so the panel also
 * re-applies the style per rendered line — see `fleet-panel.ts::render`.)
 */
export function taskColor(status) {
    switch (status) {
        case "done":
            return "success";
        case "failed":
            return "error";
        case "in-progress":
            return "borderAccent";
        case "open":
            return "warning";
        case "blocked":
            return "warning";
        case "abandoned":
            return "customMessageLabel";
        case "waiting":
        case "pending-validation":
            return "border";
        case "pending-eval":
            return "success";
        case "failed-pending-eval":
            return "error";
        case "incomplete":
            return "warning";
        default:
            return "dim";
    }
}
/** Task glyph re-exported from the viz read-model (one vocabulary, no drift). */
export function taskGlyph(status) {
    return statusGlyph(status);
}
/** in-progress/ready/blocked/done/failed task counts + live-agent count. */
export function fleetCounts(agents, tasks) {
    const counts = taskCounts(tasks);
    return {
        active: agents.filter(isAgentAlive).length,
        inProgress: counts.inProgress,
        ready: counts.ready,
        blocked: counts.blocked,
        done: counts.done,
        failed: counts.failed,
    };
}
/** The compact one-line summary (also what `/wg-fleet` announces). */
export function fleetHeaderLine(counts) {
    return (`wg fleet · ${counts.active} active · ${counts.inProgress} in-progress · ` +
        `${counts.ready} ready · ${counts.blocked} blocked · ${counts.done} done · ${counts.failed} failed`);
}
/** Compact human elapsed for an agent: registry uptime, else age of `startedAt`. */
export function agentElapsed(agent, now = Date.now()) {
    const human = agent.uptime?.trim();
    if (human)
        return human;
    return ageOf(agent.startedAt, now) ?? "—";
}
/** Agent display label: model when known, else executor, else `?`. */
export function agentModel(agent) {
    const model = agent.model?.trim();
    if (model)
        return model;
    const executor = agent.executor?.trim();
    return executor && executor.length > 0 ? executor : "?";
}
/** One agent row: `● agent-7 · task-id · model · 12m`. */
export function agentLine(agent, now = Date.now()) {
    const alive = isAgentAlive(agent);
    return {
        text: `${agentGlyph(agent.status, alive)} ${agent.id} · ${agent.taskId} · ` +
            `${agentModel(agent)} · ${agentElapsed(agent, now)}`,
        color: agentColor(agent.status, alive),
    };
}
/**
 * Render the panel lines. Compact by default (header + up to `maxCompactAgents`
 * live agent rows + a discoverability hint); expanded lists every live agent.
 * Always at least two lines so the surface reads as a panel, not a footer.
 */
export function renderFleetLines(snapshot, opts = {}) {
    const expanded = opts.expanded ?? false;
    const maxCompact = opts.maxCompactAgents ?? 3;
    const now = opts.now ?? Date.now();
    const counts = fleetCounts(snapshot.agents, snapshot.tasks);
    const lines = [{ text: fleetHeaderLine(counts), color: "accent" }];
    const live = snapshot.agents
        .filter(isAgentAlive)
        .slice()
        .sort((a, b) => a.id.localeCompare(b.id));
    const shown = expanded ? live : live.slice(0, maxCompact);
    for (const agent of shown)
        lines.push(agentLine(agent, now));
    if (!expanded && live.length > shown.length) {
        lines.push({ text: `+${live.length - shown.length} more · /wg-fleet to expand`, color: "dim" });
    }
    if (expanded && live.length === 0) {
        lines.push({ text: "no live agents", color: "dim" });
    }
    lines.push({
        text: expanded ? "/wg-fleet to collapse · /wg-viz opens the full graph" : "/wg-fleet to expand · /wg-viz opens the full graph",
        color: "dim",
    });
    return lines;
}
/** Plain-text one-line summary for `/wg-fleet` notifications. */
export function fleetSummaryLine(agents, tasks) {
    return fleetHeaderLine(fleetCounts(agents, tasks));
}
//# sourceMappingURL=fleet-readmodel.js.map