/**
 * viz-panel.ts — the embedded VizView: a live, clickable work-graph panel
 * inside a Pi session, opened **on demand** by the `/wg-viz` command.
 *
 * The panel is strictly **read-only** (lifecycle authority stays with workers
 * and the coordinator — the panel only ever sends the read-only `viz_snapshot`
 * IPC request or runs the read-only `wg viz` fallback):
 *
 *  **`/wg-viz`** — `ctx.ui.custom()` full panel: the dependency tree with
 *     keyboard navigation (up/down move selection, enter/o expand/collapse,
 *     l toggles the selected task's detail lines, q/escape closes) and mouse
 *     clicks via the component's `handleMouse` hit-testing over the rendered
 *     tree lines — where the host pi-tui build dispatches mouse events to
 *     custom components (pi ≥0.80 MouseRegion); keyboard always works.
 *     When the daemon socket is unavailable the panel renders the `wg viz`
 *     ASCII fallback (read-only CLI) instead of the live snapshot.
 *
 * There is deliberately **no ambient widget**: `installVizPanel` mounts zero
 * surfaces at load time. The one-line `setWidget("wg-viz", …)` footer that
 * used to sit above the editor (ungated + polling, experienced by the operator
 * as a HUD) was removed; the panel is only ever created by the command handler
 * and disposed when the user closes it.
 *
 * Slice boundary (first slice, documented deferrals): no filters, no agency
 * lanes UI, no chat surfaces, no back-edge arc rendering, no scrollback
 * search — full viz_viewer parity is deferred. HUD-only detail sections
 * (admission waiting, agency identity, route receipts) are likewise deferred
 * because the `viz_snapshot` projection is deliberately bounded. In non-TUI
 * modes (`rpc`/`json`/`print`) the panel degrades silently: no custom component
 * (guarded on `ctx.mode`).
 */
import { matchesKey, Key, truncateToWidth } from "@earendil-works/pi-tui";
import { VizPoller, resolveSocketPath, fetchVizSnapshot, vizSnapshotWithFallback, } from "./viz-snapshot.js";
import { buildTree, detailLines, lineTaskMap, taskCounts, treeText, widgetLine, } from "./viz-readmodel.js";
/** Default bounded poll cadence for the on-demand `/wg-viz` panel. */
export const VIZ_PANEL_POLL_MS = 5000;
/**
 * The interactive panel component. Implements pi-tui's Component contract
 * (`render` / `invalidate` / `handleInput`) plus a `handleMouse` hit-test
 * entry point for pi builds that dispatch mouse events to custom components.
 */
export class VizPanelComponent {
    tui;
    poller;
    onClose;
    theme;
    snapshot;
    asciiText;
    selectedId;
    collapsed = new Set();
    showDetail = false;
    cachedLines = null;
    cachedWidth = -1;
    hitMap = [];
    disposed = false;
    /** Detail-mode offset for long detail/log sections. */
    detailScroll = 0;
    constructor(snapshot, tui, poller, onClose, theme, asciiText = null) {
        this.tui = tui;
        this.poller = poller;
        this.onClose = onClose;
        this.theme = theme;
        this.snapshot = snapshot;
        this.asciiText = asciiText;
        // Start the selection at the top of the visible (tree) order.
        this.selectedId = snapshot
            ? ((buildTree(snapshot).lines.find((l) => l.taskId !== null)?.taskId ?? null))
            : null;
    }
    /** Apply a fresh snapshot (from the poller) and re-render. */
    update(snapshot) {
        this.snapshot = snapshot;
        this.asciiText = null;
        if (this.selectedId && !snapshot.tasks.some((t) => t.id === this.selectedId)) {
            this.selectedId = snapshot.tasks[0]?.id ?? null;
        }
        this.invalidate();
        this.tui.requestRender();
    }
    dispose() {
        this.disposed = true;
        this.poller?.stop();
    }
    /** Move the selection through the visible (tree) order. */
    moveSelection(delta) {
        const order = this.visibleOrder();
        if (order.length === 0)
            return;
        const idx = this.selectedId ? order.indexOf(this.selectedId) : -1;
        const next = Math.min(order.length - 1, Math.max(0, (idx === -1 ? 0 : idx) + delta));
        this.selectedId = order[next];
        this.showDetail = false;
        this.detailScroll = 0;
        this.invalidate();
        this.tui.requestRender();
    }
    selectFirst() {
        this.selectedId = this.visibleOrder()[0] ?? null;
        this.showDetail = false;
        this.invalidate();
        this.tui.requestRender();
    }
    selectLast() {
        const order = this.visibleOrder();
        this.selectedId = order[order.length - 1] ?? null;
        this.showDetail = false;
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
        this.invalidate();
        this.tui.requestRender();
    }
    /** Toggle the selected task's detail lines (the TUI HUD detail model). */
    toggleDetail() {
        this.showDetail = !this.showDetail;
        this.detailScroll = 0;
        this.invalidate();
        this.tui.requestRender();
    }
    /** Scroll within an open detail view. */
    scrollDetail(delta) {
        if (!this.showDetail)
            return;
        this.detailScroll = Math.max(0, this.detailScroll + delta);
        this.invalidate();
        this.tui.requestRender();
    }
    /** Select the task under a rendered line (mouse hit-testing). */
    selectLine(lineIndex) {
        const id = this.hitMap[lineIndex] ?? null;
        if (!id)
            return false;
        this.selectedId = id;
        this.showDetail = false;
        this.invalidate();
        this.tui.requestRender();
        return true;
    }
    close() {
        this.onClose();
    }
    handleInput(data) {
        // Detail view: Enter is deliberately inert (some terminals deliver Enter as
        // `\r\n`, and when Enter both opened and closed detail it looked like
        // "Enter only opens/closes"). Use ←/h/Backspace/l to go back, q/Esc to close.
        if (this.showDetail) {
            if (matchesKey(data, Key.escape) || data === "q")
                this.close();
            else if (data === "l" || data === "h" || matchesKey(data, Key.left) || matchesKey(data, Key.backspace)) {
                this.toggleDetail();
            }
            else if (matchesKey(data, Key.up))
                this.scrollDetail(-1);
            else if (matchesKey(data, Key.down))
                this.scrollDetail(1);
            else if (matchesKey(data, Key.pageUp))
                this.scrollDetail(-10);
            else if (matchesKey(data, Key.pageDown))
                this.scrollDetail(10);
            return;
        }
        if (matchesKey(data, Key.up))
            this.moveSelection(-1);
        else if (matchesKey(data, Key.down))
            this.moveSelection(1);
        else if (matchesKey(data, Key.enter) || data === "l" || matchesKey(data, Key.right))
            this.toggleDetail();
        else if (data === "o" || matchesKey(data, Key.space))
            this.toggleExpand();
        else if (matchesKey(data, Key.escape) || data === "q")
            this.close();
        else if (data === "g")
            this.selectFirst();
        else if (data === "G")
            this.selectLast();
    }
    invalidate() {
        this.cachedLines = null;
    }
    render(width) {
        if (this.cachedLines && this.cachedWidth === width)
            return this.cachedLines;
        const lines = [];
        const snapshot = this.snapshot;
        if (!snapshot) {
            // ASCII fallback (socket unavailable): the static `wg viz` rendering,
            // read-only and non-interactive.
            lines.push(this.color("dim", "wg-viz · daemon offline · wg viz ASCII fallback"));
            for (const l of (this.asciiText ?? "wg-viz: no graph data available").split("\n")) {
                lines.push(l);
            }
            this.hitMap = [];
            this.cachedLines = lines.map((l) => truncateToWidth(l, width));
            this.cachedWidth = width;
            return this.cachedLines;
        }
        if (snapshot.tasks.length === 0) {
            lines.push(this.color("dim", "wg-viz: no tasks to display"));
            this.hitMap = [];
            this.cachedLines = lines.map((l) => truncateToWidth(l, width));
            this.cachedWidth = width;
            return this.cachedLines;
        }
        const counts = taskCounts(snapshot.tasks);
        const selected = this.selectedId
            ? (snapshot.tasks.find((t) => t.id === this.selectedId) ?? null)
            : null;
        if (this.showDetail && selected) {
            lines.push(this.color("accent", "wg-viz · detail · q close · l back to tree"));
            const detail = detailLines(selected, snapshot.tasks);
            lines.push(...detail.lines.slice(this.detailScroll));
        }
        else {
            lines.push(this.color("accent", widgetLine(counts)));
            lines.push(this.color("dim", "wg-viz · ↑/↓ select · enter detail · o/space expand · q close"));
            lines.push(...treeText(buildTree(snapshot, this.collapsed), this.selectedId));
        }
        this.hitMap = lineTaskMap(buildTree(snapshot, this.collapsed));
        this.cachedLines = lines.map((l) => truncateToWidth(l, width));
        this.cachedWidth = width;
        return this.cachedLines;
    }
    color(style, text) {
        try {
            return this.theme ? this.theme.fg(style, text) : text;
        }
        catch {
            return text;
        }
    }
    visibleOrder() {
        const render = buildTree(this.snapshot ?? { tasks: [] }, this.collapsed);
        return render.lines.map((l) => l.taskId).filter((id) => id !== null);
    }
    /**
     * Mouse support: dispatched by pi-tui builds that route mouse to custom
     * components. Returns pi's `PiMouseResult` shape — `{ handled: true }` when a
     * line is selected, `undefined` otherwise — NEVER a bare boolean (a truthy
     * non-object crashes pi core in `dispatchMouseEvent`; see `mouse.ts`). Wheel
     * events are left unhandled so the host scroll view scrolls the panel.
     */
    handleMouse(event) {
        if (event.type !== "mouse.press" && event.type !== "click")
            return undefined;
        return this.selectLine(event.y) ? { handled: true } : undefined;
    }
    /** Exposed for tests / future MouseRegion wiring. */
    get hitLines() {
        return this.hitMap;
    }
    get selected() {
        return this.selectedId;
    }
    get detailVisible() {
        return this.showDetail;
    }
    get isDisposed() {
        return this.disposed;
    }
}
/** Open the full panel via `ctx.ui.custom()` (TUI mode only). */
export async function openVizPanel(ctx, env, initial, asciiFallback = null) {
    await ctx.ui.custom((tui, theme, _keybindings, done) => {
        let component = null;
        const socketPath = resolveSocketPath(env).socket;
        const poller = socketPath
            ? new VizPoller(() => fetchVizSnapshot(socketPath), (snapshot) => component?.update(snapshot), VIZ_PANEL_POLL_MS)
            : null;
        component = new VizPanelComponent(initial, tui, poller, () => done(), theme, asciiFallback);
        poller?.start();
        return {
            render: (width) => component?.render(width) ?? [],
            handleInput: (data) => {
                component?.handleInput(data);
                tui.requestRender();
            },
            invalidate: () => component?.invalidate(),
            dispose: () => {
                poller?.stop();
                component?.dispose();
            },
        };
    });
}
/**
 * Install the panel: the on-demand `/wg-viz` command only.
 *
 * Deliberately mounts **no** ambient surface at load time — no `setWidget`,
 * no poll timer, no session lifecycle hooks. The full panel is created on
 * demand by the command handler (`openVizPanel`) and disposed when the user
 * closes it. Non-TUI modes (`rpc`/`json`/`print`) degrade silently — the
 * command no-ops.
 */
export function installVizPanel(pi, backend, env) {
    pi.registerCommand("wg-viz", {
        description: "Live WG work-graph panel (read-only): tree, selection, task detail, log tail",
        handler: async (_args, ctx) => {
            // TUI-only surface: silently no-op in rpc/json/print modes.
            if (ctx.mode !== "tui")
                return;
            try {
                const result = await vizSnapshotWithFallback(backend, env, { signal: ctx.signal });
                const initial = result.kind === "snapshot" ? result.snapshot : null;
                const ascii = result.kind === "ascii" ? result.text : null;
                await openVizPanel(ctx, env, initial, ascii);
            }
            catch {
                // Silent degrade: the panel must never throw into the chat.
            }
        },
    });
}
//# sourceMappingURL=viz-panel.js.map