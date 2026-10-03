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

import type { ExtensionAPI, ExtensionContext, Theme, ThemeColor } from "@earendil-works/pi-coding-agent";
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { WgBackend, WgEnv } from "./wg-backend.js";
import {
  DEFAULT_FLEET_VIEW_CONFIG,
  readFleetViewConfig,
  type FleetViewConfig,
} from "./fleet-config.js";
import {
  fleetSummaryLine,
  renderFleetLines,
  type FleetLine,
  type FleetSnapshot,
} from "./fleet-readmodel.js";
import { fleetSnapshotWithFallback } from "./fleet-snapshot.js";
import { DEFAULT_TRANSCRIPT_LINES } from "./fleet-panel-model.js";
import { makeFleetFetcher, openFleetPanel } from "./fleet-panel.js";

/** Widget registration key (also the settings-visible surface id). */
export const FLEET_WIDGET_KEY = "wg-fleet";

/** Default bounded poll cadence for the panel. */
export const FLEET_POLL_MS = 5000;

/** Widget lines are bounded here; pi further fits them to the terminal. */
const FLEET_LINE_MAX_WIDTH = 200;

export interface InstallFleetOptions {
  /** Override the resolved config (tests). Merged over the safe defaults. */
  config?: Partial<FleetViewConfig>;
  /** Bounded poll cadence override. */
  pollMs?: number;
}

/** The install handle: lets tests/commands drive the panel without a live pi. */
export interface FleetViewController {
  config: FleetViewConfig;
  isEnabled(): boolean;
  isExpanded(): boolean;
  setExpanded(value: boolean): void;
  /** Flip expanded/compact; returns the new expanded state. */
  toggle(): boolean;
  /** Re-render immediately from a bounded fetch (or clear when offline). */
  refresh(ctx: ExtensionContext): Promise<void>;
  /** Last payload pushed to the widget (`null` = cleared, `undefined` = never). */
  lastPayload(): string[] | null | undefined;
  /** Pure render of a snapshot to (painted, width-bounded) widget lines. */
  renderPayload(snapshot: FleetSnapshot | null): string[];
}

/**
 * Paint one line through the active theme, tolerating a theme-less context.
 * Colour is re-applied per line (never a stateful style span), and the result
 * is width-bounded with pi-tui's ANSI-aware `visibleWidth`/`truncateToWidth`
 * (never naive string math, which would cut through an escape sequence).
 */
function paint(theme: Theme | null, line: FleetLine): string {
  let text = line.text;
  if (visibleWidth(text) > FLEET_LINE_MAX_WIDTH) text = truncateToWidth(text, FLEET_LINE_MAX_WIDTH);
  if (!theme) return text;
  try {
    const painted = theme.fg(line.color as ThemeColor, text);
    return visibleWidth(painted) > FLEET_LINE_MAX_WIDTH ? truncateToWidth(painted, FLEET_LINE_MAX_WIDTH) : painted;
  } catch {
    return text;
  }
}

function themeOf(ctx: ExtensionContext): Theme | null {
  try {
    return ((ctx.ui as { theme?: Theme }).theme as Theme | undefined) ?? null;
  } catch {
    return null;
  }
}

/**
 * Install the panel: the `/wg-fleet` command plus the config-gated, TUI-only
 * below-editor widget. Non-TUI modes degrade silently — no widget, no polling.
 */
export function installFleetView(
  pi: ExtensionAPI,
  backend: Pick<WgBackend, "run"> & Partial<Pick<WgBackend, "getFleet">>,
  env: Pick<WgEnv, "daemonSocket" | "dir">,
  options: InstallFleetOptions = {},
): FleetViewController {
  const config: FleetViewConfig = { ...DEFAULT_FLEET_VIEW_CONFIG, ...options.config };
  if (!options.config) Object.assign(config, readFleetViewConfig());
  config.pollMs = options.pollMs ?? config.pollMs;

  let expanded = config.expanded;
  let widgetTimer: ReturnType<typeof setInterval> | null = null;
  let inFlight = false;
  let snapshot: FleetSnapshot | null = null;
  let activeTheme: Theme | null = null;
  /** Sentinel so the very first refresh always writes (even a clear). */
  let lastPayloadValue: string[] | null | undefined = undefined;

  const renderPayload = (next: FleetSnapshot | null): string[] => {
    if (!next) return [];
    return renderFleetLines(next, { expanded }).map((line) => paint(activeTheme, line));
  };

  const setWidgetLines = (ctx: ExtensionContext, lines: string[] | null): void => {
    const payload = lines && lines.length > 0 ? lines : null;
    const signature = payload ? payload.join("\n") : "\u0000clear";
    const previous = lastPayloadValue === undefined
      ? "\u0000never"
      : lastPayloadValue
        ? lastPayloadValue.join("\n")
        : "\u0000clear";
    if (signature === previous) return;
    lastPayloadValue = payload;
    try {
      // The lines form of setWidget re-renders on every call; pushing a fresh
      // payload on state change IS the render trigger (the component form's
      // requestRender is the equivalent, but RPC ignores component factories).
      ctx.ui.setWidget(FLEET_WIDGET_KEY, payload ?? undefined, { placement: config.placement });
    } catch {
      /* UI already gone */
    }
  };

  const refresh = async (ctx: ExtensionContext): Promise<void> => {
    if (ctx.mode !== "tui" || !config.enabled || inFlight) return;
    inFlight = true;
    try {
      activeTheme = themeOf(ctx);
      const next = await fleetSnapshotWithFallback(backend, env, { timeoutMs: 2000 });
      snapshot = next;
      // A bounded fetch that returns no data clears the panel (silent degrade).
      setWidgetLines(ctx, next ? renderPayload(next) : null);
    } catch {
      snapshot = null;
      setWidgetLines(ctx, null);
    } finally {
      inFlight = false;
    }
  };

  const summaryText = (): string => {
    if (!config.enabled) {
      return (
        "wg fleet: disabled. Enable it with " +
        '{"fleetView": true} in ~/.pi/agent/extensions/pi-worksgood/config.json (or WG_PI_FLEET_VIEW=1).'
      );
    }
    if (!snapshot) return "wg fleet: daemon offline (no snapshot yet)";
    return fleetSummaryLine(snapshot.agents, snapshot.tasks);
  };

  pi.registerCommand("wg-fleet", {
    description:
      "Open the scrollable read-only fleet view (also: /wg-fleet expand|collapse toggles the ambient strip)",
    handler: async (args: string, ctx: ExtensionContext) => {
      // TUI-only surface: silently no-op in rpc/json/print modes.
      if (ctx.mode !== "tui") return;
      if (!config.enabled) {
        try {
          ctx.ui.notify(summaryText(), "info");
        } catch {
          /* no UI */
        }
        return;
      }
      const action = args.trim().toLowerCase();
      // Explicit expand/collapse sub-commands keep driving the ambient strip.
      if (action === "expand" || action === "on" || action === "collapse" || action === "compact" || action === "toggle") {
        if (action === "expand" || action === "on") expanded = true;
        else if (action === "collapse" || action === "compact") expanded = false;
        else expanded = !expanded;
        try {
          await refresh(ctx);
          ctx.ui.notify(`${expanded ? "expanded" : "compact"} · ${summaryText()}`, "info");
        } catch {
          // Silent degrade: the panel must never throw into the chat.
        }
        return;
      }
      // Default (and `/wg-fleet open`): the scrollable custom-component view.
      try {
        await openFleetPanel(ctx, makeFleetFetcher(backend), env.dir, {
          pollMs: config.pollMs,
          transcriptLines: DEFAULT_TRANSCRIPT_LINES,
        });
        void refresh(ctx);
      } catch {
        // Silent degrade: the panel must never throw into the chat.
      }
    },
  });

  pi.on("session_start", (_event, ctx) => {
    if (ctx.mode !== "tui" || !config.enabled) return; // opt-in, TUI only
    if (widgetTimer) clearInterval(widgetTimer);
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
    setExpanded: (value: boolean) => {
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
