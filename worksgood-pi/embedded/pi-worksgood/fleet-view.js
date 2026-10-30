/**
 * fleet-view.ts — the FleetView-style bottom panel: a CLICKABLE ambient strip.
 *
 * A persistent, optional, **read-only** status surface anchored BELOW the editor
 * (`ctx.ui.setWidget("wg-fleet", factory, { placement: "belowEditor" })`), styled
 * after pi-subagents' under-editor async widget / FleetView: a compact header
 * (active agents + in-progress/ready/blocked/done/failed task counts) over a few
 * live worker rows, expandable to the full cockpit via `/wg-fleet` or a click.
 *
 * **It is mounted through the COMPONENT form of `setWidget`, not the lines form**,
 * because a clickable surface needs a component that can receive pi mouse
 * dispatch (`handleMouse`). That has a second, load-bearing consequence: pi keeps
 * the mounted component alive, and each poll MUTATES it (`setSnapshot`) instead
 * of re-registering the widget key. So the fold state survives every progress
 * update (see `fleet-strip.ts`), which is exactly what pi-subagents' async widget
 * does and what the operator asked for ("clickable … to open it, to navigate, to
 * investigate").
 *
 * Three states drive what mounts (`sync`):
 *
 *   - **live** — this session is active (`/wg on`) OR the project is marked
 *     active: the one-line live status strip, clicking to fold/unfold and to open
 *     the cockpit.
 *   - **inactive** — WG is dormant here but the project HAS a `.wg/`: a one-line
 *     hint strip (`/wg on to activate`). Clicking opens the fleet panel in a
 *     read-only `inactive` view (counts + hint), so discovery works WITHOUT
 *     booting WG. Nothing polls the daemon in this state.
 *   - **off** — no `.wg/` and no activation: nothing is mounted at all.
 *
 * This is a distinct, config-gated surface — NOT a revival of the removed
 * passive "ready tasks" widget (`graph-widget.ts` is a deliberate no-op shim). It
 * is TUI-only (`ctx.mode === "tui"`; print/json/rpc degrade silently) and it
 * never mutates graph state — it only sends the read-only `viz_snapshot` /
 * `agents` IPC requests (or the read-only `wg agents` / `wg list` CLI fallback).
 *
 * The `/wg-viz` panel remains the optional *fuller* graph view; this is its
 * bottom-panel companion, not a replacement.
 */
import { existsSync } from "node:fs";
import { join } from "node:path";
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import { ACTIVATION_STATE_DIR, projectRootFromDir } from "./activation.js";
import { DEFAULT_FLEET_VIEW_CONFIG, readFleetViewConfig, writeFleetViewHeightPct, writeFleetViewLayout, } from "./fleet-config.js";
import { fleetSummaryLine } from "./fleet-readmodel.js";
import { fleetSnapshotWithFallback } from "./fleet-snapshot.js";
import { DEFAULT_TRANSCRIPT_LINES } from "./fleet-panel-model.js";
import { FleetStripComponent } from "./fleet-strip.js";
import { makeFleetDetailFetcher, makeFleetDetailJsonFetcher, makeFleetFetcher, openFleetPanel, } from "./fleet-panel.js";
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
        return visibleWidth(painted) > FLEET_LINE_MAX_WIDTH
            ? truncateToWidth(painted, FLEET_LINE_MAX_WIDTH)
            : painted;
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
/** Default `.wg/` presence check (the activation IO seam allows a test override). */
function defaultHasWgDir(root) {
    if (!root)
        return false;
    try {
        return existsSync(join(root, ACTIVATION_STATE_DIR));
    }
    catch {
        return false;
    }
}
/**
 * Install the surface: the `/wg-fleet` command plus the config-gated, TUI-only
 * below-editor CLICKABLE strip. Non-TUI modes degrade silently — no widget, no
 * polling.
 */
export function installFleetView(pi, backend, env, options = {}) {
    const config = { ...DEFAULT_FLEET_VIEW_CONFIG, ...options.config };
    if (!options.config)
        Object.assign(config, readFleetViewConfig());
    config.pollMs = options.pollMs ?? config.pollMs;
    const hasWgDir = options.hasWgDir ?? defaultHasWgDir;
    let expanded = config.expanded;
    let folded = config.collapsed;
    let widgetTimer = null;
    let inFlight = false;
    let snapshot = null;
    let activeTheme = null;
    /** Sentinel so the very first refresh always writes (even a clear). */
    let lastPayloadValue = undefined;
    /** The live pi TUI handle captured by the widget factory (in-place render). */
    let tuiRef = null;
    /** The mounted strip component; `null` when nothing is mounted. */
    let strip = null;
    /** The activation state the current surface was mounted for. */
    let mountedMode = "off";
    /** The context the surface is mounted in (click callbacks need it). */
    let mountedCtx = null;
    const projectRoot = () => {
        const explicit = options.projectRoot?.();
        if (explicit)
            return explicit;
        const dir = env.dir?.trim();
        return dir ? projectRootFromDir(dir) : undefined;
    };
    /**
     * Resolve the surface for the current activation state. A WG-active session (or
     * a project marked active) gets the live strip; a dormant session in a project
     * that still has `.wg/` gets the one-line hint strip; anything else gets
     * nothing.
     */
    const resolveMode = () => {
        if (!config.enabled)
            return "off";
        const active = options.isActive ? options.isActive() : true;
        if (active)
            return "live";
        if (options.isProjectActive?.())
            return "live";
        if (hasWgDir(projectRoot()))
            return "inactive";
        return "off";
    };
    const renderPayload = (next) => {
        if (!next)
            return [];
        const probe = new FleetStripComponent({
            collapsed: strip ? strip.isCollapsed : folded,
            mode: "live",
            expanded,
            paint: (line) => paint(activeTheme, line),
        });
        probe.setSnapshot(next);
        return probe.render(FLEET_LINE_MAX_WIDTH);
    };
    const stopPoller = () => {
        if (widgetTimer)
            clearInterval(widgetTimer);
        widgetTimer = null;
    };
    /** Push a fresh snapshot into the mounted component (no key re-registration). */
    const pushSnapshot = (next) => {
        snapshot = next;
        if (!strip)
            return;
        strip.setSnapshot(next);
        try {
            tuiRef?.requestRender?.();
        }
        catch {
            /* UI already gone */
        }
        lastPayloadValue = strip.textLines();
    };
    const refresh = async (ctx) => {
        if (ctx.mode !== "tui" || !config.enabled)
            return;
        // Dormant: nothing polls. The hint strip is static by design.
        if (mountedMode === "inactive") {
            snapshot = null;
            if (strip) {
                strip.setSnapshot(null);
                lastPayloadValue = strip.textLines();
                try {
                    tuiRef?.requestRender?.();
                }
                catch {
                    /* UI already gone */
                }
            }
            return;
        }
        if (inFlight)
            return;
        inFlight = true;
        try {
            activeTheme = themeOf(ctx);
            const next = await fleetSnapshotWithFallback(backend, env, { timeoutMs: 2000 });
            pushSnapshot(next);
        }
        catch {
            pushSnapshot(null);
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
    /** Open the full scrollable cockpit (optionally preselected on a task). */
    const openCockpit = async (taskId, explicitCtx) => {
        const ctx = explicitCtx ?? mountedCtx;
        if (!ctx || ctx.mode !== "tui")
            return;
        try {
            await openFleetPanel(ctx, makeFleetFetcher(backend), env.dir, {
                pollMs: config.pollMs,
                transcriptLines: DEFAULT_TRANSCRIPT_LINES,
                fetchDetail: makeFleetDetailFetcher(backend),
                fetchDetailJson: makeFleetDetailJsonFetcher(backend),
                // The operator's chosen screen fraction is sticky: read the persisted
                // `fleetViewHeightPct` on open, write it back on drag-resize/`+`/`-`.
                heightPct: config.heightPct,
                persistHeightPct: (pct) => {
                    config.heightPct = pct;
                    writeFleetViewHeightPct(pct);
                },
                // The inspector-dock layout is sticky too (`fleetViewLayout`): the dock
                // percentage/presets and the divider drag survive the next open.
                layout: config.layout,
                persistLayout: (layout) => {
                    config.layout = layout;
                    writeFleetViewLayout(layout);
                },
                // In-panel task actions drive the SAME `wg` binary the tools use.
                runAction: (argv) => backend.run([...argv]),
                onRefresh: () => refresh(ctx),
                ...(taskId ? { initialTaskId: taskId } : {}),
            });
            await refresh(ctx);
        }
        catch {
            // Silent degrade: the panel must never throw into the chat.
        }
    };
    /** Open the read-only `inactive` view (dormant discovery; no cockpit). */
    const openInactiveView = async () => {
        const ctx = mountedCtx;
        if (!ctx || ctx.mode !== "tui")
            return;
        try {
            await openFleetPanel(ctx, makeFleetFetcher(backend), env.dir, {
                inactive: true,
                heightPct: config.heightPct,
                persistHeightPct: (pct) => {
                    config.heightPct = pct;
                    writeFleetViewHeightPct(pct);
                },
            });
        }
        catch {
            // Silent degrade: the panel must never throw into the chat.
        }
    };
    /** Build the clickable strip for `mode` and register it as the widget. */
    const attach = async (ctx, mode) => {
        if (ctx.mode !== "tui" || !config.enabled)
            return;
        unmount();
        // A fresh component per mount: the fold state RESETS when the widget is
        // removed/re-mounted (pi-subagents semantics), starting from the configured
        // default — the operator's last fold is never smuggled across a mount.
        folded = config.collapsed;
        mountedCtx = ctx;
        mountedMode = mode;
        activeTheme = themeOf(ctx);
        // A fresh component per mount: the fold state RESETS when the widget is
        // removed/re-mounted (pi-subagents semantics), starting from config.
        const component = new FleetStripComponent({
            collapsed: folded,
            mode,
            expanded,
            paint: (line) => paint(activeTheme, line),
            onToggle: (collapsed) => {
                folded = collapsed;
                lastPayloadValue = component.textLines();
            },
            onOpenPanel: (taskId) => void openCockpit(taskId),
            onOpenInactive: () => void openInactiveView(),
        });
        strip = component;
        tuiRef = null;
        try {
            // COMPONENT form: pi keeps this instance mounted so clicks land on it and
            // so progress updates mutate it in place instead of re-registering the key.
            ctx.ui.setWidget(FLEET_WIDGET_KEY, ((tui) => {
                tuiRef = tui;
                return component;
            }), { placement: config.placement });
        }
        catch {
            /* UI already gone */
        }
        lastPayloadValue = undefined;
        if (mode === "inactive") {
            // Dormant: no poller, no daemon traffic — just the static hint row.
            await refresh(ctx);
            return;
        }
        stopPoller();
        widgetTimer = setInterval(() => void refresh(ctx), config.pollMs);
        await refresh(ctx);
    };
    pi.registerCommand("wg-fleet", {
        description: "Open the scrollable read-only fleet view (click the ambient strip to fold/unfold; " +
            "/wg-fleet fold|unfold|expand|collapse toggles it)",
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
            // Fold/unfold drives the ambient STRIP; expand/collapse additionally pick
            // the unfolded body layout. Both are mirrored by a click on the strip.
            if (action === "fold" ||
                action === "unfold" ||
                action === "expand" ||
                action === "on" ||
                action === "collapse" ||
                action === "compact" ||
                action === "toggle") {
                if (action === "expand" || action === "on") {
                    expanded = true;
                    folded = false;
                }
                else if (action === "unfold") {
                    folded = false;
                }
                else if (action === "fold") {
                    folded = true;
                }
                else if (action === "collapse" || action === "compact") {
                    folded = true;
                }
                else {
                    folded = !folded;
                }
                strip?.setCollapsed(folded);
                try {
                    await refresh(ctx);
                    ctx.ui.notify(`${folded ? "folded" : "expanded"} · ${summaryText()}`, "info");
                }
                catch {
                    // Silent degrade: the panel must never throw into the chat.
                }
                return;
            }
            // Default (and `/wg-fleet open`): the scrollable custom-component view.
            await openCockpit(null, ctx);
        },
    });
    const unmount = (ctx) => {
        stopPoller();
        const component = strip;
        strip = null;
        mountedMode = "off";
        tuiRef = null;
        // Only touch the UI when something is actually mounted, so a session that
        // never mounted the strip leaves no trace at all.
        const hadWidget = component !== null || (Array.isArray(lastPayloadValue) && lastPayloadValue.length > 0);
        if (ctx && hadWidget) {
            try {
                ctx.ui.setWidget(FLEET_WIDGET_KEY, undefined, { placement: config.placement });
            }
            catch {
                /* UI already gone */
            }
        }
        lastPayloadValue = undefined;
        if (ctx)
            mountedCtx = null;
    };
    const mount = async (ctx) => {
        const mode = resolveMode();
        if (mode === "off") {
            unmount(ctx);
            return;
        }
        await attach(ctx, mode);
    };
    const sync = async (ctx) => {
        if (!ctx)
            return;
        const mode = resolveMode();
        if (mode === mountedMode && strip !== null) {
            // Same surface: keep the mounted component (so a fold survives), just
            // refresh its data.
            await refresh(ctx);
            return;
        }
        if (mode === "off") {
            unmount(ctx);
            return;
        }
        await attach(ctx, mode);
    };
    pi.on("session_start", (_event, ctx) => {
        void sync(ctx);
    });
    pi.on("session_shutdown", () => {
        unmount();
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
        isFolded: () => (strip ? strip.isCollapsed : folded),
        setFolded: (value) => {
            folded = value;
            strip?.setCollapsed(value);
        },
        toggleFold: () => {
            folded = strip ? strip.toggle() : !folded;
            return folded;
        },
        mode: () => resolveMode(),
        strip: () => strip,
        refresh,
        lastPayload: () => lastPayloadValue,
        renderPayload,
        mount,
        sync,
        unmount,
    };
}
//# sourceMappingURL=fleet-view.js.map