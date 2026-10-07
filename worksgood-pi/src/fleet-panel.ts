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
 *   - **Enter / → / l** drill into the selected task's detail (reusing
 *     `detailLines` from the viz read-model, plus the live agent activity and a
 *     bounded stream tail); **← / h / Backspace / l** return to the tree;
 *   - **o / space** expand/collapse the selected subtree;
 *   - **q / Esc** close.
 *
 * Strictly read-only: the only data source is the daemon's `GetFleet` read
 * (`WgBackend.getFleet`, with its CLI fallback) and a bounded tail read of the
 * agent's own stream file. It never mutates graph state, is TUI-only (guarded
 * on `ctx.mode`), and degrades silently elsewhere.
 */

import type { ExtensionContext, Theme, ThemeColor } from "@earendil-works/pi-coding-agent";
import { Key, matchesKey, truncateToWidth } from "@earendil-works/pi-tui";
import type { GetFleetSnapshot, WgBackend } from "./wg-backend.js";
import { taskColor, type FleetColor } from "./fleet-readmodel.js";
import {
  paintStatusText,
  paletteNotice,
  resolveStatusPalette,
  type StatusPalette,
} from "./status-palette.js";
import {
  DEFAULT_TRANSCRIPT_LINES,
  FLEET_PANEL_DEFAULT_HEIGHT,
  FLEET_PANEL_MIN_HEIGHT,
  agentActivityLabel,
  agentForTask,
  agentUsageDetailLabel,
  agentUsageLabel,
  boundTranscriptBody,
  buildFleetTree,
  clampScroll,
  fleetCountsHeader,
  getFleetToVizSnapshot,
  maxScroll,
  pageScroll,
  readAgentStreamTail,
  renderWgTree,
  scrollToKeepVisible,
} from "./fleet-panel-model.js";
import { detailLines } from "./viz-readmodel.js";
import type { PiMouseResult } from "./mouse.js";

/** Default bounded poll cadence for the open panel. */
export const FLEET_PANEL_POLL_MS = 5000;

/** The slice of pi-tui's `TUI` the panel needs. */
export type FleetTui = {
  requestRender: (force?: boolean) => void;
  terminal?: { rows?: number; columns?: number };
};

export interface FleetPanelOptions {
  /** Viewport height in rows. Defaults to `tui.terminal.rows`, else 24. */
  height?: () => number;
  /** Transcript lines requested for the detail tail. */
  transcriptLines?: number;
}

/** A renderable panel line plus the semantic colour to paint it with. */
interface PaintedLine {
  text: string;
  color: FleetColor;
  /** Task status, when this line is a task row — painted from WG's RGB palette. */
  status?: string;
}

/**
 * The scrollable panel component. Implements pi-tui's `Component` contract
 * (`render` / `invalidate` / `handleInput`) plus a `handleMouse` entry point for
 * hosts that dispatch normalized mouse events (`wheel` scrolls; press/click
 * selects the hit tree line).
 */
export class FleetPanelComponent {
  private snapshot: GetFleetSnapshot | null;
  private selectedId: string | null;
  private collapsed = new Set<string>();
  private mode: "tree" | "detail" = "tree";
  private treeScroll = 0;
  private detailScroll = 0;
  /** Cached, bounded transcript tail lines for the selected task (detail mode). */
  private detailTail: string[] = [];
  private hitMap: Array<string | null> = [];
  /**
   * An active press-and-drag pan gesture. Set on a content-area press and kept
   * until release; while set, every drag/move keeps panning the active pane
   * even when the pointer leaves the content area (capture semantics).
   */
  private drag: { prevY: number; moved: boolean } | null = null;
  /** True while a press gesture is awaiting its synthesized `click`. */
  private pressGesture = false;
  /** Whether the row under the current press was selected *before* the press. */
  private pressWasSelected = false;
  private cachedLines: string[] | null = null;
  private cachedWidth = -1;
  private cachedHeight = -1;
  private disposed = false;

  constructor(
    snapshot: GetFleetSnapshot | null,
    private readonly tui: FleetTui,
    private readonly onClose: () => void,
    private readonly theme: Theme | null = null,
    private readonly options: FleetPanelOptions = {},
    private readonly liveDir: string | undefined = undefined,
  ) {
    this.snapshot = snapshot;
    this.selectedId = this.firstVisibleId();
    this.refreshDetailTail();
  }

  // ── state / data ──────────────────────────────────────────────────────────

  /** Apply a fresh snapshot from the poller and re-render. */
  setSnapshot(snapshot: GetFleetSnapshot | null): void {
    this.snapshot = snapshot;
    if (this.selectedId && !snapshot?.tasks.some((t) => t.id === this.selectedId)) {
      this.selectedId = this.firstVisibleId();
    }
    this.refreshDetailTail();
    this.invalidate();
    this.tui.requestRender();
  }

  dispose(): void {
    this.disposed = true;
  }

  // ── viewport geometry ─────────────────────────────────────────────────────

  private viewportHeight(): number {
    let rows = FLEET_PANEL_DEFAULT_HEIGHT;
    try {
      rows = this.options.height?.() ?? this.tui.terminal?.rows ?? FLEET_PANEL_DEFAULT_HEIGHT;
    } catch {
      rows = FLEET_PANEL_DEFAULT_HEIGHT;
    }
    if (!Number.isFinite(rows)) rows = FLEET_PANEL_DEFAULT_HEIGHT;
    return Math.max(FLEET_PANEL_MIN_HEIGHT, Math.trunc(rows));
  }

  /** Body rows between the fixed header and footer. */
  bodyViewport(): number {
    return Math.max(1, this.viewportHeight() - 2);
  }

  /**
   * Inclusive panel rows that hold scrollable body content: row 0 is the
   * header, the last row is the footer, everything between is content. Presses
   * outside this range are ignored (never start a selection or a drag).
   */
  private contentRows(): { top: number; bottom: number } {
    return { top: 1, bottom: this.viewportHeight() - 2 };
  }

  /**
   * Map a panel-local `y` to the index of the rendered tree line it hit, or
   * `null` when it is outside the content area. The index addresses the *whole*
   * rendered line list (including blank/arc rows with `taskId === null`), which
   * is what keeps hit-testing correct when the tree is scrolled or has sections
   * collapsed — the panel's own window is `[treeScroll, treeScroll + body)`.
   */
  private contentLineIndex(y: number | undefined): number | null {
    if (typeof y !== "number" || !Number.isFinite(y)) return null;
    const { top, bottom } = this.contentRows();
    const row = Math.trunc(y);
    if (row < top || row > bottom) return null;
    return this.treeScroll + (row - top);
  }

  // ── tree / selection helpers ──────────────────────────────────────────────

  /**
   * The active tree rows. Prefers WG's own rendered text (`GetFleet.tree`);
   * only when it is unavailable does it fall back to the legacy TS re-derivation
   * (best-effort — explicitly not the default path).
   */
  private treeLines(): Array<{ text: string; taskId: string | null }> {
    if (!this.snapshot) return [];
    const wg = renderWgTree(this.snapshot.tree, this.collapsed);
    if (wg.fromWg) return wg.lines.map((l) => ({ text: l.text, taskId: l.taskId }));
    return buildFleetTree(this.snapshot, this.collapsed).lines.map((l) => ({
      text: l.text,
      taskId: l.taskId,
    }));
  }

  private visibleOrder(): string[] {
    return this.treeLines()
      .map((l) => l.taskId)
      .filter((id): id is string => id !== null);
  }

  /** Total rendered tree line count (includes blank/arc rows). */
  private treeLineCount(): number {
    return this.treeLines().length;
  }

  /** Index of the selected task within the *rendered* line list, or -1. */
  private selectedLineIndex(): number {
    if (!this.selectedId) return -1;
    return this.treeLines().findIndex((l) => l.taskId !== null && l.taskId === this.selectedId);
  }

  private firstVisibleId(): string | null {
    return this.visibleOrder()[0] ?? null;
  }

  private selectionIndex(): number {
    if (!this.selectedId) return -1;
    return this.visibleOrder().indexOf(this.selectedId);
  }

  /**
   * Keep the selection inside the current viewport after a scroll/page: if the
   * selected line fell outside the window, snap the *selection* to the nearest
   * visible task (the "selection is dragged along" rule). The scroll offset is
   * left untouched here — wheel/PgDn own it, `moveSelection` moves the view.
   */
  private dragSelectionIntoView(): void {
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
    } else if (idx >= this.treeScroll + view) {
      const window = lines.slice(this.treeScroll, this.treeScroll + view);
      const hit = [...window].reverse().find((l) => l.taskId !== null);
      this.selectedId = hit?.taskId ?? this.selectedId;
    }
  }

  // ── navigation ────────────────────────────────────────────────────────────

  /** Move the selection by `delta` visible rows and scroll it into view. */
  moveSelection(delta: number): void {
    const order = this.visibleOrder();
    if (order.length === 0) return;
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

  selectFirst(): void {
    const order = this.visibleOrder();
    this.selectedId = order[0] ?? null;
    this.treeScroll = 0;
    this.mode = "tree";
    this.invalidate();
    this.tui.requestRender();
  }

  selectLast(): void {
    const order = this.visibleOrder();
    const last = order.length - 1;
    this.selectedId = order[last] ?? null;
    this.treeScroll = maxScroll(this.treeLineCount(), this.bodyViewport());
    this.mode = "tree";
    this.invalidate();
    this.tui.requestRender();
  }

  /** Scroll the active pane by `delta` rows, keeping the selection visible. */
  scrollBy(delta: number): void {
    if (!Number.isFinite(delta) || delta === 0) return;
    if (this.mode === "detail") {
      const total = this.detailContent().length;
      this.detailScroll = clampScroll(this.detailScroll + delta, total, this.bodyViewport());
    } else {
      const total = this.treeLineCount();
      this.treeScroll = clampScroll(this.treeScroll + delta, total, this.bodyViewport());
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
  private panBy(delta: number): void {
    if (!Number.isFinite(delta) || delta === 0) return;
    if (this.mode === "detail") {
      const total = this.detailContent().length;
      this.detailScroll = clampScroll(this.detailScroll + delta, total, this.bodyViewport());
    } else {
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
  private selectTreeLine(lineIndex: number): string | null {
    if (lineIndex < 0) return null;
    const id = this.treeLines()[lineIndex]?.taskId ?? null;
    if (!id) return null;
    this.selectedId = id;
    this.mode = "tree";
    this.detailScroll = 0;
    this.invalidate();
    this.tui.requestRender();
    return id;
  }

  /** Page the active pane by `direction` (±1), keeping the selection visible. */
  pageBy(direction: number): void {
    const view = this.bodyViewport();
    if (this.mode === "detail") {
      const total = this.detailContent().length;
      this.detailScroll = pageScroll(this.detailScroll, view, direction, total);
    } else {
      const total = this.treeLineCount();
      this.treeScroll = pageScroll(this.treeScroll, view, direction, total);
      this.dragSelectionIntoView();
    }
    this.invalidate();
    this.tui.requestRender();
  }

  /** Expand/collapse the selected task's subtree. */
  toggleExpand(): void {
    if (!this.selectedId) return;
    if (this.collapsed.has(this.selectedId)) this.collapsed.delete(this.selectedId);
    else this.collapsed.add(this.selectedId);
    this.refreshDetailTail();
    this.invalidate();
    this.tui.requestRender();
  }

  /** Drill into the selected task's detail view. */
  openDetail(): void {
    if (!this.selectedId || !this.snapshot) return;
    this.mode = "detail";
    this.detailScroll = 0;
    this.refreshDetailTail();
    this.invalidate();
    this.tui.requestRender();
  }

  /** Return from the detail view to the tree. */
  closeDetail(): void {
    if (this.mode !== "detail") return;
    this.mode = "tree";
    this.detailScroll = 0;
    this.invalidate();
    this.tui.requestRender();
  }

  close(): void {
    this.onClose();
  }

  // ── transcript tail ───────────────────────────────────────────────────────

  private refreshDetailTail(): void {
    this.detailTail = [];
    if (!this.snapshot || !this.selectedId || !this.liveDir) return;
    const agent = agentForTask(this.snapshot.agents, this.selectedId);
    if (!agent) return;
    const tail = readAgentStreamTail(
      this.liveDir,
      agent.id,
      this.options.transcriptLines ?? DEFAULT_TRANSCRIPT_LINES,
    );
    if (tail && tail.lines.length > 0) this.detailTail = boundTranscriptBody(tail.lines);
  }

  // ── input ─────────────────────────────────────────────────────────────────

  handleInput(data: string): void {
    if (this.disposed) return;
    if (matchesKey(data, Key.escape) || data === "q") {
      this.close();
      return;
    }
    if (this.mode === "detail") {
      // NOTE: Enter deliberately does NOT close the detail view. Some terminals
      // deliver Enter as `\r\n`, which `matchesKey` accepts twice; when Enter
      // both opened and closed the view it appeared to "only open/close tasks".
      // Go back with ←/h/Backspace, close the panel with q/Esc.
      if (
        matchesKey(data, Key.left) ||
        matchesKey(data, Key.backspace) ||
        data === "h" ||
        data === "l"
      ) {
        this.closeDetail();
      } else if (matchesKey(data, Key.up)) this.scrollBy(-1);
      else if (matchesKey(data, Key.down)) this.scrollBy(1);
      else if (matchesKey(data, Key.pageUp)) this.pageBy(-1);
      else if (matchesKey(data, Key.pageDown)) this.pageBy(1);
      else if (matchesKey(data, Key.home)) this.scrollBy(-this.detailContent().length);
      else if (matchesKey(data, Key.end)) this.scrollBy(this.detailContent().length);
      return;
    }
    if (matchesKey(data, Key.up)) this.moveSelection(-1);
    else if (matchesKey(data, Key.down)) this.moveSelection(1);
    else if (matchesKey(data, Key.pageUp)) this.pageBy(-1);
    else if (matchesKey(data, Key.pageDown)) this.pageBy(1);
    else if (matchesKey(data, Key.home) || data === "g") this.selectFirst();
    else if (matchesKey(data, Key.end) || data === "G") this.selectLast();
    else if (matchesKey(data, Key.enter) || data === "l" || matchesKey(data, Key.right)) this.openDetail();
    else if (data === "o" || matchesKey(data, Key.space)) this.toggleExpand();
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
  handleMouse(event: {
    type?: string;
    button?: string;
    y?: number;
    wheelDelta?: number;
    clickCount?: number;
  }): PiMouseResult {
    if (this.disposed) return undefined;
    const type = event.type ?? "";

    if (type === "wheel" || type === "mouse.wheel") {
      const delta = typeof event.wheelDelta === "number" && event.wheelDelta !== 0 ? event.wheelDelta : 0;
      if (delta !== 0) this.scrollBy(delta);
      return { handled: true };
    }

    // A live pan gesture owns the pointer stream until release. pi retargets
    // drag/release to the component that handled the press, so this runs even
    // for coordinates outside the content area (capture semantics).
    if (type === "drag" || type === "mouse.drag" || type === "move" || type === "mouse.move") {
      if (!this.drag) return undefined;
      const y =
        typeof event.y === "number" && Number.isFinite(event.y) ? Math.trunc(event.y) : this.drag.prevY;
      const dy = this.drag.prevY - y;
      if (dy !== 0) {
        this.drag.moved = true;
        this.panBy(dy);
      }
      this.drag.prevY = y;
      return { handled: true, capture: true };
    }

    if (type === "release" || type === "mouse.release") {
      if (!this.drag && !this.pressGesture) return undefined;
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

    if (type === "press" || type === "mouse.press") {
      const lineIndex = this.contentLineIndex(event.y);
      if (lineIndex === null) return undefined; // header/footer/out of content area
      const y = Math.trunc(event.y as number);
      const wasSelected = this.selectedId;
      this.drag = { prevY: y, moved: false };
      this.pressGesture = true;
      const hitId = this.mode === "tree" ? this.selectTreeLine(lineIndex) : null;
      this.pressWasSelected = hitId !== null && hitId === wasSelected;
      (this.tui as { requestRender?: (force?: boolean) => void }).requestRender?.();
      return { handled: true, capture: true, focus: true };
    }

    if (type === "click") {
      const lineIndex = this.contentLineIndex(event.y);
      const fromPress = this.pressGesture;
      const preSelected = fromPress && this.pressWasSelected;
      const double = typeof event.clickCount === "number" && event.clickCount >= 2;
      const wasSelected = this.selectedId;
      this.pressGesture = false;
      this.pressWasSelected = false;
      this.drag = null;
      if (this.mode !== "tree") return fromPress ? { handled: true } : undefined;
      if (lineIndex === null) return fromPress ? { handled: true } : undefined;
      const id = this.selectTreeLine(lineIndex);
      if (!id) return { handled: true };
      // Open detail when the row was already selected before the press (the
      // "click the selected node" affordance), on a double-click, or when a
      // standalone click (no preceding press) hit the current selection.
      if (double || preSelected || (!fromPress && id === wasSelected)) this.openDetail();
      return { handled: true };
    }

    return undefined;
  }

  // ── rendering ─────────────────────────────────────────────────────────────

  invalidate(): void {
    this.cachedLines = null;
  }

  /** The full (unwindowed) detail view content for the selected task. */
  private detailContent(): PaintedLine[] {
    if (!this.snapshot || !this.selectedId) return [];
    const viz = getFleetToVizSnapshot(this.snapshot);
    const task = viz.tasks.find((t) => t.id === this.selectedId);
    if (!task) return [];
    const out: PaintedLine[] = [];
    for (const text of detailLines(task, viz.tasks).lines) out.push({ text, color: "text" });
    const agent = agentForTask(this.snapshot.agents, task.id);
    const activity = agentActivityLabel(agent);
    const usage = agentUsageDetailLabel(agent);
    if (activity || usage) {
      out.push({ text: "", color: "dim" });
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
      for (const line of this.detailTail) out.push({ text: line, color: "dim" });
    }
    return out;
  }

  private treeContent(): { lines: PaintedLine[]; taskIds: Array<string | null> } {
    if (!this.snapshot) return { lines: [], taskIds: [] };
    const statusById = new Map(this.snapshot.tasks.map((t) => [t.id, t.status]));
    const lines: PaintedLine[] = [];
    const taskIds: Array<string | null> = [];
    for (const line of this.treeLines()) {
      const marker = line.taskId !== null && line.taskId === this.selectedId ? "❯ " : "  ";
      let text = `${marker}${line.text}`;
      if (line.taskId) {
        const agent = agentForTask(this.snapshot.agents, line.taskId);
        const activity = agentActivityLabel(agent);
        if (activity) text += ` · ${activity}`;
        // Compact live usage alongside the activity (same format as the
        // ambient agent rows: `12.3k tok · 14 turns · 31 tools`).
        const usage = agentUsageLabel(agent);
        if (usage) text += ` · ${usage}`;
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

  render(width: number): string[] {
    const height = this.viewportHeight();
    if (this.cachedLines && this.cachedWidth === width && this.cachedHeight === height) {
      return this.cachedLines;
    }
    const body = Math.max(1, height - 2);
    const snapshot = this.snapshot;
    const palette = resolveStatusPalette(snapshot?.tree?.palette);
    const lines: PaintedLine[] = [];
    let hitMap: Array<string | null> = [];

    if (!snapshot) {
      lines.push({ text: "wg-fleet · daemon offline", color: "dim" });
      lines.push({ text: "  no fleet snapshot available · q close", color: "dim" });
    } else if (snapshot.tasks.length === 0) {
      lines.push({ text: fleetCountsHeader(snapshot.counts), color: "accent" });
      lines.push({ text: "  no tasks in the graph · q close", color: "dim" });
    } else if (this.mode === "detail") {
      const detail = this.detailContent();
      lines.push({
        text: this.color("accent", `wg-fleet · ${this.selectedId ?? "?"} · detail`),
        color: "accent",
      });
      const windowed = detail.slice(this.detailScroll, this.detailScroll + body);
      lines.push(...windowed);
      if (detail.length > this.detailScroll + body) {
        lines.push({ text: `  … +${detail.length - this.detailScroll - body} more (↓/PgDn)`, color: "dim" });
      } else {
        lines.push({ text: "  ↑/↓ scroll · ← back · q close", color: "dim" });
      }
    } else {
      const notice = paletteNotice(palette, snapshot.tasks.map((t) => t.status));
      const header = fleetCountsHeader(snapshot.counts);
      lines.push({ text: notice ? `${header} · ${notice}` : header, color: "accent" });
      const tree = this.treeContent();
      this.treeScroll = clampScroll(this.treeScroll, tree.lines.length, body);
      const windowed = tree.lines.slice(this.treeScroll, this.treeScroll + body);
      lines.push(...windowed);
      hitMap = tree.taskIds.slice(this.treeScroll, this.treeScroll + body);
      // Footer: bounded so the panel never renders past `height`.
      const overflow = tree.lines.length > this.treeScroll + body;
      const footer = overflow
        ? `  ↓ ${tree.lines.length - (this.treeScroll + body)} more · wheel/drag pan · enter detail · o expand · q close`
        : "  ↑/↓ select · drag pan · enter detail · o expand · q close";
      lines.push({ text: footer, color: "dim" });
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

  private color(style: ThemeColor, text: string): string {
    try {
      return this.theme ? this.theme.fg(style, text) : text;
    } catch {
      return text;
    }
  }

  /**
   * Paint one panel line. A task row is painted from WG's exact RGB palette
   * (`paintStatusText`); chrome/separator lines use the semantic theme role.
   */
  private paintLine(line: PaintedLine, palette: StatusPalette): string {
    if (line.status) {
      return paintStatusText(this.theme, line.text, line.status, palette, line.color);
    }
    return this.color(line.color, line.text);
  }

  // ── test-visible accessors ────────────────────────────────────────────────

  get selected(): string | null {
    return this.selectedId;
  }

  /** Scroll offset of the currently active pane. */
  get scrollOffset(): number {
    return this.mode === "detail" ? this.detailScroll : this.treeScroll;
  }

  /** True while a press-and-drag pan gesture is active (until release). */
  get isDragging(): boolean {
    return this.drag !== null;
  }

  get treeScrollOffset(): number {
    return this.treeScroll;
  }

  get detailVisible(): boolean {
    return this.mode === "detail";
  }

  get isDisposed(): boolean {
    return this.disposed;
  }

  /** Rendered line → task id hit map for the last `render`. */
  get hitLines(): Array<string | null> {
    return this.hitMap;
  }
}

export interface OpenFleetPanelOptions {
  /** Bounded poll cadence. Defaults to 5s. */
  pollMs?: number;
  /** Transcript lines requested for the detail tail. */
  transcriptLines?: number;
}

/**
 * The panel's bounded snapshot fetcher. `columns` (when supplied) is passed to
 * the backend so a rendered tree is truncated to the panel width WG-side; the
 * text is still rendered verbatim.
 */
export type FleetSnapshotFetcher = (columns?: number) => Promise<GetFleetSnapshot | null>;

/**
 * Open the scrollable panel via `ctx.ui.custom()` (TUI mode only). `done()` is
 * supplied to the component so q/Esc resolves the custom prompt; a bounded
 * poller keeps the snapshot live.
 */
export async function openFleetPanel(
  ctx: ExtensionContext,
  fetchSnapshot: FleetSnapshotFetcher,
  liveDir: string | undefined,
  options: OpenFleetPanelOptions = {},
): Promise<void> {
  const initial = await fetchSnapshot().catch(() => null);
  await ctx.ui.custom<void>((tui, theme, _keybindings, done) => {
    const height = (): number => {
      try {
        return tui.terminal?.rows ?? FLEET_PANEL_DEFAULT_HEIGHT;
      } catch {
        return FLEET_PANEL_DEFAULT_HEIGHT;
      }
    };
    const columns = (): number | undefined => {
      try {
        const c = tui.terminal?.columns;
        return typeof c === "number" && c > 0 ? c : undefined;
      } catch {
        return undefined;
      }
    };
    const component = new FleetPanelComponent(
      initial,
      tui,
      () => done(),
      theme,
      { height, transcriptLines: options.transcriptLines },
      liveDir,
    );
    let inFlight = false;
    const timer = setInterval(() => {
      if (inFlight || component.isDisposed) return;
      inFlight = true;
      void fetchSnapshot(columns())
        .then((snapshot) => component.setSnapshot(snapshot))
        .catch(() => undefined)
        .finally(() => {
          inFlight = false;
        });
    }, options.pollMs ?? FLEET_PANEL_POLL_MS);
    const instance = {
      render: (width: number) => component.render(width),
      handleInput: (data: string) => {
        component.handleInput(data);
        tui.requestRender();
      },
      handleMouse: (event: unknown): PiMouseResult =>
        component.handleMouse(
          event as {
            type?: string;
            button?: string;
            y?: number;
            wheelDelta?: number;
            clickCount?: number;
          },
        ),
      invalidate: () => component.invalidate(),
      dispose: () => {
        clearInterval(timer);
        component.dispose();
      },
    };
    return instance as never;
  });
}

/**
 * The `/wg-fleet` data fetcher: prefer the daemon `GetFleet` read (which itself
 * falls back to the read-only CLI), and degrade to `null` when unavailable.
 *
 * `includeTree` (default true) asks the daemon for WG's own rendered `wg viz`
 * tree so the panel has structure parity without spawning `wg` per refresh.
 */
export function makeFleetFetcher(
  backend: Pick<WgBackend, "run"> & Partial<Pick<WgBackend, "getFleet">>,
  options: { includeTree?: boolean } = {},
): FleetSnapshotFetcher {
  const includeTree = options.includeTree ?? true;
  return async (columns?: number) => {
    if (typeof backend.getFleet === "function") {
      try {
        return await backend.getFleet({
          timeoutMs: 2000,
          includeTree,
          treeColumns:
            typeof columns === "number" && Number.isFinite(columns) && columns > 0
              ? Math.trunc(columns)
              : undefined,
        });
      } catch {
        return null;
      }
    }
    return null;
  };
}

export type { WgBackend };
