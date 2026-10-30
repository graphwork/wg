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
 * Structure is WG's own: the tree is `GetFleet.tree.text` — the exact ASCII
 * `wg viz` renders — returned verbatim, with per-line status colour from
 * `tree.node_lines`. The legacy TypeScript tree (`buildFleetTree`) is a
 * best-effort fallback only (when no rendered tree is available).
 *
 * Interactions:
 *   - **wheel / PgUp / PgDn / Home / End** scroll the dependency TREE; the
 *     selection is dragged along so it always stays visible;
 *   - **press-and-drag** over the content pans the tree (touch-style drag-pan)
 *     and the gesture is *captured* so it keeps panning even when the pointer
 *     leaves the content area; release ends the drag;
 *   - **click** selects the hit tree row; clicking the already-selected row
 *     (or a double-click) opens its detail — mirroring the TUI's
 *     click-selects/inspector-shows model;
 *   - **↑/↓** move the selection and scroll it into view;
 *   - **Enter / → / l** drill into the selected task's detail. The detail is
 *     WG's OWN text — the exact `wg show <task>` body, fetched from the daemon
 *     (`GetTaskDetail`) with a CLI fallback — rendered VERBATIM and full-height
 *     in the panel, with a visible scroll/position indicator. The pi-side live
 *     agent activity and a bounded stream tail are APPENDED after WG's text and
 *     clearly labelled as additions, never mixed into it;
 *   - in the detail, **← / h / Backspace / q / Esc / Enter** all return to the
 *     tree (one consistent escape; only `q`/`Esc` from the TREE close the panel);
 *   - **o / space** expand/collapse the selected subtree;
 *   - **q / Esc** close (from the tree).
 *
 * Strictly read-only: the only data sources are the daemon's `GetFleet` and
 * `GetTaskDetail` reads (`WgBackend.getFleet` / `getTaskDetail`, each with its
 * CLI fallback) and a bounded tail read of the agent's own stream file. It never
 * mutates graph state, is TUI-only (guarded on `ctx.mode`), and degrades silently
 * elsewhere.
 */
import { Key, matchesKey, truncateToWidth } from "@earendil-works/pi-tui";
import { taskColor } from "./fleet-readmodel.js";
import { paintStatusText, paletteLabel, paletteWarning, resolveStatusPalette, } from "./status-palette.js";
import { MAX_FLEET_HEIGHT_PCT, clampHeightFraction } from "./fleet-config.js";
import { DEFAULT_TRANSCRIPT_LINES, FLEET_PANEL_DEFAULT_HEIGHT, FLEET_PANEL_MIN_HEIGHT, agentActivityLabel, agentForTask, agentUsageDetailLabel, agentUsageLabel, boundTranscriptBody, buildFleetTree, clampScroll, fleetCountsHeader, maxScroll, pageScroll, readAgentStreamTail, renderWgTree, scrollToKeepVisible, } from "./fleet-panel-model.js";
/** Default bounded poll cadence for the open panel. */
export const FLEET_PANEL_POLL_MS = 5000;
/**
 * Keyboard resize step as a fraction of the terminal height. `+`/`]` grow the
 * panel, `-`/`[` shrink it, in 5% increments (2 rows on a 40-row terminal).
 */
export const FLEET_RESIZE_STEP_PCT = 0.05;
/** How long a transient in-panel notice (e.g. the Ctrl+C explanation) stays. */
export const FLEET_NOTICE_MS = 4000;
/**
 * Keys that resize the panel: `+`/`=`/`]` grow, `-`/`_`/`[` shrink.
 *
 * There is a keyboard path for every mouse gesture (pi's regular mode has no
 * mouse at all), and this is the resize path — the operator-reported gap
 * ("no way to modulate the screen fraction covered") applies to both.
 */
export function resizeDeltaForInput(data) {
    if (data === "+" || data === "=" || data === "]")
        return 1;
    if (data === "-" || data === "_" || data === "[")
        return -1;
    return 0;
}
/**
 * Enter is both "open detail" (tree) and "back to tree" (detail). Some
 * terminals deliver Enter as two events (`\r` then `\n`); ignore a second
 * Enter within this window so a single keypress cannot open-then-immediately
 * close the view. A real second press lands well outside the window.
 */
export const DETAIL_ENTER_GUARD_MS = 150;
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
    detailFetcher;
    snapshot;
    selectedId;
    collapsed = new Set();
    mode = "tree";
    treeScroll = 0;
    detailScroll = 0;
    /** Cached, bounded transcript tail lines for the selected task (detail mode). */
    detailTail = [];
    /** WG's OWN detail text (`wg show`-equivalent), split into physical lines. */
    detailLines = [];
    /** Set when the WG detail fetch failed (daemon + CLI both unavailable). */
    detailError = null;
    /** True while a detail fetch is in flight. */
    detailLoading = false;
    /** Monotonic guard so a late response for a stale selection is dropped. */
    detailRequestSeq = 0;
    /** Timestamp of the last detail open (Enter double-delivery guard). */
    detailOpenedAt = 0;
    hitMap = [];
    /**
     * An active press-and-drag pan gesture. Set on a content-area press and kept
     * until release; while set, every drag/move keeps panning the active pane
     * even when the pointer leaves the content area (capture semantics).
     */
    drag = null;
    /** True while a press gesture is awaiting its synthesized `click`. */
    pressGesture = false;
    /** Whether the row under the current press was selected *before* the press. */
    pressWasSelected = false;
    /**
     * An active top-edge RESIZE gesture (press on the header row). Kept until
     * release so the drag keeps resizing even when the pointer leaves the header,
     * and so the chosen fraction is persisted exactly once, on release.
     */
    resize = null;
    /** Cockpit height as a fraction of the terminal (persisted by the caller). */
    heightPct;
    /** A transient in-panel notice (e.g. "ctrl+c ignored") and its expiry. */
    notice = null;
    /** The trigger that closed the panel (`q` / `escape`), for the close notify. */
    closeTrigger = null;
    cachedLines = null;
    cachedWidth = -1;
    cachedHeight = -1;
    disposed = false;
    /** Read-only dormant view (counts + hint; no cockpit, no navigation). */
    inactive;
    constructor(snapshot, tui, onClose, theme = null, options = {}, liveDir = undefined, detailFetcher = undefined) {
        this.tui = tui;
        this.onClose = onClose;
        this.theme = theme;
        this.options = options;
        this.liveDir = liveDir;
        this.detailFetcher = detailFetcher;
        this.snapshot = snapshot;
        this.inactive = options.inactive === true;
        this.heightPct = clampHeightFraction(options.heightPct ?? MAX_FLEET_HEIGHT_PCT);
        this.selectedId = options.initialTaskId ?? this.firstVisibleId();
        this.refreshDetailTail();
    }
    // ── state / data ──────────────────────────────────────────────────────────
    /** Apply a fresh snapshot from the poller and re-render. */
    setSnapshot(snapshot) {
        this.snapshot = snapshot;
        if (this.inactive) {
            this.invalidate();
            this.tui.requestRender();
            return;
        }
        if (this.selectedId && !snapshot?.tasks.some((t) => t.id === this.selectedId)) {
            this.selectedId = this.firstVisibleId();
        }
        this.refreshDetailTail();
        // Keep WG's detail text live while the detail view is open.
        if (this.mode === "detail" && this.selectedId)
            this.loadDetail();
        this.invalidate();
        this.tui.requestRender();
    }
    dispose() {
        this.disposed = true;
    }
    // ── viewport geometry ─────────────────────────────────────────────────────
    viewportHeight() {
        const rows = this.terminalRows();
        const scaled = Math.round(rows * this.heightPct);
        return Math.max(FLEET_PANEL_MIN_HEIGHT, Math.min(rows, scaled));
    }
    /** The full (unscaled) row budget the panel may occupy. */
    terminalRows() {
        let rows = FLEET_PANEL_DEFAULT_HEIGHT;
        try {
            rows = this.options.height?.() ?? this.tui.terminal?.rows ?? FLEET_PANEL_DEFAULT_HEIGHT;
        }
        catch {
            rows = FLEET_PANEL_DEFAULT_HEIGHT;
        }
        if (!Number.isFinite(rows) || rows <= 0)
            rows = FLEET_PANEL_DEFAULT_HEIGHT;
        return Math.max(FLEET_PANEL_MIN_HEIGHT, Math.trunc(rows));
    }
    /**
     * The palette FALLBACK warning row, or `null` when WG's RGB is in play.
     *
     * It occupies its own chrome row (never squeezed into the header), so a
     * wrong-coloured panel always says why: `palette: fallback (daemon older than
     * plugin) — run wg dev-sync`.
     */
    paletteWarningRow() {
        if (!this.snapshot)
            return null;
        return paletteWarning(resolveStatusPalette(this.snapshot.tree?.palette), {
            hasWgTree: !!this.snapshot.tree?.text,
        });
    }
    /** Chrome rows above the body in the live view (header + palette warning). */
    chromeRows() {
        return 1 + (this.paletteWarningRow() ? 1 : 0);
    }
    /** Body rows between the fixed header and footer. */
    bodyViewport() {
        return Math.max(1, this.viewportHeight() - 2 - (this.paletteWarningRow() ? 1 : 0));
    }
    /**
     * Scrollable rows for the DORMANT view: it reserves 4 chrome rows (counts +
     * two hint lines + a spacer), the palette warning row when present, and 1
     * footer row, so its window is smaller than the live cockpit's. Scroll
     * clamping and rendering MUST agree on this or the offset jumps on the next
     * render.
     */
    inactiveBody() {
        return Math.max(1, this.bodyViewport() - 5);
    }
    /** Scrollable rows of the currently active pane. */
    activeBody() {
        return this.inactive ? this.inactiveBody() : this.bodyViewport();
    }
    /**
     * Inclusive panel rows that hold scrollable body content: the header (and the
     * palette warning row, when present) above, the last row is the footer,
     * everything between is content. Presses outside this range are ignored (never
     * start a selection or a drag/resize).
     */
    contentRows() {
        return { top: this.chromeRows(), bottom: this.viewportHeight() - 2 };
    }
    /**
     * Map a panel-local `y` to the index of the rendered tree line it hit, or
     * `null` when it is outside the content area. The index addresses the *whole*
     * rendered line list (including blank/arc rows with `taskId === null`), which
     * is what keeps hit-testing correct when the tree is scrolled or has sections
     * collapsed — the panel's own window is `[treeScroll, treeScroll + body)`.
     */
    contentLineIndex(y) {
        if (typeof y !== "number" || !Number.isFinite(y))
            return null;
        const { top, bottom } = this.contentRows();
        const row = Math.trunc(y);
        if (row < top || row > bottom)
            return null;
        return this.treeScroll + (row - top);
    }
    // ── tree / selection helpers ──────────────────────────────────────────────
    /**
     * The active tree rows. Prefers WG's own rendered text (`GetFleet.tree`);
     * only when it is unavailable does it fall back to the legacy TS re-derivation
     * (best-effort — explicitly not the default path).
     */
    treeLines() {
        if (!this.snapshot)
            return [];
        const wg = renderWgTree(this.snapshot.tree, this.collapsed);
        if (wg.fromWg)
            return wg.lines.map((l) => ({ text: l.text, taskId: l.taskId }));
        return buildFleetTree(this.snapshot, this.collapsed).lines.map((l) => ({
            text: l.text,
            taskId: l.taskId,
        }));
    }
    visibleOrder() {
        return this.treeLines()
            .map((l) => l.taskId)
            .filter((id) => id !== null);
    }
    /** Total rendered tree line count (includes blank/arc rows). */
    treeLineCount() {
        return this.treeLines().length;
    }
    /** Index of the selected task within the *rendered* line list, or -1. */
    selectedLineIndex() {
        if (!this.selectedId)
            return -1;
        return this.treeLines().findIndex((l) => l.taskId !== null && l.taskId === this.selectedId);
    }
    firstVisibleId() {
        return this.visibleOrder()[0] ?? null;
    }
    selectionIndex() {
        if (!this.selectedId)
            return -1;
        return this.visibleOrder().indexOf(this.selectedId);
    }
    /**
     * Keep the selection inside the current viewport after a scroll/page: if the
     * selected line fell outside the window, snap the *selection* to the nearest
     * visible task (the "selection is dragged along" rule). The scroll offset is
     * left untouched here — wheel/PgDn own it, `moveSelection` moves the view.
     */
    dragSelectionIntoView() {
        const lines = this.treeLines();
        if (lines.length === 0) {
            this.selectedId = null;
            this.treeScroll = 0;
            return;
        }
        const view = this.bodyViewport();
        const idx = this.selectedLineIndex();
        if (idx === -1 || idx < this.treeScroll) {
            const hit = lines.slice(this.treeScroll).find((l) => l.taskId !== null);
            this.selectedId = hit?.taskId ?? this.selectedId;
        }
        else if (idx >= this.treeScroll + view) {
            const window = lines.slice(this.treeScroll, this.treeScroll + view);
            const hit = [...window].reverse().find((l) => l.taskId !== null);
            this.selectedId = hit?.taskId ?? this.selectedId;
        }
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
        const lines = this.treeLines();
        const lineIdx = lines.findIndex((l) => l.taskId !== null && l.taskId === this.selectedId);
        this.treeScroll = scrollToKeepVisible(this.treeScroll, this.bodyViewport(), lineIdx, lines.length);
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
        this.treeScroll = maxScroll(this.treeLineCount(), this.bodyViewport());
        this.mode = "tree";
        this.invalidate();
        this.tui.requestRender();
    }
    /** Scroll the active pane by `delta` rows, keeping the selection visible. */
    scrollBy(delta) {
        if (!Number.isFinite(delta) || delta === 0)
            return;
        if (this.mode === "detail" && !this.inactive) {
            const total = this.detailContent().length;
            this.detailScroll = clampScroll(this.detailScroll + delta, total, this.bodyViewport());
        }
        else {
            const total = this.treeLineCount();
            this.treeScroll = clampScroll(this.treeScroll + delta, total, this.activeBody());
            if (!this.inactive)
                this.dragSelectionIntoView();
        }
        this.invalidate();
        this.tui.requestRender();
    }
    /**
     * Pan the active pane by `delta` rows *without* dragging the selection.
     *
     * This is the drag gesture's scroll primitive: a pan moves the content under
     * the pointer (the TUI's `graph_pan_last` behaviour) and leaves the selection
     * where the user put it, unlike wheel/PgDn which drag the selection along.
     */
    panBy(delta) {
        if (!Number.isFinite(delta) || delta === 0)
            return;
        if (this.mode === "detail") {
            const total = this.detailContent().length;
            this.detailScroll = clampScroll(this.detailScroll + delta, total, this.bodyViewport());
        }
        else {
            this.treeScroll = clampScroll(this.treeScroll + delta, this.treeLineCount(), this.bodyViewport());
        }
        this.invalidate();
        this.tui.requestRender();
    }
    /**
     * Select the task rendered at `lineIndex` (an index into the *rendered* line
     * list). Returns the selected id, or `null` for a blank/arc row that maps to
     * no task (the press is then consumed but changes nothing).
     */
    selectTreeLine(lineIndex) {
        if (lineIndex < 0)
            return null;
        const id = this.treeLines()[lineIndex]?.taskId ?? null;
        if (!id)
            return null;
        this.selectedId = id;
        this.mode = "tree";
        this.detailScroll = 0;
        this.invalidate();
        this.tui.requestRender();
        return id;
    }
    /** Page the active pane by `direction` (±1), keeping the selection visible. */
    pageBy(direction) {
        const view = this.activeBody();
        if (this.mode === "detail" && !this.inactive) {
            const total = this.detailContent().length;
            this.detailScroll = pageScroll(this.detailScroll, view, direction, total);
        }
        else {
            const total = this.treeLineCount();
            this.treeScroll = pageScroll(this.treeScroll, view, direction, total);
            if (!this.inactive)
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
        this.detailOpenedAt = Date.now();
        this.refreshDetailTail();
        this.loadDetail();
        this.invalidate();
        this.tui.requestRender();
    }
    /**
     * Fetch WG's OWN detail text for the selected task (the exact `wg show`
     * body) and re-render. Best-effort: a failed fetch shows a labelled error and
     * still renders the pi-side additions, never an approximation of WG's text.
     */
    loadDetail() {
        const taskId = this.selectedId;
        if (!taskId) {
            this.detailLines = [];
            this.detailError = null;
            return;
        }
        if (!this.detailFetcher) {
            this.detailLines = [];
            this.detailError = "no detail source (daemon/CLI unavailable)";
            return;
        }
        const seq = ++this.detailRequestSeq;
        this.detailLoading = true;
        this.detailError = null;
        const columns = this.detailWidth();
        void this.detailFetcher(taskId, columns)
            .then((detail) => {
            if (seq !== this.detailRequestSeq || taskId !== this.selectedId)
                return;
            if (detail && detail.text) {
                // WG's text verbatim — only split into physical lines for the pane.
                this.detailLines = detail.text.replace(/\n$/, "").split("\n");
                this.detailError = null;
            }
            else {
                this.detailLines = [];
                this.detailError = "WG detail unavailable (daemon + `wg show` both failed)";
            }
        })
            .catch((err) => {
            if (seq !== this.detailRequestSeq || taskId !== this.selectedId)
                return;
            this.detailLines = [];
            this.detailError = err instanceof Error ? err.message : String(err);
        })
            .finally(() => {
            if (seq !== this.detailRequestSeq)
                return;
            this.detailLoading = false;
            this.invalidate();
            this.tui.requestRender();
        });
    }
    /** Panel width for the daemon-side word wrap; falls back to the raw column count. */
    detailWidth() {
        try {
            const c = this.tui.terminal?.columns;
            return typeof c === "number" && c > 0 ? Math.trunc(c) : undefined;
        }
        catch {
            return undefined;
        }
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
    /**
     * Close the panel from a NAMED trigger (`q` / `escape`) so the caller can
     * announce it (`fleet closed (q)`). Closing is always deliberate: it happens
     * only on an explicit close key, never on Ctrl+C (see {@link handleInput}).
     */
    closeWith(trigger) {
        this.closeTrigger = trigger;
        this.close();
    }
    /** The trigger that closed the panel, or `null` while it is open. */
    get closedWith() {
        return this.closeTrigger;
    }
    // ── height fraction (drag-resize + keyboard) ──────────────────────────────
    /** The cockpit height as a fraction of the terminal (`0.2`–`1`). */
    get heightFraction() {
        return this.heightPct;
    }
    /**
     * Set the cockpit height fraction and re-render.
     *
     * `persist` (default true) reports the new value to the caller's
     * `onHeightPctChange` so it can be written to the plugin config — the
     * operator-reported gap that the chosen size did not stick. A drag passes
     * `persist: false` per step and reports once on release, so a drag is one
     * write, not fifty.
     */
    setHeightPct(pct, opts = {}) {
        const next = clampHeightFraction(pct, MAX_FLEET_HEIGHT_PCT);
        if (next === this.heightPct)
            return this.heightPct;
        this.heightPct = next;
        // A shorter pane must not leave the scroll offset past the end.
        const body = this.activeBody();
        this.treeScroll = clampScroll(this.treeScroll, this.treeLineCount(), body);
        if (this.mode === "detail") {
            this.detailScroll = clampScroll(this.detailScroll, this.detailContent().length, body);
        }
        this.invalidate();
        this.tui.requestRender();
        if (opts.persist !== false)
            this.reportHeightPct(next);
        return this.heightPct;
    }
    /** Grow/shrink the panel by `steps` × {@link FLEET_RESIZE_STEP_PCT}. */
    resizeBy(steps) {
        if (!Number.isFinite(steps) || steps === 0)
            return this.heightPct;
        const next = Math.round((this.heightPct + steps * FLEET_RESIZE_STEP_PCT) * 100) / 100;
        return this.setHeightPct(next);
    }
    reportHeightPct(pct) {
        try {
            this.options.onHeightPctChange?.(pct);
        }
        catch {
            /* a persistence failure must never break the panel */
        }
    }
    /**
     * Show a transient in-panel notice (footer line) for `ms`. Used to make a
     * swallowed key knowable: an ignored Ctrl+C says so instead of silently
     * vanishing (the operator-reported "i clicked and somehow got out?" mystery).
     */
    showNotice(text, ms = FLEET_NOTICE_MS) {
        this.notice = { text, until: Date.now() + ms };
        this.invalidate();
        this.tui.requestRender();
    }
    /** The currently-visible transient notice, or `null` once it expired. */
    get visibleNotice() {
        if (!this.notice)
            return null;
        if (Date.now() >= this.notice.until)
            return null;
        return this.notice.text;
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
        // Ctrl+C NEVER closes the fleet panel.
        //
        // The operator-reported mystery ("i clicked and somehow got out? escape?
        // control+c? unclear which") is exactly this: Ctrl+C is pi's global
        // clear/interrupt, and a panel that lets it through can vanish under a
        // habitual keypress with no explanation. pi-tui deliberately hands Ctrl+C to
        // the focused component ("the focused component can decide how to handle
        // Ctrl+C"), so we swallow it here and SAY SO instead of exiting. Closing is
        // deliberate: `q` or `Esc` from the tree (announced by the close notify).
        if (matchesKey(data, Key.ctrl("c")) || data === "\x03") {
            this.showNotice("ctrl+c ignored — press q or Esc to close the fleet view");
            return;
        }
        // Height control has a keyboard path in EVERY state (inactive included):
        // `+`/`]` grow, `-`/`[` shrink, and the fraction is persisted by the caller.
        const resizeSteps = resizeDeltaForInput(data);
        if (resizeSteps !== 0) {
            const pct = this.resizeBy(resizeSteps);
            this.showNotice(`fleet height ${Math.round(pct * 100)}%`);
            return;
        }
        // The dormant view is READ-ONLY: there is no cockpit to navigate. Only
        // close and scroll are meaningful.
        if (this.inactive) {
            if (matchesKey(data, Key.escape)) {
                this.closeWith("esc");
                return;
            }
            if (data === "q") {
                this.closeWith("q");
                return;
            }
            if (matchesKey(data, Key.up))
                this.scrollBy(-1);
            else if (matchesKey(data, Key.down))
                this.scrollBy(1);
            else if (matchesKey(data, Key.pageUp))
                this.pageBy(-1);
            else if (matchesKey(data, Key.pageDown))
                this.pageBy(1);
            else if (matchesKey(data, Key.home))
                this.scrollBy(-this.treeLineCount());
            else if (matchesKey(data, Key.end))
                this.scrollBy(this.treeLineCount());
            return;
        }
        if (this.mode === "detail") {
            // In the detail view q/Esc (and ←/h/Backspace) go BACK to the tree — the
            // same key that drilled in, so there is one consistent escape. Enter also
            // returns, with a short debounce so a terminal that delivers Enter as
            // `\r` then `\n` cannot open and immediately close the view.
            if (matchesKey(data, Key.escape) ||
                data === "q" ||
                data === "h" ||
                data === "l" ||
                matchesKey(data, Key.left) ||
                matchesKey(data, Key.backspace)) {
                this.closeDetail();
                return;
            }
            if (matchesKey(data, Key.enter)) {
                const guard = this.options.enterGuardMs ?? DETAIL_ENTER_GUARD_MS;
                if (Date.now() - this.detailOpenedAt >= guard)
                    this.closeDetail();
                return;
            }
            if (matchesKey(data, Key.up))
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
        // Tree view: q/Esc closes the whole panel — deliberately, and announced.
        if (matchesKey(data, Key.escape)) {
            this.closeWith("esc");
            return;
        }
        if (data === "q") {
            this.closeWith("q");
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
        else if (data === "o" || matchesKey(data, Key.space))
            this.toggleExpand();
    }
    /**
     * Mouse support, mirroring the WG TUI's mouse model.
     *
     *   - **wheel** scrolls the active pane (selection follows);
     *   - **press** in the content area selects the hit row (from the rendered
     *     line hit-map, so it is correct under scroll/collapse), focuses the
     *     panel, and begins a pan gesture with `capture: true` so the subsequent
     *     drag/release keep routing here even when the pointer leaves the
     *     content area;
     *   - **drag** pans the active pane by the pointer delta (no selection drag);
     *   - **release** ends the gesture;
     *   - **click** (pi synthesizes it on a release that did not move) selects the
     *     hit row and — when it was *already* selected before the press, or on a
     *     double-click — opens its detail, mirroring the TUI's click-selects /
     *     inspector-shows behaviour with an explicit second click.
     *
     * Returns pi's `PiMouseResult` shape — a truthy object for a consumed event
     * and `undefined` when the event is not ours — NEVER a bare boolean (a
     * truthy non-object crashes pi core in `dispatchMouseEvent`; see `mouse.ts`).
     */
    handleMouse(event) {
        if (this.disposed)
            return undefined;
        const type = event.type ?? "";
        if (type === "wheel" || type === "mouse.wheel") {
            const delta = typeof event.wheelDelta === "number" && event.wheelDelta !== 0 ? event.wheelDelta : 0;
            if (delta !== 0)
                this.scrollBy(delta);
            return { handled: true };
        }
        // Dormant read-only view: there is nothing to select, open or resize.
        // Clicks are ignored (unhandled) so the host keeps its own behaviour.
        if (this.inactive)
            return undefined;
        // A live RESIZE gesture (top-edge press) owns the pointer stream until
        // release. `capture: true` on the press keeps drag/release routed here even
        // when the pointer leaves the header row, and the panel's height follows the
        // pointer: drag UP → taller, drag DOWN → shorter. Because pi reports the
        // pointer relative to the CAPTURED target's origin (fixed at press time),
        // the local `y` delta is the screen delta even while the panel resizes.
        if (type === "drag" || type === "mouse.drag" || type === "move" || type === "mouse.move") {
            if (this.resize) {
                const y = typeof event.y === "number" && Number.isFinite(event.y)
                    ? Math.trunc(event.y)
                    : this.resize.startY;
                const dy = this.resize.startY - y;
                const startRows = Math.round(this.resize.rows * this.resize.startPct);
                const pct = clampHeightFraction((startRows + dy) / this.resize.rows);
                if (pct !== this.heightPct) {
                    this.resize.moved = true;
                    this.setHeightPct(pct, { persist: false });
                }
                return { handled: true, capture: true };
            }
            if (!this.drag)
                return undefined;
            const y = typeof event.y === "number" && Number.isFinite(event.y) ? Math.trunc(event.y) : this.drag.prevY;
            const dy = this.drag.prevY - y;
            if (dy !== 0) {
                this.drag.moved = true;
                this.panBy(dy);
            }
            this.drag.prevY = y;
            return { handled: true, capture: true };
        }
        if (type === "release" || type === "mouse.release") {
            if (this.resize) {
                const { moved } = this.resize;
                this.resize = null;
                // Persist ONCE per gesture (on release), not on every drag step.
                if (moved) {
                    this.reportHeightPct(this.heightPct);
                    this.showNotice(`fleet height ${Math.round(this.heightPct * 100)}% (saved)`);
                }
                return { handled: true };
            }
            if (!this.drag && !this.pressGesture)
                return undefined;
            const moved = this.drag?.moved ?? false;
            this.drag = null;
            // A moved gesture never produces a synthesized click, so its press state
            // must not linger into the next gesture.
            if (moved) {
                this.pressGesture = false;
                this.pressWasSelected = false;
            }
            return { handled: true };
        }
        // DETAIL VIEW: the header row is the clickable BACK affordance (`[< Back]`).
        // A click on the header is outside the content area, so it never starts a
        // press gesture; pi synthesizes the standalone click and it lands here —
        // mirroring `←`/`h`/`q`/`Esc` exactly.
        if (type === "click" || type === "mouse.click") {
            const headerRow = typeof event.y === "number" && Number.isFinite(event.y) ? Math.trunc(event.y) : -1;
            if (this.mode === "detail" && headerRow === 0) {
                this.pressGesture = false;
                this.pressWasSelected = false;
                this.drag = null;
                this.resize = null;
                this.closeDetail();
                return { handled: true };
            }
        }
        if (type === "press" || type === "mouse.press") {
            const y = typeof event.y === "number" && Number.isFinite(event.y) ? Math.trunc(event.y) : Number.NaN;
            // TREE VIEW, header row (0) = the resize grip: the panel's visible top
            // edge. Pressing it starts a captured height drag instead of a selection.
            if (this.mode === "tree" && y === 0) {
                this.drag = null;
                this.pressGesture = false;
                this.pressWasSelected = false;
                this.resize = {
                    startY: y,
                    startPct: this.heightPct,
                    rows: this.terminalRows(),
                    moved: false,
                };
                return { handled: true, capture: true, focus: true };
            }
            const lineIndex = this.contentLineIndex(event.y);
            if (lineIndex === null)
                return undefined; // header/footer/out of content area
            const wasSelected = this.selectedId;
            this.drag = { prevY: y, moved: false };
            this.pressGesture = true;
            const hitId = this.mode === "tree" ? this.selectTreeLine(lineIndex) : null;
            this.pressWasSelected = hitId !== null && hitId === wasSelected;
            this.tui.requestRender?.();
            return { handled: true, capture: true, focus: true };
        }
        if (type === "click" || type === "mouse.click") {
            const lineIndex = this.contentLineIndex(event.y);
            const fromPress = this.pressGesture;
            const preSelected = fromPress && this.pressWasSelected;
            const double = typeof event.clickCount === "number" && event.clickCount >= 2;
            const wasSelected = this.selectedId;
            this.pressGesture = false;
            this.pressWasSelected = false;
            this.drag = null;
            this.resize = null;
            if (this.mode !== "tree")
                return fromPress ? { handled: true } : undefined;
            if (lineIndex === null)
                return fromPress ? { handled: true } : undefined;
            const id = this.selectTreeLine(lineIndex);
            if (!id)
                return { handled: true };
            // Open detail when the row was already selected before the press (the
            // "click the selected node" affordance), on a double-click, or when a
            // standalone click (no preceding press) hit the current selection.
            if (double || preSelected || (!fromPress && id === wasSelected))
                this.openDetail();
            return { handled: true };
        }
        return undefined;
    }
    // ── rendering ─────────────────────────────────────────────────────────────
    invalidate() {
        this.cachedLines = null;
    }
    /** The full (unwindowed) detail view content for the selected task. */
    detailContent() {
        if (!this.snapshot || !this.selectedId)
            return [];
        const out = [];
        // (1) WG's OWN detail text (`wg show`-equivalent), rendered verbatim.
        if (this.detailLines.length > 0) {
            for (const text of this.detailLines)
                out.push({ text, color: "text" });
        }
        else if (this.detailError) {
            out.push({ text: "wg-fleet: WG detail unavailable", color: "warning" });
            out.push({ text: `  ${this.detailError}`, color: "dim" });
        }
        else if (this.detailLoading) {
            out.push({ text: "loading WG detail (wg show)…", color: "dim" });
        }
        else {
            out.push({ text: "wg-fleet: no WG detail for this task", color: "dim" });
        }
        // (2) pi-side additions — clearly separated and labelled so they are never
        // mistaken for WG's own output.
        const agent = agentForTask(this.snapshot.agents, this.selectedId);
        const activity = agentActivityLabel(agent);
        const usage = agentUsageDetailLabel(agent);
        if (activity || usage || this.detailTail.length > 0) {
            out.push({ text: "", color: "dim" });
            out.push({ text: "── pi-side additions (not WG detail) ──", color: "accent" });
        }
        if (activity || usage) {
            out.push({ text: `── Live activity ──`, color: "accent" });
            if (activity) {
                out.push({ text: `  ${agent?.id ?? "agent"} · ${activity}`, color: "warning" });
            }
            if (usage) {
                // Full, un-abbreviated counts (the detail/inspector view).
                out.push({ text: `  usage · ${usage}`, color: "accent" });
            }
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
        const lines = [];
        const taskIds = [];
        for (const line of this.treeLines()) {
            // The dormant view is a STATIC read-only projection: no selection marker
            // and no live per-agent activity (there is no cockpit and nothing polls).
            const marker = !this.inactive && line.taskId !== null && line.taskId === this.selectedId ? "❯ " : "  ";
            let text = `${marker}${line.text}`;
            if (line.taskId && !this.inactive) {
                const agent = agentForTask(this.snapshot.agents, line.taskId);
                const activity = agentActivityLabel(agent);
                if (activity)
                    text += ` · ${activity}`;
                // Compact live usage alongside the activity (same format as the
                // ambient agent rows: `12.3k tok · 14 turns · 31 tools`).
                const usage = agentUsageLabel(agent);
                if (usage)
                    text += ` · ${usage}`;
            }
            // Every rendered line is painted with WG's status palette (task lines) or
            // dim (separators/arc rows); the caller re-applies this per line in
            // `render` so the computed style actually reaches the terminal.
            const status = line.taskId ? statusById.get(line.taskId) : undefined;
            lines.push({ text, color: status ? taskColor(status) : "dim", status });
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
        const palette = resolveStatusPalette(snapshot?.tree?.palette);
        const lines = [];
        let hitMap = [];
        if (this.inactive) {
            // ── dormant read-only view: counts + hint, NOT the live cockpit ──
            // Checked FIRST: a dormant session usually has NO daemon running, and the
            // whole point of this view is that it still explains itself.
            const counts = snapshot
                ? `${fleetCountsHeader(snapshot.counts).replace("wg-fleet ▸ ", "")} · inactive`
                : "inactive · no live counts (daemon offline)";
            const palette = resolveStatusPalette(snapshot?.tree?.palette);
            const header = `wg-fleet ▸ ⇕ ${Math.round(this.heightPct * 100)}% · inactive · ${paletteLabel(palette)} · ${counts}`;
            lines.push({ text: header, color: "accent" });
            const warning = this.paletteWarningRow();
            if (warning)
                lines.push({ text: `  ⚠ ${warning}`, color: "warning" });
            lines.push({
                text: "  WG is dormant in this session — nothing is polling and no agents are tracked.",
                color: "dim",
            });
            lines.push({
                text: "  Run /wg on to activate the wg tool family and the live fleet cockpit.",
                color: "warning",
            });
            lines.push({ text: "", color: "dim" });
            const tree = this.treeContent();
            // Leave room for the 4 chrome rows above and the footer below so the
            // hint/exit line is never the part that gets clipped.
            const room = this.inactiveBody();
            this.treeScroll = clampScroll(this.treeScroll, tree.lines.length, room);
            lines.push(...tree.lines.slice(this.treeScroll, this.treeScroll + room));
            lines.push({ text: "  read-only · ↑/↓ scroll · +/- resize · q/Esc close · /wg on to activate", color: "dim" });
        }
        else if (!snapshot) {
            lines.push({ text: "wg-fleet · daemon offline", color: "dim" });
            lines.push({ text: "  no fleet snapshot available · q close", color: "dim" });
        }
        else if (snapshot.tasks.length === 0) {
            lines.push({ text: fleetCountsHeader(snapshot.counts), color: "accent" });
            lines.push({ text: "  no tasks in the graph · q close", color: "dim" });
        }
        else if (this.mode === "detail") {
            const detail = this.detailContent();
            // Position indicator lives in the HEADER (always visible even when a
            // long body is clipped at the bottom of pi's overlay) as well as the
            // footer (key hints). `[< Back]` leads the header so the back affordance
            // is VISIBLE at the top-left — the operator-reported "we can't click at the
            // top to get back to the normal view" is a discovery failure, not only a
            // wiring one.
            const total = detail.length;
            const maxOff = maxScroll(total, body);
            const pct = maxOff > 0 ? Math.round((this.detailScroll / maxOff) * 100) : 100;
            const top = total === 0 ? 0 : this.detailScroll + 1;
            const bottom = Math.min(total, this.detailScroll + body);
            this.detailScroll = clampScroll(this.detailScroll, total, body);
            lines.push({
                text: this.color("accent", `[< Back] · wg-fleet · ${this.selectedId ?? "?"} · detail (wg show) · ${pct}% [lines ${top}-${bottom}/${total}]`),
                color: "accent",
            });
            const windowed = detail.slice(this.detailScroll, this.detailScroll + body);
            lines.push(...windowed);
            lines.push({
                text: this.footerText("  PgUp/PgDn ↑/↓ Home/End scroll · ←/q/Esc or click [< Back] to return to the tree · +/- resize"),
                color: "dim",
            });
        }
        else {
            // The palette state is ALWAYS stated (never a silent fallback): a short
            // label right after the resize grip (so truncation can never hide it) plus
            // a dedicated WARNING row when the local mirror is painting. This is the
            // answer to "colors are not those in wg viz/wg tui".
            const counts = fleetCountsHeader(snapshot.counts).replace("wg-fleet ▸ ", "");
            const header = `wg-fleet ▸ ⇕ ${Math.round(this.heightPct * 100)}% · ${paletteLabel(palette)} · ${counts}`;
            lines.push({ text: header, color: "accent" });
            const warning = this.paletteWarningRow();
            if (warning)
                lines.push({ text: `  ⚠ ${warning}`, color: "warning" });
            const tree = this.treeContent();
            this.treeScroll = clampScroll(this.treeScroll, tree.lines.length, body);
            const windowed = tree.lines.slice(this.treeScroll, this.treeScroll + body);
            lines.push(...windowed);
            hitMap = tree.taskIds.slice(this.treeScroll, this.treeScroll + body);
            // Footer: bounded so the panel never renders past `height`.
            const overflow = tree.lines.length > this.treeScroll + body;
            const footer = overflow
                ? `  ↓ ${tree.lines.length - (this.treeScroll + body)} more · wheel/drag pan · enter detail · o expand · ⇕ drag top edge or +/- resize · q close`
                : "  ↑/↓ select · drag pan · enter detail · o expand · ⇕ drag top edge or +/- resize · q close";
            lines.push({ text: this.footerText(footer), color: "dim" });
        }
        // Never render past the panel bounds; truncate ANSI-safely per line.
        const bounded = lines
            .slice(0, height)
            .map((line) => truncateToWidth(this.paintLine(line, palette), width));
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
    /**
     * A footer line, carrying the transient notice when one is live.
     *
     * The notice is how a swallowed key becomes KNOWABLE: an ignored Ctrl+C says
     * `ctrl+c ignored — press q or Esc to close the fleet view` in the panel
     * instead of the panel silently vanishing (or, worse, silently doing nothing
     * with no explanation of why the session feels stuck).
     */
    footerText(base) {
        const notice = this.visibleNotice;
        return notice ? `  ⚠ ${notice}` : base;
    }
    /**
     * Paint one panel line. A task row is painted from WG's exact RGB palette
     * (`paintStatusText`); chrome/separator lines use the semantic theme role.
     */
    paintLine(line, palette) {
        if (line.status) {
            return paintStatusText(this.theme, line.text, line.status, palette, line.color);
        }
        return this.color(line.color, line.text);
    }
    // ── test-visible accessors ────────────────────────────────────────────────
    get selected() {
        return this.selectedId;
    }
    /** Scroll offset of the currently active pane. */
    get scrollOffset() {
        return this.mode === "detail" ? this.detailScroll : this.treeScroll;
    }
    /** True while a press-and-drag pan gesture is active (until release). */
    get isDragging() {
        return this.drag !== null;
    }
    /** True while a top-edge RESIZE gesture is active (until release). */
    get isResizing() {
        return this.resize !== null;
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
    // The close TRIGGER (`q` / `esc`) is carried out through `done()` so the
    // caller can announce it. Without this the panel just disappears and the
    // operator cannot tell what they pressed (the reported "somehow got out?").
    const closedWith = await ctx.ui.custom((tui, theme, _keybindings, done) => {
        const height = () => {
            try {
                return tui.terminal?.rows ?? FLEET_PANEL_DEFAULT_HEIGHT;
            }
            catch {
                return FLEET_PANEL_DEFAULT_HEIGHT;
            }
        };
        const columns = () => {
            try {
                const c = tui.terminal?.columns;
                return typeof c === "number" && c > 0 ? c : undefined;
            }
            catch {
                return undefined;
            }
        };
        // `closeWith` records the trigger BEFORE invoking the close callback, so the
        // resolved custom-prompt value carries the key that closed the panel.
        const component = new FleetPanelComponent(initial, tui, () => done(component.closedWith ?? undefined), theme, {
            height,
            transcriptLines: options.transcriptLines,
            enterGuardMs: options.enterGuardMs,
            inactive: options.inactive === true,
            ...(options.initialTaskId ? { initialTaskId: options.initialTaskId } : {}),
            ...(options.heightPct !== undefined ? { heightPct: options.heightPct } : {}),
            onHeightPctChange: (pct) => options.persistHeightPct?.(pct),
        }, liveDir, options.fetchDetail);
        let inFlight = false;
        // The dormant view is STATIC: one bounded read, no poller (nothing is live
        // while WG is dormant, so polling would be pure churn).
        const timer = options.inactive
            ? null
            : setInterval(() => {
                if (inFlight || component.isDisposed)
                    return;
                inFlight = true;
                void fetchSnapshot(columns())
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
                if (timer)
                    clearInterval(timer);
                component.dispose();
            },
        };
        return instance;
    });
    // Announce the close, naming the trigger, so a panel exit is never mysterious.
    if (closedWith) {
        try {
            ctx.ui.notify(`fleet closed (${closedWith})`, "info");
        }
        catch {
            /* no UI — the close itself already happened */
        }
    }
}
/**
 * The `/wg-fleet` data fetcher: prefer the daemon `GetFleet` read (which itself
 * falls back to the read-only CLI), and degrade to `null` when unavailable.
 *
 * `includeTree` (default true) asks the daemon for WG's own rendered `wg viz`
 * tree so the panel has structure parity without spawning `wg` per refresh.
 */
export function makeFleetFetcher(backend, options = {}) {
    const includeTree = options.includeTree ?? true;
    return async (columns) => {
        if (typeof backend.getFleet === "function") {
            try {
                return await backend.getFleet({
                    timeoutMs: 2000,
                    includeTree,
                    treeColumns: typeof columns === "number" && Number.isFinite(columns) && columns > 0
                        ? Math.trunc(columns)
                        : undefined,
                });
            }
            catch {
                return null;
            }
        }
        return null;
    };
}
/**
 * The `/wg-fleet` detail fetcher: prefer the daemon `GetTaskDetail` read and
 * fall back to the read-only CLI (`wg show <task>`), which itself prints the
 * same text. Returns `null` only when both sources fail, so the panel shows a
 * labelled error rather than a client-side approximation of WG's detail.
 */
export function makeFleetDetailFetcher(backend) {
    return async (taskId, columns) => {
        if (typeof backend.getTaskDetail === "function") {
            try {
                return await backend.getTaskDetail(taskId, {
                    timeoutMs: 2000,
                    columns: typeof columns === "number" && Number.isFinite(columns) && columns > 0
                        ? Math.trunc(columns)
                        : undefined,
                });
            }
            catch {
                return null;
            }
        }
        return null;
    };
}
//# sourceMappingURL=fleet-panel.js.map