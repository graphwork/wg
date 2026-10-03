/**
 * fleet-panel.ts — the **scrollable** FleetView panel (`/wg-fleet`).
 *
 * The ambient below-editor widget (`fleet-view.ts`, `setWidget("wg-fleet", …)`)
 * is a fixed block of lines: it can neither scroll nor take clicks. This is the
 * custom-component counterpart cribbed from pi-subagents' `FleetView`
 * (`src/runs/background/fleet-view.js`): a full-screen `ctx.ui.custom()`
 * component with a real scroll offset, a selection, a counts header, live
 * per-task activity/transcript tails, and drill-down into the existing task
 * detail rendering.
 *
 * Interactions:
 *   - **wheel / PgUp / PgDn / Home / End** scroll the dependency TREE; the
 *     selection is dragged along so it always stays visible;
 *   - **↑/↓** move the selection and scroll it into view;
 *   - **Enter / → / l** drill into the selected task's detail (reusing
 *     `detailLines` from the viz read-model, plus the live agent activity and a
 *     bounded stream tail); **← / Enter / l** return to the tree;
 *   - **o** expand/collapse the selected subtree;
 *   - **q / Esc** close.
 *
 * Strictly read-only: the only data source is the daemon's `GetFleet` read
 * (`WgBackend.getFleet`, with its CLI fallback) and a bounded tail read of the
 * agent's own stream file. It never mutates graph state, is TUI-only (guarded
 * on `ctx.mode`), and degrades silently elsewhere.
 */
import { Key, matchesKey, truncateToWidth } from "@earendil-works/pi-tui";
import { taskColor } from "./fleet-readmodel.js";
import { DEFAULT_TRANSCRIPT_LINES, FLEET_PANEL_DEFAULT_HEIGHT, FLEET_PANEL_MIN_HEIGHT, agentActivityLabel, agentForTask, boundTranscriptBody, buildFleetTree, clampScroll, fleetCountsHeader, getFleetToVizSnapshot, maxScroll, pageScroll, readAgentStreamTail, scrollToKeepVisible, } from "./fleet-panel-model.js";
import { detailLines } from "./viz-readmodel.js";
/** Default bounded poll cadence for the open panel. */
export const FLEET_PANEL_POLL_MS = 5000;
/**
 * The scrollable panel component. Implements pi-tui's `Component` contract
 * (`render` / `invalidate` / `handleInput`) plus a `handleMouse` entry point for
 * hosts that dispatch normalized mouse events (`wheel` scrolls; press/click
 * selects the hit tree line).
 */
export class FleetPanelComponent {
    tui;
    onClose;
    theme;
    options;
    liveDir;
    snapshot;
    selectedId;
    collapsed = new Set();
    mode = "tree";
    treeScroll = 0;
    detailScroll = 0;
    /** Cached, bounded transcript tail lines for the selected task (detail mode). */
    detailTail = [];
    hitMap = [];
    cachedLines = null;
    cachedWidth = -1;
    cachedHeight = -1;
    disposed = false;
    constructor(snapshot, tui, onClose, theme = null, options = {}, liveDir = undefined) {
        this.tui = tui;
        this.onClose = onClose;
        this.theme = theme;
        this.options = options;
        this.liveDir = liveDir;
        this.snapshot = snapshot;
        this.selectedId = this.firstVisibleId();
        this.refreshDetailTail();
    }
    // ── state / data ──────────────────────────────────────────────────────────
    /** Apply a fresh snapshot from the poller and re-render. */
    setSnapshot(snapshot) {
        this.snapshot = snapshot;
        if (this.selectedId && !snapshot?.tasks.some((t) => t.id === this.selectedId)) {
            this.selectedId = this.firstVisibleId();
        }
        this.refreshDetailTail();
        this.invalidate();
        this.tui.requestRender();
    }
    dispose() {
        this.disposed = true;
    }
    // ── viewport geometry ─────────────────────────────────────────────────────
    viewportHeight() {
        let rows = FLEET_PANEL_DEFAULT_HEIGHT;
        try {
            rows = this.options.height?.() ?? this.tui.terminal?.rows ?? FLEET_PANEL_DEFAULT_HEIGHT;
        }
        catch {
            rows = FLEET_PANEL_DEFAULT_HEIGHT;
        }
        if (!Number.isFinite(rows))
            rows = FLEET_PANEL_DEFAULT_HEIGHT;
        return Math.max(FLEET_PANEL_MIN_HEIGHT, Math.trunc(rows));
    }
    /** Body rows between the fixed header and footer. */
    bodyViewport() {
        return Math.max(1, this.viewportHeight() - 2);
    }
    // ── tree / selection helpers ──────────────────────────────────────────────
    visibleOrder() {
        const tree = this.snapshot ? buildFleetTree(this.snapshot, this.collapsed) : { lines: [], roots: 0 };
        return tree.lines.map((l) => l.taskId).filter((id) => id !== null);
    }
    firstVisibleId() {
        return this.visibleOrder()[0] ?? null;
    }
    selectionIndex() {
        if (!this.selectedId)
            return -1;
        return this.visibleOrder().indexOf(this.selectedId);
    }
    /** Keep the selected id inside the current viewport (drag it if needed). */
    dragSelectionIntoView() {
        const order = this.visibleOrder();
        if (order.length === 0) {
            this.selectedId = null;
            this.treeScroll = 0;
            return;
        }
        const idx = this.selectedId ? order.indexOf(this.selectedId) : -1;
        if (idx === -1) {
            this.selectedId = order[this.treeScroll] ?? order[0] ?? null;
            return;
        }
        const view = this.bodyViewport();
        if (idx < this.treeScroll)
            this.selectedId = order[this.treeScroll] ?? null;
        else if (idx >= this.treeScroll + view)
            this.selectedId = order[this.treeScroll + view - 1] ?? null;
    }
    // ── navigation ────────────────────────────────────────────────────────────
    /** Move the selection by `delta` visible rows and scroll it into view. */
    moveSelection(delta) {
        const order = this.visibleOrder();
        if (order.length === 0)
            return;
        const idx = this.selectedId ? order.indexOf(this.selectedId) : -1;
        const next = Math.min(order.length - 1, Math.max(0, (idx === -1 ? 0 : idx) + delta));
        this.selectedId = order[next] ?? null;
        this.mode = "tree";
        this.detailScroll = 0;
        this.treeScroll = scrollToKeepVisible(this.treeScroll, this.bodyViewport(), next, order.length);
        this.invalidate();
        this.tui.requestRender();
    }
    selectFirst() {
        const order = this.visibleOrder();
        this.selectedId = order[0] ?? null;
        this.treeScroll = 0;
        this.mode = "tree";
        this.invalidate();
        this.tui.requestRender();
    }
    selectLast() {
        const order = this.visibleOrder();
        const last = order.length - 1;
        this.selectedId = order[last] ?? null;
        this.treeScroll = maxScroll(order.length, this.bodyViewport());
        this.mode = "tree";
        this.invalidate();
        this.tui.requestRender();
    }
    /** Scroll the active pane by `delta` rows, keeping the selection visible. */
    scrollBy(delta) {
        if (!Number.isFinite(delta) || delta === 0)
            return;
        if (this.mode === "detail") {
            const total = this.detailContent().length;
            this.detailScroll = clampScroll(this.detailScroll + delta, total, this.bodyViewport());
        }
        else {
            const total = this.visibleOrder().length;
            this.treeScroll = clampScroll(this.treeScroll + delta, total, this.bodyViewport());
            this.dragSelectionIntoView();
        }
        this.invalidate();
        this.tui.requestRender();
    }
    /** Page the active pane by `direction` (±1), keeping the selection visible. */
    pageBy(direction) {
        const view = this.bodyViewport();
        if (this.mode === "detail") {
            const total = this.detailContent().length;
            this.detailScroll = pageScroll(this.detailScroll, view, direction, total);
        }
        else {
            const total = this.visibleOrder().length;
            this.treeScroll = pageScroll(this.treeScroll, view, direction, total);
            this.dragSelectionIntoView();
        }
        this.invalidate();
        this.tui.requestRender();
    }
    /** Expand/collapse the selected task's subtree. */
    toggleExpand() {
        if (!this.selectedId)
            return;
        if (this.collapsed.has(this.selectedId))
            this.collapsed.delete(this.selectedId);
        else
            this.collapsed.add(this.selectedId);
        this.refreshDetailTail();
        this.invalidate();
        this.tui.requestRender();
    }
    /** Drill into the selected task's detail view. */
    openDetail() {
        if (!this.selectedId || !this.snapshot)
            return;
        this.mode = "detail";
        this.detailScroll = 0;
        this.refreshDetailTail();
        this.invalidate();
        this.tui.requestRender();
    }
    /** Return from the detail view to the tree. */
    closeDetail() {
        if (this.mode !== "detail")
            return;
        this.mode = "tree";
        this.detailScroll = 0;
        this.invalidate();
        this.tui.requestRender();
    }
    close() {
        this.onClose();
    }
    // ── transcript tail ───────────────────────────────────────────────────────
    refreshDetailTail() {
        this.detailTail = [];
        if (!this.snapshot || !this.selectedId || !this.liveDir)
            return;
        const agent = agentForTask(this.snapshot.agents, this.selectedId);
        if (!agent)
            return;
        const tail = readAgentStreamTail(this.liveDir, agent.id, this.options.transcriptLines ?? DEFAULT_TRANSCRIPT_LINES);
        if (tail && tail.lines.length > 0)
            this.detailTail = boundTranscriptBody(tail.lines);
    }
    // ── input ─────────────────────────────────────────────────────────────────
    handleInput(data) {
        if (this.disposed)
            return;
        if (matchesKey(data, Key.escape) || data === "q") {
            this.close();
            return;
        }
        if (this.mode === "detail") {
            if (matchesKey(data, Key.enter) || data === "l" || matchesKey(data, Key.left)) {
                this.closeDetail();
            }
            else if (matchesKey(data, Key.up))
                this.scrollBy(-1);
            else if (matchesKey(data, Key.down))
                this.scrollBy(1);
            else if (matchesKey(data, Key.pageUp))
                this.pageBy(-1);
            else if (matchesKey(data, Key.pageDown))
                this.pageBy(1);
            else if (matchesKey(data, Key.home))
                this.scrollBy(-this.detailContent().length);
            else if (matchesKey(data, Key.end))
                this.scrollBy(this.detailContent().length);
            return;
        }
        if (matchesKey(data, Key.up))
            this.moveSelection(-1);
        else if (matchesKey(data, Key.down))
            this.moveSelection(1);
        else if (matchesKey(data, Key.pageUp))
            this.pageBy(-1);
        else if (matchesKey(data, Key.pageDown))
            this.pageBy(1);
        else if (matchesKey(data, Key.home) || data === "g")
            this.selectFirst();
        else if (matchesKey(data, Key.end) || data === "G")
            this.selectLast();
        else if (matchesKey(data, Key.enter) || data === "l" || matchesKey(data, Key.right))
            this.openDetail();
        else if (data === "o")
            this.toggleExpand();
    }
    /** Mouse support: `wheel` scrolls; press/click selects the hit tree line. */
    handleMouse(event) {
        if (this.disposed)
            return false;
        const type = event.type ?? "";
        if (type === "wheel" || type === "mouse.wheel") {
            const delta = typeof event.wheelDelta === "number" && event.wheelDelta !== 0 ? event.wheelDelta : 0;
            if (delta !== 0)
                this.scrollBy(delta);
            return true;
        }
        if (type === "press" || type === "click" || type === "mouse.press") {
            if (this.mode !== "tree")
                return false;
            const y = typeof event.y === "number" ? event.y : -1;
            const idx = y - 1 + this.treeScroll; // header occupies line 0
            const order = this.visibleOrder();
            if (idx >= 0 && idx < order.length) {
                this.selectedId = order[idx] ?? null;
                this.invalidate();
                this.tui.requestRender();
                return true;
            }
        }
        return false;
    }
    // ── rendering ─────────────────────────────────────────────────────────────
    invalidate() {
        this.cachedLines = null;
    }
    /** The full (unwindowed) detail view content for the selected task. */
    detailContent() {
        if (!this.snapshot || !this.selectedId)
            return [];
        const viz = getFleetToVizSnapshot(this.snapshot);
        const task = viz.tasks.find((t) => t.id === this.selectedId);
        if (!task)
            return [];
        const out = [];
        for (const text of detailLines(task, viz.tasks).lines)
            out.push({ text, color: "text" });
        const agent = agentForTask(this.snapshot.agents, task.id);
        const activity = agentActivityLabel(agent);
        if (activity) {
            out.push({ text: "", color: "dim" });
            out.push({ text: `── Live activity ──`, color: "accent" });
            out.push({ text: `  ${agent?.id ?? "agent"} · ${activity}`, color: "warning" });
        }
        if (this.detailTail.length > 0) {
            out.push({ text: "", color: "dim" });
            out.push({ text: "── Transcript tail (bounded) ──", color: "accent" });
            for (const line of this.detailTail)
                out.push({ text: line, color: "dim" });
        }
        return out;
    }
    treeContent() {
        if (!this.snapshot)
            return { lines: [], taskIds: [] };
        const statusById = new Map(this.snapshot.tasks.map((t) => [t.id, t.status]));
        const tree = buildFleetTree(this.snapshot, this.collapsed);
        const lines = [];
        const taskIds = [];
        for (const line of tree.lines) {
            const marker = line.taskId !== null && line.taskId === this.selectedId ? "❯ " : "  ";
            let text = `${marker}${line.text}`;
            if (line.taskId) {
                const agent = agentForTask(this.snapshot.agents, line.taskId);
                const activity = agentActivityLabel(agent);
                if (activity)
                    text += ` · ${activity}`;
            }
            const status = line.taskId ? statusById.get(line.taskId) : undefined;
            lines.push({ text, color: status ? taskColor(status) : "dim" });
            taskIds.push(line.taskId);
        }
        return { lines, taskIds };
    }
    render(width) {
        const height = this.viewportHeight();
        if (this.cachedLines && this.cachedWidth === width && this.cachedHeight === height) {
            return this.cachedLines;
        }
        const body = Math.max(1, height - 2);
        const snapshot = this.snapshot;
        const lines = [];
        let hitMap = [];
        if (!snapshot) {
            lines.push({ text: "wg-fleet · daemon offline", color: "dim" });
            lines.push({ text: "  no fleet snapshot available · q close", color: "dim" });
        }
        else if (snapshot.tasks.length === 0) {
            lines.push({ text: fleetCountsHeader(snapshot.counts), color: "accent" });
            lines.push({ text: "  no tasks in the graph · q close", color: "dim" });
        }
        else if (this.mode === "detail") {
            const detail = this.detailContent();
            lines.push({
                text: this.color("accent", `wg-fleet · ${this.selectedId ?? "?"} · detail`),
                color: "accent",
            });
            const windowed = detail.slice(this.detailScroll, this.detailScroll + body);
            lines.push(...windowed);
            if (detail.length > this.detailScroll + body) {
                lines.push({ text: `  … +${detail.length - this.detailScroll - body} more (↓/PgDn)`, color: "dim" });
            }
            else {
                lines.push({ text: "  ↑/↓ scroll · ← or enter back · q close", color: "dim" });
            }
        }
        else {
            const header = fleetCountsHeader(snapshot.counts);
            lines.push({ text: header, color: "accent" });
            const tree = this.treeContent();
            this.treeScroll = clampScroll(this.treeScroll, tree.lines.length, body);
            const windowed = tree.lines.slice(this.treeScroll, this.treeScroll + body);
            lines.push(...windowed);
            hitMap = tree.taskIds.slice(this.treeScroll, this.treeScroll + body);
            // Footer: bounded so the panel never renders past `height`.
            const overflow = tree.lines.length > this.treeScroll + body;
            const footer = overflow
                ? `  ↓ ${tree.lines.length - (this.treeScroll + body)} more · wheel/PgDn · enter detail · q close`
                : "  wheel/PgUp/PgDn/Home/End scroll · enter detail · q close";
            lines.push({ text: footer, color: "dim" });
        }
        // Never render past the panel bounds; truncate ANSI-safely per line.
        const bounded = lines.slice(0, height).map((line) => truncateToWidth(this.color(line.color, line.text), width));
        this.hitMap = hitMap;
        this.cachedLines = bounded;
        this.cachedWidth = width;
        this.cachedHeight = height;
        return bounded;
    }
    color(style, text) {
        try {
            return this.theme ? this.theme.fg(style, text) : text;
        }
        catch {
            return text;
        }
    }
    // ── test-visible accessors ────────────────────────────────────────────────
    get selected() {
        return this.selectedId;
    }
    /** Scroll offset of the currently active pane. */
    get scrollOffset() {
        return this.mode === "detail" ? this.detailScroll : this.treeScroll;
    }
    get treeScrollOffset() {
        return this.treeScroll;
    }
    get detailVisible() {
        return this.mode === "detail";
    }
    get isDisposed() {
        return this.disposed;
    }
    /** Rendered line → task id hit map for the last `render`. */
    get hitLines() {
        return this.hitMap;
    }
}
/**
 * Open the scrollable panel via `ctx.ui.custom()` (TUI mode only). `done()` is
 * supplied to the component so q/Esc resolves the custom prompt; a bounded
 * poller keeps the snapshot live.
 */
export async function openFleetPanel(ctx, fetchSnapshot, liveDir, options = {}) {
    const initial = await fetchSnapshot().catch(() => null);
    await ctx.ui.custom((tui, theme, _keybindings, done) => {
        const height = () => {
            try {
                return tui.terminal?.rows ?? FLEET_PANEL_DEFAULT_HEIGHT;
            }
            catch {
                return FLEET_PANEL_DEFAULT_HEIGHT;
            }
        };
        const component = new FleetPanelComponent(initial, tui, () => done(), theme, { height, transcriptLines: options.transcriptLines }, liveDir);
        let inFlight = false;
        const timer = setInterval(() => {
            if (inFlight || component.isDisposed)
                return;
            inFlight = true;
            void fetchSnapshot()
                .then((snapshot) => component.setSnapshot(snapshot))
                .catch(() => undefined)
                .finally(() => {
                inFlight = false;
            });
        }, options.pollMs ?? FLEET_PANEL_POLL_MS);
        const instance = {
            render: (width) => component.render(width),
            handleInput: (data) => {
                component.handleInput(data);
                tui.requestRender();
            },
            handleMouse: (event) => component.handleMouse(event),
            invalidate: () => component.invalidate(),
            dispose: () => {
                clearInterval(timer);
                component.dispose();
            },
        };
        return instance;
    });
}
/**
 * The `/wg-fleet` data fetcher: prefer the daemon `GetFleet` read (which itself
 * falls back to the read-only CLI), and degrade to `null` when unavailable.
 */
export function makeFleetFetcher(backend) {
    return async () => {
        if (typeof backend.getFleet === "function") {
            try {
                return await backend.getFleet({ timeoutMs: 2000 });
            }
            catch {
                return null;
            }
        }
        return null;
    };
}
//# sourceMappingURL=fleet-panel.js.map