/**
 * fleet-help.ts — the `/wg-fleet` help modal state, mirroring the TUI's
 * `HelpOverlayState` + `handle_help_key` (`src/tui/viz_viewer/event.rs`):
 *
 * ```text
 *   ? / Esc / q        close help
 *   ↑ / ↓, j / k       line
 *   PgUp / PgDn        page
 *   Home / End         top / bottom
 *   wheel/trackpad     scroll Help only
 * ```
 *
 * The BODY is never authored by hand: it is generated from the binding table
 * (`fleet-bindings.ts::helpLines`), so a new binding appears in the help the
 * moment it exists and the panel's help text cannot drift from what the panel
 * actually does.
 *
 * Pure windowing math — no terminal, no I/O.
 */

/** Rows the modal needs before it is worth rendering (header + 2 + footer). */
export const FLEET_HELP_MODAL_MIN_ROWS = 5;

/** Largest valid help scroll offset. */
export function helpMaxScroll(total: number, viewport: number): number {
  return Math.max(0, Math.trunc(total) - Math.max(1, Math.trunc(viewport)));
}

export interface HelpWindow {
  /** The visible lines. */
  lines: string[];
  /** Clamped scroll offset backing the window. */
  scroll: number;
  /** 1-based index of the first visible line (`0` when the body is empty). */
  first: number;
  /** 1-based index of the last visible line. */
  last: number;
  /** Total (unwindowed) line count. */
  total: number;
  /** True when content is hidden below the window. */
  overflow: boolean;
}

/**
 * Window the help body to `viewport` rows. `scroll` is clamped into
 * `[0, maxScroll]`, and `first`/`last` are the 1-based position indicator the
 * modal's title shows (`? Keybindings  1-20/45`), matching the TUI's
 * `Keybindings {first}-{last}/{total}` title.
 */
export function helpWindow(
  lines: readonly string[],
  scroll: number,
  viewport: number,
): HelpWindow {
  const rows = Math.max(1, Math.trunc(viewport));
  const total = lines.length;
  const clamped = Math.max(0, Math.min(Number.isFinite(scroll) ? Math.trunc(scroll) : 0, helpMaxScroll(total, rows)));
  const slice = lines.slice(clamped, clamped + rows);
  const first = total === 0 ? 0 : clamped + 1;
  const last = total === 0 ? 0 : Math.min(total, clamped + slice.length);
  return { lines: [...slice], scroll: clamped, first, last, total, overflow: clamped + slice.length < total };
}
