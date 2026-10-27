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
import {
  fleetCounts,
  fleetCountsText,
  renderFleetLines,
  type FleetLine,
  type FleetSnapshot,
} from "./fleet-readmodel.js";
import type { PiMouseResult } from "./mouse.js";

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
 * Which surface the strip is: `live` (a WG-active session/project — real
 * counts, real agents) or `inactive` (WG is dormant here but the project has a
 * `.wg/`, so the strip advertises discovery without booting anything).
 */
export type FleetStripMode = "live" | "inactive";

/** What a rendered strip row is, for click hit-testing. */
export type StripRowKind = "header" | "agent" | "chrome";

export interface StripRow {
  kind: StripRowKind;
  /** The WG task for an `agent` row; `null` for the header/chrome rows. */
  taskId: string | null;
  text: string;
}

/**
 * The one-line live summary shown while folded: `▸ wg fleet (click) · 2 active ·
 * 1 in-progress · …`. The `(click)` affordance sits near the front so it
 * survives width truncation.
 */
export function stripSummaryLine(snapshot: FleetSnapshot | null): string {
  const head = `${STRIP_FOLDED_GLYPH} wg fleet (${STRIP_CLICK_HINT})`;
  if (!snapshot) return `${head} · awaiting the WG daemon`;
  return `${head} · ${fleetCountsText(fleetCounts(snapshot.agents, snapshot.tasks))}`;
}

/** The one-line discovery hint shown while WG is dormant but present. */
export function dormantStripLine(hint: string = STRIP_DORMANT_HINT): string {
  return `wg fleet · dormant · ${STRIP_DORMANT_CLICK_HINT} · ${hint}`;
}

export interface FleetStripInit {
  /** Start folded (config `fleetViewCollapsed`, default true). */
  collapsed?: boolean;
  /** `live` (default) or `inactive`. */
  mode?: FleetStripMode;
  /** Dormant hint text (default `/wg on to activate`). */
  hint?: string;
  /** Unfolded body layout: every live agent, or a few. */
  expanded?: boolean;
  /** Max agent rows in the unfolded compact body. */
  maxCompactAgents?: number;
  /** Paints one read-model line through pi's theme (ANSI-aware). */
  paint?: (line: FleetLine) => string;
  /** Called after a header-row click flips the fold state. */
  onToggle?: (collapsed: boolean) => void;
  /** Called when an agent row is clicked (open the panel on that task). */
  onOpenPanel?: (taskId: string | null) => void;
  /** Called when the dormant strip is clicked (open the read-only view). */
  onOpenInactive?: () => void;
  /** Clock seam (tests). */
  now?: () => number;
}

/**
 * The clickable ambient strip. Implements pi-tui's `Component` contract
 * (`render` / `invalidate`) plus `handleMouse`; pi's widget host keeps this
 * instance mounted and the owner MUTATES it on each refresh, which is what keeps
 * the fold state stable across updates.
 */
export class FleetStripComponent {
  private snapshot: FleetSnapshot | null = null;
  private collapsedState: boolean;
  private modeState: FleetStripMode;
  private readonly hint: string;
  private readonly expanded: boolean;
  private readonly maxCompactAgents: number | undefined;
  private readonly paintLine: (line: FleetLine) => string;
  private readonly onToggle: ((collapsed: boolean) => void) | undefined;
  private readonly onOpenPanel: ((taskId: string | null) => void) | undefined;
  private readonly onOpenInactive: (() => void) | undefined;
  private readonly now: () => number;
  private cached: { width: number; lines: string[]; rows: StripRow[] } | null = null;

  constructor(init: FleetStripInit = {}) {
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
  setSnapshot(snapshot: FleetSnapshot | null): void {
    this.snapshot = snapshot;
    this.invalidate();
  }

  setMode(mode: FleetStripMode): void {
    if (this.modeState === mode) return;
    this.modeState = mode;
    this.invalidate();
  }

  setCollapsed(collapsed: boolean): void {
    if (this.collapsedState === collapsed) return;
    this.collapsedState = collapsed;
    this.invalidate();
  }

  /** Flip folded ⇄ unfolded. Returns the new folded state. */
  toggle(): boolean {
    this.collapsedState = !this.collapsedState;
    this.invalidate();
    this.onToggle?.(this.collapsedState);
    return this.collapsedState;
  }

  get isCollapsed(): boolean {
    return this.collapsedState;
  }

  get mode(): FleetStripMode {
    return this.modeState;
  }

  get snapshotValue(): FleetSnapshot | null {
    return this.snapshot;
  }

  /** Drop the render cache (pi calls this too). */
  invalidate(): void {
    this.cached = null;
  }

  // ── render model ─────────────────────────────────────────────────────────

  /** The read-model lines for the current state (before painting/bounding). */
  lines(): FleetLine[] {
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
    const head = header ?? { text: stripSummaryLine(this.snapshot), color: "accent" as const };
    // `wg fleet · <counts>` → the affordances go right after the label so a
    // narrow terminal cannot truncate them away.
    const counts = head.text.replace(/^wg fleet · /, "");
    return [
      {
        text:
          `${STRIP_UNFOLDED_GLYPH} wg fleet (${STRIP_UNFOLD_HINT} · ${STRIP_ROW_HINT}) · ${counts}`,
        color: head.color,
      },
      ...rest,
    ];
  }

  /** The rendered rows: text plus the click-target classification. */
  rows(): StripRow[] {
    return this.entries().map((entry) => ({
      kind: entry.kind,
      taskId: entry.taskId,
      text: entry.text,
    }));
  }

  /** Plain text lines (no ANSI), width-bounded — the test-visible payload. */
  textLines(width: number = STRIP_LINE_MAX_WIDTH): string[] {
    return this.entries().map((entry) => bound(entry.text, width));
  }

  render(width: number): string[] {
    const safeWidth = Number.isFinite(width) && width > 0 ? Math.trunc(width) : STRIP_LINE_MAX_WIDTH;
    if (this.cached && this.cached.width === safeWidth) return this.cached.lines;
    const entries = this.entries();
    const lines = entries.map((entry) =>
      bound(this.paintLine({ text: entry.text, color: entry.color, taskId: entry.taskId }), safeWidth),
    );
    this.cached = {
      width: safeWidth,
      lines,
      rows: entries.map((entry) => ({ kind: entry.kind, taskId: entry.taskId, text: entry.text })),
    };
    return lines;
  }

  /** Rows + colour in one pass (the single source for render, rows and tests). */
  private entries(): Array<FleetLine & { kind: StripRowKind; taskId: string | null }> {
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
  handleMouse(event: {
    type?: string;
    button?: string;
    y?: number;
    shift?: boolean;
    alt?: boolean;
    ctrl?: boolean;
  }): PiMouseResult {
    if (!event || event.type !== "click") return undefined;
    if (event.button !== undefined && event.button !== "left") return undefined;
    if (event.shift || event.alt || event.ctrl) return undefined;
    const row = typeof event.y === "number" && Number.isFinite(event.y) ? Math.trunc(event.y) : -1;
    if (row < 0) return undefined;

    if (this.modeState === "inactive") {
      // The dormant strip is ONE line; only that line is a click target.
      if (row !== 0) return undefined;
      this.onOpenInactive?.();
      return { handled: true };
    }

    if (row === 0) {
      this.toggle();
      return { handled: true };
    }
    if (this.collapsedState) return undefined; // folded: nothing below the header
    const hit = this.rows()[row];
    if (!hit || hit.kind !== "agent") return undefined;
    this.onOpenPanel?.(hit.taskId);
    return { handled: true };
  }
}

/** ANSI-safe width bound (never naive string math, which cuts an escape). */
function bound(text: string, width: number): string {
  const max = Math.min(width, STRIP_LINE_MAX_WIDTH);
  return visibleWidth(text) > max ? truncateToWidth(text, max) : text;
}
