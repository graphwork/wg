/**
 * fleet-snapshot.ts — live agent + task data for the FleetView bottom panel.
 *
 * Reads from the **same daemon IPC lane** the viz panel uses (one JSON-line
 * `IpcRequest`/`IpcResponse` per connection over `<wg-dir>/service/daemon.sock`)
 * with two **read-only** requests:
 *
 *   - `viz_snapshot` — the bounded task projection (counts: in-progress / ready
 *     / blocked / done / failed), via `fetchVizSnapshot`;
 *   - `agents`       — the runtime worker registry (id, task, model, uptime).
 *
 * Neither request mutates graph or control-plane state. When the socket is
 * unavailable the panel falls back to the read-only CLI (`wg agents --json` +
 * `wg list --json`); if that also fails the panel clears itself (silent
 * degrade), matching the viz widget's offline behaviour.
 *
 * Polling is bounded: fixed interval, an in-flight guard (no overlapping
 * requests), a bounded per-request timeout, and change-detection so identical
 * snapshots never re-render.
 */

import { connect, type Socket } from "node:net";
import type { WgBackend, WgEnv } from "./wg-backend.js";
import { fetchVizSnapshot, resolveSocketPath, type VizTask } from "./viz-snapshot.js";
import type { FleetAgent, FleetSnapshot } from "./fleet-readmodel.js";

export interface FleetFetchOptions {
  /** Per-request deadline (matches the Rust IPC client's 2s). */
  timeoutMs?: number;
  logTail?: number;
  signal?: AbortSignal;
}

/** One-shot daemon IPC round trip for an arbitrary read-only request. */
function ipcRequest(
  socketPath: string,
  request: Record<string, unknown>,
  timeoutMs: number,
  signal?: AbortSignal,
): Promise<Record<string, unknown>> {
  const payload = JSON.stringify(request);
  return new Promise<Record<string, unknown>>((resolve, reject) => {
    let settled = false;
    let buffer = "";
    let socket: Socket | null = null;
    const finish = (err: Error | null, value?: Record<string, unknown>) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (signal) signal.removeEventListener("abort", onAbort);
      try {
        socket?.destroy();
      } catch {
        /* socket already gone */
      }
      if (err) reject(err);
      else resolve(value as Record<string, unknown>);
    };
    const timer = setTimeout(() => finish(new Error(`ipc request timed out after ${timeoutMs}ms`)), timeoutMs);
    const onAbort = () => finish(new Error("ipc request aborted"));
    if (signal) {
      if (signal.aborted) return finish(new Error("ipc request aborted"));
      signal.addEventListener("abort", onAbort, { once: true });
    }

    socket = connect(socketPath, () => {
      socket?.write(`${payload}\n`);
    });
    socket.once("error", (err: Error) => finish(err));
    socket.on("data", (chunk: Buffer) => {
      buffer += chunk.toString("utf8");
      let idx = buffer.indexOf("\n");
      while (idx >= 0) {
        const line = buffer.slice(0, idx).trim();
        buffer = buffer.slice(idx + 1);
        if (line) {
          try {
            const response = JSON.parse(line) as Record<string, unknown> & { ok?: boolean; error?: string };
            if (response.ok === false) return finish(new Error(response.error ?? "ipc request failed"));
            return finish(null, response);
          } catch (err) {
            return finish(err instanceof Error ? err : new Error(String(err)));
          }
        }
        idx = buffer.indexOf("\n");
      }
    });
    socket.once("close", () => {
      if (!settled && !buffer.trim()) finish(new Error("daemon closed the connection without a response"));
    });
  });
}

/** Normalize a raw registry row (daemon `agents` or `wg agents --json`) to a FleetAgent. */
export function normalizeAgent(raw: unknown): FleetAgent | null {
  if (!raw || typeof raw !== "object") return null;
  const a = raw as Record<string, unknown>;
  const id = typeof a.id === "string" ? a.id : undefined;
  const taskId = typeof a.task_id === "string" ? a.task_id : undefined;
  if (!id || !taskId) return null;
  const rawStatus = typeof a.status === "string" ? a.status : "unknown";
  // CLI statuses may be decorated ("dead (process exited)"); keep the base token.
  const status = rawStatus.split(" ")[0] ?? rawStatus;
  let alive: boolean | undefined;
  if (typeof a.process_alive === "boolean") alive = a.process_alive && !["done", "failed", "dead"].includes(status);
  else if (typeof a.alive === "boolean") alive = a.alive;
  return {
    id,
    taskId,
    executor: typeof a.executor === "string" ? a.executor : null,
    model: typeof a.model === "string" ? a.model : null,
    status,
    alive,
    startedAt: typeof a.started_at === "string" ? a.started_at : null,
    uptime: typeof a.uptime === "string" ? a.uptime : null,
  };
}

/** Fetch the runtime worker registry over the read-only `agents` IPC request. */
export async function fetchAgents(socketPath: string, opts: FleetFetchOptions = {}): Promise<FleetAgent[]> {
  const response = await ipcRequest(socketPath, { cmd: "agents" }, opts.timeoutMs ?? 2000, opts.signal);
  const raw = response.agents;
  if (!Array.isArray(raw)) throw new Error("agents response missing agents");
  return raw.map(normalizeAgent).filter((a): a is FleetAgent => a !== null);
}

/** Both read-only requests over the socket, as one snapshot. */
export async function fetchFleetOverSocket(
  socketPath: string,
  opts: FleetFetchOptions = {},
): Promise<FleetSnapshot> {
  const [snapshot, agents] = await Promise.all([
    fetchVizSnapshot(socketPath, opts),
    fetchAgents(socketPath, opts),
  ]);
  return { agents, tasks: snapshot.tasks };
}

function parseJsonArray(out: string): unknown[] | null {
  try {
    const parsed = JSON.parse(out.trim());
    return Array.isArray(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

/** Minimal VizTask projection from `wg list --json` (enough for the counts). */
function cliTaskToVizTask(raw: unknown): VizTask | null {
  if (!raw || typeof raw !== "object") return null;
  const t = raw as Record<string, unknown>;
  if (typeof t.id !== "string" || typeof t.status !== "string") return null;
  const after = Array.isArray(t.after) ? t.after.filter((d): d is string => typeof d === "string") : [];
  const before = Array.isArray(t.before) ? t.before.filter((d): d is string => typeof d === "string") : [];
  return {
    id: t.id,
    title: typeof t.title === "string" ? t.title : t.id,
    status: t.status,
    after,
    before,
    assigned: typeof t.assigned === "string" ? t.assigned : null,
  };
}

/**
 * Snapshot with CLI fallback: try the daemon socket (both requests); on any
 * failure shell the read-only `wg agents --json` + `wg list --json`. Returns
 * null when neither source produced data (the panel then clears).
 */
export async function fleetSnapshotWithFallback(
  backend: Pick<WgBackend, "run">,
  env: Pick<WgEnv, "daemonSocket" | "dir">,
  opts: FleetFetchOptions = {},
): Promise<FleetSnapshot | null> {
  const resolved = resolveSocketPath(env);
  if (resolved.socket) {
    try {
      return await fetchFleetOverSocket(resolved.socket, opts);
    } catch {
      // fall through to the CLI fallback
    }
  }
  try {
    const [agentsResult, tasksResult] = await Promise.all([
      backend.run(["agents"], { json: true, signal: opts.signal }),
      backend.run(["list"], { json: true, signal: opts.signal }),
    ]);
    const rawAgents = parseJsonArray(agentsResult.stdout || "");
    const rawTasks = parseJsonArray(tasksResult.stdout || "");
    if (!rawAgents && !rawTasks) return null;
    const agents = (rawAgents ?? []).map(normalizeAgent).filter((a): a is FleetAgent => a !== null);
    const tasks = (rawTasks ?? []).map(cliTaskToVizTask).filter((t): t is VizTask => t !== null);
    return { agents, tasks };
  } catch {
    return null;
  }
}

export type FleetPollCallback = (snapshot: FleetSnapshot) => void;

/**
 * Bounded poller: fixed interval, no overlapping in-flight requests, and a
 * change guard so identical snapshots are never re-delivered. `null` results
 * (offline) are delivered once so the panel can clear itself.
 */
export class FleetPoller {
  private timer: ReturnType<typeof setInterval> | null = null;
  private inFlight = false;
  private lastJson: string | null = null;
  private stopped = true;

  constructor(
    private readonly fetcher: () => Promise<FleetSnapshot | null>,
    private readonly onChange: (snapshot: FleetSnapshot | null) => void,
    private readonly intervalMs = 5000,
  ) {}

  start(): void {
    if (!this.stopped) return;
    this.stopped = false;
    this.timer = setInterval(() => void this.refresh(), this.intervalMs);
    void this.refresh();
  }

  stop(): void {
    this.stopped = true;
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }

  /** One immediate bounded fetch; safe to call while a fetch is in flight. */
  async refresh(): Promise<FleetSnapshot | null> {
    if (this.inFlight || this.stopped) return null;
    this.inFlight = true;
    try {
      const snapshot = await this.fetcher();
      const json = snapshot ? JSON.stringify(snapshot) : "null";
      if (json !== this.lastJson) {
        this.lastJson = json;
        this.onChange(snapshot);
      }
      return snapshot;
    } catch {
      // Silent degrade: an offline daemon is a normal state.
      return null;
    } finally {
      this.inFlight = false;
    }
  }
}
