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
  /** The chat session this handler drives, if any (`$WG_CHAT_ID`). */
  chatId?: string;
  /** WG state directory (`$WG_STATE_DIR`). */
  stateDir?: string;
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
    if (v != null && v.trim() !== "") return v;
  }
  return undefined;
}

/** Read the WG context from the process environment (or an injected map for tests). */
export function readWgEnv(env: Record<string, string | undefined> = process.env): WgEnv {
  return {
    taskId: firstNonEmpty(env.WG_TASK_ID),
    agentId: firstNonEmpty(env.WG_AGENT_ID),
    // WG_CHAT_ID is the spec'd name; WG_CHAT_REF is the addressable alias WG
    // also exports — accept either.
    chatId: firstNonEmpty(env.WG_CHAT_ID, env.WG_CHAT_REF),
    // WG_STATE_DIR is the spec'd name (forward-looking); WG_PROJECT_ROOT /
    // WG_GLOBAL_DIR are what WG exports today.
    stateDir: firstNonEmpty(env.WG_STATE_DIR, env.WG_PROJECT_ROOT, env.WG_GLOBAL_DIR),
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
    return this.env.dir ? ["--dir", this.env.dir] : [];
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

  done(id: string, opts: WgRunOptions = {}): Promise<ExecResult> {
    return this.run(["done", id], opts);
  }

  fail(id: string, reason: string, opts: WgRunOptions = {}): Promise<ExecResult> {
    return this.run(["fail", id, "--reason", reason], opts);
  }

  log(id: string, message: string, opts: WgRunOptions = {}): Promise<ExecResult> {
    return this.run(["log", id, message], opts);
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

  /**
   * Persist a model choice into the chat's `CoordinatorState.model_override`
   * so a pi-native warm `setModel` survives a WG-side respawn (the identity
   * bridge, plugin-research.md §1.3).
   *
   * This shells the `wg chat model <chat> <spec>` verb, which is delivered by
   * the downstream `pi-plugin-impl-chat-model-verb` task. Until that verb
   * lands the call is a no-op-with-error at runtime, but the bridge logic
   * (model_select → write-back) is complete and unit-tested here against a
   * mocked backend.
   */
  setModelOverride(spec: string, chatRef?: string, opts: WgRunOptions = {}): Promise<ExecResult> {
    const chat = chatRef ?? this.env.chatId;
    if (!chat) {
      return Promise.reject(
        new Error("wg-pi-plugin: cannot write model override — no chat id (set $WG_CHAT_ID)"),
      );
    }
    return this.run(["chat", "model", chat, spec], opts);
  }
}
