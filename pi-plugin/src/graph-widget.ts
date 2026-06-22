/**
 * graph-widget.ts — surface the WG task graph inside the pi session.
 *
 * Pushes a compact "ready tasks" view to the always-visible widget above the
 * editor and a one-line summary to the footer status, refreshed on
 * `session_start` and after every `turn_end` (plugin-research.md §1.2).
 *
 * In tui/rpc modes `ctx.hasUI` is true and these calls render; in json/print
 * modes the UI no-ops, so the refresh is cheap and harmless. Errors are
 * swallowed — a flaky `wg` call must never break the session.
 */

import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { WgBackend } from "./wg-backend.js";

const WIDGET_KEY = "wg-graph";
const STATUS_KEY = "wg";
const MAX_ROWS = 6;

interface ReadyTask {
  id?: string;
  title?: string;
}

/** Parse `wg ready --json` stdout into a typed list, tolerating junk. */
export function parseReady(stdout: string): ReadyTask[] {
  const out = stdout.trim();
  if (!out) return [];
  try {
    const parsed = JSON.parse(out);
    return Array.isArray(parsed) ? (parsed as ReadyTask[]) : [];
  } catch {
    return [];
  }
}

/** Render the widget lines for a set of ready tasks. */
export function renderWidget(ready: ReadyTask[]): string[] {
  if (ready.length === 0) return ["WG: no ready tasks"];
  const lines = [`WG ready (${ready.length}):`];
  for (const t of ready.slice(0, MAX_ROWS)) {
    const id = t.id ?? "?";
    const title = t.title ?? "";
    lines.push(` • ${id}${title ? ` ${title}` : ""}`);
  }
  if (ready.length > MAX_ROWS) lines.push(` … +${ready.length - MAX_ROWS} more`);
  return lines;
}

async function refreshGraph(backend: WgBackend, ctx: ExtensionContext): Promise<void> {
  try {
    const r = await backend.ready({ signal: ctx.signal });
    const ready = parseReady(r.stdout);
    ctx.ui.setWidget(WIDGET_KEY, renderWidget(ready));
    ctx.ui.setStatus(STATUS_KEY, `wg: ${ready.length} ready`);
  } catch {
    // Best-effort: never let a wg hiccup surface as a session error.
  }
}

export function installGraphWidget(pi: ExtensionAPI, backend: WgBackend): void {
  pi.on("session_start", async (_event, ctx) => {
    await refreshGraph(backend, ctx);
  });
  pi.on("turn_end", async (_event, ctx) => {
    await refreshGraph(backend, ctx);
  });
}
