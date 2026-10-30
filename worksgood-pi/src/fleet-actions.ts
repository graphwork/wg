/**
 * fleet-actions.ts — the `/wg-fleet` panel's **task actions**, ported from the
 * WG TUI's graph-panel quick actions (`src/tui/viz_viewer/event.rs`, help
 * heading "Quick Actions (graph panel)"):
 *
 * ```text
 *   a                 Create new task
 *   D                 Mark selected task done        (confirm)
 *   f                 Mark selected task failed      (reason prompt = the confirm)
 *   x                 Retry selected task            (confirm)
 *   e                 Edit task description          (text prompt)
 *   M                 Send message to task agent     (text prompt)
 *   r                 Force refresh
 * ```
 *
 * Every action drives the SAME `wg` binary the rest of the plugin uses, through
 * the existing `WgBackend.run` seam (never a second HTTP/CLI path), and its
 * output is surfaced as bounded in-panel notify lines rather than thrown into
 * the chat.
 *
 * The panel HIDES these actions in a dormant (inactive) session instead of
 * letting them fail: a session that has not run `/wg on` has no cockpit and no
 * authority to mutate the graph.
 *
 * Pure: argv construction and result summarising only — the caller runs it.
 */

import type { ExecResult } from "@earendil-works/pi-coding-agent";

/** The task actions, in the TUI's help order. */
export type FleetActionKind = "create" | "done" | "fail" | "retry" | "edit" | "message" | "refresh";

/** How an action's inputs are collected before it can run. */
export type FleetActionPrompt = "title" | "description" | "message" | "reason";

export interface FleetActionSpec {
  kind: FleetActionKind;
  /** The TUI's key, for the footer/help ("a", "D", "f", "x", "e", "M", "r"). */
  key: string;
  /** The TUI's help description, verbatim. */
  desc: string;
  /** Needs a selected task (`false` for create/refresh). */
  needsTask: boolean;
  /**
   * The gate before the graph is mutated: `confirm` = y/n prompt (destructive
   * one-shots), a `prompt` = the operator must type the payload (which IS the
   * explicit, deliberate step), `none` = runs immediately (read-only/refresh).
   */
  gate: "none" | "confirm" | FleetActionPrompt;
  /** The `wg` argv (no shell) for a confirmed action. */
  argv: (taskId: string | null, text: string) => string[];
}

/** Every action, keyed by kind. */
export const FLEET_ACTIONS: Record<FleetActionKind, FleetActionSpec> = {
  create: {
    kind: "create",
    key: "a",
    desc: "Create new task",
    needsTask: false,
    gate: "title",
    argv: (_taskId, text) => ["add", text],
  },
  done: {
    kind: "done",
    key: "D",
    desc: "Mark selected task done",
    needsTask: true,
    gate: "confirm",
    argv: (taskId) => ["done", taskId ?? ""],
  },
  fail: {
    kind: "fail",
    key: "f",
    desc: "Mark selected task failed",
    needsTask: true,
    gate: "reason",
    // `wg fail` demands a reason (and an intent for a deliberate stop); the
    // typed reason IS the operator's confirmation.
    argv: (taskId, text) => [
      "fail",
      taskId ?? "",
      "--intent",
      "deliberate-stop",
      "--reason",
      text,
    ],
  },
  retry: {
    kind: "retry",
    key: "x",
    desc: "Retry selected task",
    needsTask: true,
    gate: "confirm",
    argv: (taskId) => ["retry", taskId ?? ""],
  },
  edit: {
    kind: "edit",
    key: "e",
    desc: "Edit task description",
    needsTask: true,
    gate: "description",
    argv: (taskId, text) => ["edit", taskId ?? "", "--description", text],
  },
  message: {
    kind: "message",
    key: "M",
    desc: "Send message to task agent",
    needsTask: true,
    gate: "message",
    argv: (taskId, text) => ["msg", "send", taskId ?? "", text],
  },
  refresh: {
    kind: "refresh",
    key: "r",
    desc: "Force refresh",
    needsTask: false,
    gate: "none",
    argv: () => [],
  },
};

/** Resolve a TUI key to an action kind (`null` when it is not an action key). */
export function actionForKind(kind: FleetActionKind): FleetActionSpec {
  return FLEET_ACTIONS[kind];
}

/** The label the footer shows while a `confirm` gate is pending. */
export function confirmPrompt(kind: FleetActionKind, taskId: string | null): string {
  const spec = FLEET_ACTIONS[kind];
  const subject = kind === "create" ? "task" : (taskId ?? "task");
  switch (kind) {
    case "done":
      return `mark ${subject} done? y/Enter to confirm · n/Esc to cancel`;
    case "retry":
      return `retry ${subject}? y/Enter to confirm · n/Esc to cancel`;
    case "fail":
      return `fail ${subject}: type the reason · Esc to cancel`;
    default:
      return `${spec.desc.toLowerCase()} ${subject}? y/Enter to confirm · n/Esc to cancel`;
  }
}

/** The prompt line shown while a typed-input gate is pending. */
export function inputPrompt(kind: FleetActionKind, taskId: string | null): string {
  switch (kind) {
    case "create":
      return "new task title: ";
    case "edit":
      return `new description for ${taskId ?? "task"}: `;
    case "message":
      return `message to ${taskId ?? "task"}: `;
    case "fail":
      return `reason for failing ${taskId ?? "task"}: `;
    default:
      return "input: ";
  }
}

/** Bound the number of notify lines an action may print into the panel. */
export const FLEET_ACTION_NOTIFY_LINES = 6;
/** Bound each notify line's length (a hostile/verbose wg cannot flood the pane). */
export const FLEET_ACTION_NOTIFY_CHARS = 240;

/**
 * Summarise a completed action into bounded notify lines:
 *
 * ```text
 * done task-a · ok
 *   Completed task-a
 * ```
 *
 * A failure keeps the first lines of BOTH streams (stderr first) so the reason
 * is visible in the panel instead of only in a log file.
 */
export function actionNotifyLines(
  kind: FleetActionKind,
  taskId: string | null,
  result: ExecResult,
): string[] {
  const spec = FLEET_ACTIONS[kind];
  const subject = kind === "create" ? "" : ` ${taskId ?? ""}`.trimEnd();
  const ok = result.code === 0;
  const head = `${spec.key} ${spec.desc.toLowerCase()}${subject ? ` · ${subject}` : ""} · ${
    ok ? "ok" : `exit ${result.code ?? "?"}`
  }`;
  const body = ok ? (result.stdout ?? "") : `${result.stderr ?? ""}\n${result.stdout ?? ""}`;
  const lines = body
    .split(/\r?\n/)
    .map((line) => line.trimEnd())
    .filter((line) => line.trim() !== "")
    .slice(0, FLEET_ACTION_NOTIFY_LINES)
    .map((line) => `  ${clampChars(line, FLEET_ACTION_NOTIFY_CHARS)}`);
  return [head, ...lines];
}

function clampChars(text: string, max: number): string {
  const chars = [...text];
  return chars.length <= max ? text : `${chars.slice(0, max - 1).join("")}…`;
}

/**
 * The action a footer/help line names for the selected row — `null` in a
 * dormant session, where every action is HIDDEN (never attempted-and-failed).
 */
export function availableActions(opts: { inactive: boolean }): FleetActionKind[] {
  if (opts.inactive) return [];
  return (Object.keys(FLEET_ACTIONS) as FleetActionKind[]).filter((kind) => FLEET_ACTIONS[kind].gate !== "none" || kind === "refresh");
}
