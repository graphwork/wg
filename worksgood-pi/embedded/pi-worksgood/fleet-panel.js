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
 * ## Full TUI interaction parity
 *
 * The interaction model is the WG TUI's `viz_viewer`, and the SPEC is the TUI's
 * own help table (`src/tui/viz_viewer/render.rs::draw_help_overlay`). The
 * keymap lives in ONE place (`fleet-bindings.ts`), so `?` can never go stale:
 *
 *   - **wheel / PgUp / PgDn / Home / End / j / k / Ctrl-d / Ctrl-u / g / G**
 *     scroll the focused pane; the selection follows and always stays visible;
 *   - **click** selects the hit tree row; clicking the already-selected row (or
 *     a double-click) opens its detail — the TUI's click-selects/inspector-shows
 *     model; **press-and-drag** pans with capture semantics;
 *   - **↑/↓** move the selection; **Enter / → / l** inspect; **o / Space**
 *     expand/collapse the selected subtree;
 *   - the **inspector dock**: `1`/`2`/`3` dock it at 1/3 · 1/2 · 2/3, `=` cycles
 *     split → panel → graph, `\` toggles it, `i`/`v` grow/shrink the viz pane in
 *     10% steps (wrapping), `p` opens the Panel/Layout row (`r`/`f`/`0` pending
 *     Split/Full/Hidden, `Enter` applies, `Esc` restores the prior layout), and
 *     the **horizontal divider drags** (up grows the inspector, down shrinks it)
 *     with the TUI's exact percentage math;
 *   - **Alt-←/→** cycle the inspector views (WG's verbatim `wg show` text, the
 *     raw JSON, section collapse with **Space**, **R** and **J** for raw JSON);
 *   - the task **actions** `a` / `D` / `f` / `x` / `e` / `M` / `r` shell the SAME
 *     `wg` binary (confirming the destructive ones), and are HIDDEN in a dormant
 *     session;
 *   - **`/`** search (typeahead, `Enter` commits, `Esc` clears), **`s`** cycles
 *     the sort (Chrono↓/↑/Status), **`.`/`<` cycle the plumbing visibility;
 *   - **`?`** opens the auto-generated help modal (`Esc`/`q` closes, `j`/`k`/
 *     PgUp/PgDn/Home/End and the wheel scroll it) — mirroring `handle_help_key`.
 *
 * Every mouse handler returns pi's OBJECT shape (`PiMouseResult`) — never a bare
 * boolean (a truthy non-object crashes pi core in `dispatchMouseEvent`).
 *
 * Reads are the daemon's `GetFleet` / `GetTaskDetail` (each with a CLI
 * fallback); the only mutations are the explicit task actions the operator
 * confirms, issued through the same `WgBackend.run` seam the tools use. TUI-only
 * (guarded on `ctx.mode`) and silently degrading elsewhere.
 */
import { Key, matchesKey, truncateToWidth } from "@earendil-works/pi-tui";
import { taskColor } from "./fleet-readmodel.js";
import { paintStatusText, paletteLabel, paletteWarning, resolveStatusPalette, } from "./status-palette.js";
import { MAX_FLEET_HEIGHT_PCT, clampHeightFraction } from "./fleet-config.js";
import { DEFAULT_TRANSCRIPT_LINES, FLEET_PANEL_DEFAULT_HEIGHT, FLEET_PANEL_MIN_HEIGHT, agentActivityLabel, agentForTask, agentUsageDetailLabel, agentUsageLabel, boundTranscriptBody, buildFleetTree, clampScroll, fleetCountsHeader, maxScroll, pageScroll, readAgentStreamTail, renderWgTree, scrollToKeepVisible, } from "./fleet-panel-model.js";
import { FLEET_BINDINGS, helpLines, matchFleetBinding, validateFleetBindings, } from "./fleet-bindings.js";
import { DEFAULT_FLEET_LAYOUT, applyPreset, cycleLayout, dividerPercentAfterDrag, dockPercent, growDock, growVizPane, isDocked, layoutFromConfig, layoutLabel, normalizeLayout, pendingForLayoutKey, shrinkDock, shrinkVizPane, toggleDock, } from "./fleet-layout.js";
import { FLEET_ACTIONS, actionNotifyLines, availableActions, confirmPrompt, inputPrompt, } from "./fleet-actions.js";
import { FleetSearchState, filterRenderedRows, taskVisible, nextSortMode, nextVisibilityMode, sortLabel, sortTaskRows, visibilityLabel, } from "./fleet-query.js";
import { helpMaxScroll, helpWindow } from "./fleet-help.js";
import { applySectionCollapse, collapsibleSectionAt, splitDetailSections, } from "./fleet-inspector.js";
/** Default bounded poll cadence for the open panel. */
export const FLEET_PANEL_POLL_MS = 5000;
/**
 * Smallest body height that can hold the docked presentation (one tree row, the
 * divider/inspector header row, and one inspector row). Below this the panel
 * renders the full-height inspector instead of a three-sliver split.
 */
export const FLEET_SPLIT_MIN_BODY = 4;
/**
 * Keyboard resize step as a fraction of the terminal height. `+`/`]` grow the
 * panel, `-`/`[` shrink it, in 5% increments (2 rows on a 40-row terminal).
 */
export const FLEET_RESIZE_STEP_PCT = 0.05;
/** How long a transient in-panel notice (e.g. the Ctrl+C explanation) stays. */
export const FLEET_NOTICE_MS = 4000;
/**
 * Keys that resize the PANEL HEIGHT: `+`/`]` grow, `-`/`[` shrink.
 *
 * There is a keyboard path for every mouse gesture (pi's regular mode has no
 * mouse at all), and this is the resize path — the operator-reported gap
 * ("no way to modulate the screen fraction covered") applies to both. The
 * inspector DOCK has its own resize model (`1`/`2`/`3`, `=`, `i`/`v`, the layout
 * row and the divider drag); this one moves the whole panel.
 */
export function resizeDeltaForInput(data) {
    if (data === "+" || data === "]")
        return 1;
    if (data === "-" || data === "[")
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
    /** Raw JSON (`wg show <task> --json`) lines for the JSON inspector view. */
    detailJsonLines = [];
    detailJsonError = null;
    detailJsonLoading = false;
    /** True when the raw JSON is a client-side projection (labelled as such). */
    detailJsonProjection = false;
    detailJsonSeq = 0;
    /** The inspector view: WG's verbatim detail text, or the raw JSON. */
    inspectorView = "detail";
    /** Collapsed detail sections (keyed by section heading row + heading). */
    collapsedSections = new Set();
    /** Timestamp of the last detail open (Enter double-delivery guard). */
    detailOpenedAt = 0;
    /** The rendered frame of the last `render` (hit-testing + regions). */
    frame = [];
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
    /**
     * An active DIVIDER drag (press on the tree↔inspector divider row): the TUI's
     * `ScrollbarDragTarget::HorizontalDivider`. Drag UP grows the inspector.
     */
    divider = null;
    /** Cockpit height as a fraction of the terminal (persisted by the caller). */
    heightPct;
    /** The inspector-dock layout (persisted by the caller). */
    layout;
    /** A pending layout chosen in the Panel/Layout row (`r`/`f`/`0`). */
    pendingLayout = null;
    /** The layout captured before the current edit, for `Esc` restore. */
    restorePoint = null;
    /** True while the Panel/Layout row owns the keyboard. */
    layoutRow = false;
    /** Typeahead search transaction (`/`). */
    search;
    sortModeValue = "chronoDesc";
    visibilityValue = "hidden";
    /** The auto-generated help modal. */
    helpOpenValue = false;
    helpScroll = 0;
    /** A pending destructive-action confirmation (`D`/`x`). */
    confirm = null;
    /** A pending typed-input action (`a`/`e`/`M`/`f`). */
    input = null;
    /** Bounded notify lines from the last action (shown above the footer). */
    actionLines = [];
    /** The last key delivered to `handleInput` (for the close trigger name). */
    lastKey = "";
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
        validateFleetBindings();
        this.snapshot = snapshot;
        this.inactive = options.inactive === true;
        this.heightPct = clampHeightFraction(options.heightPct ?? MAX_FLEET_HEIGHT_PCT);
        this.layout = layoutFromConfig(options.layout ?? DEFAULT_FLEET_LAYOUT);
        this.search = new FleetSearchState(snapshot?.tasks ?? []);
        this.selectedId = options.initialTaskId ?? this.firstVisibleId();
        this.refreshDetailTail();
    }
    // ── state / data ──────────────────────────────────────────────────────────
    /** Apply a fresh snapshot from the poller and re-render. */
    setSnapshot(snapshot) {
        this.snapshot = snapshot;
        this.search.setRows(snapshot?.tasks ?? []);
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
        if (this.mode === "detail" && this.selectedId) {
            this.loadDetail();
            if (this.inspectorView === "json")
                this.loadDetailJson();
        }
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
     * The split-dock geometry: tree rows above, one divider row, inspector rows
     * below. `null` when the panel is too short to dock (it then renders the
     * full-height detail view instead of a one-row-per-pane sliver).
     */
    dockGeometry() {
        if (this.inactive || this.layout.mode !== "split" || this.mode !== "detail")
            return null;
        const body = this.bodyViewport();
        if (body < FLEET_SPLIT_MIN_BODY)
            return null;
        const available = body - 1; // one divider row
        const share = dockPercent(this.layout);
        const inspectorRows = Math.min(Math.max(1, Math.round((available * share) / 100)), Math.max(1, available - 1));
        const treeRows = Math.max(1, available - inspectorRows);
        return { treeRows, inspectorRows };
    }
    /** Rows of the tree pane in the current presentation. */
    treeViewRows() {
        const dock = this.dockGeometry();
        return dock ? dock.treeRows : this.bodyViewport();
    }
    /**
     * Map a panel-local `y` to the frame row it hit. The frame carries the region
     * (header/palette/tree/divider/inspector/footer) and, for tree rows, the index
     * into the WHOLE rendered line list (including blank/arc rows with
     * `taskId === null`), which is what keeps hit-testing correct when the tree is
     * scrolled, filtered or has sections collapsed.
     */
    /**
     * Rebuild the frame if the last render was invalidated, so mouse hit-testing
     * never reads a STALE frame (a mode/selection change between two mouse events
     * must not target the previous frame's regions).
     */
    ensureFrame() {
        if (this.cachedLines !== null)
            return;
        this.render(this.cachedWidth > 0 ? this.cachedWidth : 100);
    }
    frameRow(y) {
        if (typeof y !== "number" || !Number.isFinite(y))
            return null;
        const row = Math.trunc(y);
        if (row < 0 || row >= this.frame.length)
            return null;
        return this.frame[row] ?? null;
    }
    // ── tree / selection helpers ──────────────────────────────────────────────
    /**
     * The active tree rows. Prefers WG's own rendered text (`GetFleet.tree`);
     * only when it is unavailable does it fall back to the legacy TS re-derivation
     * (best-effort — explicitly not the default path).
     *
     * The rows are WG's verbatim text; the plumbing visibility (`./<`) and the
     * live search typeahead only choose WHICH of them are shown (the same overlay
     * rule the subtree collapse uses). A non-chronological sort (`s`) is a FLAT
     * re-ordering of the task rows, so it is announced in the header — a tree
     * cannot be silently re-sorted without lying about its structure.
     */
    treeLines() {
        if (!this.snapshot)
            return [];
        const wg = renderWgTree(this.snapshot.tree, this.collapsed);
        const base = wg.fromWg
            ? wg.lines.map((l) => ({ text: l.text, taskId: l.taskId }))
            : buildFleetTree(this.snapshot, this.collapsed).lines.map((l) => ({
                text: l.text,
                taskId: l.taskId,
            }));
        const statusById = new Map(this.snapshot.tasks.map((t) => [t.id, t.status]));
        const rows = filterRenderedRows(base, (row) => {
            if (row.taskId === null)
                return true;
            const task = this.snapshot?.tasks.find((t) => t.id === row.taskId);
            if (!task)
                return true;
            if (!this.search.matchesRow(task))
                return false;
            if (this.search.active && this.search.query.trim() !== "")
                return true;
            return this.rowVisibleForPlumbing(task.id, statusById);
        });
        return this.flatSortedRows(rows, statusById);
    }
    /** Does a task row survive the plumbing visibility mode (`.`/`<`)? */
    rowVisibleForPlumbing(id, statusById) {
        return taskVisible({ id, title: "", status: statusById.get(id) ?? "" }, this.visibilityValue);
    }
    /**
     * Apply the sort mode. `Chrono ↓` keeps WG's own rendered order (the tree is
     * WG's text, verbatim); the other modes re-order the TASK rows into a flat
     * time/status list, which the header announces.
     */
    flatSortedRows(rows, statusById) {
        if (this.sortModeValue === "chronoDesc" || !this.snapshot)
            return rows;
        const taskRows = rows.filter((row) => row.taskId !== null);
        if (taskRows.length < 2)
            return rows;
        const ordered = sortTaskRows(this.snapshot.tasks, this.sortModeValue)
            .map((task) => taskRows.find((row) => row.taskId === task.id))
            .filter((row) => row !== undefined);
        // Any task row the snapshot did not order (a stale row) keeps its place at
        // the end rather than disappearing.
        for (const row of taskRows) {
            if (!ordered.includes(row))
                ordered.push(row);
        }
        void statusById;
        return ordered;
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
        const view = this.treeViewRows();
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
        this.treeScroll = scrollToKeepVisible(this.treeScroll, this.treeViewRows(), lineIdx, lines.length);
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
        this.treeScroll = maxScroll(this.treeLineCount(), this.treeViewRows());
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
            this.detailScroll = clampScroll(this.detailScroll + delta, total, this.inspectorRows());
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
     * Visible inspector rows in the current presentation. The FULL inspector
     * renders its own `[< Back]` header (no palette row), so it owns
     * `height - 2` rows; the docked inspector is windowed by the dock geometry.
     */
    inspectorRows() {
        const dock = this.dockGeometry();
        return dock ? dock.inspectorRows : Math.max(1, this.viewportHeight() - 2);
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
            this.detailScroll = clampScroll(this.detailScroll + delta, total, this.inspectorRows());
        }
        else {
            this.treeScroll = clampScroll(this.treeScroll + delta, this.treeLineCount(), this.treeViewRows());
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
        // In the docked presentation the inspector STAYS open on the newly selected
        // task (that is the whole point of the dock); alone, a tree click returns to
        // the tree view.
        if (!isDocked(this.layout))
            this.mode = "tree";
        this.detailScroll = 0;
        if (this.mode === "detail")
            this.loadDetail();
        this.collapsedSections.clear();
        this.invalidate();
        this.tui.requestRender();
        return id;
    }
    /** Page the active pane by `direction` (±1), keeping the selection visible. */
    pageBy(direction) {
        const view = this.mode === "detail" ? this.inspectorRows() : this.activeBody();
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
        if (this.inactive || !this.selectedId || !this.snapshot)
            return;
        this.mode = "detail";
        this.detailScroll = 0;
        this.detailOpenedAt = Date.now();
        this.collapsedSections.clear();
        this.refreshDetailTail();
        this.loadDetail();
        if (this.inspectorView === "json")
            this.loadDetailJson();
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
        void this.detailFetcher(taskId, this.detailWidth())
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
    /**
     * Fetch the task's RAW JSON (`wg show <task> --json`) for the inspector's JSON
     * view — the TUI's `R` (Detail) / `J` (Log) raw-JSON mode. When no source is
     * wired the view falls back to a LABELLED client-side projection of the fleet
     * row, so it is never mistaken for WG's output.
     */
    loadDetailJson() {
        const taskId = this.selectedId;
        if (!taskId)
            return;
        const seq = ++this.detailJsonSeq;
        this.detailJsonLoading = true;
        this.detailJsonError = null;
        const projection = () => {
            const task = this.snapshot?.tasks.find((t) => t.id === taskId) ?? null;
            const agent = agentForTask(this.snapshot?.agents ?? [], taskId);
            this.detailJsonProjection = true;
            return JSON.stringify({ task, agent }, null, 2).split("\n");
        };
        if (!this.options.fetchDetailJson) {
            this.detailJsonLines = projection();
            this.detailJsonLoading = false;
            this.invalidate();
            this.tui.requestRender();
            return;
        }
        void this.options
            .fetchDetailJson(taskId)
            .then((text) => {
            if (seq !== this.detailJsonSeq || taskId !== this.selectedId)
                return;
            if (text && text.trim() !== "") {
                this.detailJsonProjection = false;
                this.detailJsonLines = text.replace(/\n$/, "").split("\n");
                this.detailJsonError = null;
            }
            else {
                this.detailJsonLines = projection();
                this.detailJsonError = null;
            }
        })
            .catch((err) => {
            if (seq !== this.detailJsonSeq || taskId !== this.selectedId)
                return;
            this.detailJsonError = err instanceof Error ? err.message : String(err);
            this.detailJsonLines = projection();
        })
            .finally(() => {
            if (seq !== this.detailJsonSeq)
                return;
            this.detailJsonLoading = false;
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
            this.detailScroll = clampScroll(this.detailScroll, this.detailContent().length, this.inspectorRows());
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
    // ── inspector dock layout ─────────────────────────────────────────────────
    /** The current inspector-dock layout. */
    get layoutState() {
        return { ...this.layout };
    }
    /** The inspector dock's share of the panel body (`0..100`). */
    get dockFraction() {
        return dockPercent(this.layout);
    }
    /** True while the tree and the inspector are both visible. */
    get isSplit() {
        return isDocked(this.layout);
    }
    /** The pending layout chosen in the Panel/Layout row, if any. */
    get pendingLayoutState() {
        return this.pendingLayout ? { ...this.pendingLayout } : null;
    }
    /** True while the Panel/Layout row owns the keyboard. */
    get layoutRowOpen() {
        return this.layoutRow;
    }
    /**
     * Apply an inspector-dock layout.
     *
     * `restore` (default true) records the pre-change layout as the `Esc` restore
     * point (the TUI's "restore complete prior layout"), and `persist` (default
     * true) reports it so the caller writes `fleetViewLayout` — the layout is
     * sticky across opens, exactly like the panel height.
     */
    setLayout(next, opts = {}) {
        const normalized = normalizeLayout(next);
        if (opts.restore !== false && this.restorePoint === null) {
            this.restorePoint = { ...this.layout };
        }
        this.layout = normalized;
        const body = this.inspectorRows();
        this.detailScroll = clampScroll(this.detailScroll, this.detailContent().length, body);
        this.treeScroll = clampScroll(this.treeScroll, this.treeLineCount(), this.treeViewRows());
        this.invalidate();
        this.tui.requestRender();
        if (opts.persist !== false)
            this.reportLayout(normalized);
        return { ...this.layout };
    }
    /** Persist a layout through the caller's `fleetViewLayout` writer. */
    reportLayout(layout) {
        try {
            this.options.persistLayout?.({ ...layout });
        }
        catch {
            /* a persistence failure must never break the panel */
        }
    }
    /** `Esc` in the tree: restore the layout captured before the current edit. */
    restorePriorLayout() {
        if (!this.restorePoint)
            return false;
        const prior = this.restorePoint;
        this.restorePoint = null;
        this.pendingLayout = null;
        this.layout = prior;
        this.invalidate();
        this.tui.requestRender();
        this.reportLayout(prior);
        this.showNotice(`layout restored: ${layoutLabel(prior)}`);
        return true;
    }
    /** Open the Panel/Layout row (`p`) — the TUI's layout control page. */
    openLayoutRow() {
        if (this.inactive)
            return;
        this.layoutRow = true;
        this.pendingLayout = null;
        this.restorePoint = { ...this.layout };
        this.invalidate();
        this.tui.requestRender();
    }
    /** Close the Panel/Layout row, discarding any pending choice. */
    closeLayoutRow() {
        this.layoutRow = false;
        this.pendingLayout = null;
        this.invalidate();
        this.tui.requestRender();
    }
    /** `Enter` in the Layout row: apply the pending layout (or keep the current). */
    applyPendingLayout() {
        const pending = this.pendingLayout ?? this.layout;
        this.pendingLayout = null;
        this.layoutRow = false;
        this.layout = normalizeLayout(pending);
        this.invalidate();
        this.tui.requestRender();
        this.reportLayout(this.layout);
        this.showNotice(`layout applied: ${layoutLabel(this.layout)}`);
    }
    // ── inspector views ───────────────────────────────────────────────────────
    /** The active inspector view (`detail` = WG's verbatim text, `json` = raw). */
    get inspectorViewKind() {
        return this.inspectorView;
    }
    /** True when the inspector is showing raw JSON. */
    get rawJsonVisible() {
        return this.inspectorView === "json";
    }
    /** Switch the inspector view; the JSON view fetches on first use. */
    setInspectorView(view) {
        if (this.inspectorView === view)
            return;
        this.inspectorView = view;
        this.detailScroll = 0;
        if (view === "json")
            this.loadDetailJson();
        this.invalidate();
        this.tui.requestRender();
    }
    /** Cycle the inspector views (`Alt-←/→`), the TUI's slide between views. */
    cycleInspectorView(direction) {
        const views = ["detail", "json"];
        const idx = views.indexOf(this.inspectorView);
        const next = views[(idx + direction + views.length) % views.length] ?? "detail";
        this.setInspectorView(next);
        this.showNotice(`inspector view: ${next === "json" ? "raw JSON" : "WG detail"}`);
    }
    /** Collapsed detail-section keys (test/`Space` visible state). */
    get collapsedSectionKeys() {
        return [...this.collapsedSections];
    }
    /**
     * `Space` in the detail view: toggle the collapse of the section under the
     * first visible detail row (the TUI's "Toggle section collapse in Detail").
     */
    toggleSectionCollapse() {
        if (this.inspectorView !== "detail") {
            this.showNotice("section collapse belongs to the WG detail view (Alt-← switches back)");
            return;
        }
        const sections = splitDetailSections(this.detailLines);
        const section = collapsibleSectionAt(sections, this.detailScroll);
        if (!section) {
            this.showNotice("no collapsible section here");
            return;
        }
        const key = section.key;
        if (this.collapsedSections.has(key))
            this.collapsedSections.delete(key);
        else
            this.collapsedSections.add(key);
        this.invalidate();
        this.tui.requestRender();
        this.showNotice(this.collapsedSections.has(key) ? "section collapsed" : "section expanded");
    }
    // ── search / sort / filter ────────────────────────────────────────────────
    /** The typeahead search state (test-visible). */
    get searchState() {
        return this.search;
    }
    /** The active sort mode. */
    get sortMode() {
        return this.sortModeValue;
    }
    /** The active plumbing visibility mode. */
    get visibilityMode() {
        return this.visibilityValue;
    }
    /** Begin a search transaction (`/`) — the tree filters as the operator types. */
    openSearch() {
        if (this.inactive)
            return;
        this.search.open();
        this.invalidate();
        this.tui.requestRender();
    }
    /** Commit the highlighted match (`Enter`): select it and clear all match state. */
    commitSearch() {
        const id = this.search.commit();
        if (id) {
            this.selectedId = id;
            this.mode = "tree";
            const lines = this.treeLines();
            const lineIdx = lines.findIndex((l) => l.taskId === id);
            this.treeScroll = scrollToKeepVisible(this.treeScroll, this.treeViewRows(), lineIdx, lines.length);
        }
        this.invalidate();
        this.tui.requestRender();
    }
    /** Clear the search transaction (`Esc`) — never quits. */
    clearSearch() {
        this.search.clear();
        this.invalidate();
        this.tui.requestRender();
    }
    /** `s` — cycle the sort Chrono↓ → Chrono↑ → Status. */
    cycleSort() {
        this.sortModeValue = nextSortMode(this.sortModeValue);
        this.showNotice(`sort: ${sortLabel(this.sortModeValue)}`);
        const lines = this.treeLines();
        this.treeScroll = clampScroll(this.treeScroll, lines.length, this.treeViewRows());
        this.invalidate();
        this.tui.requestRender();
    }
    /** `. / <` — cycle the plumbing visibility hidden → running → all. */
    cycleVisibility() {
        this.visibilityValue = nextVisibilityMode(this.visibilityValue);
        this.showNotice(`plumbing: ${visibilityLabel(this.visibilityValue)}`);
        const lines = this.treeLines();
        this.treeScroll = clampScroll(this.treeScroll, lines.length, this.treeViewRows());
        if (this.selectedId && !this.visibleOrder().includes(this.selectedId)) {
            this.selectedId = this.firstVisibleId();
        }
        this.invalidate();
        this.tui.requestRender();
    }
    // ── help modal ────────────────────────────────────────────────────────────
    /** True while the help modal is open. */
    get helpVisible() {
        return this.helpOpenValue;
    }
    /** Help scroll offset (test-visible). */
    get helpScrollOffset() {
        return this.helpScroll;
    }
    /** The help modal's body, auto-generated from the binding table. */
    helpBody() {
        return helpLines({ inactive: this.inactive });
    }
    /** Open the help modal (`?`). */
    openHelp() {
        this.helpOpenValue = true;
        this.helpScroll = 0;
        this.invalidate();
        this.tui.requestRender();
    }
    /** Close the help modal (`?` / `Esc` / `q`). */
    closeHelp() {
        this.helpOpenValue = false;
        this.helpScroll = 0;
        this.invalidate();
        this.tui.requestRender();
    }
    helpViewport() {
        return Math.max(1, this.viewportHeight() - 2);
    }
    scrollHelp(delta) {
        const max = helpMaxScroll(this.helpBody().length, this.helpViewport());
        this.helpScroll = Math.max(0, Math.min(max, this.helpScroll + delta));
        this.invalidate();
        this.tui.requestRender();
    }
    // ── task actions ──────────────────────────────────────────────────────────
    /** Bounded notify lines from the last action (test-visible). */
    get actionNotify() {
        return [...this.actionLines];
    }
    /** The pending destructive confirmation, if any. */
    get pendingConfirm() {
        return this.confirm ? { ...this.confirm } : null;
    }
    /** The pending typed input, if any. */
    get pendingInput() {
        return this.input ? { ...this.input } : null;
    }
    /**
     * Begin an action (`a`/`D`/`f`/`x`/`e`/`M`/`r`). The gate decides what
     * happens next: `confirm` sets a y/n prompt, a typed prompt opens the input
     * row, and `none` runs immediately (refresh).
     *
     * A dormant session never reaches here: those bindings are hidden.
     */
    beginAction(kind) {
        // A dormant session HIDES the actions: the binding is not dispatchable and
        // this guard makes the contract hold even if a caller invokes the method
        // directly (never "attempted and failed").
        if (!availableActions({ inactive: this.inactive }).includes(kind))
            return;
        const spec = FLEET_ACTIONS[kind];
        const taskId = this.selectedId;
        if (spec.needsTask && !taskId) {
            this.showNotice(`${spec.key}: no task selected`);
            return;
        }
        if (!this.options.runAction && kind !== "refresh") {
            this.showNotice("wg backend unavailable — actions disabled");
            return;
        }
        this.actionLines = [];
        if (kind === "refresh") {
            this.actionLines = [`r force refresh · …`];
            void Promise.resolve(this.options.onRefresh?.()).then(() => {
                this.actionLines = ["r force refresh · ok"];
                this.invalidate();
                this.tui.requestRender();
            });
            this.invalidate();
            this.tui.requestRender();
            return;
        }
        if (spec.gate === "confirm") {
            this.confirm = { kind, taskId: taskId ?? "" };
            this.invalidate();
            this.tui.requestRender();
            this.showNotice(confirmPrompt(kind, taskId));
            return;
        }
        if (spec.gate !== "none") {
            this.input = { kind, taskId: taskId ?? null, buffer: "" };
            this.invalidate();
            this.tui.requestRender();
            return;
        }
        void this.runAction(kind, taskId ?? null, "");
    }
    /** Resolve the pending confirmation (`y`/Enter = yes, `n`/Esc = no). */
    resolveConfirm(confirmed) {
        const pending = this.confirm;
        this.confirm = null;
        if (!pending)
            return;
        if (!confirmed) {
            this.showNotice("cancelled");
            return;
        }
        void this.runAction(pending.kind, pending.taskId, "");
    }
    /** Type into the pending action input row. */
    typeActionInput(text) {
        if (!this.input)
            return;
        this.input.buffer += text;
        this.invalidate();
        this.tui.requestRender();
    }
    /** Delete the last character of the pending action input. */
    backspaceActionInput() {
        if (!this.input)
            return;
        this.input.buffer = this.input.buffer.slice(0, -1);
        this.invalidate();
        this.tui.requestRender();
    }
    /** Cancel the pending action input (`Esc`). */
    cancelActionInput() {
        this.input = null;
        this.showNotice("cancelled");
        this.invalidate();
        this.tui.requestRender();
    }
    /** Submit the pending action input (`Enter`) — runs the `wg` verb. */
    submitActionInput() {
        const pending = this.input;
        if (!pending)
            return;
        const text = pending.buffer.trim();
        if (text === "") {
            this.showNotice("nothing entered — Esc cancels");
            return;
        }
        this.input = null;
        void this.runAction(pending.kind, pending.taskId, text);
    }
    /**
     * Run a confirmed action through the caller's `runAction` (the shared
     * `WgBackend.run` seam) and surface its output as bounded notify lines.
     */
    async runAction(kind, taskId, text) {
        const run = this.options.runAction;
        if (!run || kind === "refresh")
            return;
        const argv = FLEET_ACTIONS[kind].argv(taskId, text);
        this.actionLines = [`${FLEET_ACTIONS[kind].key} ${FLEET_ACTIONS[kind].desc.toLowerCase()} · …`];
        this.invalidate();
        this.tui.requestRender();
        let result;
        try {
            result = await run(argv);
        }
        catch (err) {
            result = {
                stdout: "",
                stderr: err instanceof Error ? err.message : String(err),
                code: 1,
                killed: false,
            };
        }
        this.actionLines = actionNotifyLines(kind, taskId, result);
        this.showNotice(this.actionLines[0] ?? "");
        // A graph-mutating action changes the fleet: re-read it.
        if (result.code === 0) {
            try {
                await this.options.onRefresh?.();
            }
            catch {
                /* a refresh failure never breaks the action result */
            }
        }
        this.invalidate();
        this.tui.requestRender();
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
    /** The scope the keyboard is currently in (see `fleet-bindings.ts`). */
    inputScope() {
        if (this.helpOpenValue)
            return "help";
        if (this.confirm)
            return "confirm";
        if (this.input)
            return "input";
        if (this.search.active)
            return "search";
        if (this.layoutRow)
            return "layout";
        return this.mode === "detail" ? "detail" : "tree";
    }
    /**
     * Keyboard entry point. Dispatch is DATA-DRIVEN from the one keymap
     * (`fleet-bindings.ts`), so every key the panel understands is also a line in
     * the `?` help modal — the two can never drift.
     *
     * Two scopes own raw text: the search prompt and the action-input row. While
     * they are open, printable characters are text (never bindings), which is why
     * those scopes are matched WITHOUT the `any`-scope fallback.
     */
    handleInput(data) {
        if (this.disposed)
            return;
        this.lastKey = data;
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
        const scope = this.inputScope();
        const textScope = scope === "search" || scope === "input";
        const id = matchFleetBinding(data, scope, {
            inactive: this.inactive,
            anyFallback: !textScope,
        });
        if (id) {
            this.runBinding(id);
            return;
        }
        if (scope === "input" && this.input) {
            if (matchesKey(data, Key.backspace) || matchesKey(data, Key.delete)) {
                this.backspaceActionInput();
                return;
            }
            const text = printableText(data);
            if (text !== null)
                this.typeActionInput(text);
            return;
        }
        if (scope === "search" && this.search.active) {
            if (matchesKey(data, Key.backspace) || matchesKey(data, Key.delete)) {
                this.search.backspace();
                this.invalidate();
                this.tui.requestRender();
                return;
            }
            const text = printableText(data);
            if (text !== null) {
                this.search.type(text);
                this.invalidate();
                this.tui.requestRender();
            }
            return;
        }
        // Modal help swallows every unhandled key (the TUI's `_ => {}`).
    }
    /** Execute one keymap binding. The switch is exhaustive by construction. */
    runBinding(id) {
        switch (id) {
            // ── navigation ──
            case "nav.up":
                if (this.mode === "detail")
                    this.scrollBy(-1);
                else
                    this.moveSelection(-1);
                return;
            case "nav.down":
                if (this.mode === "detail")
                    this.scrollBy(1);
                else
                    this.moveSelection(1);
                return;
            case "nav.pageUp":
                this.pageBy(-1);
                return;
            case "nav.pageDown":
                this.pageBy(1);
                return;
            case "nav.top":
                if (this.mode === "detail")
                    this.scrollBy(-this.detailContent().length);
                else
                    this.selectFirst();
                return;
            case "nav.bottom":
                if (this.mode === "detail")
                    this.scrollBy(this.detailContent().length);
                else
                    this.selectLast();
                return;
            case "nav.scrollUp":
                this.scrollBy(-1);
                return;
            case "nav.scrollDown":
                this.scrollBy(1);
                return;
            // ── layout / dock ──
            case "layout.third":
                this.reportDockChange(applyPreset(this.layout, "third"));
                return;
            case "layout.half":
                this.reportDockChange(applyPreset(this.layout, "half"));
                return;
            case "layout.twoThirds":
                this.reportDockChange(applyPreset(this.layout, "twoThirds"));
                return;
            case "layout.grow": {
                const pct = this.resizeBy(1);
                this.showNotice(`fleet height ${Math.round(pct * 100)}%`);
                return;
            }
            case "layout.shrink": {
                const pct = this.resizeBy(-1);
                this.showNotice(`fleet height ${Math.round(pct * 100)}%`);
                return;
            }
            case "layout.vizGrow":
                this.reportDockChange(growVizPane(this.layout));
                return;
            case "layout.vizShrink":
                this.reportDockChange(shrinkVizPane(this.layout));
                return;
            case "layout.cycle":
                this.reportDockChange(cycleLayout(this.layout));
                return;
            case "layout.toggleDock":
                this.reportDockChange(toggleDock(this.layout));
                return;
            case "layout.dockGrow":
                this.pendingLayout = growDock(this.pendingLayout ?? this.layout);
                this.showNotice(`pending: ${layoutLabel(this.pendingLayout)} · Enter applies · Esc restores`);
                this.invalidate();
                this.tui.requestRender();
                return;
            case "layout.dockShrink":
                this.pendingLayout = shrinkDock(this.pendingLayout ?? this.layout);
                this.showNotice(`pending: ${layoutLabel(this.pendingLayout)} · Enter applies · Esc restores`);
                this.invalidate();
                this.tui.requestRender();
                return;
            case "layout.row":
                if (this.layoutRow)
                    this.closeLayoutRow();
                else
                    this.openLayoutRow();
                return;
            case "layout.pendingSplit":
            case "layout.pendingFull":
            case "layout.pendingHidden":
                this.pendingLayout = pendingForLayoutKey(id, this.layout);
                this.showNotice(`pending: ${layoutLabel(this.pendingLayout)} · Enter applies · Esc restores`);
                this.invalidate();
                this.tui.requestRender();
                return;
            // ── inspector ──
            case "inspector.next":
                this.cycleInspectorView(1);
                return;
            case "inspector.prev":
                this.cycleInspectorView(-1);
                return;
            case "inspector.rawJson":
            case "inspector.jsonMode":
                if (this.mode !== "detail") {
                    this.showNotice("open a task first (Enter) — raw JSON lives in the inspector");
                    return;
                }
                this.setInspectorView(this.inspectorView === "json" ? "detail" : "json");
                this.showNotice(`inspector view: ${this.inspectorView === "json" ? "raw JSON" : "WG detail"}`);
                return;
            case "inspector.collapseSection":
                this.toggleSectionCollapse();
                return;
            case "inspector.scrollUp":
                this.scrollBy(-1);
                return;
            case "inspector.scrollDown":
                this.scrollBy(1);
                return;
            // ── flow ──
            case "flow.enter":
                this.enterFlow();
                return;
            case "flow.back":
                this.backFlow();
                return;
            case "flow.expand":
                this.toggleExpand();
                return;
            case "flow.close":
                this.closeWith(this.lastKey === "q" ? "q" : "esc");
                return;
            // ── actions ──
            case "action.create":
                this.beginAction("create");
                return;
            case "action.done":
                this.beginAction("done");
                return;
            case "action.fail":
                this.beginAction("fail");
                return;
            case "action.retry":
                this.beginAction("retry");
                return;
            case "action.edit":
                this.beginAction("edit");
                return;
            case "action.message":
                this.beginAction("message");
                return;
            case "action.refresh":
                this.beginAction("refresh");
                return;
            case "confirm.yes":
                this.resolveConfirm(true);
                return;
            case "confirm.no":
                this.resolveConfirm(false);
                return;
            // ── search / sort / filter ──
            case "search.open":
                this.openSearch();
                return;
            case "search.next":
                this.search.next();
                this.invalidate();
                this.tui.requestRender();
                return;
            case "search.prev":
                this.search.prev();
                this.invalidate();
                this.tui.requestRender();
                return;
            case "sort.cycle":
                this.cycleSort();
                return;
            case "filter.cycle":
                this.cycleVisibility();
                return;
            // ── help ──
            case "help.open":
                if (this.helpOpenValue)
                    this.closeHelp();
                else
                    this.openHelp();
                return;
            case "help.close":
                this.closeHelp();
                return;
            case "help.up":
                this.scrollHelp(-1);
                return;
            case "help.down":
                this.scrollHelp(1);
                return;
            case "help.pageUp":
                this.scrollHelp(-Math.max(1, this.helpViewport() - 1));
                return;
            case "help.pageDown":
                this.scrollHelp(Math.max(1, this.helpViewport() - 1));
                return;
            case "help.top":
                this.helpScroll = 0;
                this.invalidate();
                this.tui.requestRender();
                return;
            case "help.bottom":
                this.helpScroll = helpMaxScroll(this.helpBody().length, this.helpViewport());
                this.invalidate();
                this.tui.requestRender();
                return;
            default: {
                const never = id;
                throw new Error(`unhandled fleet binding: ${String(never)}`);
            }
        }
    }
    /** Apply a dock change and announce it (one notice, one persisted write). */
    reportDockChange(next) {
        this.setLayout(next);
        this.showNotice(`layout: ${layoutLabel(this.layout)}`);
    }
    /** `Enter`: activate the selected task / apply a pending layout / go back. */
    enterFlow() {
        if (this.helpOpenValue)
            return;
        if (this.confirm) {
            this.resolveConfirm(true);
            return;
        }
        if (this.input) {
            this.submitActionInput();
            return;
        }
        if (this.search.active) {
            this.commitSearch();
            return;
        }
        if (this.layoutRow) {
            this.applyPendingLayout();
            return;
        }
        if (this.pendingLayout) {
            this.applyPendingLayout();
            return;
        }
        if (this.mode === "detail") {
            const guard = this.options.enterGuardMs ?? DETAIL_ENTER_GUARD_MS;
            if (Date.now() - this.detailOpenedAt >= guard)
                this.closeDetail();
            return;
        }
        this.openDetail();
    }
    /** `Esc` / `q` / `←` / Backspace: back out one level, never a surprise. */
    backFlow() {
        if (this.helpOpenValue) {
            this.closeHelp();
            return;
        }
        if (this.confirm) {
            this.resolveConfirm(false);
            return;
        }
        if (this.input) {
            this.cancelActionInput();
            return;
        }
        if (this.search.active) {
            this.clearSearch();
            return;
        }
        if (this.layoutRow) {
            this.closeLayoutRow();
            this.restorePriorLayout();
            return;
        }
        if (this.mode === "detail") {
            this.closeDetail();
            return;
        }
        // From the tree: an explicit `q`/`Esc` first RESTORES the layout the operator
        // changed since the panel opened (the TUI's "restore complete prior
        // layout"); with nothing to restore it closes the panel deliberately.
        if (this.lastKey !== "q" && this.restorePriorLayout())
            return;
        this.closeWith(this.lastKey === "q" ? "q" : "esc");
    }
    /**
     * Mouse support, mirroring the WG TUI's mouse model.
     *
     *   - **wheel** scrolls the active pane (selection follows);
     *   - **press** in the tree selects the hit row (from the rendered frame, so
     *     it is correct under scroll/collapse/filter), focuses the panel, and
     *     begins a pan gesture with `capture: true` so the subsequent drag/release
     *     keep routing here even when the pointer leaves the content area;
     *   - **press on the top edge** (row 0) starts the captured height drag;
     *   - **press on the tree↔inspector divider** starts the captured dock drag
     *     (up grows the inspector) — the TUI's `HorizontalDivider`;
     *   - **drag** pans the active pane by the pointer delta (no selection drag);
     *   - **release** ends the gesture (a resize/dock drag persists once, here);
     *   - **click** selects the hit row and — when it was *already* selected
     *     before the press, or on a double-click — opens its detail, mirroring the
     *     TUI's click-selects / inspector-shows behaviour with an explicit second
     *     click; a click on the divider row (the inspector's `[< Back]` header)
     *     returns to the tree.
     *
     * Returns pi's `PiMouseResult` shape — a truthy object for a consumed event
     * and `undefined` when the event is not ours — NEVER a bare boolean (a
     * truthy non-object crashes pi core in `dispatchMouseEvent`; see `mouse.ts`).
     */
    handleMouse(event) {
        if (this.disposed)
            return undefined;
        this.ensureFrame();
        const type = event.type ?? "";
        if (type === "wheel" || type === "mouse.wheel") {
            const delta = typeof event.wheelDelta === "number" && event.wheelDelta !== 0 ? event.wheelDelta : 0;
            if (delta === 0)
                return { handled: true };
            if (this.helpOpenValue)
                this.scrollHelp(delta);
            else
                this.scrollBy(delta);
            return { handled: true };
        }
        // Dormant read-only view: there is nothing to select, open or resize.
        // Clicks are ignored (unhandled) so the host keeps its own behaviour.
        if (this.inactive)
            return undefined;
        // A live RESIZE/DOCK drag owns the pointer stream until release.
        // `capture: true` on the press keeps drag/release routed here even when the
        // pointer leaves the grip, and the panel's geometry follows the pointer:
        // drag UP → taller panel / bigger inspector, drag DOWN → shorter / smaller.
        if (type === "drag" || type === "mouse.drag" || type === "move" || type === "mouse.move") {
            if (this.divider) {
                const y = typeof event.y === "number" && Number.isFinite(event.y)
                    ? Math.trunc(event.y)
                    : this.divider.startRow;
                const pct = dividerPercentAfterDrag({
                    startPercent: this.divider.startPercent,
                    startRow: this.divider.startRow,
                    currentRow: y,
                    rows: this.divider.rows,
                });
                if (pct !== this.dockFraction) {
                    this.divider.moved = true;
                    this.layout = { mode: "split", sizePercent: pct };
                    this.invalidate();
                    this.tui.requestRender();
                }
                return { handled: true, capture: true };
            }
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
            if (this.divider) {
                const { moved } = this.divider;
                this.divider = null;
                if (moved) {
                    this.reportLayout(this.layout);
                    this.showNotice(`inspector ${Math.round(this.dockFraction)}% (saved)`);
                }
                return { handled: true };
            }
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
        if (type === "click" || type === "mouse.click") {
            const row = this.frameRow(event.y);
            // The inspector's header row IS the `[< Back]` affordance in the docked
            // presentation, and row 0 is it in the full presentation — a click there
            // returns to the tree, mirroring `←`/`h`/`q`/`Esc` exactly.
            if (this.mode === "detail") {
                const isBackRow = (row && row.region === "divider") ||
                    (this.layout.mode !== "split" && typeof event.y === "number" && Math.trunc(event.y) === 0);
                if (isBackRow) {
                    this.pressGesture = false;
                    this.pressWasSelected = false;
                    this.drag = null;
                    this.resize = null;
                    this.divider = null;
                    this.closeDetail();
                    return { handled: true };
                }
            }
        }
        if (type === "press" || type === "mouse.press") {
            const y = typeof event.y === "number" && Number.isFinite(event.y) ? Math.trunc(event.y) : Number.NaN;
            const row = this.frameRow(event.y);
            // The PANEL header (row 0) is the resize grip: the panel's visible top
            // edge. In the full-height detail view row 0 is `[< Back]` instead, so the
            // grip only exists where the header is the panel's own chrome.
            const headerIsGrip = row?.region === "header" && y === 0;
            if (headerIsGrip) {
                this.drag = null;
                this.pressGesture = false;
                this.pressWasSelected = false;
                this.divider = null;
                this.resize = {
                    startY: y,
                    startPct: this.heightPct,
                    rows: this.terminalRows(),
                    moved: false,
                };
                return { handled: true, capture: true, focus: true };
            }
            // The tree↔inspector divider is the DOCK grip (TUI HorizontalDivider).
            if (row?.region === "divider") {
                this.drag = null;
                this.pressGesture = false;
                this.pressWasSelected = false;
                this.resize = null;
                this.divider = {
                    startRow: y,
                    startPercent: this.dockFraction,
                    rows: this.terminalRows(),
                    moved: false,
                };
                return { handled: true, capture: true, focus: true };
            }
            if (!row || (row.region !== "tree" && row.region !== "inspector"))
                return undefined;
            if (row.region === "inspector") {
                this.drag = { prevY: y, moved: false };
                this.pressGesture = false;
                this.pressWasSelected = false;
                return { handled: true, capture: true, focus: true };
            }
            const lineIndex = row.lineIndex ?? null;
            if (lineIndex === null)
                return undefined;
            const wasSelected = this.selectedId;
            this.drag = { prevY: y, moved: false };
            this.pressGesture = true;
            const hitId = this.selectTreeLine(lineIndex);
            this.pressWasSelected = hitId !== null && hitId === wasSelected;
            this.tui.requestRender?.();
            return { handled: true, capture: true, focus: true };
        }
        if (type === "click" || type === "mouse.click") {
            const row = this.frameRow(event.y);
            const fromPress = this.pressGesture;
            const preSelected = fromPress && this.pressWasSelected;
            const double = typeof event.clickCount === "number" && event.clickCount >= 2;
            const wasSelected = this.selectedId;
            this.pressGesture = false;
            this.pressWasSelected = false;
            this.drag = null;
            this.resize = null;
            this.divider = null;
            if (row?.region === "divider") {
                this.closeDetail();
                return { handled: true };
            }
            if (!row || row.region !== "tree")
                return fromPress ? { handled: true } : undefined;
            const lineIndex = row.lineIndex ?? null;
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
        if (this.inspectorView === "json")
            return this.jsonContent();
        const out = [];
        // (1) WG's OWN detail text (`wg show`-equivalent), rendered verbatim —
        //     optionally with collapsed sections (the TUI's Detail-tab `Space`).
        if (this.detailLines.length > 0) {
            const collapsed = applySectionCollapse(this.detailLines, this.collapsedSections);
            for (const text of collapsed.lines)
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
        const agent = this.snapshot ? agentForTask(this.snapshot.agents, this.selectedId) : null;
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
    /** The inspector's RAW JSON view (`wg show <task> --json`). */
    jsonContent() {
        const out = [];
        if (this.detailJsonProjection) {
            out.push({
                text: "raw JSON · client projection (no `wg show --json` source wired)",
                color: "warning",
            });
        }
        if (this.detailJsonError) {
            out.push({ text: `wg-fleet: raw JSON unavailable`, color: "warning" });
            out.push({ text: `  ${this.detailJsonError}`, color: "dim" });
        }
        if (this.detailJsonLines.length > 0) {
            for (const text of this.detailJsonLines)
                out.push({ text, color: "text" });
        }
        else if (this.detailJsonLoading) {
            out.push({ text: "loading raw JSON (wg show --json)…", color: "dim" });
        }
        else if (!this.detailJsonError) {
            out.push({ text: "wg-fleet: no raw JSON for this task", color: "dim" });
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
    /** The header's state suffix: sort/filter/search/layout, always stated. */
    stateSuffix() {
        // The layout is ALWAYS stated (it is a live mode, like the palette), and the
        // sort/filter/search state joins it. Everything leads the header so terminal
        // truncation can never hide the panel's current mode.
        const parts = [];
        if (this.layout.mode !== "full")
            parts.push(layoutLabel(this.layout));
        if (this.sortModeValue !== "chronoDesc")
            parts.push(`sort ${sortLabel(this.sortModeValue)}`);
        if (this.visibilityValue !== "hidden")
            parts.push(visibilityLabel(this.visibilityValue));
        if (this.search.active) {
            const q = [...this.search.query].slice(0, 24).join("");
            parts.push(`/${q} (${this.search.matches.length} match)`);
        }
        return parts.length > 0 ? ` · ${parts.join(" · ")}` : "";
    }
    render(width) {
        const height = this.viewportHeight();
        if (this.cachedLines && this.cachedWidth === width && this.cachedHeight === height) {
            return this.cachedLines;
        }
        const body = Math.max(1, height - 2);
        const snapshot = this.snapshot;
        const palette = resolveStatusPalette(snapshot?.tree?.palette);
        const frame = [];
        const push = (row) => {
            frame.push(row);
        };
        if (this.helpOpenValue) {
            this.renderHelp(frame, height);
        }
        else if (this.inactive) {
            // ── dormant read-only view: counts + hint, NOT the live cockpit ──
            // Checked FIRST: a dormant session usually has NO daemon running, and the
            // whole point of this view is that it still explains itself.
            const counts = snapshot
                ? `${fleetCountsHeader(snapshot.counts).replace("wg-fleet ▸ ", "")} · inactive`
                : "inactive · no live counts (daemon offline)";
            const paletteLabelText = paletteLabel(resolveStatusPalette(snapshot?.tree?.palette));
            const header = `wg-fleet ▸ ⇕ ${Math.round(this.heightPct * 100)}% · inactive · ${paletteLabelText} · ${counts}`;
            push({ text: header, color: "accent", region: "header" });
            const warning = this.paletteWarningRow();
            if (warning)
                push({ text: `  ⚠ ${warning}`, color: "warning", region: "palette" });
            push({
                text: "  WG is dormant in this session — nothing is polling and no agents are tracked.",
                color: "dim",
                region: "palette",
            });
            push({
                text: "  Run /wg on to activate the wg tool family and the live fleet cockpit.",
                color: "warning",
                region: "palette",
            });
            push({ text: "", color: "dim", region: "palette" });
            const tree = this.treeContent();
            // Leave room for the 4 chrome rows above and the footer below so the
            // hint/exit line is never the part that gets clipped.
            const room = this.inactiveBody();
            this.treeScroll = clampScroll(this.treeScroll, tree.lines.length, room);
            tree.taskIds.slice(this.treeScroll, this.treeScroll + room).forEach((taskId, idx) => {
                const line = tree.lines[this.treeScroll + idx];
                if (!line)
                    return;
                push({ ...line, region: "tree", lineIndex: this.treeScroll + idx, taskId });
            });
            push({
                text: "  read-only · ↑/↓ scroll · +/- resize · ? help · q/Esc close · /wg on to activate",
                color: "dim",
                region: "footer",
            });
        }
        else if (!snapshot) {
            push({ text: "wg-fleet · daemon offline", color: "dim", region: "header" });
            push({ text: "  no fleet snapshot available · q close", color: "dim", region: "tree" });
        }
        else if (snapshot.tasks.length === 0) {
            push({ text: fleetCountsHeader(snapshot.counts), color: "accent", region: "header" });
            push({ text: "  no tasks in the graph · q close", color: "dim", region: "tree" });
        }
        else if (this.mode === "detail" && this.dockGeometry() !== null) {
            this.renderSplit(frame);
        }
        else if (this.mode === "detail") {
            const detail = this.detailContent();
            // Position indicator lives in the HEADER (always visible even when a
            // long body is clipped at the bottom of pi's overlay) as well as the
            // footer (key hints). `[< Back]` leads the header so the back affordance
            // is VISIBLE at the top-left — the operator-reported "we can't click at the
            // top to get back to the normal view" is a discovery failure, not only a
            // wiring one.
            const view = this.inspectorView === "json" ? "raw json" : "detail (wg show)";
            const total = detail.length;
            const maxOff = maxScroll(total, body);
            const pct = maxOff > 0 ? Math.round((this.detailScroll / maxOff) * 100) : 100;
            const top = total === 0 ? 0 : this.detailScroll + 1;
            const bottom = Math.min(total, this.detailScroll + body);
            this.detailScroll = clampScroll(this.detailScroll, total, body);
            push({
                text: `[< Back] · wg-fleet · ${this.selectedId ?? "?"} · ${view} · ${pct}% [lines ${top}-${bottom}/${total}]`,
                color: "accent",
                region: "header",
            });
            detail.slice(this.detailScroll, this.detailScroll + body).forEach((line, idx) => {
                push({ ...line, region: "inspector", lineIndex: this.detailScroll + idx });
            });
            push({
                text: this.footerText(this.detailFooter()),
                color: "dim",
                region: "footer",
            });
        }
        else {
            // The palette state is ALWAYS stated (never a silent fallback): a short
            // label right after the resize grip (so truncation can never hide it) plus
            // a dedicated WARNING row when the local mirror is painting. This is the
            // answer to "colors are not those in wg viz/wg tui".
            const counts = fleetCountsHeader(snapshot.counts).replace("wg-fleet ▸ ", "");
            const header = `wg-fleet ▸ ⇕ ${Math.round(this.heightPct * 100)}%${this.stateSuffix()} · ${paletteLabel(palette)} · ${counts}`;
            push({ text: header, color: "accent", region: "header" });
            const warning = this.paletteWarningRow();
            if (warning)
                push({ text: `  ⚠ ${warning}`, color: "warning", region: "palette" });
            const tree = this.treeContent();
            // `bodyViewport()` (not the raw `height - 2`) so the palette-warning row is
            // subtracted: the FOOTER must never be the row that gets clipped.
            const rows = this.bodyViewport();
            this.treeScroll = clampScroll(this.treeScroll, tree.lines.length, rows);
            const windowed = tree.lines.slice(this.treeScroll, this.treeScroll + rows);
            windowed.forEach((line, idx) => {
                const taskId = tree.taskIds[this.treeScroll + idx] ?? null;
                push({ ...line, region: "tree", lineIndex: this.treeScroll + idx, taskId });
            });
            // Footer: bounded so the panel never renders past `height`.
            push({ text: this.footerText(this.treeFooter(windowed.length < tree.lines.length)), color: "dim", region: "footer" });
        }
        // Chrome overlays: the action notify lines and the pending prompt replace
        // the footer row (they are never allowed to push the panel past `height`).
        this.appendPromptRow(frame);
        // Never render past the panel bounds; truncate ANSI-safely per line.
        const bounded = frame
            .slice(0, height)
            .map((line) => truncateToWidth(this.paintLine(line, palette), width));
        this.frame = frame.slice(0, height);
        this.cachedLines = bounded;
        this.cachedWidth = width;
        this.cachedHeight = height;
        return bounded;
    }
    /** The docked presentation: tree above, divider header, inspector below. */
    renderSplit(frame) {
        const snapshot = this.snapshot;
        if (!snapshot)
            return;
        const geometry = this.dockGeometry();
        if (!geometry)
            return;
        const palette = resolveStatusPalette(snapshot.tree?.palette);
        const counts = fleetCountsHeader(snapshot.counts).replace("wg-fleet ▸ ", "");
        frame.push({
            text: `wg-fleet ▸ ⇕ ${Math.round(this.heightPct * 100)}%${this.stateSuffix()} · ${paletteLabel(palette)} · ${counts}`,
            color: "accent",
            region: "header",
        });
        const warning = this.paletteWarningRow();
        if (warning)
            frame.push({ text: `  ⚠ ${warning}`, color: "warning", region: "palette" });
        const tree = this.treeContent();
        this.treeScroll = clampScroll(this.treeScroll, tree.lines.length, geometry.treeRows);
        tree.lines
            .slice(this.treeScroll, this.treeScroll + geometry.treeRows)
            .forEach((line, idx) => {
            const taskId = tree.taskIds[this.treeScroll + idx] ?? null;
            frame.push({ ...line, region: "tree", lineIndex: this.treeScroll + idx, taskId });
        });
        // The divider row doubles as the inspector's header: it is the drag grip
        // AND the visible `[< Back]` affordance.
        const detail = this.detailContent();
        const total = detail.length;
        const rows = geometry.inspectorRows;
        const maxOff = maxScroll(total, rows);
        const pct = maxOff > 0 ? Math.round((this.detailScroll / maxOff) * 100) : 100;
        const top = total === 0 ? 0 : this.detailScroll + 1;
        const bottom = Math.min(total, this.detailScroll + rows);
        this.detailScroll = clampScroll(this.detailScroll, total, rows);
        const view = this.inspectorView === "json" ? "raw json" : "detail (wg show)";
        frame.push({
            text: `  ${"─".repeat(2)} [< Back] · ${this.selectedId ?? "?"} · ${view} · ⇕ ${Math.round(this.dockFraction)}% · ${pct}% [lines ${top}-${bottom}/${total}] ${"─".repeat(2)}`,
            color: "accent",
            region: "divider",
        });
        detail
            .slice(this.detailScroll, this.detailScroll + rows)
            .forEach((line, idx) => {
            frame.push({ ...line, region: "inspector", lineIndex: this.detailScroll + idx });
        });
        frame.push({ text: this.footerText(this.splitFooter()), color: "dim", region: "footer" });
    }
    /** The auto-generated help modal, windowed with a position indicator. */
    renderHelp(frame, height) {
        const lines = this.helpBody();
        const rows = Math.max(1, height - 2);
        const win = helpWindow(lines, this.helpScroll, rows);
        this.helpScroll = win.scroll;
        frame.push({
            text: `? Keybindings  ${win.first}-${win.last}/${win.total} `,
            color: "accent",
            region: "header",
        });
        for (const line of win.lines) {
            // The generated body already carries the TUI's two-space key column; a
            // section heading is the only line that starts at column 0.
            const isHeading = line !== "" && !line.startsWith("  ");
            frame.push({
                text: isHeading ? line : line,
                color: isHeading ? "accent" : "text",
                region: "inspector",
            });
        }
        // Pad so the footer always lands on the last row.
        while (frame.length < height - 1) {
            frame.push({ text: "", color: "dim", region: "inspector" });
        }
        frame.push({
            text: this.footerText("  ? / Esc / q close · ↑/↓ j/k · PgUp/PgDn · Home/End · wheel scrolls Help"),
            color: "dim",
            region: "footer",
        });
    }
    /**
     * Replace the footer row with the ACTIVE prompt (confirm / action input /
     * search) or the action notify lines. Every state change is reflected here so
     * the footer hints are never stale: what the panel is waiting for is what the
     * footer says.
     */
    appendPromptRow(frame) {
        if (this.helpOpenValue)
            return;
        const prompt = this.promptText();
        const notify = this.actionLines.length > 0 && !this.noticeLive() ? this.actionLines : [];
        if (!prompt && notify.length === 0)
            return;
        const footerIdx = frame.findIndex((row) => row.region === "footer");
        if (footerIdx < 0)
            return;
        const tail = notify.slice(-3).map((text) => ({
            text,
            color: "dim",
            region: "inspector",
        }));
        if (prompt)
            tail.push({ text: `  ${prompt}`, color: "warning", region: "footer" });
        // Overwrite the rows ENDING at the footer: the block never grows the frame
        // (the panel must never render past `height`).
        const start = Math.max(1, footerIdx + 1 - tail.length);
        const slice = tail.slice(tail.length - (footerIdx + 1 - start));
        frame.splice(start, footerIdx + 1 - start, ...slice);
    }
    /** True while a transient notice is live (it owns the footer row). */
    noticeLive() {
        return this.visibleNotice !== null;
    }
    /** The footer prompt for the pending interaction, or `null`. */
    promptText() {
        if (this.confirm)
            return confirmPrompt(this.confirm.kind, this.confirm.taskId);
        if (this.input)
            return `${inputPrompt(this.input.kind, this.input.taskId)}${this.input.buffer}▏`;
        if (this.search.active) {
            const matches = this.search.matches.length;
            const hl = this.search.highlighted;
            return `/${this.search.query}▏ · ${matches} match${matches === 1 ? "" : "es"}${hl ? ` · ${hl}` : ""} · ↑/↓ pick · Enter jump · Esc clear`;
        }
        if (this.layoutRow) {
            const pending = this.pendingLayout ? layoutLabel(this.pendingLayout) : layoutLabel(this.layout);
            return `layout row · pending ${pending} · Enter apply · Esc restore · r/f/0 split/full/hidden · +/- dock 5% · now ${layoutLabel(this.layout)}`;
        }
        return null;
    }
    /** Tree-mode footer hints. */
    treeFooter(overflow) {
        // Ordered so the essentials survive a narrow terminal: what the panel is,
        // the resize path, then help/close. `1/2/3` (the dock) is listed in `?`.
        const base = overflow
            ? `  ↓ ${this.treeLineCount() - (this.treeScroll + this.treeViewRows())} more · enter detail · o expand · ⇕ drag top edge or +/- resize · q close`
            : "  ↑/↓ select · enter detail · o expand · ⇕ drag top edge or +/- resize · q close";
        return `${base} · ? help`;
    }
    /** Inspector-mode footer hints. */
    detailFooter() {
        return "  PgUp/PgDn ↑/↓ scroll · ←/q/Esc or click [< Back] to return to the tree · Alt-←/→ view · R JSON · Space collapse · ? help";
    }
    /** Docked-mode footer hints (the inspector's own actions). */
    splitFooter() {
        return "  ↑/↓ select · Alt-←/→ view · R JSON · Space collapse · drag ⇕ divider · ←/Esc tree · ? help · q";
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
    /** True while a tree↔inspector DIVIDER drag is active (until release). */
    get isDockDragging() {
        return this.divider !== null;
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
        return this.frame.filter((row) => row.region === "tree").map((row) => row.taskId ?? null);
    }
    /** The rendered frame's regions (test-visible mouse contract). */
    get frameRegions() {
        return this.frame.map((row) => row.region);
    }
    /** The binding table the help modal is generated from. */
    get bindings() {
        return FLEET_BINDINGS;
    }
}
/**
 * The printable text a key event carries (a single typed character), or `null`
 * for an escape sequence / control key. Used only by the text-owning scopes
 * (search typeahead, action input).
 */
export function printableText(data) {
    if (data.length === 0)
        return null;
    if (data.startsWith("\x1b"))
        return null;
    // eslint-disable-next-line no-control-regex
    if (/[\u0000-\u001f\u007f]/.test(data))
        return null;
    return data;
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
            ...(options.layout !== undefined ? { layout: options.layout } : {}),
            ...(options.persistLayout ? { persistLayout: options.persistLayout } : {}),
            ...(options.runAction ? { runAction: options.runAction } : {}),
            ...(options.onRefresh ? { onRefresh: options.onRefresh } : {}),
            ...(options.fetchDetailJson ? { fetchDetailJson: options.fetchDetailJson } : {}),
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
/**
 * The `/wg-fleet` raw-JSON fetcher: `wg show <task> --json` through the shared
 * backend (the daemon read is text-only, so the CLI JSON form is the raw view).
 * Returns `null` when the verb produced no JSON (the panel then labels its
 * client-side projection instead of pretending it is WG's output).
 */
export function makeFleetDetailJsonFetcher(backend) {
    return async (taskId) => {
        if (typeof backend.run !== "function")
            return null;
        try {
            const res = await backend.run(["show", taskId], { json: true });
            const text = res.stdout ?? "";
            if (res.code !== 0 || text.trim() === "")
                return null;
            // Re-indent so the raw view is readable without changing a byte of the
            // JSON the operator is inspecting.
            try {
                return JSON.stringify(JSON.parse(text), null, 2);
            }
            catch {
                return text;
            }
        }
        catch {
            return null;
        }
    };
}
//# sourceMappingURL=fleet-panel.js.map