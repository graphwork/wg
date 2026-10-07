/**
 * wg-backend.ts — the bridge between the pi session and the WG task graph.
 *
 * Today every call shells out to the `wg` binary via `pi.exec("wg", …)`
 * (works in every pi mode and every topology). The class is intentionally
 * small and dependency-free so it can later be swapped for a daemon-IPC
 * client (talking to `WG_DAEMON_SOCKET`) without touching the tool/command
 * surface that depends on it — see integration-plan-v2.md §2 / plugin-research.md §4.4.
 */

import type { ExecOptions, ExecResult } from "@earendil-works/pi-coding-agent";
import { connect, type Socket } from "node:net";
import { resolveSocketPath } from "./viz-snapshot.js";
import { normalizeStatusPalette } from "./status-palette.js";

/**
 * The slice of `ExtensionAPI` the backend needs. Declaring it structurally
 * (rather than importing the whole `ExtensionAPI`) keeps the backend trivially
 * mockable in unit tests and decouples it from the pi version.
 */
export interface ExecHost {
  exec(command: string, args: string[], options?: ExecOptions): Promise<ExecResult>;
}

/**
 * WG context handed to every pi handler via environment variables. WG already
 * exports these to its CLI handlers (integration-plan.md §1.3); the plugin
 * reads them inside the extension factory and never assumes a global daemon.
 */
export interface WgEnv {
  /** The WG task this session is bound to (`$WG_TASK_ID`). */
  taskId?: string;
  /** This agent's id (`$WG_AGENT_ID`). */
  agentId?: string;
  /**
   * Canonical graph task id for the chat this handler drives (`.chat-N`).
   * Read only from the explicit WG_CHAT_ID / WG_CHAT_REF launch contract.
   */
  chatId?: string;
  /** WG state directory (`$WG_STATE_DIR`). */
  stateDir?: string;
  /** Opaque attempt capability means the Rust CLI must choose its daemon lane. */
  workerCapability?: boolean;
  /** Visible effective trusted/scoped/read-only worker policy. */
  workerControlMode?: string;
  /** Daemon IPC socket path (`$WG_DAEMON_SOCKET`), for the future IPC client. */
  daemonSocket?: string;
  /**
   * Explicit project directory passed to every `wg` invocation as `--dir`.
   * Prefer this over cwd inference so the plugin binds to the right WG project
   * regardless of pi's cwd (cf. the global-daemon-hazard note in plugin-research.md §6.2).
   */
  dir?: string;
}

function firstNonEmpty(...vals: Array<string | undefined>): string | undefined {
  for (const v of vals) {
    if (v != null && v.trim() !== "") return v.trim();
  }
  return undefined;
}

/**
 * Normalize only the explicit chat launch contract into a graph task id.
 *
 * WG_CHAT_ID is canonical (`.chat-N`; legacy `.coordinator-N` remains
 * addressable for migrated graphs). WG_CHAT_REF is the supported session alias
 * (`chat-N` / `coordinator-N`). Deliberately do not consult WG_TASK_ID, cwd,
 * WG_DIR, session id, or any other ambient state: a standalone pi process in a
 * WG checkout is not thereby a managed WG chat.
 */
export function canonicalChatId(
  env: Record<string, string | undefined> = process.env,
): string | undefined {
  const raw = firstNonEmpty(env.WG_CHAT_ID, env.WG_CHAT_REF);
  if (!raw) return undefined;
  if (/^\.(?:chat|coordinator)-\d+$/.test(raw)) return raw;
  const alias = raw.match(/^(chat|coordinator)-(\d+)$/);
  return alias ? `.${alias[1]}-${alias[2]}` : undefined;
}

/** Read the WG context from the process environment (or an injected map for tests). */
export function readWgEnv(env: Record<string, string | undefined> = process.env): WgEnv {
  return {
    taskId: firstNonEmpty(env.WG_TASK_ID),
    agentId: firstNonEmpty(env.WG_AGENT_ID),
    chatId: canonicalChatId(env),
    // WG_STATE_DIR is the spec'd name (forward-looking); WG_PROJECT_ROOT /
    // WG_GLOBAL_DIR are what WG exports today.
    stateDir: firstNonEmpty(env.WG_STATE_DIR, env.WG_PROJECT_ROOT, env.WG_GLOBAL_DIR),
    workerCapability: firstNonEmpty(env.WG_WORKER_CAPABILITY) !== undefined,
    workerControlMode: firstNonEmpty(env.WG_WORKER_CONTROL_MODE),
    // Forward-looking: the daemon IPC socket for the future direct-IPC client.
    daemonSocket: firstNonEmpty(env.WG_DAEMON_SOCKET),
    // The explicit project dir passed to every `wg` call as `--dir`. WG_DIR is
    // what WG exports today; WG_PROJECT_DIR / WG_PROJECT_ROOT are fallbacks.
    dir: firstNonEmpty(env.WG_DIR, env.WG_PROJECT_DIR, env.WG_PROJECT_ROOT),
  };
}

export interface WgRunOptions {
  signal?: AbortSignal;
  /** Append `--json` (only for verbs that support it). */
  json?: boolean;
}

/**
 * Thin client over the `wg` CLI. Every method returns the raw {@link ExecResult}
 * so tools can surface stdout/stderr/exit-code faithfully; helpers that parse
 * JSON are layered on top.
 */
export class WgBackend {
  constructor(
    private readonly host: ExecHost,
    public readonly env: WgEnv,
  ) {}

  /** `--dir <project>` prefix applied to every invocation when known. */
  private baseArgs(): string[] {
    // A managed worker is graph-bound by its opaque daemon capability. Passing
    // a guessed/raw --dir would defeat that handshake and is refused by WG.
    return this.env.dir && !this.env.workerCapability ? ["--dir", this.env.dir] : [];
  }

  /** Run an arbitrary `wg` sub-command. Callers pass verb + args; we add `--dir`. */
  async run(args: string[], opts: WgRunOptions = {}): Promise<ExecResult> {
    const full = [...this.baseArgs(), ...args];
    if (opts.json) full.push("--json");
    return this.host.exec("wg", full, { signal: opts.signal });
  }

  /** Run a verb and JSON-parse stdout, tolerating empty / non-JSON output. */
  async runJson<T = unknown>(args: string[], opts: WgRunOptions = {}): Promise<T | null> {
    const r = await this.run(args, { ...opts, json: true });
    const out = r.stdout.trim();
    if (!out) return null;
    try {
      return JSON.parse(out) as T;
    } catch {
      return null;
    }
  }

  // ── task verbs ──────────────────────────────────────────────────────────

  capabilities(opts: WgRunOptions = {}): Promise<ExecResult> {
    return this.run(["capabilities"], { ...opts, json: true });
  }

  ready(opts: WgRunOptions = {}): Promise<ExecResult> {
    return this.run(["ready"], { ...opts, json: true });
  }

  readyJson<T = unknown>(opts: WgRunOptions = {}): Promise<T | null> {
    return this.runJson<T>(["ready"], opts);
  }

  show(id: string, opts: WgRunOptions = {}): Promise<ExecResult> {
    return this.run(["show", id], { ...opts, json: true });
  }

  add(title: string, extra: string[] = [], opts: WgRunOptions = {}): Promise<ExecResult> {
    return this.run(["add", title, ...extra], opts);
  }

  publish(id: string, opts: WgRunOptions = {}): Promise<ExecResult> {
    return this.run(["publish", id, "--only"], opts);
  }

  done(id: string, opts: WgRunOptions = {}): Promise<ExecResult> {
    return this.run(["done", id], opts);
  }

  fail(id: string, reason: string, opts: WgRunOptions = {}): Promise<ExecResult> {
    return this.run(["fail", id, "--reason", reason], opts);
  }

  log(id: string, message: string, opts: WgRunOptions = {}): Promise<ExecResult> {
    return this.run(["log", id, message], opts);
  }

  wait(until: string, checkpoint?: string, opts: WgRunOptions = {}): Promise<ExecResult> {
    if (!this.env.taskId) throw new Error("wg_wait requires a managed WG task");
    const args = ["wait", this.env.taskId, "--until", until];
    if (checkpoint) args.push("--checkpoint", checkpoint);
    return this.run(args, opts);
  }

  landingTurn(
    action: "request" | "status" | "renew" | "release" | "cancel",
    integrationRef: string,
    progress?: string,
    checkpoint?: string,
    opts: WgRunOptions = {},
  ): Promise<ExecResult> {
    if (!this.env.taskId) throw new Error("wg_landing_turn requires a managed WG task");
    const args =
      action === "status"
        ? ["landing-turn", "status", integrationRef, "--task", this.env.taskId]
        : ["landing-turn", action, this.env.taskId, "--integration-ref", integrationRef];
    if (progress) args.push("--progress", progress);
    if (checkpoint) args.push("--checkpoint", checkpoint);
    return this.run(args, opts);
  }

  // ── messaging verbs ─────────────────────────────────────────────────────

  msgSend(target: string, message: string, opts: WgRunOptions = {}): Promise<ExecResult> {
    return this.run(["msg", "send", target, message], opts);
  }

  msgRead(target: string, agent?: string, opts: WgRunOptions = {}): Promise<ExecResult> {
    const args = ["msg", "read", target];
    if (agent) args.push("--agent", agent);
    return this.run(args, { ...opts, json: true });
  }

  // ── model bridge ────────────────────────────────────────────────────────

  /** True only when this process was explicitly launched for a WG chat. */
  hasChatContext(): boolean {
    return this.env.chatId !== undefined;
  }

  /**
   * Persist a pi-native warm model choice into the managed chat override.
   *
   * Standalone pi sessions are a normal topology, not an error. The event
   * boundary checks `hasChatContext()` and this backend repeats the guard so a
   * future caller cannot accidentally mutate a graph or produce stack noise.
   * A managed failure remains an error: `pi.exec` resolves non-zero exits, so
   * inspect the code and reject with one bounded, actionable line.
   */
  async setModelOverride(
    spec: string,
    chatRef?: string,
    opts: WgRunOptions = {},
  ): Promise<ExecResult | null> {
    const chat = chatRef
      ? canonicalChatId({ WG_CHAT_ID: chatRef })
      : this.env.chatId;
    if (!chat) return null;

    const r = await this.run(
      ["chat", "model", chat, spec, "--warm-pi-writeback"],
      opts,
    );
    if (r.code !== 0) {
      // Clap/IPC errors can be multi-line. One line is enough here; the full
      // command names the exact target and can be rerun directly by the user.
      const detail = (r.stderr || r.stdout)
        .split(/\r?\n/)
        .map((line) => line.trim())
        .find(Boolean);
      throw new Error(
        `model override for ${chat} failed (wg exit ${r.code})` +
          (detail ? `: ${detail}` : "; rerun `wg chat model` for details"),
      );
    }
    return r;
  }

  // ── daemon read surface (GetFleet) ──────────────────────────────────────

  /**
   * Read the bounded fleet snapshot: revision + task rows + agent rows (with a
   * current-activity step) + aggregate counts.
   *
   * The daemon path is tried first (the read-only `get_fleet` IPC request over
   * `WG_DAEMON_SOCKET` or the standard `<wg-dir>/service/daemon.sock`). On ANY
   * error — daemon down, connect/timeout, error response, or a protocol
   * mismatch — it falls back to the **existing CLI path** (`wg list --json` +
   * `wg agents --json` + `wg ready --json`), so nothing regresses when no
   * daemon is running. Returns `null` only when neither source produced data.
   *
   * Strictly read-only in both paths: it never mutates graph state.
   */
  async getFleet(opts: GetFleetOptions = {}): Promise<GetFleetSnapshot | null> {
    const resolved = resolveSocketPath(this.env);
    if (resolved.socket) {
      try {
        const raw = await ipcRoundTrip(
          resolved.socket,
          "get_fleet",
          {
            cmd: "get_fleet",
            since_revision: opts.sinceRevision,
            max_rows: opts.maxRows,
            include_tree: opts.includeTree ?? false,
            tree_columns: opts.treeColumns,
          },
          opts.timeoutMs ?? 2000,
          opts.signal,
        );
        // A well-formed get_fleet response MUST carry counts + a task array.
        // Anything else is a protocol mismatch (an older daemon) → CLI.
        if (!isRecord(raw) || !isRecord(raw.counts) || !Array.isArray(raw.tasks)) {
          throw new Error("get_fleet protocol mismatch");
        }
        const snapshot = normalizeGetFleet(raw, "daemon");
        // A daemon that predates the `tree` field still satisfies the protocol;
        // when the caller asked for the rendered tree, back-fill it from the
        // read-only CLI (`wg viz --json`) rather than dropping to the fallback.
        if (opts.includeTree && !snapshot.tree) {
          const cli = await this.getFleetViaCli(opts).catch(() => null);
          if (cli?.tree) snapshot.tree = cli.tree;
        }
        return snapshot;
      } catch {
        // Any daemon failure degrades to the CLI path below.
      }
    }
    return this.getFleetViaCli(opts);
  }

  /** The safe default: derive the same fleet shape from read-only CLI verbs. */
  private async getFleetViaCli(opts: GetFleetOptions): Promise<GetFleetSnapshot | null> {
    try {
      const vizArgs = ["viz", "--json"];
      if (typeof opts.treeColumns === "number" && Number.isFinite(opts.treeColumns)) {
        vizArgs.push("--columns", String(Math.trunc(opts.treeColumns)));
      }
      const [listRes, agentsRes, readyRes, vizRes] = await Promise.all([
        this.run(["list"], { json: true, signal: opts.signal }),
        this.run(["agents"], { json: true, signal: opts.signal }),
        this.run(["ready"], { json: true, signal: opts.signal }),
        // Only render WG's tree on the CLI fallback when a caller asked for it.
        opts.includeTree
          ? this.run(vizArgs, { signal: opts.signal }).catch(() => null)
          : Promise.resolve(null),
      ]);
      const tasksRaw = parseJsonArray(listRes.stdout);
      const agentsRaw = parseJsonArray(agentsRes.stdout);
      const readyRaw = parseJsonArray(readyRes.stdout);
      if (!tasksRaw && !agentsRaw && !readyRaw) return null;
      const tree = vizRes ? parseVizJson(vizRes.stdout) : undefined;
      return buildCliFleet(tasksRaw ?? [], agentsRaw ?? [], readyRaw ?? [], tree);
    } catch {
      return null;
    }
  }

  // ── daemon read surface (GetTaskDetail) ─────────────────────────────────

  /**
   * Read WG's OWN task-detail text — the exact body `wg show <task>` prints —
   * so the fleet panel's detail view renders WG's sections/ordering/wording
   * verbatim instead of a client-side approximation.
   *
   * The daemon path is tried first (the read-only `get_task_detail` IPC request
   * over `WG_DAEMON_SOCKET` / `<wg-dir>/service/daemon.sock`). On ANY error —
   * daemon down, connect/timeout, error response, unknown task, or a protocol
   * mismatch (an older daemon without the request) — it falls back to running
   * the read-only CLI `wg show <task>`, whose stdout IS the same text. Returns
   * `null` only when neither source produced text.
   *
   * Strictly read-only in both paths: it never mutates graph state.
   */
  async getTaskDetail(
    taskId: string,
    opts: GetTaskDetailOptions = {},
  ): Promise<GetTaskDetail | null> {
    if (!taskId || taskId.trim() === "") return null;
    const resolved = resolveSocketPath(this.env);
    if (resolved.socket) {
      try {
        const raw = await ipcRoundTrip(
          resolved.socket,
          "get_task_detail",
          {
            cmd: "get_task_detail",
            task_id: taskId,
            columns: opts.columns,
          },
          opts.timeoutMs ?? 2000,
          opts.signal,
        );
        if (!isRecord(raw) || typeof raw.text !== "string") {
          throw new Error("get_task_detail protocol mismatch");
        }
        return {
          task_id: typeof raw.task_id === "string" ? raw.task_id : taskId,
          text: raw.text,
          source: "daemon",
        };
      } catch {
        // Any daemon failure degrades to the CLI path below.
      }
    }
    return this.getTaskDetailViaCli(taskId, opts);
  }

  /** The safe default: `wg show <task>` prints WG's own detail text directly. */
  private async getTaskDetailViaCli(
    taskId: string,
    opts: GetTaskDetailOptions,
  ): Promise<GetTaskDetail | null> {
    try {
      const res = await this.run(["show", taskId], { signal: opts.signal });
      const text = res.stdout ?? "";
      if (!text.trim()) return null;
      return { task_id: taskId, text, source: "cli" };
    } catch {
      return null;
    }
  }
}

// ── GetFleet read surface ───────────────────────────────────────────────────

/** Aggregate token usage summary on a task row. */
export interface GetFleetTokenSummary {
  input_tokens?: number;
  output_tokens?: number;
  total_tokens?: number;
  cost_usd?: number;
}

/** One task row of the bounded fleet snapshot. */
export interface GetFleetTaskRow {
  id: string;
  title: string;
  status: string;
  presentation?: string;
  assigned?: string | null;
  model?: string | null;
  parent?: string | null;
  depends_on?: string[];
  dependency_count?: number;
  paused?: boolean;
  age_secs?: number | null;
  started_at?: string | null;
  completed_at?: string | null;
  last_interaction_at?: string | null;
  token_usage?: GetFleetTokenSummary | null;
  failure_reason?: string | null;
}

/** One runtime worker row, with a current-activity step. */
export interface GetFleetAgentRow {
  id: string;
  task_id: string;
  executor?: string | null;
  model?: string | null;
  status: string;
  started_at?: string | null;
  elapsed_ms?: number | null;
  activity?: string | null;
}

/**
 * WG's OWN rendered `wg viz` tree, emitted by the `GetFleet` read so the panel
 * renders WG's structure verbatim (top-level rows unindented, `└→` edges with
 * WG's own 2-space-per-depth prefix) instead of re-deriving it in TypeScript.
 *
 * `text` is the exact plain-text ASCII `wg viz` prints; `node_lines` maps a
 * task id to the rendered line index that shows it (for per-line status colour).
 */
export interface GetFleetTree {
  text: string;
  node_lines: Record<string, number>;
  /**
   * WG's canonical graph-view status palette (`status -> [r, g, b]`), exported
   * so the panel paints exact RGB rather than a semantic theme role. Absent on
   * older daemons/CLIs, in which case the panel visibly falls back.
   */
  palette?: Record<string, [number, number, number]>;
}

/** Aggregate counts rendered as the fleet header. */
export interface GetFleetCounts {
  in_progress: number;
  ready: number;
  blocked: number;
  done: number;
  failed?: number;
  total?: number;
  active_agents?: number;
}

/** The bounded fleet snapshot returned by {@link WgBackend.getFleet}. */
export interface GetFleetSnapshot {
  /** Graph revision token (`<identity>#<digest>`); empty on the CLI path. */
  revision: string;
  /** True when the caller's `since_revision` matched (delta: counts only). */
  unchanged: boolean;
  counts: GetFleetCounts;
  tasks: GetFleetTaskRow[];
  agents: GetFleetAgentRow[];
  graph?: { identity?: string | null; task_count?: number; agent_count?: number };
  truncated?: boolean;
  /** WG's own rendered `wg viz` tree (present when requested). */
  tree?: GetFleetTree;
  /** Which source produced the snapshot. */
  source: "daemon" | "cli";
}

export interface GetFleetOptions {
  /** Echo the previous revision to request a bounded delta (counts only). */
  sinceRevision?: string;
  /** Max rows per section (server clamps to 1..=2000). */
  maxRows?: number;
  /**
   * Ask the daemon to include WG's own rendered `wg viz` tree
   * (`tree.text` + `tree.node_lines`). Off by default so the ambient widget
   * (which renders only counts/agent rows) never pays for the tree render.
   */
  includeTree?: boolean;
  /** Panel width in columns for the rendered tree (server clamps 20..=400). */
  treeColumns?: number;
  /** Per-request deadline for the daemon path. Default 2000ms. */
  timeoutMs?: number;
  signal?: AbortSignal;
}

/**
 * WG's OWN task-detail text (the exact body `wg show <task>` prints), served by
 * the daemon's read-only `get_task_detail` request (or the CLI `wg show`
 * fallback). `text` is rendered verbatim by the fleet panel's detail view.
 */
export interface GetTaskDetail {
  task_id: string;
  text: string;
  source: "daemon" | "cli";
}

export interface GetTaskDetailOptions {
  /** Panel width in columns; the daemon word-wraps the body to it (20..=400). */
  columns?: number;
  /** Per-request deadline for the daemon path. Default 2000ms. */
  timeoutMs?: number;
  signal?: AbortSignal;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseJsonArray(out: string | undefined): unknown[] | null {
  try {
    const parsed = JSON.parse((out ?? "").trim());
    return Array.isArray(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

function num(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function str(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

/**
 * One-shot daemon IPC round trip for a read-only request, mirroring the Rust
 * client's one-request-per-connection contract. Any transport error (connect
 * refused, timeout, error response, malformed JSON) rejects so the caller can
 * fall back to the CLI.
 */
function ipcRoundTrip(
  socketPath: string,
  label: string,
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
    const timer = setTimeout(
      () => finish(new Error(`${label} timed out after ${timeoutMs}ms`)),
      timeoutMs,
    );
    const onAbort = () => finish(new Error(`${label} aborted`));
    if (signal) {
      if (signal.aborted) return finish(new Error(`${label} aborted`));
      signal.addEventListener("abort", onAbort, { once: true });
    }

    socket = connect(socketPath, () => {
      socket?.write(`${payload}\n`);
    });
    socket.once("error", (err: Error) => finish(err));
    socket.on("data", (chunk: Buffer) => {
      buffer += chunk.toString("utf8");
      const idx = buffer.indexOf("\n");
      if (idx < 0) return;
      const line = buffer.slice(0, idx).trim();
      if (!line) return;
      try {
        const response = JSON.parse(line) as Record<string, unknown> & {
          ok?: boolean;
          error?: string;
        };
        if (response.ok === false) {
          return finish(new Error(response.error ?? `${label} failed`));
        }
        return finish(null, response);
      } catch (err) {
        return finish(err instanceof Error ? err : new Error(String(err)));
      }
    });
    socket.once("close", () => {
      if (!settled && !buffer.trim()) {
        finish(new Error("daemon closed the connection without a response"));
      }
    });
  });
}

function normalizeTaskRow(raw: unknown): GetFleetTaskRow | null {
  if (!isRecord(raw) || typeof raw.id !== "string" || typeof raw.status !== "string") {
    return null;
  }
  return {
    id: raw.id,
    title: typeof raw.title === "string" ? raw.title : raw.id,
    status: raw.status,
    presentation: str(raw.presentation) ?? undefined,
    assigned: str(raw.assigned),
    model: str(raw.model),
    parent: str(raw.parent),
    depends_on: Array.isArray(raw.depends_on)
      ? raw.depends_on.filter((d): d is string => typeof d === "string")
      : [],
    dependency_count: num(raw.dependency_count),
    paused: raw.paused === true,
    age_secs: typeof raw.age_secs === "number" ? raw.age_secs : null,
    started_at: str(raw.started_at),
    completed_at: str(raw.completed_at),
    last_interaction_at: str(raw.last_interaction_at),
    token_usage: isRecord(raw.token_usage)
      ? {
          input_tokens: num(raw.token_usage.input_tokens),
          output_tokens: num(raw.token_usage.output_tokens),
          total_tokens: num(raw.token_usage.total_tokens),
          cost_usd: num(raw.token_usage.cost_usd),
        }
      : null,
    failure_reason: str(raw.failure_reason),
  };
}

function normalizeAgentRow(raw: unknown): GetFleetAgentRow | null {
  if (!isRecord(raw) || typeof raw.id !== "string") return null;
  const status = typeof raw.status === "string" ? raw.status.split(" ")[0] ?? raw.status : "unknown";
  return {
    id: raw.id,
    task_id: typeof raw.task_id === "string" ? raw.task_id : "",
    executor: str(raw.executor),
    model: str(raw.model),
    status,
    started_at: str(raw.started_at),
    elapsed_ms: typeof raw.elapsed_ms === "number" ? raw.elapsed_ms : null,
    activity: str(raw.activity),
  };
}

/** Normalize a decoded `get_fleet` body into the backend's snapshot shape. */
export function normalizeGetFleet(raw: unknown, source: "daemon" | "cli" = "daemon"): GetFleetSnapshot {
  const r = isRecord(raw) ? raw : {};
  const counts = isRecord(r.counts) ? r.counts : {};
  const graph = isRecord(r.graph) ? r.graph : undefined;
  return {
    revision: typeof r.revision === "string" ? r.revision : "",
    unchanged: r.unchanged === true,
    counts: {
      in_progress: num(counts.in_progress),
      ready: num(counts.ready),
      blocked: num(counts.blocked),
      done: num(counts.done),
      failed: num(counts.failed),
      total: num(counts.total),
      active_agents: num(counts.active_agents),
    },
    tasks: Array.isArray(r.tasks)
      ? r.tasks.map(normalizeTaskRow).filter((t): t is GetFleetTaskRow => t !== null)
      : [],
    agents: Array.isArray(r.agents)
      ? r.agents.map(normalizeAgentRow).filter((a): a is GetFleetAgentRow => a !== null)
      : [],
    graph: graph
      ? {
          identity: str(graph.identity),
          task_count: num(graph.task_count),
          agent_count: num(graph.agent_count),
        }
      : undefined,
    truncated: r.truncated === true,
    tree: normalizeTree(r.tree),
    source,
  };
}

/** Normalize a decoded `tree` payload into WG's rendered-tree shape. */
function normalizeTree(raw: unknown): GetFleetTree | undefined {
  if (!isRecord(raw) || typeof raw.text !== "string") return undefined;
  const nodeLines: Record<string, number> = {};
  if (isRecord(raw.node_lines)) {
    for (const [id, line] of Object.entries(raw.node_lines)) {
      if (typeof line === "number" && Number.isFinite(line)) nodeLines[id] = line;
    }
  }
  const tree: GetFleetTree = { text: raw.text, node_lines: nodeLines };
  const palette = normalizeStatusPalette(raw.palette);
  if (Object.keys(palette).length > 0) tree.palette = palette;
  return tree;
}

/**
 * Parse the JSON emitted by `wg viz --json` into the panel's rendered-tree
 * shape. This is the CLI fallback for the daemon's `GetFleet.tree` field — the
 * bytes are still WG's own renderer output, so structure stays identical.
 */
export function parseVizJson(out: string | undefined): GetFleetTree | undefined {
  try {
    const parsed = JSON.parse((out ?? "").trim());
    if (!isRecord(parsed) || typeof parsed.text !== "string") return undefined;
    return normalizeTree(parsed);
  } catch {
    return undefined;
  }
}

const TERMINAL_AGENT_STATUSES = new Set(["done", "failed", "dead"]);

function agentAlive(status: string, processAlive: unknown): boolean {
  if (typeof processAlive === "boolean") {
    return processAlive && !TERMINAL_AGENT_STATUSES.has(status);
  }
  return !TERMINAL_AGENT_STATUSES.has(status);
}

/**
 * Build the fleet snapshot from the read-only CLI verbs (the safe fallback):
 * `wg list --json` (task rows + status counts), `wg ready --json` (ready
 * count), `wg agents --json` (runtime worker rows). No revision is available
 * on this path, so `revision` is empty and `unchanged` is always false.
 */
export function buildCliFleet(
  rawTasks: unknown[],
  rawAgents: unknown[],
  rawReady: unknown[],
  tree?: GetFleetTree,
): GetFleetSnapshot {
  const tasks = rawTasks
    .map(normalizeTaskRow)
    .filter((t): t is GetFleetTaskRow => t !== null);
  const agents = rawAgents
    .map(normalizeAgentRow)
    .filter((a): a is GetFleetAgentRow => a !== null);
  const counts: GetFleetCounts = {
    in_progress: 0,
    ready: rawReady.filter((r) => !isRecord(r) || r.ready !== false).length,
    blocked: 0,
    done: 0,
    failed: 0,
    total: tasks.length,
    active_agents: 0,
  };
  for (const task of tasks) {
    switch (task.status) {
      case "in-progress":
        counts.in_progress += 1;
        break;
      case "blocked":
        counts.blocked += 1;
        break;
      case "done":
        counts.done += 1;
        break;
      case "failed":
      case "abandoned":
      case "failed-pending-eval":
        counts.failed = (counts.failed ?? 0) + 1;
        break;
      default:
        break;
    }
  }
  for (const raw of rawAgents) {
    if (!isRecord(raw) || typeof raw.status !== "string") continue;
    const status = raw.status.split(" ")[0] ?? raw.status;
    if (agentAlive(status, raw.process_alive)) counts.active_agents = (counts.active_agents ?? 0) + 1;
  }
  return {
    revision: "",
    unchanged: false,
    counts,
    tasks,
    agents,
    tree,
    source: "cli",
  };
}
