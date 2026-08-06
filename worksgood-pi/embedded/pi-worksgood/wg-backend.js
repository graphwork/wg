/**
 * wg-backend.ts — the bridge between the pi session and the WG task graph.
 *
 * Today every call shells out to the `wg` binary via `pi.exec("wg", …)`
 * (works in every pi mode and every topology). The class is intentionally
 * small and dependency-free so it can later be swapped for a daemon-IPC
 * client (talking to `WG_DAEMON_SOCKET`) without touching the tool/command
 * surface that depends on it — see integration-plan-v2.md §2 / plugin-research.md §4.4.
 */
function firstNonEmpty(...vals) {
    for (const v of vals) {
        if (v != null && v.trim() !== "")
            return v.trim();
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
export function canonicalChatId(env = process.env) {
    const raw = firstNonEmpty(env.WG_CHAT_ID, env.WG_CHAT_REF);
    if (!raw)
        return undefined;
    if (/^\.(?:chat|coordinator)-\d+$/.test(raw))
        return raw;
    const alias = raw.match(/^(chat|coordinator)-(\d+)$/);
    return alias ? `.${alias[1]}-${alias[2]}` : undefined;
}
/** Read the WG context from the process environment (or an injected map for tests). */
export function readWgEnv(env = process.env) {
    return {
        taskId: firstNonEmpty(env.WG_TASK_ID),
        agentId: firstNonEmpty(env.WG_AGENT_ID),
        chatId: canonicalChatId(env),
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
/**
 * Thin client over the `wg` CLI. Every method returns the raw {@link ExecResult}
 * so tools can surface stdout/stderr/exit-code faithfully; helpers that parse
 * JSON are layered on top.
 */
export class WgBackend {
    host;
    env;
    constructor(host, env) {
        this.host = host;
        this.env = env;
    }
    /** `--dir <project>` prefix applied to every invocation when known. */
    baseArgs() {
        return this.env.dir ? ["--dir", this.env.dir] : [];
    }
    /** Run an arbitrary `wg` sub-command. Callers pass verb + args; we add `--dir`. */
    async run(args, opts = {}) {
        const full = [...this.baseArgs(), ...args];
        if (opts.json)
            full.push("--json");
        return this.host.exec("wg", full, { signal: opts.signal });
    }
    /** Run a verb and JSON-parse stdout, tolerating empty / non-JSON output. */
    async runJson(args, opts = {}) {
        const r = await this.run(args, { ...opts, json: true });
        const out = r.stdout.trim();
        if (!out)
            return null;
        try {
            return JSON.parse(out);
        }
        catch {
            return null;
        }
    }
    // ── task verbs ──────────────────────────────────────────────────────────
    ready(opts = {}) {
        return this.run(["ready"], { ...opts, json: true });
    }
    readyJson(opts = {}) {
        return this.runJson(["ready"], opts);
    }
    show(id, opts = {}) {
        return this.run(["show", id], { ...opts, json: true });
    }
    add(title, extra = [], opts = {}) {
        return this.run(["add", title, ...extra], opts);
    }
    publish(id, opts = {}) {
        return this.run(["publish", id, "--only"], opts);
    }
    done(id, opts = {}) {
        return this.run(["done", id], opts);
    }
    fail(id, reason, opts = {}) {
        return this.run(["fail", id, "--reason", reason], opts);
    }
    log(id, message, opts = {}) {
        return this.run(["log", id, message], opts);
    }
    // ── authoritative Pi compaction kick (managed task workers only) ────────
    async compactionKickAuthorize(request, opts = {}) {
        const task = this.env.taskId;
        if (!task)
            throw new Error("compaction kick requires a managed WG task");
        const args = [
            "pi-watchdog", "compaction-kick", "authorize", task,
            "--reason", request.reason,
            "--compaction-entry-id", request.compactionEntryId,
            "--compaction-parent-id", request.compactionParentId,
            "--session-id", request.sessionId,
            "--session-file", request.sessionFile,
            "--session-leaf-id", request.sessionLeafId,
            "--pid", String(request.pid),
            "--provider", request.provider,
            "--model", request.model,
            "--plugin-compat", request.pluginCompat,
        ];
        if (request.reasoning)
            args.push("--reasoning", request.reasoning);
        if (request.willRetry)
            args.push("--will-retry");
        if (request.quiescent)
            args.push("--quiescent");
        if (request.hostIdle)
            args.push("--host-idle");
        if (request.queueEmpty)
            args.push("--queue-empty");
        if (request.toolClear)
            args.push("--tool-clear");
        return this.requiredJson(args, opts);
    }
    async compactionKickPermit(actionId, opts = {}) {
        return this.requiredJson(this.kickArgs("permit", actionId), opts);
    }
    async compactionKickAck(actionId, promptVersion, promptDigest, opts = {}) {
        return this.requiredJson([
            ...this.kickArgs("ack", actionId),
            "--prompt-version", promptVersion,
            "--prompt-digest", promptDigest,
        ], opts);
    }
    async compactionKickCancel(actionId, reason, opts = {}) {
        return this.requiredJson([
            ...this.kickArgs("cancel", actionId), "--reason", reason,
        ], opts);
    }
    async compactionKickSettle(actionId, opts = {}) {
        return this.requiredJson(this.kickArgs("settle", actionId), opts);
    }
    async compactionKickAbortAck(actionId, opts = {}) {
        return this.requiredJson(this.kickArgs("abort-ack", actionId), opts);
    }
    async compactionKickEffectBegin(actionId, toolCallId, opts = {}) {
        return this.requiredJson([
            ...this.kickArgs("effect-begin", actionId), "--tool-call", toolCallId,
        ], opts);
    }
    async compactionKickEffectEnd(actionId, toolCallId, opts = {}) {
        return this.requiredJson([
            ...this.kickArgs("effect-end", actionId), "--tool-call", toolCallId,
        ], opts);
    }
    kickArgs(verb, actionId) {
        const task = this.env.taskId;
        if (!task)
            throw new Error("compaction kick requires a managed WG task");
        return ["pi-watchdog", "compaction-kick", verb, task, "--action", actionId];
    }
    async requiredJson(args, opts) {
        const result = await this.run(args, { ...opts, json: true });
        if (result.code !== 0) {
            const detail = (result.stderr || result.stdout).split(/\r?\n/).find(Boolean) ?? "refused";
            throw new Error(`WG compaction-kick broker refused: ${detail}`);
        }
        try {
            return JSON.parse(result.stdout);
        }
        catch {
            throw new Error("WG compaction-kick broker returned invalid JSON");
        }
    }
    // ── messaging verbs ─────────────────────────────────────────────────────
    msgSend(target, message, opts = {}) {
        return this.run(["msg", "send", target, message], opts);
    }
    msgRead(target, agent, opts = {}) {
        const args = ["msg", "read", target];
        if (agent)
            args.push("--agent", agent);
        return this.run(args, { ...opts, json: true });
    }
    // ── model bridge ────────────────────────────────────────────────────────
    /** True only when this process was explicitly launched for a WG chat. */
    hasChatContext() {
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
    async setModelOverride(spec, chatRef, opts = {}) {
        const chat = chatRef
            ? canonicalChatId({ WG_CHAT_ID: chatRef })
            : this.env.chatId;
        if (!chat)
            return null;
        const r = await this.run(["chat", "model", chat, spec, "--warm-pi-writeback"], opts);
        if (r.code !== 0) {
            // Clap/IPC errors can be multi-line. One line is enough here; the full
            // command names the exact target and can be rerun directly by the user.
            const detail = (r.stderr || r.stdout)
                .split(/\r?\n/)
                .map((line) => line.trim())
                .find(Boolean);
            throw new Error(`model override for ${chat} failed (wg exit ${r.code})` +
                (detail ? `: ${detail}` : "; rerun `wg chat model` for details"));
        }
        return r;
    }
}
//# sourceMappingURL=wg-backend.js.map