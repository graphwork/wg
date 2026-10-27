/**
 * fleet-strip.ts — the CLICKABLE ambient fleet strip (below the editor).
 *
 * This is WG's answer to pi-subagents' under-editor async widget: a one-line
 * live status strip whose **header row is a click target**. Left-click folds it
 * to the one-line summary, click again restores the usual layout. That is the
 * exact `asyncWidgetCollapsed` interaction from
 * `pi-subagents/docs/observability.md`:
 *
 *   > In Pi fullscreen mode with mouse dispatch, left-click anywhere on the
 *   > async widget's header row to fold it into a live one-line status summary.
 *   > Click again to restore the usual layout. ... Folding stays in effect
 *   > across progress updates and does not change Pi's global expand setting.
 *
 * The mechanism behind it (and behind this module) is the **component form** of
 * `ctx.ui.setWidget(key, factory, opts)`: pi keeps the mounted component alive
 * and each refresh MUTATES it (`setSnapshot`) instead of re-registering the key,
 * so widget fold state survives every progress update and never touches global
 * pi state. The state resets when the widget is removed (`/wg off`, session
 * shutdown) or pi reloads — a fresh factory starts from the configured default.
 *
 * Interaction model (documented in `README.md` § "What clicks do where"):
 *
 * | where                  | left-click                                        |
 * |------------------------|---------------------------------------------------|
 * | header row (row 0)     | fold ⇄ unfold the strip                           |
 * | an agent row (unfolded)| open the full fleet panel, selected on that task  |
 * | anywhere (dormant)     | open the panel in the read-only `inactive` view   |
 * | blank chrome rows      | ignored (unhandled → the host keeps the event)    |
 * | drag / wheel / modifier| ignored (unhandled)                               |
 *
 * Mouse returns are pi's `PiMouseResult` OBJECT shape (`{handled: true}`), never
 * a bare boolean: a truthy non-object crashes pi core's `dispatchMouseEvent`
 * ("Cannot use 'in' operator to search for 'target' in true"). See `mouse.ts`.
 */
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import { fleetCounts, fleetCountsText, renderFleetLines, } from "./fleet-readmodel.js";
/** Widget lines are bounded here; pi further fits them to the terminal. */
export const STRIP_LINE_MAX_WIDTH = 200;
/** Folded-strip glyph (a closed disclosure triangle) — the click affordance. */
export const STRIP_FOLDED_GLYPH = "▸";
/** Unfolded-strip glyph (an open disclosure triangle). */
export const STRIP_UNFOLDED_GLYPH = "▾";
/** Dormant-strip glyph: WG is present in the project but not booted here. */
export const STRIP_DORMANT_GLYPH = "◇";
/**
 * The click hints. One affordance per state, and kept EARLY in the line so a
 * narrow terminal truncates the counts rather than the thing that tells the
 * operator the surface is clickable at all.
 */
export const STRIP_CLICK_HINT = "click";
export const STRIP_FOLD_HINT = "click to expand";
export const STRIP_UNFOLD_HINT = "click to fold";
export const STRIP_ROW_HINT = "click a row to open";
export const STRIP_DORMANT_HINT = "/wg on to activate";
export const STRIP_DORMANT_CLICK_HINT = "click to inspect";
/**
 * The one-line live summary shown while folded: `▸ wg fleet (click) · 2 active ·
 * 1 in-progress · …`. The `(click)` affordance sits near the front so it
 * survives width truncation.
 */
export function stripSummaryLine(snapshot) {
    const head = `${STRIP_FOLDED_GLYPH} wg fleet (${STRIP_CLICK_HINT})`;
    if (!snapshot)
        return `${head} · awaiting the WG daemon`;
    return `${head} · ${fleetCountsText(fleetCounts(snapshot.agents, snapshot.tasks))}`;
}
/** The one-line discovery hint shown while WG is dormant but present. */
export function dormantStripLine(hint = STRIP_DORMANT_HINT) {
    return `wg fleet · dormant · ${STRIP_DORMANT_CLICK_HINT} · ${hint}`;
}
/**
 * The clickable ambient strip. Implements pi-tui's `Component` contract
 * (`render` / `invalidate`) plus `handleMouse`; pi's widget host keeps this
 * instance mounted and the owner MUTATES it on each refresh, which is what keeps
 * the fold state stable across updates.
 */
export class FleetStripComponent {
    snapshot = null;
    collapsedState;
    modeState;
    hint;
    expanded;
    maxCompactAgents;
    paintLine;
    onToggle;
    onOpenPanel;
    onOpenInactive;
    now;
    cached = null;
    constructor(init = {}) {
        this.collapsedState = init.collapsed ?? true;
        this.modeState = init.mode ?? "live";
        this.hint = init.hint ?? STRIP_DORMANT_HINT;
        this.expanded = init.expanded ?? false;
        this.maxCompactAgents = init.maxCompactAgents;
        this.paintLine = init.paint ?? ((line) => line.text);
        this.onToggle = init.onToggle;
        this.onOpenPanel = init.onOpenPanel;
        this.onOpenInactive = init.onOpenInactive;
        this.now = init.now ?? (() => Date.now());
    }
    // ── state ────────────────────────────────────────────────────────────────
    /**
     * Take a fresh snapshot (a progress update). Folding is deliberately NOT
     * reset here: pi-subagents keeps the fold across updates, and so do we.
     */
    setSnapshot(snapshot) {
        this.snapshot = snapshot;
        this.invalidate();
    }
    setMode(mode) {
        if (this.modeState === mode)
            return;
        this.modeState = mode;
        this.invalidate();
    }
    setCollapsed(collapsed) {
        if (this.collapsedState === collapsed)
            return;
        this.collapsedState = collapsed;
        this.invalidate();
    }
    /** Flip folded ⇄ unfolded. Returns the new folded state. */
    toggle() {
        this.collapsedState = !this.collapsedState;
        this.invalidate();
        this.onToggle?.(this.collapsedState);
        return this.collapsedState;
    }
    get isCollapsed() {
        return this.collapsedState;
    }
    get mode() {
        return this.modeState;
    }
    get snapshotValue() {
        return this.snapshot;
    }
    /** Drop the render cache (pi calls this too). */
    invalidate() {
        this.cached = null;
    }
    // ── render model ─────────────────────────────────────────────────────────
    /** The read-model lines for the current state (before painting/bounding). */
    lines() {
        if (this.modeState === "inactive") {
            return [{ text: dormantStripLine(this.hint), color: "warning" }];
        }
        if (this.collapsedState) {
            return [{ text: stripSummaryLine(this.snapshot), color: "accent" }];
        }
        if (!this.snapshot) {
            return [{ text: `${STRIP_UNFOLDED_GLYPH} ${stripSummaryLine(null)}`, color: "dim" }];
        }
        const body = renderFleetLines(this.snapshot, {
            expanded: this.expanded,
            ...(this.maxCompactAgents === undefined ? {} : { maxCompactAgents: this.maxCompactAgents }),
            now: this.now(),
        });
        const [header, ...rest] = body;
        const head = header ?? { text: stripSummaryLine(this.snapshot), color: "accent" };
        // `wg fleet · <counts>` → the affordances go right after the label so a
        // narrow terminal cannot truncate them away.
        const counts = head.text.replace(/^wg fleet · /, "");
        return [
            {
                text: `${STRIP_UNFOLDED_GLYPH} wg fleet (${STRIP_UNFOLD_HINT} · ${STRIP_ROW_HINT}) · ${counts}`,
                color: head.color,
            },
            ...rest,
        ];
    }
    /** The rendered rows: text plus the click-target classification. */
    rows() {
        return this.entries().map((entry) => ({
            kind: entry.kind,
            taskId: entry.taskId,
            text: entry.text,
        }));
    }
    /** Plain text lines (no ANSI), width-bounded — the test-visible payload. */
    textLines(width = STRIP_LINE_MAX_WIDTH) {
        return this.entries().map((entry) => bound(entry.text, width));
    }
    render(width) {
        const safeWidth = Number.isFinite(width) && width > 0 ? Math.trunc(width) : STRIP_LINE_MAX_WIDTH;
        if (this.cached && this.cached.width === safeWidth)
            return this.cached.lines;
        const entries = this.entries();
        const lines = entries.map((entry) => bound(this.paintLine({ text: entry.text, color: entry.color, taskId: entry.taskId }), safeWidth));
        this.cached = {
            width: safeWidth,
            lines,
            rows: entries.map((entry) => ({ kind: entry.kind, taskId: entry.taskId, text: entry.text })),
        };
        return lines;
    }
    /** Rows + colour in one pass (the single source for render, rows and tests). */
    entries() {
        return this.lines().map((line, index) => ({
            ...line,
            kind: index === 0 ? "header" : line.taskId ? "agent" : "chrome",
            taskId: line.taskId ?? null,
        }));
    }
    // ── mouse ────────────────────────────────────────────────────────────────
    /**
     * pi mouse dispatch. Only a bare LEFT CLICK is ours:
     *
     *   - dormant (`inactive`) strip → open the read-only view;
     *   - header row (y 0) → fold ⇄ unfold;
     *   - an agent row of the unfolded body → open the panel on that task;
     *   - everything else (blank rows, drag, wheel, press, release, modifier
     *     clicks) is left UNHANDLED so the host keeps its own behaviour.
     *
     * Returns pi's object shape or `undefined` — never a bare boolean.
     */
    handleMouse(event) {
        if (!event || event.type !== "click")
            return undefined;
        if (event.button !== undefined && event.button !== "left")
            return undefined;
        if (event.shift || event.alt || event.ctrl)
            return undefined;
        const row = typeof event.y === "number" && Number.isFinite(event.y) ? Math.trunc(event.y) : -1;
        if (row < 0)
            return undefined;
        if (this.modeState === "inactive") {
            // The dormant strip is ONE line; only that line is a click target.
            if (row !== 0)
                return undefined;
            this.onOpenInactive?.();
            return { handled: true };
        }
        if (row === 0) {
            this.toggle();
            return { handled: true };
        }
        if (this.collapsedState)
            return undefined; // folded: nothing below the header
        const hit = this.rows()[row];
        if (!hit || hit.kind !== "agent")
            return undefined;
        this.onOpenPanel?.(hit.taskId);
        return { handled: true };
    }
}
/** ANSI-safe width bound (never naive string math, which cuts an escape). */
function bound(text, width) {
    const max = Math.min(width, STRIP_LINE_MAX_WIDTH);
    return visibleWidth(text) > max ? truncateToWidth(text, max) : text;
}
//# sourceMappingURL=fleet-strip.js.map