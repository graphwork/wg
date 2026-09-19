/**
 * completion-watcher.ts — tell a live Pi session when the WG graph changes.
 *
 * The plugin is pull-only today: `wg_*` tools shell `wg` on demand, `wg msg` is
 * daemon-side, and the VizView panel is a view. This module adds a **wake**: a
 * bounded poll of the graph that emits one actionable message when a task
 * reaches a terminal (or needs-attention) transition.
 *
 * Design: `docs/design-pi-completion-wakeups.md`.
 *
 * Read-only by construction. Graph state is read through the existing
 * {@link WgBackend} (`wg list --json` + `wg show <id> --json`), so no daemon
 * protocol change is needed for this slice. The daemon-push/event-stream
 * follow-up is documented in the design doc §6.1.
 */
import { rename, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
export const DEFAULT_COMPLETION_WAKE_CONFIG = {
    enabled: true,
    failures: true,
    completions: "top-level",
    attention: "top-level",
    quiet: false,
    intervalMs: 15_000,
};
const FAILURE_STATUSES = new Set(["failed", "abandoned"]);
const COMPLETION_STATUSES = new Set(["done"]);
const ATTENTION_STATUSES = new Set(["blocked", "waiting", "incomplete"]);
/** Internal/plumbing ids (`.chat-N`, `.evaluate-*`, `.flip-*`, `.assign-*`, …). */
export function isInternalTask(id) {
    return id.startsWith(".");
}
/**
 * A task is top-level when it has no prerequisite inside the snapshot — a root
 * of the dependency tree the VizView panel renders. A user's headline task is
 * created first and subtasks are added `--after <headline>`, so the headline is
 * the root and subtask churn stays quiet by default.
 */
export function isTopLevelTask(task, byId) {
    return (task.after ?? []).every((dep) => !byId.has(dep));
}
function scopeAllows(scope, topLevel) {
    if (scope === "all")
        return true;
    if (scope === "top-level")
        return topLevel;
    return false;
}
function kindOf(status) {
    if (FAILURE_STATUSES.has(status))
        return "failed";
    if (COMPLETION_STATUSES.has(status))
        return "completed";
    if (ATTENTION_STATUSES.has(status))
        return "attention";
    return null;
}
/**
 * Pure transition detection. `prev` is the previous `taskId → status` map, or
 * `null` for a fresh baseline (which emits nothing — attaching to a
 * long-running graph must not replay history).
 */
export function planWakes(prev, tasks, config) {
    const byId = new Map();
    const nextStatuses = {};
    for (const task of tasks) {
        if (!task || typeof task.id !== "string")
            continue;
        byId.set(task.id, task);
        if (!isInternalTask(task.id))
            nextStatuses[task.id] = task.status;
    }
    if (prev === null)
        return { wakes: [], nextStatuses };
    if (!config.enabled)
        return { wakes: [], nextStatuses };
    const wakes = [];
    for (const id of Object.keys(nextStatuses).sort()) {
        const task = byId.get(id);
        if (!task)
            continue;
        const previous = prev[id];
        if (previous === task.status)
            continue;
        const kind = kindOf(task.status);
        if (!kind)
            continue;
        const topLevel = isTopLevelTask(task, byId);
        const gate = kind === "failed"
            ? config.failures
            : kind === "completed"
                ? scopeAllows(config.completions, topLevel)
                : scopeAllows(config.attention, topLevel);
        if (!gate || config.quiet)
            continue;
        wakes.push({
            taskId: id,
            title: task.title ?? id,
            kind,
            from: previous ?? "(new)",
            to: task.status,
            topLevel,
        });
    }
    return { wakes, nextStatuses };
}
/** Render one actionable wake. Never a bare "something changed". */
export function formatWakeMessage(wake, detail = null) {
    const glyph = wake.kind === "failed" ? "✗" : wake.kind === "completed" ? "✓" : "⏸";
    const label = wake.kind === "failed" ? "failed" : wake.kind === "completed" ? "completed" : "needs attention";
    const lines = [`[WG] ${glyph} ${wake.taskId} ${label} (${wake.from} → ${wake.to})`];
    lines.push(`Task: ${wake.title}`);
    if (wake.kind === "failed") {
        if (detail?.failure_reason)
            lines.push(`Reason: ${detail.failure_reason}`);
    }
    else if (wake.kind === "completed") {
        if (detail?.completion_disposition) {
            lines.push(`Summary: landed (${detail.completion_disposition})`);
        }
        else if (detail?.last_log) {
            lines.push(`Summary: ${detail.last_log}`);
        }
        if (detail?.completion_receipt)
            lines.push(`Receipt: ${detail.completion_receipt}`);
        if (detail?.actual_model)
            lines.push(`Model: ${detail.actual_model}`);
    }
    else if (detail?.last_log) {
        lines.push(`Summary: ${detail.last_log}`);
    }
    lines.push("Detail: call wg_show / run /wg graph, or open /wg-viz.");
    return lines.join("\n");
}
function asBool(value, fallback) {
    if (value == null || value.trim() === "")
        return fallback;
    const v = value.trim().toLowerCase();
    if (["off", "0", "false", "no"].includes(v))
        return false;
    if (["on", "1", "true", "yes"].includes(v))
        return true;
    return fallback;
}
function asScope(value, fallback) {
    return value === "all" || value === "top-level" || value === "off" ? value : fallback;
}
/** Resolve plugin-side settings from the environment with safe defaults. */
export function readCompletionWakeConfig(env = process.env) {
    const rawInterval = Number(env.WG_PI_COMPLETION_INTERVAL_MS);
    const intervalMs = Number.isFinite(rawInterval) && rawInterval >= 1000
        ? Math.floor(rawInterval)
        : DEFAULT_COMPLETION_WAKE_CONFIG.intervalMs;
    return {
        enabled: asBool(env.WG_PI_COMPLETION_WAKES, DEFAULT_COMPLETION_WAKE_CONFIG.enabled),
        failures: asBool(env.WG_PI_COMPLETION_FAILURES, DEFAULT_COMPLETION_WAKE_CONFIG.failures),
        completions: asScope(env.WG_PI_COMPLETION_COMPLETIONS, DEFAULT_COMPLETION_WAKE_CONFIG.completions),
        attention: asScope(env.WG_PI_COMPLETION_ATTENTION, DEFAULT_COMPLETION_WAKE_CONFIG.attention),
        quiet: asBool(env.WG_PI_COMPLETION_QUIET, DEFAULT_COMPLETION_WAKE_CONFIG.quiet),
        intervalMs,
    };
}
/** In-memory cursor store (tests, and sessions with no session file). */
export class MemoryCursorStore {
    cursor = null;
    async load() {
        return this.cursor;
    }
    async save(cursor) {
        this.cursor = cursor;
    }
}
/**
 * Atomic JSON sidecar store. Missing or corrupt files load as `null` (a fresh
 * baseline), never an error.
 */
export function fileCursorStore(path) {
    return {
        async load() {
            try {
                const text = await readFile(path, "utf8");
                const parsed = JSON.parse(text);
                if (parsed && parsed.version === 1 && parsed.statuses && typeof parsed.statuses === "object") {
                    return { version: 1, initialized: parsed.initialized === true, statuses: parsed.statuses };
                }
                return null;
            }
            catch {
                return null;
            }
        },
        async save(cursor) {
            const tmp = `${path}.tmp`;
            await writeFile(tmp, JSON.stringify(cursor), { encoding: "utf8", mode: 0o600 });
            await rename(tmp, path);
        },
    };
}
/**
 * Bounded poller: fixed interval, at most one in-flight read, and a
 * status-map cursor so an unchanged graph never re-announces a transition.
 *
 * Emit-before-persist is deliberate: a crash in between can duplicate a wake
 * (visible, harmless) but can never silently drop one.
 */
export class CompletionWatcher {
    readTasks;
    store;
    deliver;
    config;
    enrich;
    signal;
    timer = null;
    inFlight = false;
    stopped = true;
    cursor = null;
    lastPersisted = null;
    constructor(readTasks, store, deliver, config, enrich, signal) {
        this.readTasks = readTasks;
        this.store = store;
        this.deliver = deliver;
        this.config = config;
        this.enrich = enrich;
        this.signal = signal;
    }
    /** Load the persisted cursor and start the bounded poll loop. */
    async start() {
        if (!this.stopped)
            return;
        this.stopped = false;
        await this.hydrate();
        if (this.stopped)
            return;
        this.timer = setInterval(() => void this.refresh(), this.config.intervalMs);
        await this.refresh();
    }
    stop() {
        this.stopped = true;
        if (this.timer) {
            clearInterval(this.timer);
            this.timer = null;
        }
    }
    /** One immediate bounded read; never overlaps with an in-flight read. */
    async refresh() {
        if (this.inFlight)
            return [];
        this.inFlight = true;
        try {
            const tasks = await this.readTasks(this.signal);
            if (!tasks)
                return [];
            const { wakes, nextStatuses } = planWakes(this.cursor, tasks, this.config);
            this.cursor = nextStatuses;
            for (const wake of wakes) {
                let detail = null;
                if (this.enrich) {
                    try {
                        detail = await this.enrich(wake.taskId);
                    }
                    catch {
                        detail = null;
                    }
                }
                try {
                    await this.deliver(wake, formatWakeMessage(wake, detail), detail);
                }
                catch {
                    // A wake must never throw into the session; keep polling.
                }
            }
            await this.persist();
            return wakes;
        }
        catch {
            // Offline daemon / transient CLI failure is a normal state: keep the
            // cursor unchanged so the transition is not lost.
            return [];
        }
        finally {
            this.inFlight = false;
        }
    }
    async hydrate() {
        try {
            const loaded = await this.store.load();
            if (loaded && loaded.initialized) {
                this.cursor = loaded.statuses;
                this.lastPersisted = JSON.stringify(loaded.statuses);
            }
        }
        catch {
            this.cursor = null;
        }
    }
    async persist() {
        if (this.cursor === null)
            return;
        const json = JSON.stringify(this.cursor);
        if (json === this.lastPersisted)
            return;
        this.lastPersisted = json;
        try {
            await this.store.save({ version: 1, initialized: true, statuses: this.cursor });
        }
        catch {
            // Best-effort persistence; a failed save may duplicate a future wake.
        }
    }
}
/** `wg show <id> --json` → the bounded detail a wake can cite. */
export async function readTaskDetail(backend, id) {
    const d = await backend.runJson(["show", id]);
    if (!d || typeof d !== "object")
        return null;
    const log = Array.isArray(d.log) ? d.log : [];
    const last = log.length ? log[log.length - 1] : null;
    const str = (v) => (typeof v === "string" && v.trim() ? v : null);
    return {
        failure_reason: str(d.failure_reason),
        completion_receipt: str(d.completion_receipt),
        completion_disposition: str(d.completion_disposition),
        completed_at: str(d.completed_at),
        actual_model: str(d.actual_model),
        last_log: last ? str(last.message) : null,
    };
}
/** Per-session cursor location: the session-file sidecar, else a temp sidecar. */
function resolveCursorStore(ctx) {
    try {
        const file = ctx.sessionManager?.getSessionFile?.();
        if (file)
            return fileCursorStore(`${file}.wg-wake-cursor.json`);
        const id = ctx.sessionManager?.getSessionId?.();
        if (id)
            return fileCursorStore(join(tmpdir(), `wg-pi-wake-${id}.json`));
    }
    catch {
        /* no session manager (or an older pi): fall through to memory */
    }
    return new MemoryCursorStore();
}
function configSummary(config) {
    if (!config.enabled)
        return "off (disabled by WG_PI_COMPLETION_WAKES)";
    if (config.quiet)
        return "muted for this session";
    return `on · failures always · completions ${config.completions} · attention ${config.attention} · every ${config.intervalMs}ms`;
}
/**
 * Install the completion watcher: `/wg-wake` plus the session lifecycle that
 * starts/stops a bounded poll. Wake delivery uses `pi.sendMessage` (the agent
 * sees it; `triggerTurn` when idle) with a best-effort `ctx.ui.notify` ping.
 */
export function installCompletionWatcher(pi, backend, env, options = {}) {
    void env; // the backend already carries the project dir; kept for parity
    const config = {
        ...readCompletionWakeConfig(process.env),
        ...options.config,
    };
    const readTasks = options.readTasks ?? ((b, signal) => b.runJson(["list"], { signal }));
    const deliver = options.deliver ?? ((wake, message, detail, ctx) => defaultDeliver(pi, wake, message, detail, ctx));
    let watcher = null;
    pi.registerCommand("wg-wake", {
        description: "WG completion wakeups: /wg-wake [on|off] — show or toggle the per-session mute",
        handler: async (args, ctx) => {
            const arg = args.trim().toLowerCase();
            let text;
            if (arg === "on") {
                config.quiet = false;
                text = "WG completion wakeups: on";
            }
            else if (arg === "off") {
                config.quiet = true;
                text = "WG completion wakeups: muted for this session";
            }
            else {
                text = `WG completion wakeups: ${configSummary(config)}`;
            }
            if (ctx.hasUI)
                ctx.ui.notify(text, "info");
            else
                console.log(text);
        },
    });
    pi.on("session_start", (_event, ctx) => {
        watcher?.stop();
        watcher = null;
        // Interactive/rpc sessions only: a WG worker's `json`/`print` session has
        // no conversation to wake and must not be interrupted by graph chatter.
        if (ctx.mode === "json" || ctx.mode === "print")
            return;
        if (!config.enabled)
            return;
        watcher = new CompletionWatcher((signal) => readTasks(backend, signal), resolveCursorStore(ctx), (wake, message, detail) => deliver(wake, message, detail, ctx), config, (id) => readTaskDetail(backend, id));
        void watcher.start();
    });
    pi.on("session_shutdown", () => {
        watcher?.stop();
        watcher = null;
    });
}
function defaultDeliver(pi, wake, message, _detail, ctx) {
    try {
        const idle = ctx.isIdle?.() ?? false;
        pi.sendMessage({
            customType: "wg-completion",
            content: message,
            display: true,
            details: { taskId: wake.taskId, status: wake.to, kind: wake.kind },
        }, idle ? { triggerTurn: true } : { deliverAs: "followUp" });
    }
    catch {
        /* never throw into the watcher */
    }
    try {
        if (ctx.hasUI)
            ctx.ui.notify(message, wake.kind === "failed" ? "warning" : "info");
    }
    catch {
        /* best-effort ping only */
    }
}
//# sourceMappingURL=completion-watcher.js.map