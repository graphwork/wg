/**
 * fleet-inspector.ts — the `/wg-fleet` inspector's DETAIL-model helpers: section
 * splitting for the TUI's Detail-tab `Space` ("Toggle section collapse in
 * Detail") and the raw-JSON view's line model (`R` / `J`).
 *
 * WG's detail text is the verbatim `wg show <task>` body: a sequence of
 * top-level `Field: value` lines, each followed by indented continuation lines.
 * A **section** is therefore one column-0 line plus the lines under it. Collapse
 * hides a section's body and appends `(+N lines)` to its heading — the same
 * "hidden descendant count" convention the rendered tree already uses for
 * collapsed subtrees, so the operator never sees content vanish silently.
 *
 * Pure: no I/O.
 */

/** One collapsible section of WG's detail text. */
export interface DetailSection {
  /** Stable identity: heading row + heading text (headings may repeat). */
  key: string;
  /** The heading line (a column-0, non-empty line). */
  heading: string;
  /** Row index of the heading within the full detail line list. */
  headingRow: number;
  /** Rows belonging to the section's body (continuations, blanks). */
  bodyRows: number[];
}

/** True when a detail line starts a new section (column 0, non-empty). */
export function isSectionHeading(line: string): boolean {
  if (line.trim() === "") return false;
  return !line.startsWith(" ") && !line.startsWith("\t");
}

/**
 * Split WG's verbatim detail text into collapsible sections.
 *
 * Leading lines that precede the first column-0 line (rare: a wrapped or
 * indented preamble) form an implicit section keyed `#preamble` so `Space` still
 * has something to act on.
 */
export function splitDetailSections(lines: readonly string[]): DetailSection[] {
  const sections: DetailSection[] = [];
  let current: DetailSection | null = null;
  const preamble: number[] = [];
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i] ?? "";
    if (isSectionHeading(line)) {
      current = { key: `${i}:${line}`, heading: line, headingRow: i, bodyRows: [] };
      sections.push(current);
      continue;
    }
    if (current) current.bodyRows.push(i);
    else if (line.trim() !== "") preamble.push(i);
  }
  if (preamble.length > 0) {
    sections.unshift({
      key: `0:#preamble`,
      heading: lines[preamble[0] ?? 0] ?? "",
      headingRow: preamble[0] ?? 0,
      bodyRows: preamble.slice(1),
    });
  }
  return sections;
}

/**
 * The collapse key of the section containing a (full-list) row, or `null` when
 * the row is not inside any section.
 */
export function sectionKeyAt(
  sections: readonly DetailSection[],
  row: number,
): string | null {
  const target = Math.max(0, Math.trunc(row));
  for (const section of sections) {
    if (section.headingRow === target) return section.key;
    if (section.bodyRows.includes(target)) return section.key;
  }
  // The row may sit inside a collapsed block in the RENDERED list; callers pass
  // full-list rows, so falling back to the last section that starts at or before
  // the row keeps `Space` predictable rather than silently doing nothing.
  let best: DetailSection | null = null;
  for (const section of sections) {
    if (section.headingRow <= target) best = section;
  }
  return best ? best.key : null;
}

/**
 * The section `Space` should act on for a given (full-list) row: the section at
 * that row when it has a body, else the nearest PRECEDING section with a body,
 * else the first section with a body (so a field-only line still reaches a real
 * collapsible section instead of doing nothing).
 *
 * A section whose body is empty is not collapsible — the heading alone is the
 * whole section, so there is nothing to hide.
 */
export function collapsibleSectionAt(
  sections: readonly DetailSection[],
  row: number,
): DetailSection | null {
  const target = Math.max(0, Math.trunc(row));
  const collapsible = sections.filter((section) => section.bodyRows.length > 0);
  if (collapsible.length === 0) return null;
  const at = collapsible.find((section) => section.headingRow === target);
  if (at) return at;
  let best: DetailSection | null = null;
  for (const section of collapsible) {
    if (section.headingRow <= target) best = section;
  }
  return best ?? collapsible[0] ?? null;
}

/**
 * Apply the collapsed-section set to WG's verbatim detail lines.
 *
 * A collapsed section keeps its heading (with a `(+N lines)` marker appended to
 * the heading TEXT — never replacing it) and drops its body. Every other line is
 * returned unchanged, byte-for-byte.
 */
export function applySectionCollapse(
  lines: readonly string[],
  collapsed: ReadonlySet<string>,
): { lines: string[]; hidden: number } {
  if (collapsed.size === 0) return { lines: [...lines], hidden: 0 };
  const sections = splitDetailSections(lines);
  const hiddenRows = new Set<number>();
  const markerByHeadingRow = new Map<number, number>();
  let hidden = 0;
  for (const section of sections) {
    if (!collapsed.has(section.key)) continue;
    // A heading with no body is not collapsible (nothing would change).
    if (section.bodyRows.length === 0) continue;
    for (const row of section.bodyRows) hiddenRows.add(row);
    markerByHeadingRow.set(section.headingRow, section.bodyRows.length);
    hidden += section.bodyRows.length;
  }
  if (hidden === 0) return { lines: [...lines], hidden: 0 };
  const out: string[] = [];
  for (let i = 0; i < lines.length; i++) {
    if (hiddenRows.has(i)) continue;
    const marker = markerByHeadingRow.get(i);
    out.push(marker === undefined ? (lines[i] ?? "") : `${lines[i] ?? ""} (+${marker} lines)`);
  }
  return { lines: out, hidden };
}
