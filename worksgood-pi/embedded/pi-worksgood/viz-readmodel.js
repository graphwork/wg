/**
 * viz-readmodel.ts — the pure read-model for the embedded VizView panel.
 *
 * This mirrors the TUI viz viewer's read-model vocabulary (see
 * `src/tui/viz_viewer/state.rs` — the selected-task → detail-lines HUD model
 * and the tree rendering of `src/commands/viz/ascii.rs`) over the bounded
 * `VizSnapshot` projection, instead of forking a second vocabulary. The
 * detail-lines sections (header → Title/Status/Presentation/Agent →
 * Dependencies → Description → Log tail) follow the same order and line shapes
 * as `load_hud_detail_for_task` for the sections the bounded snapshot carries;
 * HUD-only sections (admission waiting, agency identity, route receipts) are
 * documented panel deferrals because the snapshot projection is deliberately
 * bounded.
 *
 * Everything here is pure: snapshot in, renderable lines + a line→task hit map
 * out. No I/O, no mutation — the panel is strictly read-only.
 */
const ACTIVE_STATUSES = new Set(["in-progress", "waiting", "pending-validation", "pending-eval", "failed-pending-eval"]);
const TERMINAL_STATUSES = new Set(["done", "failed", "abandoned"]);
/** Stable topological-ish order: dependencies before dependents, id as tiebreak. */
export function orderTasks(tasks) {
    const byId = new Map(tasks.map((t) => [t.id, t]));
    const visited = new Set();
    const ordered = [];
    const visiting = new Set();
    const visit = (id, depth) => {
        if (visited.has(id) || depth > tasks.length + 1)
            return;
        const task = byId.get(id);
        if (!task)
            return;
        if (visiting.has(id)) {
            // Cycle: emit in insertion order, no infinite recursion.
            if (!visited.has(id)) {
                visited.add(id);
                ordered.push(task);
            }
            return;
        }
        visiting.add(id);
        for (const dep of [...(task.after ?? [])].sort())
            visit(dep, depth + 1);
        visiting.delete(id);
        visited.add(id);
        ordered.push(task);
    };
    for (const t of [...tasks].sort((a, b) => a.id.localeCompare(b.id)))
        visit(t.id, 0);
    return ordered;
}
function isReady(task, statusOf) {
    return (task.after ?? []).every((dep) => {
        const depStatus = statusOf.get(dep);
        return depStatus === undefined || TERMINAL_STATUSES.has(depStatus);
    });
}
/** in-progress / ready / blocked / done / failed — the widget's one-line summary inputs. */
export function taskCounts(tasks) {
    const statusOf = new Map(tasks.map((t) => [t.id, t.status]));
    const counts = { inProgress: 0, ready: 0, blocked: 0, done: 0, failed: 0 };
    for (const task of tasks) {
        if (ACTIVE_STATUSES.has(task.status)) {
            counts.inProgress++;
        }
        else if (task.status === "done") {
            counts.done++;
        }
        else if (task.status === "failed" || task.status === "abandoned") {
            counts.failed++;
        }
        else if (task.status === "open" || task.status === "blocked") {
            if (task.status === "blocked" || !isReady(task, statusOf))
                counts.blocked++;
            else
                counts.ready++;
        }
    }
    return counts;
}
/** One-line widget summary, e.g. `wg ▸ 2 in-progress · 3 ready · 1 blocked · 5 done · 0 failed`. */
export function widgetLine(counts) {
    return (`wg ▸ ${counts.inProgress} in-progress · ${counts.ready} ready · ` +
        `${counts.blocked} blocked · ${counts.done} done · ${counts.failed} failed`);
}
/** Compact human age like the TUI prints (35m / 5h / 12d). */
export function ageOf(iso, now = Date.now()) {
    if (!iso)
        return null;
    const ts = Date.parse(iso);
    if (!Number.isFinite(ts))
        return null;
    const secs = Math.max(0, Math.floor((now - ts) / 1000));
    if (secs < 60)
        return `${secs}s`;
    const mins = Math.floor(secs / 60);
    if (mins < 60)
        return `${mins}m`;
    const hours = Math.floor(mins / 60);
    if (hours < 24)
        return `${hours}h`;
    const days = Math.floor(hours / 24);
    return `${days}d`;
}
/** TUI-style bounded token display: `→12k ←3.4k ◎1.2` (in / out / cost). */
export function tokenDisplay(usage) {
    if (!usage)
        return null;
    const compact = (n) => {
        if (n === undefined || n === null || n === 0)
            return null;
        if (n >= 1_000_000)
            return `${(n / 1_000_000).toFixed(1)}M`;
        if (n >= 1_000)
            return `${Math.round(n / 1_000)}k`;
        return String(n);
    };
    const parts = [];
    const input = compact(usage.input_tokens);
    const output = compact(usage.output_tokens);
    if (input)
        parts.push(`→${input}`);
    if (output)
        parts.push(`←${output}`);
    if (usage.cost_usd && usage.cost_usd > 0) {
        parts.push(`◎${usage.cost_usd >= 1 ? usage.cost_usd.toFixed(1) : usage.cost_usd.toFixed(2)}`);
    }
    return parts.length ? parts.join(" ") : null;
}
function statusGlyph(status) {
    switch (status) {
        case "open":
            return "○";
        case "in-progress":
            return "●";
        case "waiting":
            return "◌";
        case "done":
            return "✓";
        case "blocked":
            return "⏸";
        case "failed":
            return "✗";
        case "abandoned":
            return "⨯";
        case "pending-validation":
        case "pending-eval":
            return "◔";
        default:
            return "·";
    }
}
/**
 * Render the graph as a dependency tree: roots (no in-set `after`) at depth 0,
 * dependents nested under their first blocker. Collapsed nodes with hidden
 * children get a `(+N)` marker. Deterministic: children sorted by id.
 */
export function buildTree(snapshot, collapsedIds = new Set()) {
    const tasks = snapshot.tasks;
    const inSet = new Set(tasks.map((t) => t.id));
    const childrenOf = new Map();
    const roots = [];
    for (const t of orderTasks(tasks)) {
        const parents = (t.after ?? []).filter((d) => inSet.has(d));
        if (parents.length === 0) {
            roots.push(t.id);
        }
        else {
            const parent = parents[0];
            const list = childrenOf.get(parent) ?? [];
            list.push(t.id);
            childrenOf.set(parent, list);
        }
    }
    const byId = new Map(tasks.map((t) => [t.id, t]));
    const lines = [];
    const visited = new Set();
    const hiddenByCollapse = new Set();
    const renderNode = (id, depth, connector) => {
        if (visited.has(id))
            return;
        visited.add(id);
        const task = byId.get(id);
        if (!task)
            return;
        const kids = (childrenOf.get(id) ?? []).filter((k) => !visited.has(k));
        const collapsed = collapsedIds.has(id) && kids.length > 0;
        if (collapsed) {
            // Walk the hidden subtree so the orphan pass below never re-renders it.
            const markHidden = (hid) => {
                if (hiddenByCollapse.has(hid))
                    return;
                hiddenByCollapse.add(hid);
                for (const k of childrenOf.get(hid) ?? [])
                    markHidden(k);
            };
            for (const k of kids)
                markHidden(k);
        }
        const age = ageOf(task.started_at ?? task.created_at);
        const tokens = tokenDisplay(task.token_usage);
        const extras = [task.status];
        if (tokens)
            extras.push(tokens);
        if (age)
            extras.push(age);
        const collapsedMark = collapsed ? ` (+${kids.length})` : "";
        const indent = "  ".repeat(depth);
        lines.push({
            text: `${indent}${connector} ${statusGlyph(task.status)} ${task.id}${collapsedMark} (${extras.join(" · ")})`,
            taskId: id,
            depth,
        });
        if (!collapsed) {
            const sorted = [...kids].sort();
            sorted.forEach((kid, i) => {
                const last = i === sorted.length - 1;
                renderNode(kid, depth + 1, last ? "└→" : "├→");
            });
        }
    };
    for (const root of roots.sort()) {
        renderNode(root, 0, "┌→");
    }
    // Tasks inside a cycle (or otherwise unreached) still render, in order —
    // but never resurrect children hidden by a collapse marker.
    for (const t of orderTasks(tasks)) {
        if (!visited.has(t.id) && !hiddenByCollapse.has(t.id))
            renderNode(t.id, 0, "·");
    }
    return { lines, roots: roots.length };
}
/** Plain lines with the selection marker applied. */
export function treeText(render, selectedId) {
    return render.lines.map((line) => {
        const marker = line.taskId !== null && line.taskId === selectedId ? "❯ " : "  ";
        return `${marker}${line.text}`;
    });
}
/**
 * Detail lines for the selected task — the same selected-task → detail-lines
 * model the TUI HUD uses (`load_hud_detail_for_task`), restricted to the
 * sections the bounded snapshot carries.
 */
export function detailLines(task, tasks) {
    const byId = new Map(tasks.map((t) => [t.id, t]));
    const lines = [];
    lines.push(`── ${task.id} ──`);
    lines.push(`Title: ${task.title}`);
    lines.push(`Presentation: ${task.presentation ?? "primary"}`);
    lines.push(`Status: ${task.status}`);
    if (task.assigned)
        lines.push(`Agent: ${task.assigned}`);
    const age = ageOf(task.started_at ?? task.created_at);
    if (age)
        lines.push(`Active: ${age}`);
    if (task.retry_count)
        lines.push(`Retries: ${task.retry_count}`);
    if (task.failure_reason)
        lines.push(`Failure: ${task.failure_reason}`);
    const upstream = (task.after ?? []).map((id) => `${id} (${byId.get(id)?.status ?? "?"})`);
    const downstream = (task.before ?? []).map((id) => `${id} (${byId.get(id)?.status ?? "?"})`);
    lines.push("");
    lines.push("── Dependencies ──");
    lines.push(upstream.length ? `  after: ${upstream.join(", ")}` : "  after: (none)");
    if (downstream.length)
        lines.push(`  before: ${downstream.join(", ")}`);
    if (task.description_head) {
        lines.push("");
        lines.push("── Description ──");
        for (const l of task.description_head.split("\n").slice(0, 12))
            lines.push(`  ${l}`);
    }
    const tail = task.log_tail ?? [];
    if (tail.length || (task.log_count ?? 0) > 0) {
        lines.push("");
        lines.push(`── Log (last ${tail.length} of ${task.log_count ?? tail.length}) ──`);
        for (const entry of tail) {
            const ts = entry.timestamp ? ` ${entry.timestamp}` : "";
            const actor = entry.actor ? ` [${entry.actor}]` : "";
            lines.push(`  ${entry.message}${actor}${ts}`);
        }
    }
    return { lines, hasLog: tail.length > 0 };
}
/** Map a rendered line index to its task id (for mouse hit-testing / keyboard sync). */
export function lineTaskMap(render) {
    return render.lines.map((l) => l.taskId);
}
//# sourceMappingURL=viz-readmodel.js.map