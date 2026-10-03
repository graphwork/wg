/**
 * fleet-view.ts — the FleetView-style bottom panel.
 *
 * A persistent, optional, **read-only** multi-line widget anchored BELOW the
 * editor (`ctx.ui.setWidget("wg-fleet", lines, { placement: "belowEditor" })`),
 * styled after pi-subagents' under-editor async widget / FleetView: a compact
 * header (active agents + in-progress/ready/blocked/done/failed task counts)
 * over a few live worker rows, expandable via `/wg-fleet`.
 *
 * This is a distinct, config-gated surface — NOT a revival of the removed
 * passive "ready tasks" widget (`graph-widget.ts` is a deliberate no-op shim).
 * It mounts nothing unless `fleetView: true` (see `fleet-config.ts`), it is
 * TUI-only (`ctx.mode === "tui"`; print/json/rpc degrade silently), and it
 * never mutates graph state — it only sends the read-only `viz_snapshot` /
 * `agents` IPC requests (or the read-only `wg agents` / `wg list` CLI
 * fallback).
 *
 * The `/wg-viz` panel remains the optional *fuller* graph view; this is its
 * bottom-panel companion, not a replacement.
 */
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import { DEFAULT_FLEET_VIEW_CONFIG, readFleetViewConfig, } from "./fleet-config.js";
import { fleetSummaryLine, renderFleetLines, } from "./fleet-readmodel.js";
import { fleetSnapshotWithFallback } from "./fleet-snapshot.js";
/** Widget registration key (also the settings-visible surface id). */
export const FLEET_WIDGET_KEY = "wg-fleet";
/** Default bounded poll cadence for the panel. */
export const FLEET_POLL_MS = 5000;
/** Widget lines are bounded here; pi further fits them to the terminal. */
const FLEET_LINE_MAX_WIDTH = 200;
/**
 * Paint one line through the active theme, tolerating a theme-less context.
 * Colour is re-applied per line (never a stateful style span), and the result
 * is width-bounded with pi-tui's ANSI-aware `visibleWidth`/`truncateToWidth`
 * (never naive string math, which would cut through an escape sequence).
 */
function paint(theme, line) {
    let text = line.text;
    if (visibleWidth(text) > FLEET_LINE_MAX_WIDTH)
        text = truncateToWidth(text, FLEET_LINE_MAX_WIDTH);
    if (!theme)
        return text;
    try {
        const painted = theme.fg(line.color, text);
        return visibleWidth(painted) > FLEET_LINE_MAX_WIDTH ? truncateToWidth(painted, FLEET_LINE_MAX_WIDTH) : painted;
    }
    catch {
        return text;
    }
}
function themeOf(ctx) {
    try {
        return ctx.ui.theme ?? null;
    }
    catch {
        return null;
    }
}
/**
 * Install the panel: the `/wg-fleet` command plus the config-gated, TUI-only
 * below-editor widget. Non-TUI modes degrade silently — no widget, no polling.
 */
export function installFleetView(pi, backend, env, options = {}) {
    const config = { ...DEFAULT_FLEET_VIEW_CONFIG, ...options.config };
    if (!options.config)
        Object.assign(config, readFleetViewConfig());
    config.pollMs = options.pollMs ?? config.pollMs;
    let expanded = config.expanded;
    let widgetTimer = null;
    let inFlight = false;
    let snapshot = null;
    let activeTheme = null;
    /** Sentinel so the very first refresh always writes (even a clear). */
    let lastPayloadValue = undefined;
    const renderPayload = (next) => {
        if (!next)
            return [];
        return renderFleetLines(next, { expanded }).map((line) => paint(activeTheme, line));
    };
    const setWidgetLines = (ctx, lines) => {
        const payload = lines && lines.length > 0 ? lines : null;
        const signature = payload ? payload.join("\n") : "\u0000clear";
        const previous = lastPayloadValue === undefined
            ? "\u0000never"
            : lastPayloadValue
                ? lastPayloadValue.join("\n")
                : "\u0000clear";
        if (signature === previous)
            return;
        lastPayloadValue = payload;
        try {
            // The lines form of setWidget re-renders on every call; pushing a fresh
            // payload on state change IS the render trigger (the component form's
            // requestRender is the equivalent, but RPC ignores component factories).
            ctx.ui.setWidget(FLEET_WIDGET_KEY, payload ?? undefined, { placement: config.placement });
        }
        catch {
            /* UI already gone */
        }
    };
    const refresh = async (ctx) => {
        if (ctx.mode !== "tui" || !config.enabled || inFlight)
            return;
        inFlight = true;
        try {
            activeTheme = themeOf(ctx);
            const next = await fleetSnapshotWithFallback(backend, env, { timeoutMs: 2000 });
            snapshot = next;
            // A bounded fetch that returns no data clears the panel (silent degrade).
            setWidgetLines(ctx, next ? renderPayload(next) : null);
        }
        catch {
            snapshot = null;
            setWidgetLines(ctx, null);
        }
        finally {
            inFlight = false;
        }
    };
    const summaryText = () => {
        if (!config.enabled) {
            return ("wg fleet: disabled. Enable it with " +
                '{"fleetView": true} in ~/.pi/agent/extensions/pi-worksgood/config.json (or WG_PI_FLEET_VIEW=1).');
        }
        if (!snapshot)
            return "wg fleet: daemon offline (no snapshot yet)";
        return fleetSummaryLine(snapshot.agents, snapshot.tasks);
    };
    pi.registerCommand("wg-fleet", {
        description: "Toggle/expand the read-only FleetView bottom panel (live WG agents + task counts)",
        handler: async (args, ctx) => {
            // TUI-only surface: silently no-op in rpc/json/print modes.
            if (ctx.mode !== "tui")
                return;
            if (!config.enabled) {
                try {
                    ctx.ui.notify(summaryText(), "info");
                }
                catch {
                    /* no UI */
                }
                return;
            }
            const action = args.trim().toLowerCase();
            if (action === "expand" || action === "on")
                expanded = true;
            else if (action === "collapse" || action === "compact")
                expanded = false;
            else if (action === "" || action === "toggle")
                expanded = !expanded;
            try {
                await refresh(ctx);
                ctx.ui.notify(`${expanded ? "expanded" : "compact"} · ${summaryText()}`, "info");
            }
            catch {
                // Silent degrade: the panel must never throw into the chat.
            }
        },
    });
    pi.on("session_start", (_event, ctx) => {
        if (ctx.mode !== "tui" || !config.enabled)
            return; // opt-in, TUI only
        if (widgetTimer)
            clearInterval(widgetTimer);
        lastPayloadValue = undefined;
        activeTheme = themeOf(ctx);
        widgetTimer = setInterval(() => void refresh(ctx), config.pollMs);
        void refresh(ctx);
    });
    pi.on("session_shutdown", () => {
        if (widgetTimer) {
            clearInterval(widgetTimer);
            widgetTimer = null;
        }
    });
    return {
        config,
        isEnabled: () => config.enabled,
        isExpanded: () => expanded,
        setExpanded: (value) => {
            expanded = value;
        },
        toggle: () => {
            expanded = !expanded;
            return expanded;
        },
        refresh,
        lastPayload: () => lastPayloadValue,
        renderPayload,
    };
}
//# sourceMappingURL=fleet-view.js.map