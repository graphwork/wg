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
import { connect } from "node:net";
import { existsSync } from "node:fs";
import { join } from "node:path";
/**
 * Ordered daemon-socket candidates. WG_DIR may be either the workgraph dir
 * itself (`<project>/.wg`) or the project root depending on the launch path,
 * so both spellings are probed, then the cwd walk-below candidates.
 */
export function socketCandidates(env, cwd = process.cwd()) {
    const candidates = [];
    if (env.daemonSocket)
        candidates.push(env.daemonSocket);
    if (env.dir) {
        candidates.push(join(env.dir, "service", "daemon.sock"));
        candidates.push(join(env.dir, ".wg", "service", "daemon.sock"));
    }
    candidates.push(join(cwd, ".wg", "service", "daemon.sock"));
    candidates.push(join(cwd, "service", "daemon.sock"));
    return candidates;
}
/** Resolve the first existing daemon socket path, or null when offline. */
export function resolveSocketPath(env, cwd = process.cwd(), exists = existsSync) {
    if (env.daemonSocket)
        return { socket: env.daemonSocket, source: "env" };
    if (env.dir) {
        const wgDir = join(env.dir, "service", "daemon.sock");
        if (exists(wgDir))
            return { socket: wgDir, source: "wg-dir" };
        const projectRoot = join(env.dir, ".wg", "service", "daemon.sock");
        if (exists(projectRoot))
            return { socket: projectRoot, source: "wg-dir" };
    }
    const cwdCandidate = join(cwd, ".wg", "service", "daemon.sock");
    if (exists(cwdCandidate))
        return { socket: cwdCandidate, source: "cwd" };
    const legacy = join(cwd, "service", "daemon.sock");
    if (exists(legacy))
        return { socket: legacy, source: "cwd" };
    return { socket: null, source: "none" };
}
/**
 * One-shot daemon IPC round trip: connect, write `{"cmd":"viz_snapshot"…}\n`,
 * read the single JSON response line, close. Mirrors the Rust client's
 * one-request-per-connection contract exactly.
 */
export function fetchVizSnapshot(socketPath, opts = {}) {
    const timeoutMs = opts.timeoutMs ?? 2000;
    const request = JSON.stringify({ cmd: "viz_snapshot", log_tail: opts.logTail ?? 20 });
    return new Promise((resolve, reject) => {
        let settled = false;
        let buffer = "";
        let socket = null;
        const finish = (err, value) => {
            if (settled)
                return;
            settled = true;
            clearTimeout(timer);
            if (opts.signal)
                opts.signal.removeEventListener("abort", onAbort);
            try {
                socket?.destroy();
            }
            catch {
                /* socket already gone */
            }
            if (err)
                reject(err);
            else
                resolve(value);
        };
        const timer = setTimeout(() => finish(new Error(`viz_snapshot timed out after ${timeoutMs}ms`)), timeoutMs);
        const onAbort = () => finish(new Error("viz_snapshot aborted"));
        if (opts.signal) {
            if (opts.signal.aborted)
                return finish(new Error("viz_snapshot aborted"));
            opts.signal.addEventListener("abort", onAbort, { once: true });
        }
        socket = connect(socketPath, () => {
            socket?.write(`${request}\n`);
        });
        socket.once("error", (err) => finish(err));
        socket.on("data", (chunk) => {
            buffer += chunk.toString("utf8");
            let idx = buffer.indexOf("\n");
            while (idx >= 0) {
                const line = buffer.slice(0, idx).trim();
                buffer = buffer.slice(idx + 1);
                if (line) {
                    try {
                        const response = JSON.parse(line);
                        if (response.ok === false) {
                            return finish(new Error(response.error ?? "viz_snapshot failed"));
                        }
                        const tasks = response.tasks;
                        if (!Array.isArray(tasks)) {
                            return finish(new Error("viz_snapshot response missing tasks"));
                        }
                        return finish(null, { tasks });
                    }
                    catch (err) {
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
/**
 * Bounded poller: fixed interval, no overlapping in-flight requests, and a
 * change guard so identical snapshots are never re-delivered.
 */
export class VizPoller {
    fetcher;
    onChange;
    intervalMs;
    timer = null;
    inFlight = false;
    lastJson = null;
    stopped = true;
    constructor(fetcher, onChange, intervalMs = 5000) {
        this.fetcher = fetcher;
        this.onChange = onChange;
        this.intervalMs = intervalMs;
    }
    start() {
        if (!this.stopped)
            return;
        this.stopped = false;
        this.timer = setInterval(() => void this.refresh(), this.intervalMs);
        void this.refresh();
    }
    stop() {
        this.stopped = true;
        if (this.timer) {
            clearInterval(this.timer);
            this.timer = null;
        }
    }
    /** One immediate bounded fetch; safe to call while a fetch is in flight. */
    async refresh() {
        if (this.inFlight || this.stopped)
            return null;
        this.inFlight = true;
        try {
            const snapshot = await this.fetcher();
            const json = JSON.stringify(snapshot);
            if (json !== this.lastJson) {
                this.lastJson = json;
                this.onChange(snapshot);
            }
            return snapshot;
        }
        catch {
            // Silent degrade: an offline daemon is a normal state (the panel falls
            // back to ASCII), never an error surface inside pi.
            return null;
        }
        finally {
            this.inFlight = false;
        }
    }
}
/**
 * Snapshot-with-fallback: try the daemon socket; on any failure (offline,
 * timeout, error response) shell `wg viz --all --no-tui` and surface the same
 * ASCII the static viz prints. The fallback is a read-only verb.
 */
export async function vizSnapshotWithFallback(backend, env, opts = {}) {
    const resolved = resolveSocketPath(env);
    if (resolved.socket) {
        try {
            const snapshot = await fetchVizSnapshot(resolved.socket, opts);
            return { kind: "snapshot", snapshot, source: resolved.source };
        }
        catch {
            // fall through to the ASCII fallback
        }
    }
    const r = await backend.run(["viz", "--all", "--no-tui"], { signal: opts.signal });
    return { kind: "ascii", text: (r.stdout || r.stderr || "").trimEnd() };
}
//# sourceMappingURL=viz-snapshot.js.map