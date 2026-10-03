/**
 * fleet-panel-model.ts — the pure read-model + bounded-read helpers for the
 * **scrollable** FleetView panel (`/wg-fleet`).
 *
 * This is the crib of pi-subagents' `FleetView`
 * (`src/runs/background/fleet-view.js`) onto the WG graph, kept pure so the
 * scroll math, tail bounding, and glyph/status mapping are unit-testable
 * without a terminal:
 *
 *  - **bounded transcript tail** — the same byte/line budget pi-subagents uses
 *    (`TRANSCRIPT_TAIL_BYTES` 256 KiB, `DEFAULT_TRANSCRIPT_LINES` 80,
 *    `MAX_TRANSCRIPT_LINES` 500, `MAX_TRANSCRIPT_BODY_BYTES` 32 KiB) applied to
 *    the WG equivalent of a child transcript: an agent's live NDJSON stream tail
 *    (`.wg/agents/<id>/raw_stream.jsonl`, the file the Rust
 *    `agent_activity::current_step` deriver also reads). Reads seek to the end
 *    and never take a lock — strictly read-only.
 *  - **nested status over the dependency tree** — reuse the WG viz read-model
 *    (`buildTree`) on a `GetFleet` projection, then decorate each node with its
 *    live agent activity.
 *  - **scroll math** — `clampScroll` / `scrollToKeepVisible` / `pageScroll`,
 *    the selection-follows-scroll + clamping rules the panel obeys.
 *
 * No I/O happens at import time and nothing here mutates graph state.
 */

import { closeSync, existsSync, openSync, readSync, statSync } from "node:fs";
import { join } from "node:path";
import type {
  GetFleetAgentRow,
  GetFleetCounts,
  GetFleetSnapshot,
  GetFleetTaskRow,
} from "./wg-backend.js";
import type { VizSnapshot, VizTask } from "./viz-snapshot.js";
import { buildTree, type TreeRender } from "./viz-readmodel.js";

// ── pi-subagents' transcript bounds (kept identical on purpose) ──────────────

/** Read at most this many bytes from the tail of a stream file. */
export const TRANSCRIPT_TAIL_BYTES = 256 * 1024;
/** Default number of transcript lines rendered. */
export const DEFAULT_TRANSCRIPT_LINES = 80;
/** Hard cap on rendered transcript lines (a caller can never raise it). */
export const MAX_TRANSCRIPT_LINES = 500;
/** Max characters kept from a single transcript line. */
export const MAX_TRANSCRIPT_LINE_CHARS = 2000;
/** Max bytes of transcript body rendered (terminal escaping can expand it). */
export const MAX_TRANSCRIPT_BODY_BYTES = 32 * 1024;

/** Minimum panel height (header + one body line + footer). */
export const FLEET_PANEL_MIN_HEIGHT = 3;
/** Fallback height when the terminal size is unknown. */
export const FLEET_PANEL_DEFAULT_HEIGHT = 24;

/** Clamp a requested transcript line count into `1..=MAX_TRANSCRIPT_LINES`. */
export function transcriptLineLimit(value?: number): number {
  if (value === undefined || !Number.isFinite(value)) return DEFAULT_TRANSCRIPT_LINES;
  return Math.max(1, Math.min(MAX_TRANSCRIPT_LINES, Math.trunc(value)));
}

// ── scroll math (pure) ──────────────────────────────────────────────────────

/** Largest valid scroll offset for a `total`-line list in a `viewport`. */
export function maxScroll(total: number, viewport: number): number {
  return Math.max(0, Math.trunc(total) - Math.max(0, Math.trunc(viewport)));
}

/** Clamp a scroll offset into `0..=maxScroll` (NaN/∞ degrade to 0). */
export function clampScroll(offset: number, total: number, viewport: number): number {
  if (!Number.isFinite(offset)) return 0;
  return Math.max(0, Math.min(Math.trunc(offset), maxScroll(total, viewport)));
}

/**
 * Adjust `offset` so `index` is inside `[offset, offset+viewport)`; this is the
 * "selection stays visible / moving the selection scrolls it into view" rule.
 */
export function scrollToKeepVisible(
  offset: number,
  viewport: number,
  index: number,
  total: number,
): number {
  const view = Math.max(0, Math.trunc(viewport));
  const start = clampScroll(offset, total, view);
  if (view <= 0 || index < 0) return start;
  if (index < start) return clampScroll(index, total, view);
  if (index >= start + view) return clampScroll(index - view + 1, total, view);
  return start;
}

/** Page the viewport by `direction` (±1) pages (a full viewport minus one line). */
export function pageScroll(
  offset: number,
  viewport: number,
  direction: number,
  total: number,
): number {
  const view = Math.max(1, Math.trunc(viewport));
  const step = Math.max(1, view - 1);
  return clampScroll(clampScroll(offset, total, view) + (direction >= 0 ? step : -step), total, view);
}

// ── GetFleet → Viz projection (reuse the tree + detail renderers) ────────────

/** Project a `GetFleet` task row onto the viz read-model's `VizTask`. */
export function getFleetRowToVizTask(row: GetFleetTaskRow): VizTask {
  return {
    id: row.id,
    title: row.title,
    presentation: row.presentation,
    status: row.status,
    assigned: row.assigned ?? null,
    after: row.depends_on ?? [],
    before: [],
    created_at: row.started_at ?? null,
    started_at: row.started_at ?? null,
    completed_at: row.completed_at ?? null,
    last_interaction_at: row.last_interaction_at ?? null,
    token_usage: row.token_usage ?? null,
    failure_reason: row.failure_reason ?? null,
  };
}

/** Project a whole `GetFleet` snapshot into the viz snapshot the tree uses. */
export function getFleetToVizSnapshot(snapshot: GetFleetSnapshot): VizSnapshot {
  const tasks = snapshot.tasks.map(getFleetRowToVizTask);
  const byId = new Map(tasks.map((t) => [t.id, t]));
  for (const task of tasks) {
    for (const dep of task.after ?? []) {
      const upstream = byId.get(dep);
      if (upstream) (upstream.before ??= []).push(task.id);
    }
  }
  return { tasks };
}

/** Build the nested dependency tree (reusing the viz read-model) for a snapshot. */
export function buildFleetTree(
  snapshot: GetFleetSnapshot,
  collapsed: ReadonlySet<string> = new Set(),
): TreeRender {
  return buildTree(getFleetToVizSnapshot(snapshot), collapsed);
}

// ── counts header + agent lookup ────────────────────────────────────────────

/** The counts header: in-progress/ready/blocked/done (+ failed/active agents). */
export function fleetCountsHeader(counts: GetFleetCounts): string {
  const parts = [
    `${counts.in_progress} in-progress`,
    `${counts.ready} ready`,
    `${counts.blocked} blocked`,
    `${counts.done} done`,
    `${counts.failed ?? 0} failed`,
  ];
  if (counts.active_agents) parts.push(`${counts.active_agents} active`);
  return `wg-fleet ▸ ${parts.join(" · ")}`;
}

/** The live agent row for a task id, when one is registered. */
export function agentForTask(
  agents: readonly GetFleetAgentRow[],
  taskId: string,
): GetFleetAgentRow | null {
  return agents.find((agent) => agent.task_id === taskId) ?? null;
}

/** Compact per-task activity label: the bounded current step, else the status. */
export function agentActivityLabel(agent: GetFleetAgentRow | null): string | null {
  if (!agent) return null;
  const activity = agent.activity?.trim();
  if (activity) return activity;
  const status = agent.status?.trim();
  return status ? status : null;
}

// ── bounded tail read (pi-subagents' readTextTail, WG-flavoured) ─────────────

export interface TextTail {
  path: string;
  lines: string[];
  /** True when bytes or lines were dropped from the head. */
  truncated: boolean;
  error?: string;
}

/**
 * Read at most `maxLines` lines from the **end** of a text file, reading at
 * most `TRANSCRIPT_TAIL_BYTES` bytes (seek to the end; a partial first line at
 * the seek boundary is dropped). Never throws — a missing/rotating/unreadable
 * file degrades to an empty tail with `error` set.
 */
export function readTextTail(filePath: string, maxLines: number): TextTail {
  const limit = Math.max(1, Math.trunc(maxLines));
  try {
    const stat = statSync(filePath);
    if (!stat.isFile() || stat.size === 0) return { path: filePath, lines: [], truncated: false };
    const bytesToRead = Math.min(stat.size, TRANSCRIPT_TAIL_BYTES);
    const start = stat.size - bytesToRead;
    const buffer = Buffer.alloc(bytesToRead);
    const fd = openSync(filePath, "r");
    try {
      const bytesRead = readSync(fd, buffer, 0, bytesToRead, start);
      const content = buffer.subarray(0, bytesRead).toString("utf8");
      let lines = content.split(/\r?\n/);
      if (start > 0 && lines.length > 0) lines = lines.slice(1);
      if (lines.length > 0 && lines[lines.length - 1] === "") lines = lines.slice(0, -1);
      const truncated = start > 0 || lines.length > limit;
      return { path: filePath, lines: lines.slice(-limit), truncated };
    } finally {
      closeSync(fd);
    }
  } catch (err) {
    return {
      path: filePath,
      lines: [],
      truncated: false,
      error: err instanceof Error ? err.message : String(err),
    };
  }
}

/** Candidate live-stream paths for an agent (project root or `.wg/` dir). */
export function agentStreamCandidates(dir: string | undefined, agentId: string): string[] {
  if (!dir) return [];
  return [
    join(dir, "agents", agentId, "raw_stream.jsonl"),
    join(dir, ".wg", "agents", agentId, "raw_stream.jsonl"),
  ];
}

/**
 * Read the bounded tail of an agent's live NDJSON stream. Returns `null` when
 * no candidate file exists (the panel then shows the activity label only).
 */
export function readAgentStreamTail(
  dir: string | undefined,
  agentId: string,
  maxLines: number = DEFAULT_TRANSCRIPT_LINES,
): TextTail | null {
  if (!agentId) return null;
  for (const candidate of agentStreamCandidates(dir, agentId)) {
    if (existsSync(candidate)) return readTextTail(candidate, maxLines);
  }
  return null;
}

// ── transcript body bounding ────────────────────────────────────────────────

/** Strip C0 control characters (keep tab) so a hostile tail cannot corrupt the TUI. */
export function safeTailText(text: string): string {
  // eslint-disable-next-line no-control-regex
  return text.replace(/[\u0000-\u0008\u000a-\u001f\u007f]/g, "�");
}

/**
 * Bound a transcript body to `MAX_TRANSCRIPT_BODY_BYTES` while keeping whole,
 * most-recent lines (newest last). Lines longer than `MAX_TRANSCRIPT_LINE_CHARS`
 * are clipped; when the byte budget is exceeded the remaining head is replaced
 * by a single omission marker.
 */
export function boundTranscriptBody(
  lines: readonly string[],
  opts: { maxBytes?: number; maxLineChars?: number; indent?: string } = {},
): string[] {
  const maxBytes = opts.maxBytes ?? MAX_TRANSCRIPT_BODY_BYTES;
  const maxLineChars = opts.maxLineChars ?? MAX_TRANSCRIPT_LINE_CHARS;
  const indent = opts.indent ?? "  ";
  const body: string[] = [];
  let bytes = 0;
  for (let i = lines.length - 1; i >= 0; i--) {
    const safe = safeTailText(`${indent}${lines[i] ?? ""}`);
    const charLen = [...safe].length;
    const line = charLen <= maxLineChars ? safe : `${[...safe].slice(0, maxLineChars - 1).join("")}…`;
    const size = Buffer.byteLength(line) + 1;
    if (bytes + size > maxBytes) {
      // Make room for the omission marker by dropping the oldest kept lines.
      const marker = `${indent}… [earlier lines omitted]`;
      const markerBytes = Buffer.byteLength(marker) + 1;
      while (body.length > 0 && bytes + markerBytes > maxBytes) {
        bytes -= Buffer.byteLength(body.pop() ?? "") + 1;
      }
      if (bytes + markerBytes <= maxBytes) body.push(marker);
      break;
    }
    body.push(line);
    bytes += size;
  }
  return body.reverse();
}
