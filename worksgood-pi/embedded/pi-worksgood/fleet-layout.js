/**
 * fleet-layout.ts — the `/wg-fleet` **inspector dock layout model**, ported from
 * the WG TUI's `viz_viewer` layout state machine
 * (`src/tui/viz_viewer/state.rs`: `LayoutPreference`, `LayoutMode`,
 * `apply_layout_mode`, `cycle_layout_mode`, `grow_viz_pane`,
 * `shrink_viz_pane`, `layout_mode_for_percent`, and the
 * `ScrollbarDragTarget::HorizontalDivider` drag in `event.rs`).
 *
 * One model, every input: the docked inspector's share of the panel body is a
 * single percentage (`10..90` between the bounded min/max, `0` = hidden,
 * `100` = full panel) that is moved by
 *
 *   - the **presets** `1` / `2` / `3` (1/3, 1/2, 2/3),
 *   - the **layout row** (`p`) `r` / `f` / `0` pending Split / Full / Hidden and
 *     `+` / `-` (5% steps), applied on `Enter`, restored on `Esc`,
 *   - `=` cycling split → panel → graph,
 *   - `\` toggling the dock's visibility,
 *   - `i` / `v` growing / shrinking the viz pane in 10% steps (wraps),
 *   - the **horizontal divider drag** (`dividerPercentAfterDrag`), whose deltas
 *     match the TUI's `horizontal_divider_drag_*` tests bit-for-bit.
 *
 * The chosen layout is persisted in the plugin config
 * (`fleetViewLayout`) and restored on the next open, so the operator's dock
 * choice sticks exactly like the panel height (`fleetViewHeightPct`).
 *
 * Pure: no terminal, no I/O.
 */
/** TUI `LayoutPreference::MIN_PERCENT`. */
export const FLEET_DOCK_MIN_PERCENT = 10;
/** TUI `LayoutPreference::MAX_PERCENT`. */
export const FLEET_DOCK_MAX_PERCENT = 90;
/** `+` / `-` step in the layout row (TUI: 5%). */
export const FLEET_DOCK_STEP_PERCENT = 5;
/** `i` / `v` viz-pane step (task spec: 10% per press, wraps). */
export const FLEET_VIZ_STEP_PERCENT = 10;
/**
 * The default layout: the inspector is FULL — today's full-screen detail view —
 * so a plain `Enter` behaviour never changes underneath an operator (or a
 * pinned smoke). `1`/`2`/`3`, `\`, `=` and the divider drag switch to the
 * docked SPLIT presentation, where the tree and the inspector are visible
 * together (the "click to inspect" gap).
 */
export const DEFAULT_FLEET_LAYOUT = { mode: "full", sizePercent: 100 };
/** Preset percentages — the TUI's `LayoutMode::panel_percent()` values. */
export const FLEET_PRESET_PERCENT = {
    third: 33,
    half: 50,
    twoThirds: 67,
};
/** Clamp a split percentage into the TUI's bounded range. */
export function clampDockPercent(value) {
    const n = typeof value === "number" ? value : Number(value);
    if (!Number.isFinite(n))
        return FLEET_PRESET_PERCENT.half;
    return Math.min(FLEET_DOCK_MAX_PERCENT, Math.max(FLEET_DOCK_MIN_PERCENT, Math.round(n)));
}
/** Normalise any layout input into the closed state space. */
export function normalizeLayout(layout) {
    const mode = layout?.mode;
    if (mode === "hidden")
        return { mode: "hidden", sizePercent: 0 };
    if (mode === "full")
        return { mode: "full", sizePercent: 100 };
    if (mode === "split") {
        return { mode: "split", sizePercent: clampDockPercent(layout?.sizePercent) };
    }
    return { ...DEFAULT_FLEET_LAYOUT };
}
/** The TUI's `layout_mode_for_percent` thresholds, applied to a whole layout. */
export function layoutMode(layout) {
    if (layout.mode === "hidden")
        return "off";
    if (layout.mode === "full" || layout.sizePercent >= 100)
        return "full";
    if (layout.sizePercent >= 59)
        return "twoThirds";
    if (layout.sizePercent >= 42)
        return "half";
    return "third";
}
/** Short, operator-facing layout label (`split 2/3`, `panel (full)`, `graph`). */
export function layoutLabel(layout) {
    switch (layoutMode(layout)) {
        case "off":
            return "graph (dock hidden)";
        case "full":
            return "panel (full)";
        case "third":
            return "split 1/3";
        case "half":
            return "split 1/2";
        case "twoThirds":
            return "split 2/3";
    }
}
/** The dock share as a percentage of the panel body (`0..100`). */
export function dockPercent(layout) {
    if (layout.mode === "hidden")
        return 0;
    if (layout.mode === "full")
        return 100;
    return clampDockPercent(layout.sizePercent);
}
/** True when the tree and the inspector are both visible. */
export function isDocked(layout) {
    return layout.mode === "split";
}
/** Apply a preset (`1`/`2`/`3`): split, docked at 1/3 · 1/2 · 2/3. */
export function applyPreset(layout, preset) {
    void layout;
    return { mode: "split", sizePercent: FLEET_PRESET_PERCENT[preset] };
}
/** `+` in the layout row: grow the dock by 5% (entering split when it was off). */
export function growDock(layout) {
    if (layout.mode === "hidden")
        return { mode: "split", sizePercent: FLEET_DOCK_MIN_PERCENT };
    if (layout.mode === "full")
        return { mode: "split", sizePercent: FLEET_DOCK_MAX_PERCENT };
    return { mode: "split", sizePercent: clampDockPercent(layout.sizePercent + FLEET_DOCK_STEP_PERCENT) };
}
/** `-` in the layout row: shrink the dock by 5% (full → split at the ceiling). */
export function shrinkDock(layout) {
    if (layout.mode === "hidden")
        return { mode: "full", sizePercent: 100 };
    if (layout.mode === "full")
        return { mode: "split", sizePercent: FLEET_DOCK_MAX_PERCENT };
    return { mode: "split", sizePercent: clampDockPercent(layout.sizePercent - FLEET_DOCK_STEP_PERCENT) };
}
/**
 * `i` — grow the VIZ pane by 10%, i.e. shrink the inspector, WRAPPING across the
 * closed state space: full(100) → 90 → 80 … → 10 → hidden(0) → full(100).
 */
export function growVizPane(layout) {
    const current = dockPercent(layout);
    if (layout.mode === "hidden")
        return { mode: "full", sizePercent: 100 };
    if (layout.mode === "full")
        return { mode: "split", sizePercent: 100 - FLEET_VIZ_STEP_PERCENT };
    const next = current - FLEET_VIZ_STEP_PERCENT;
    if (next < FLEET_DOCK_MIN_PERCENT)
        return { mode: "hidden", sizePercent: 0 };
    return { mode: "split", sizePercent: clampDockPercent(next) };
}
/**
 * `v` — shrink the VIZ pane by 10%, i.e. grow the inspector, WRAPPING across
 * the closed state space: hidden(0) → 10 → 20 … → 90 → full(100) → hidden(0).
 */
export function shrinkVizPane(layout) {
    const current = dockPercent(layout);
    if (layout.mode === "hidden") {
        return { mode: "split", sizePercent: FLEET_DOCK_MIN_PERCENT };
    }
    if (layout.mode === "full")
        return { mode: "hidden", sizePercent: 0 };
    const next = current + FLEET_VIZ_STEP_PERCENT;
    if (next > FLEET_DOCK_MAX_PERCENT)
        return { mode: "full", sizePercent: 100 };
    return { mode: "split", sizePercent: clampDockPercent(next) };
}
/**
 * `=` — cycle split → panel → graph, exactly the TUI's
 * `LayoutMode::cycle` (1/3 → 1/2 → 2/3 → full → off → 1/3).
 */
export function cycleLayout(layout) {
    switch (layoutMode(layout)) {
        case "third":
            return { mode: "split", sizePercent: FLEET_PRESET_PERCENT.half };
        case "half":
            return { mode: "split", sizePercent: FLEET_PRESET_PERCENT.twoThirds };
        case "twoThirds":
            return { mode: "full", sizePercent: 100 };
        case "full":
            return { mode: "hidden", sizePercent: 0 };
        case "off":
            return { mode: "split", sizePercent: FLEET_PRESET_PERCENT.third };
    }
}
/** `\` — toggle the right panel (the inspector dock) visible. */
export function toggleDock(layout) {
    if (layout.mode === "hidden") {
        const restored = clampDockPercent(layout.sizePercent || FLEET_PRESET_PERCENT.half);
        return { mode: "split", sizePercent: restored };
    }
    return { mode: "hidden", sizePercent: 0 };
}
/** `r` / `f` / `0` — the PENDING mode in the layout row (not yet applied). */
export function pendingForLayoutKey(id, layout) {
    switch (id) {
        case "layout.pendingSplit":
            return {
                mode: "split",
                sizePercent: layout.mode === "split" ? layout.sizePercent : FLEET_PRESET_PERCENT.half,
            };
        case "layout.pendingFull":
            return { mode: "full", sizePercent: 100 };
        case "layout.pendingHidden":
            return { mode: "hidden", sizePercent: 0 };
    }
}
/**
 * The horizontal-divider drag math, ported VERBATIM from the TUI
 * (`horizontal_divider_drag_up_grows_inspector` /
 * `horizontal_divider_drag_down_shrinks_inspector`, `event.rs`):
 *
 * ```text
 * delta      = currentRow - startRow        // +down, -up
 * deltaPct   = delta * 100 / rows
 * percent    = startPercent - deltaPct      // drag UP grows the inspector
 * ```
 *
 * The TUI test pins 35% → drag up 4 rows on a 40-row terminal → 45%; and
 * 50% → drag down 4 rows → 40%. Those exact numbers are asserted in
 * `test/fleet-tui-parity.test.ts` so the panel and the TUI cannot drift.
 */
export function dividerPercentAfterDrag(args) {
    const rows = Math.max(1, Math.trunc(args.rows));
    const delta = Math.trunc(args.currentRow) - Math.trunc(args.startRow);
    const deltaPct = Math.round((delta * 100) / rows);
    return clampDockPercent(Math.trunc(args.startPercent) - deltaPct);
}
/** Serialise a layout for the plugin config (`fleetViewLayout`). */
export function layoutToConfig(layout) {
    const normalized = normalizeLayout(layout);
    return { mode: normalized.mode, sizePercent: normalized.sizePercent };
}
/** Parse a persisted `fleetViewLayout` value (garbage degrades to the default). */
export function layoutFromConfig(raw) {
    if (!raw || typeof raw !== "object" || Array.isArray(raw))
        return { ...DEFAULT_FLEET_LAYOUT };
    const obj = raw;
    const mode = obj.mode;
    if (mode !== "split" && mode !== "full" && mode !== "hidden")
        return { ...DEFAULT_FLEET_LAYOUT };
    return normalizeLayout({ mode, sizePercent: Number(obj.sizePercent) });
}
//# sourceMappingURL=fleet-layout.js.map