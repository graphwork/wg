/**
 * viz-snapshot.ts — live graph data for the embedded VizView panel.
 *
 * The panel reads the work graph from the **existing daemon IPC socket** (the
 * same one-request/one-response JSON-line `IpcRequest`/`IpcResponse` protocol
 * the TUI family speaks over `<wg-dir>/service/daemon.sock`, see
 * `src/commands/service/mod.rs::send_request_to_socket_with_timeout`). The
 * only request it ever sends is the read-only `viz_snapshot` — the panel never
 * mutates graph state (lifecycle authority stays with workers/controller).
 *
 * Resolution order for the socket path:
 *   1. `WG_DAEMON_SOCKET` (what WG exports / the forward-compat contract).
 *   2. `<WG_DIR>/service/daemon.sock` (WG_DIR is the workgraph dir).
 *   3. `<cwd>/.wg/service/daemon.sock` (standalone console pi in a WG repo).
 *
 * If the daemon is unreachable (no socket, connect error, timeout, error
 * response) the caller falls back to `wg viz --all --no-tui` ASCII output —
 * the same `generate_ascii` rendering the TUI shows — so the panel degrades
 * instead of disappearing.
 *
 * Polling is deliberately **bounded**: a fixed interval with an in-flight
 * guard (no overlapping requests), a bounded per-request timeout, and
 * change-detection so identical snapshots never re-render.
 */

import { connect, type Socket } from "node:net";
import { existsSync } from "node:fs";
import { join } from "node:path";
import type { WgBackend, WgEnv } from "./wg-backend.js";

/** Log entry from the bounded per-task tail. */
export interface VizLogEntry {
  timestamp?: string;
  actor?: string | null;
  message: string;
}

/** Aggregate token usage, mirroring the graph's `TokenUsage` JSON shape. */
export interface VizTokenUsage {
  cost_usd?: number;
  input_tokens?: number;
  output_tokens?: number;
  cache_read_input_tokens?: number;
  cache_creation_input_tokens?: number;
  total_tokens?: number;
}

/** One task of the bounded snapshot projection (`service::viz_snapshot`). */
export interface VizTask {
  id: string;
  title: string;
  presentation?: string;
  status: string;
  assigned?: string | null;
  after?: string[];
  before?: string[];
  description_head?: string | null;
  created_at?: string | null;
  started_at?: string | null;
  completed_at?: string | null;
  last_interaction_at?: string | null;
  token_usage?: VizTokenUsage | null;
  retry_count?: number;
  failure_reason?: string | null;
  log_count?: number;
  log_tail?: VizLogEntry[];
}

export interface VizSnapshot {
  tasks: VizTask[];
}

/** Where the resolved socket path came from (diagnostics only). */
export interface SocketResolution {
  socket: string | null;
  source: "env" | "wg-dir" | "cwd" | "none";
}

/**
 * Ordered daemon-socket candidates. WG_DIR may be either the workgraph dir
 * itself (`<project>/.wg`) or the project root depending on the launch path,
 * so both spellings are probed, then the cwd walk-below candidates.
 */
export function socketCandidates(
  env: Pick<WgEnv, "daemonSocket" | "dir">,
  cwd: string = process.cwd(),
): string[] {
  const candidates: string[] = [];
  if (env.daemonSocket) candidates.push(env.daemonSocket);
  if (env.dir) {
    candidates.push(join(env.dir, "service", "daemon.sock"));
    candidates.push(join(env.dir, ".wg", "service", "daemon.sock"));
  }
  candidates.push(join(cwd, ".wg", "service", "daemon.sock"));
  candidates.push(join(cwd, "service", "daemon.sock"));
  return candidates;
}

/** Resolve the first existing daemon socket path, or null when offline. */
export function resolveSocketPath(
  env: Pick<WgEnv, "daemonSocket" | "dir">,
  cwd: string = process.cwd(),
  exists: (p: string) => boolean = existsSync,
): SocketResolution {
  if (env.daemonSocket) return { socket: env.daemonSocket, source: "env" };
  if (env.dir) {
    const wgDir = join(env.dir, "service", "daemon.sock");
    if (exists(wgDir)) return { socket: wgDir, source: "wg-dir" };
    const projectRoot = join(env.dir, ".wg", "service", "daemon.sock");
    if (exists(projectRoot)) return { socket: projectRoot, source: "wg-dir" };
  }
  const cwdCandidate = join(cwd, ".wg", "service", "daemon.sock");
  if (exists(cwdCandidate)) return { socket: cwdCandidate, source: "cwd" };
  const legacy = join(cwd, "service", "daemon.sock");
  if (exists(legacy)) return { socket: legacy, source: "cwd" };
  return { socket: null, source: "none" };
}

export interface FetchOptions {
  /** Per-request deadline. The Rust client uses 2s for IPC; we match it. */
  timeoutMs?: number;
  /** Bounded log tail requested per task (clamped server-side to 1..=100). */
  logTail?: number;
  signal?: AbortSignal;
}

/**
 * One-shot daemon IPC round trip: connect, write `{"cmd":"viz_snapshot"…}\n`,
 * read the single JSON response line, close. Mirrors the Rust client's
 * one-request-per-connection contract exactly.
 */
export function fetchVizSnapshot(socketPath: string, opts: FetchOptions = {}): Promise<VizSnapshot> {
  const timeoutMs = opts.timeoutMs ?? 2000;
  const request = JSON.stringify({ cmd: "viz_snapshot", log_tail: opts.logTail ?? 20 });
  return new Promise<VizSnapshot>((resolve, reject) => {
    let settled = false;
    let buffer = "";
    let socket: Socket | null = null;
    const finish = (err: Error | null, value?: VizSnapshot) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (opts.signal) opts.signal.removeEventListener("abort", onAbort);
      try {
        socket?.destroy();
      } catch {
        /* socket already gone */
      }
      if (err) reject(err);
      else resolve(value as VizSnapshot);
    };
    const timer = setTimeout(() => finish(new Error(`viz_snapshot timed out after ${timeoutMs}ms`)), timeoutMs);
    const onAbort = () => finish(new Error("viz_snapshot aborted"));
    if (opts.signal) {
      if (opts.signal.aborted) return finish(new Error("viz_snapshot aborted"));
      opts.signal.addEventListener("abort", onAbort, { once: true });
    }

    socket = connect(socketPath, () => {
      socket?.write(`${request}\n`);
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
            const response = JSON.parse(line) as { ok?: boolean; error?: string } & Partial<VizSnapshot>;
            if (response.ok === false) {
              return finish(new Error(response.error ?? "viz_snapshot failed"));
            }
            const tasks = (response as VizSnapshot).tasks;
            if (!Array.isArray(tasks)) {
              return finish(new Error("viz_snapshot response missing tasks"));
            }
            return finish(null, { tasks });
          } catch (err) {
            return finish(err instanceof Error ? err : new Error(String(err)));
          }
        }
        idx = buffer.indexOf("\n");
      }
    });
    socket.once("close", () => {
      if (!settled && !buffer.trim()) {
        finish(new Error("daemon closed the connection without a response"));
      }
    });
  });
}

export type PollCallback = (snapshot: VizSnapshot) => void;

/**
 * Bounded poller: fixed interval, no overlapping in-flight requests, and a
 * change guard so identical snapshots are never re-delivered.
 */
export class VizPoller {
  private timer: ReturnType<typeof setInterval> | null = null;
  private inFlight = false;
  private lastJson: string | null = null;
  private stopped = true;

  constructor(
    private readonly fetcher: () => Promise<VizSnapshot>,
    private readonly onChange: PollCallback,
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
  async refresh(): Promise<VizSnapshot | null> {
    if (this.inFlight || this.stopped) return null;
    this.inFlight = true;
    try {
      const snapshot = await this.fetcher();
      const json = JSON.stringify(snapshot);
      if (json !== this.lastJson) {
        this.lastJson = json;
        this.onChange(snapshot);
      }
      return snapshot;
    } catch {
      // Silent degrade: an offline daemon is a normal state (the panel falls
      // back to ASCII), never an error surface inside pi.
      return null;
    } finally {
      this.inFlight = false;
    }
  }
}

export type VizDataSource =
  | { kind: "snapshot"; snapshot: VizSnapshot; source: string }
  | { kind: "ascii"; text: string };

/**
 * Snapshot-with-fallback: try the daemon socket; on any failure (offline,
 * timeout, error response) shell `wg viz --all --no-tui` and surface the same
 * ASCII the static viz prints. The fallback is a read-only verb.
 */
export async function vizSnapshotWithFallback(
  backend: Pick<WgBackend, "run">,
  env: WgEnv,
  opts: FetchOptions = {},
): Promise<VizDataSource> {
  const resolved = resolveSocketPath(env);
  if (resolved.socket) {
    try {
      const snapshot = await fetchVizSnapshot(resolved.socket, opts);
      return { kind: "snapshot", snapshot, source: resolved.source };
    } catch {
      // fall through to the ASCII fallback
    }
  }
  const r = await backend.run(["viz", "--all", "--no-tui"], { signal: opts.signal });
  return { kind: "ascii", text: (r.stdout || r.stderr || "").trimEnd() };
}
