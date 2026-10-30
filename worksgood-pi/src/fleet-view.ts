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
import type { ExtensionAPI, ExtensionContext, Theme, ThemeColor } from "@earendil-works/pi-coding-agent";
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { WgBackend, WgEnv } from "./wg-backend.js";
import { ACTIVATION_STATE_DIR, projectRootFromDir } from "./activation.js";
import {
  DEFAULT_FLEET_VIEW_CONFIG,
  readFleetViewConfig,
  writeFleetViewHeightPct,
  type FleetViewConfig,
} from "./fleet-config.js";
import { fleetSummaryLine, type FleetLine, type FleetSnapshot } from "./fleet-readmodel.js";
import { fleetSnapshotWithFallback } from "./fleet-snapshot.js";
import { DEFAULT_TRANSCRIPT_LINES } from "./fleet-panel-model.js";
import { FleetStripComponent, type FleetStripMode } from "./fleet-strip.js";
import { makeFleetDetailFetcher, makeFleetFetcher, openFleetPanel } from "./fleet-panel.js";

/** Widget registration key (also the settings-visible surface id). */
export const FLEET_WIDGET_KEY = "wg-fleet";

/** Default bounded poll cadence for the panel. */
export const FLEET_POLL_MS = 5000;

/** Widget lines are bounded here; pi further fits them to the terminal. */
const FLEET_LINE_MAX_WIDTH = 200;

/**
 * The resolved mount state of the ambient surface.
 *  - `live`:     an active session/project → live status strip;
 *  - `inactive`: WG dormant but the project has `.wg/` → one-line hint strip;
 *  - `off`:      nothing to show → the widget is removed entirely.
 */
export type FleetSurfaceMode = FleetStripMode | "off";

export interface InstallFleetOptions {
  /** Override the resolved config (tests). Merged over the safe defaults. */
  config?: Partial<FleetViewConfig>;
  /** Bounded poll cadence override. */
  pollMs?: number;
  /**
   * Is WG active in this session? The strip follows the SESSION state: a session
   * that is neither active nor in a marked-active project gets only the dormant
   * hint (and a project with no `.wg/` at all gets nothing). Defaults to "active"
   * so a standalone installer still mounts (tests, non-activation hosts).
   */
  isActive?: () => boolean;
  /** Is this project marked active on disk (`<project>/.wg/session-active`)? */
  isProjectActive?: () => boolean;
  /** The project root this session belongs to (used for the `.wg/` presence check). */
  projectRoot?: () => string | undefined;
  /** IO seam (tests): does this project have a `.wg/` directory? */
  hasWgDir?: (root: string | undefined) => boolean;
}

/** The install handle: lets tests/commands drive the panel without a live pi. */
export interface FleetViewController {
  config: FleetViewConfig;
  isEnabled(): boolean;
  /** Unfolded body layout (per-agent rows) — the `fleetViewExpanded` flag. */
  isExpanded(): boolean;
  setExpanded(value: boolean): void;
  /** Flip the body layout; returns the new expanded state. */
  toggle(): boolean;
  /** Is the ambient strip currently FOLDED to one line? */
  isFolded(): boolean;
  /** Fold/unfold the ambient strip (no-op before it is mounted). */
  setFolded(value: boolean): void;
  /** Flip the strip's fold state; returns the new folded state. */
  toggleFold(): boolean;
  /** The resolved surface mode for the current activation state. */
  mode(): FleetSurfaceMode;
  /** The mounted strip component (`null` when nothing is mounted) — test seam. */
  strip(): FleetStripComponent | null;
  /** Re-render immediately from a bounded fetch (or clear when offline). */
  refresh(ctx: ExtensionContext): Promise<void>;
  /** Last payload pushed to the widget (`null` = cleared, `undefined` = never). */
  lastPayload(): string[] | null | undefined;
  /** Pure render of a snapshot to (painted, width-bounded) widget lines. */
  renderPayload(snapshot: FleetSnapshot | null): string[];
  /** Mount the ambient surface appropriate to the current state (live or dormant). */
  mount(ctx: ExtensionContext): Promise<void>;
  /**
   * Re-evaluate the surface after an activation change (`/wg on` → live strip,
   * `/wg off` → dormant hint strip when the project still has `.wg/`). This is
   * the ONE entry point both activation hooks use, so the surface can never
   * disagree with the session state.
   */
  sync(ctx?: ExtensionContext): Promise<void>;
  /** Remove the strip + stop the poller (session shutdown); idempotent. */
  unmount(ctx?: ExtensionContext): void;
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
    return visibleWidth(painted) > FLEET_LINE_MAX_WIDTH
      ? truncateToWidth(painted, FLEET_LINE_MAX_WIDTH)
      : painted;
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

/** Default `.wg/` presence check (the activation IO seam allows a test override). */
function defaultHasWgDir(root: string | undefined): boolean {
  if (!root) return false;
  try {
    return existsSync(join(root, ACTIVATION_STATE_DIR));
  } catch {
    return false;
  }
}

/**
 * Install the surface: the `/wg-fleet` command plus the config-gated, TUI-only
 * below-editor CLICKABLE strip. Non-TUI modes degrade silently — no widget, no
 * polling.
 */
export function installFleetView(
  pi: ExtensionAPI,
  backend: Pick<WgBackend, "run"> & Partial<Pick<WgBackend, "getFleet" | "getTaskDetail">>,
  env: Pick<WgEnv, "daemonSocket" | "dir">,
  options: InstallFleetOptions = {},
): FleetViewController {
  const config: FleetViewConfig = { ...DEFAULT_FLEET_VIEW_CONFIG, ...options.config };
  if (!options.config) Object.assign(config, readFleetViewConfig());
  config.pollMs = options.pollMs ?? config.pollMs;

  const hasWgDir = options.hasWgDir ?? defaultHasWgDir;

  let expanded = config.expanded;
  let folded = config.collapsed;
  let widgetTimer: ReturnType<typeof setInterval> | null = null;
  let inFlight = false;
  let snapshot: FleetSnapshot | null = null;
  let activeTheme: Theme | null = null;
  /** Sentinel so the very first refresh always writes (even a clear). */
  let lastPayloadValue: string[] | null | undefined = undefined;
  /** The live pi TUI handle captured by the widget factory (in-place render). */
  let tuiRef: { requestRender?: (force?: boolean) => void } | null = null;
  /** The mounted strip component; `null` when nothing is mounted. */
  let strip: FleetStripComponent | null = null;
  /** The activation state the current surface was mounted for. */
  let mountedMode: FleetSurfaceMode = "off";
  /** The context the surface is mounted in (click callbacks need it). */
  let mountedCtx: ExtensionContext | null = null;

  const projectRoot = (): string | undefined => {
    const explicit = options.projectRoot?.();
    if (explicit) return explicit;
    const dir = env.dir?.trim();
    return dir ? projectRootFromDir(dir) : undefined;
  };

  /**
   * Resolve the surface for the current activation state. A WG-active session (or
   * a project marked active) gets the live strip; a dormant session in a project
   * that still has `.wg/` gets the one-line hint strip; anything else gets
   * nothing.
   */
  const resolveMode = (): FleetSurfaceMode => {
    if (!config.enabled) return "off";
    const active = options.isActive ? options.isActive() : true;
    if (active) return "live";
    if (options.isProjectActive?.()) return "live";
    if (hasWgDir(projectRoot())) return "inactive";
    return "off";
  };

  const renderPayload = (next: FleetSnapshot | null): string[] => {
    if (!next) return [];
    const probe = new FleetStripComponent({
      collapsed: strip ? strip.isCollapsed : folded,
      mode: "live",
      expanded,
      paint: (line) => paint(activeTheme, line),
    });
    probe.setSnapshot(next);
    return probe.render(FLEET_LINE_MAX_WIDTH);
  };

  const stopPoller = (): void => {
    if (widgetTimer) clearInterval(widgetTimer);
    widgetTimer = null;
  };

  /** Push a fresh snapshot into the mounted component (no key re-registration). */
  const pushSnapshot = (next: FleetSnapshot | null): void => {
    snapshot = next;
    if (!strip) return;
    strip.setSnapshot(next);
    try {
      tuiRef?.requestRender?.();
    } catch {
      /* UI already gone */
    }
    lastPayloadValue = strip.textLines();
  };

  const refresh = async (ctx: ExtensionContext): Promise<void> => {
    if (ctx.mode !== "tui" || !config.enabled) return;
    // Dormant: nothing polls. The hint strip is static by design.
    if (mountedMode === "inactive") {
      snapshot = null;
      if (strip) {
        strip.setSnapshot(null);
        lastPayloadValue = strip.textLines();
        try {
          tuiRef?.requestRender?.();
        } catch {
          /* UI already gone */
        }
      }
      return;
    }
    if (inFlight) return;
    inFlight = true;
    try {
      activeTheme = themeOf(ctx);
      const next = await fleetSnapshotWithFallback(backend, env, { timeoutMs: 2000 });
      pushSnapshot(next);
    } catch {
      pushSnapshot(null);
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

  /** Open the full scrollable cockpit (optionally preselected on a task). */
  const openCockpit = async (
    taskId?: string | null,
    explicitCtx?: ExtensionContext | null,
  ): Promise<void> => {
    const ctx = explicitCtx ?? mountedCtx;
    if (!ctx || ctx.mode !== "tui") return;
    try {
      await openFleetPanel(ctx, makeFleetFetcher(backend), env.dir, {
        pollMs: config.pollMs,
        transcriptLines: DEFAULT_TRANSCRIPT_LINES,
        fetchDetail: makeFleetDetailFetcher(backend),
        // The operator's chosen screen fraction is sticky: read the persisted
        // `fleetViewHeightPct` on open, write it back on drag-resize/`+`/`-`.
        heightPct: config.heightPct,
        persistHeightPct: (pct) => {
          config.heightPct = pct;
          writeFleetViewHeightPct(pct);
        },
        ...(taskId ? { initialTaskId: taskId } : {}),
      });
      await refresh(ctx);
    } catch {
      // Silent degrade: the panel must never throw into the chat.
    }
  };

  /** Open the read-only `inactive` view (dormant discovery; no cockpit). */
  const openInactiveView = async (): Promise<void> => {
    const ctx = mountedCtx;
    if (!ctx || ctx.mode !== "tui") return;
    try {
      await openFleetPanel(ctx, makeFleetFetcher(backend), env.dir, {
        inactive: true,
        heightPct: config.heightPct,
        persistHeightPct: (pct) => {
          config.heightPct = pct;
          writeFleetViewHeightPct(pct);
        },
      });
    } catch {
      // Silent degrade: the panel must never throw into the chat.
    }
  };

  /** Build the clickable strip for `mode` and register it as the widget. */
  const attach = async (ctx: ExtensionContext, mode: FleetStripMode): Promise<void> => {
    if (ctx.mode !== "tui" || !config.enabled) return;
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
      ctx.ui.setWidget(
        FLEET_WIDGET_KEY,
        ((tui: unknown) => {
          tuiRef = tui as { requestRender?: (force?: boolean) => void };
          return component as never;
        }) as never,
        { placement: config.placement },
      );
    } catch {
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
    description:
      "Open the scrollable read-only fleet view (click the ambient strip to fold/unfold; " +
      "/wg-fleet fold|unfold|expand|collapse toggles it)",
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
      // Fold/unfold drives the ambient STRIP; expand/collapse additionally pick
      // the unfolded body layout. Both are mirrored by a click on the strip.
      if (
        action === "fold" ||
        action === "unfold" ||
        action === "expand" ||
        action === "on" ||
        action === "collapse" ||
        action === "compact" ||
        action === "toggle"
      ) {
        if (action === "expand" || action === "on") {
          expanded = true;
          folded = false;
        } else if (action === "unfold") {
          folded = false;
        } else if (action === "fold") {
          folded = true;
        } else if (action === "collapse" || action === "compact") {
          folded = true;
        } else {
          folded = !folded;
        }
        strip?.setCollapsed(folded);
        try {
          await refresh(ctx);
          ctx.ui.notify(`${folded ? "folded" : "expanded"} · ${summaryText()}`, "info");
        } catch {
          // Silent degrade: the panel must never throw into the chat.
        }
        return;
      }
      // Default (and `/wg-fleet open`): the scrollable custom-component view.
      await openCockpit(null, ctx);
    },
  });

  const unmount = (ctx?: ExtensionContext): void => {
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
      } catch {
        /* UI already gone */
      }
    }
    lastPayloadValue = undefined;
    if (ctx) mountedCtx = null;
  };

  const mount = async (ctx: ExtensionContext): Promise<void> => {
    const mode = resolveMode();
    if (mode === "off") {
      unmount(ctx);
      return;
    }
    await attach(ctx, mode);
  };

  const sync = async (ctx?: ExtensionContext): Promise<void> => {
    if (!ctx) return;
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
    setExpanded: (value: boolean) => {
      expanded = value;
    },
    toggle: () => {
      expanded = !expanded;
      return expanded;
    },
    isFolded: () => (strip ? strip.isCollapsed : folded),
    setFolded: (value: boolean) => {
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
