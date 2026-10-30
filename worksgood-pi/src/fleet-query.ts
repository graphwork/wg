/**
 * fleet-query.ts — the `/wg-fleet` **search / sort / filter** state, ported from
 * the WG TUI's graph transaction (`viz_viewer`):
 *
 *   - `/` — "Start exact-task search in command mode" (typeahead; `Enter/tap`
 *     "Commit stable task ID; clear all match state"; `Esc` "Clear totally
 *     without quitting"; `↑/k/BackTab` "Previous result", `↓/j/Tab` "Next
 *     result");
 *   - `s` — "Cycle sort: Chrono↓/↑/Status" (`SortMode` in
 *     `src/tui/viz_viewer/state.rs`, labels copied verbatim);
 *   - `. / <` — "Cycle plumbing: hidden → running only → all"
 *     (`AutomationVisibilityMode`, the same three-state cycle the TUI's
 *     keyboard AND mouse share).
 *
 * The rendered WG tree stays WG's OWN text (verbatim rows, never re-derived);
 * filtering and search only choose WHICH of those rows are shown, exactly like
 * the existing subtree-collapse overlay.
 *
 * Pure: no I/O, no terminal.
 */

import type { GetFleetTaskRow } from "./wg-backend.js";

// ── sort ────────────────────────────────────────────────────────────────────

/** TUI `SortMode`, in the TUI's own cycle order. */
export type FleetSortMode = "chronoDesc" | "chronoAsc" | "status";

/** The TUI's sort cycle: Chrono ↓ → Chrono ↑ → Status → Chrono ↓. */
export const FLEET_SORT_ORDER: readonly FleetSortMode[] = ["chronoDesc", "chronoAsc", "status"];

/** Next sort mode in the TUI cycle. */
export function nextSortMode(mode: FleetSortMode): FleetSortMode {
  const idx = FLEET_SORT_ORDER.indexOf(mode);
  const next = FLEET_SORT_ORDER[(idx + 1) % FLEET_SORT_ORDER.length];
  return next ?? "chronoDesc";
}

/** TUI `SortMode::label()` — the exact strings `wg tui` renders. */
export function sortLabel(mode: FleetSortMode): string {
  switch (mode) {
    case "chronoAsc":
      return "Chrono ↑";
    case "status":
      return "Status";
    case "chronoDesc":
      return "Chrono ↓";
  }
}

/** Status rank for the `Status` sort (live first → finished last). */
const STATUS_RANK: Record<string, number> = {
  "in-progress": 0,
  running: 0,
  working: 0,
  ready: 1,
  open: 1,
  pending: 1,
  blocked: 2,
  waiting: 2,
  failed: 3,
  done: 4,
  completed: 4,
  archived: 5,
};

/**
 * Timestamp of a task row: `started_at`, else `last_interaction_at`, else
 * `completed_at`. Unknown timestamps degrade to `0` (never a fabricated now).
 */
function rowTime(row: GetFleetTaskRow): number {
  const raw = row.started_at ?? row.last_interaction_at ?? row.completed_at ?? null;
  if (!raw) return 0;
  const ms = Date.parse(raw);
  return Number.isFinite(ms) ? ms : 0;
}

/**
 * Order a flat task list for the given sort mode. `chronoDesc` is the newest
 * first (the TUI's default, and the order WG's own tree renders), `chronoAsc`
 * the reverse, and `status` groups by lifecycle rank (ties keep time order).
 */
export function sortTaskRows(
  rows: readonly GetFleetTaskRow[],
  mode: FleetSortMode,
): GetFleetTaskRow[] {
  const copy = [...rows];
  if (mode === "status") {
    return copy.sort((a, b) => {
      const ra = STATUS_RANK[a.status] ?? 9;
      const rb = STATUS_RANK[b.status] ?? 9;
      if (ra !== rb) return ra - rb;
      return rowTime(b) - rowTime(a);
    });
  }
  const direction = mode === "chronoAsc" ? 1 : -1;
  return copy.sort((a, b) => direction * (rowTime(a) - rowTime(b)) || a.id.localeCompare(b.id));
}

// ── plumbing visibility (the TUI's hidden → running only → all) ──────────────

/** TUI `AutomationVisibilityMode`, spelled as the task describes it. */
export type FleetVisibilityMode = "hidden" | "running" | "all";

/** The TUI's unified cycle order. */
export const FLEET_VISIBILITY_ORDER: readonly FleetVisibilityMode[] = ["hidden", "running", "all"];

/** Next visibility mode (`. / <`). */
export function nextVisibilityMode(mode: FleetVisibilityMode): FleetVisibilityMode {
  const idx = FLEET_VISIBILITY_ORDER.indexOf(mode);
  const next = FLEET_VISIBILITY_ORDER[(idx + 1) % FLEET_VISIBILITY_ORDER.length];
  return next ?? "hidden";
}

/** Short operator-facing label for the current plumbing visibility. */
export function visibilityLabel(mode: FleetVisibilityMode): string {
  switch (mode) {
    case "hidden":
      return "plumbing hidden";
    case "running":
      return "running plumbing only";
    case "all":
      return "all plumbing";
  }
}

/**
 * Automation/plumbing task: WG's dotted-id convention (`.evaluate-*`, `.flip-*`,
 * `.assign-*`, other autonomous actors). The TUI keeps a typed population as
 * well; the dotted identity is the projection the fleet read-model carries, and
 * it is exactly what the graph panel hides by default.
 */
export function isPlumbingTask(row: Pick<GetFleetTaskRow, "id">): boolean {
  return row.id.startsWith(".");
}

/** A task row survives the visibility mode? */
export function taskVisible(row: GetFleetTaskRow, mode: FleetVisibilityMode): boolean {
  if (!isPlumbingTask(row)) return true;
  if (mode === "all") return true;
  if (mode === "running") return row.status === "in-progress" || row.status === "running";
  return false;
}

// ── search ──────────────────────────────────────────────────────────────────

/**
 * Typeahead search state (the TUI's `InputMode::Search`).
 *
 * `open()` starts a transaction; `type`/`backspace` edit the query and re-run
 * the match list; `next`/`prev` move the highlight; `commit()` returns the
 * stable task id under the highlight and CLEARS all match state ("Commit stable
 * task ID; clear all match state"); `clear()` abandons the transaction without
 * quitting.
 */
export class FleetSearchState {
  private queryValue = "";
  private activeValue = false;
  private matchesValue: string[] = [];
  private indexValue = 0;

  constructor(private rows: readonly GetFleetTaskRow[] = []) {}

  /** Replace the row population (a fresh snapshot) and re-run the match list. */
  setRows(rows: readonly GetFleetTaskRow[]): void {
    this.rows = rows;
    if (this.activeValue) this.recompute();
  }

  get active(): boolean {
    return this.activeValue;
  }

  get query(): string {
    return this.queryValue;
  }

  get matches(): readonly string[] {
    return this.matchesValue;
  }

  get index(): number {
    return this.indexValue;
  }

  /** The task id currently highlighted by the typeahead, or `null`. */
  get highlighted(): string | null {
    return this.matchesValue[this.indexValue] ?? null;
  }

  /** Begin a search transaction (`/`). */
  open(): void {
    this.activeValue = true;
    this.queryValue = "";
    this.recompute();
  }

  /** Append typed characters (`typeahead`). */
  type(text: string): void {
    if (!this.activeValue) this.open();
    this.queryValue += text;
    this.recompute();
  }

  /** Delete the last character of the query. */
  backspace(): void {
    if (!this.activeValue) return;
    this.queryValue = this.queryValue.slice(0, -1);
    this.recompute();
  }

  /** Highlight the next match (`↓/j/Tab`). */
  next(): void {
    if (!this.activeValue || this.matchesValue.length === 0) return;
    this.indexValue = (this.indexValue + 1) % this.matchesValue.length;
  }

  /** Highlight the previous match (`↑/k/BackTab`). */
  prev(): void {
    if (!this.activeValue || this.matchesValue.length === 0) return;
    this.indexValue =
      (this.indexValue - 1 + this.matchesValue.length) % this.matchesValue.length;
  }

  /**
   * Commit the highlighted match: its stable task id is returned and ALL match
   * state is cleared (the TUI's "Commit stable task ID; clear all match state").
   * Committing with no query or no match clears and returns `null`.
   */
  commit(): string | null {
    const id = this.highlighted;
    this.reset();
    return id;
  }

  /** Abandon the transaction without quitting (`Esc`). */
  clear(): null {
    this.reset();
    return null;
  }

  /** Does a task row pass the current typeahead query? */
  matchesRow(row: Pick<GetFleetTaskRow, "id" | "title">): boolean {
    if (!this.activeValue || this.queryValue.trim() === "") return true;
    return searchRowMatches(row, this.queryValue);
  }

  private reset(): void {
    this.activeValue = false;
    this.queryValue = "";
    this.matchesValue = [];
    this.indexValue = 0;
  }

  private recompute(): void {
    const q = this.queryValue.trim().toLowerCase();
    this.matchesValue =
      q === ""
        ? []
        : this.rows.filter((row) => searchRowMatches(row, q)).map((row) => row.id);
    if (this.indexValue >= this.matchesValue.length) this.indexValue = 0;
  }
}

/** Case-insensitive task-id / title match (`exact-task` search by id or title). */
export function searchRowMatches(row: Pick<GetFleetTaskRow, "id" | "title">, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (q === "") return true;
  return row.id.toLowerCase().includes(q) || (row.title ?? "").toLowerCase().includes(q);
}

// ── rendered-row filtering ───────────────────────────────────────────────────

/** One rendered row plus its task status, as the panel's tree produces them. */
export interface FilterableRow {
  taskId: string | null;
  status?: string | undefined;
}

/**
 * Keep only the rendered rows whose task passes `predicate`.
 *
 * WG's text stays verbatim: rows are never rewritten, only selected. A row that
 * belongs to no task (a blank separator or an arc-only continuation line) is
 * kept only when it sits BETWEEN two kept rows — the same "no orphan chrome"
 * rule the subtree-collapse overlay follows.
 *
 * A search predicate overrides the visibility predicate for task rows: while a
 * search transaction is live the panel shows only the matching tasks (the
 * typeahead feedback), and `Esc` restores the previous projection.
 */
export function filterRenderedRows<T extends FilterableRow>(
  rows: readonly T[],
  keep: (row: T) => boolean,
): T[] {
  const kept: T[] = [];
  let sawTask = false;
  for (let i = 0; i < rows.length; i++) {
    const row = rows[i];
    if (!row) continue;
    if (row.taskId !== null) {
      if (keep(row)) {
        kept.push(row);
        sawTask = true;
      }
      continue;
    }
    if (!sawTask) continue;
    // Keep a chrome row only when a kept task row follows it.
    const hasKeptAfter = rows
      .slice(i + 1)
      .some((next) => next.taskId !== null && keep(next));
    if (hasKeptAfter) kept.push(row);
  }
  return kept;
}
