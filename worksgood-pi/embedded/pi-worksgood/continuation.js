import { readFileSync, realpathSync } from "node:fs";
import { dirname, join } from "node:path";
export const WG_PI_COMPACTION_KICK_HOST_CONTRACT = "pi-0.83-session-compact-sync-v1";
export const WG_PI_COMPACTION_KICK_CUSTOM_TYPE = "wg-pi-compaction-kick";
export function detectPiHostVersion(entrypoint = process.argv[1]) {
    if (!entrypoint)
        return undefined;
    let current;
    try {
        current = dirname(realpathSync(entrypoint));
    }
    catch {
        return undefined;
    }
    for (let depth = 0; depth < 6; depth += 1) {
        try {
            const pkg = JSON.parse(readFileSync(join(current, "package.json"), "utf8"));
            if (pkg?.name === "@earendil-works/pi-coding-agent") {
                return typeof pkg.version === "string" ? pkg.version : undefined;
            }
        }
        catch {
            // Continue toward the package root.
        }
        const parent = dirname(current);
        if (parent === current)
            break;
        current = parent;
    }
    return undefined;
}
function enabled(env, backend, hostVersion) {
    const eligible = Boolean(backend.env.taskId &&
        env.WG_TASK_ID === backend.env.taskId &&
        env.WG_AGENT_ID &&
        env.WG_WORKER_CAPABILITY &&
        env.WG_PI_TASK_WORKER === "1" &&
        env.WG_PI_COMPACTION_KICK !== "0" &&
        env.WG_PI_PLUGIN_COMPAT_VERSION &&
        env.WG_PI_COMPACTION_KICK_HOST_CONTRACT === WG_PI_COMPACTION_KICK_HOST_CONTRACT);
    if (eligible && hostVersion !== "0.83.0") {
        boundedError("unsupported host", new Error(`pi_host_version_${hostVersion ?? "unknown"}`));
        return false;
    }
    return eligible;
}
function modelIdentity(ctx) {
    const model = ctx.model;
    if (!model || typeof model.provider !== "string" || typeof model.id !== "string")
        return null;
    return { provider: model.provider, model: model.id };
}
function sessionIdentity(ctx) {
    const sessionId = ctx.sessionManager.getSessionId();
    const sessionFile = ctx.sessionManager.getSessionFile();
    if (!sessionId || !sessionFile)
        return null;
    // Pi 0.83's nested post-followUp compaction callback can expose the prior
    // callback object and not-yet-advanced in-memory getters even though the new
    // compaction entry is already durably appended. Read only the append tail;
    // Rust independently parses the entire selected journal and proves this is
    // its unique current compaction leaf before granting any authority.
    let sessionLeaf;
    try {
        const lines = readFileSync(sessionFile, "utf8").trimEnd().split(/\r?\n/);
        sessionLeaf = JSON.parse(lines.at(-1) ?? "null");
    }
    catch {
        sessionLeaf = ctx.sessionManager.getEntries().at(-1);
    }
    const sessionLeafId = sessionLeaf?.id;
    if (!sessionLeafId || !sessionLeaf)
        return null;
    return { sessionId, sessionFile, sessionLeafId, sessionLeaf };
}
function isTerminalTool(toolName) {
    return toolName === "wg_done" || toolName === "wg_fail" || toolName === "wg_wait";
}
function boundedError(label, error) {
    const code = error instanceof Error
        ? (error.message.split(/\r?\n/, 1)[0] ?? "unknown").slice(0, 240)
        : "unknown";
    console.error(`[pi-worksgood] compaction-kick ${label}: ${code}`);
}
/**
 * Install the delivery-only half of the WG threshold-compaction protocol.
 *
 * The module is inert outside an explicitly capability-bound, hermetic JSON
 * task worker. Authority and idempotency live in the lifecycle/watchdog broker;
 * this code only observes the awaited live callback, performs local guards,
 * invokes Pi once after a fresh durable permit, and acknowledges selection.
 */
export function installCompactionContinuation(pi, backend, env = process.env, hostVersion = detectPiHostVersion()) {
    if (!enabled(env, backend, hostVersion))
        return;
    let sawAgentEnd = false;
    let sawAgentSettled = false;
    let agentStartedAfterEnd = false;
    const openTools = new Set();
    const sendAttempted = new Set();
    const permits = new Map();
    const occurrenceEntries = new Set();
    const leasedTools = new Map();
    const acknowledgedActions = new Set();
    const terminalWatches = new Map();
    let activeActionId;
    const stopTerminalWatch = (actionId) => {
        terminalWatches.get(actionId)?.abort();
        terminalWatches.delete(actionId);
    };
    const terminalAbort = async (actionId, ctx, reason) => {
        stopTerminalWatch(actionId);
        ctx.abort();
        if (acknowledgedActions.has(actionId)) {
            await backend
                .compactionKickAbortAck(actionId)
                .catch((error) => boundedError("abort ack held", error));
        }
        else {
            await backend
                .compactionKickCancel(actionId, reason)
                .catch((error) => boundedError("terminal suppression held", error));
        }
    };
    const runTerminalWatch = async (actionId, ctx, controller) => {
        for (let sequence = 1; sequence <= 64 && !controller.signal.aborted; sequence += 1) {
            try {
                const status = await backend.compactionKickTerminalWatch(actionId, sequence, 20_000, { signal: controller.signal });
                if (controller.signal.aborted)
                    return;
                if (status.abort) {
                    await terminalAbort(actionId, ctx, "terminal_watch_before_ack");
                    return;
                }
                if (status.settled) {
                    stopTerminalWatch(actionId);
                    return;
                }
            }
            catch (error) {
                if (controller.signal.aborted)
                    return;
                // Loss of the cancellation channel cannot leave the provider/effect
                // run live. Lifecycle effect-begin remains the authoritative backstop.
                ctx.abort();
                stopTerminalWatch(actionId);
                boundedError("terminal watch held", error);
                return;
            }
        }
        if (!controller.signal.aborted) {
            ctx.abort();
            stopTerminalWatch(actionId);
            boundedError("terminal watch exhausted", new Error("finite_watch_budget_exhausted"));
        }
    };
    const locallyQuiescent = (ctx) => sawAgentEnd &&
        !sawAgentSettled &&
        !agentStartedAfterEnd &&
        openTools.size === 0 &&
        !ctx.isIdle();
    pi.on("agent_start", () => {
        sawAgentSettled = false;
        if (sawAgentEnd)
            agentStartedAfterEnd = true;
        else
            agentStartedAfterEnd = false;
    });
    pi.on("agent_end", () => {
        sawAgentEnd = true;
        agentStartedAfterEnd = false;
    });
    pi.on("tool_execution_start", (event) => {
        openTools.add(event.toolCallId);
    });
    pi.on("tool_execution_end", (event) => {
        openTools.delete(event.toolCallId);
    });
    pi.on("session_compact", async (event, ctx) => {
        if (event.reason !== "threshold")
            return;
        if (event.willRetry) {
            boundedError("suppressed host retry", new Error("will_retry"));
            return;
        }
        // Pi 0.83 emits session_compact only after compaction succeeded and its
        // CompactionEntry was appended. Failed/aborted attempts emit only the
        // public compaction_end event and therefore never reach this handler.
        // Also reject explicit failure fields defensively so a malformed or newer
        // host cannot turn such an event into a claimed successful occurrence.
        const outcome = event;
        if (outcome.aborted === true || typeof outcome.errorMessage === "string") {
            boundedError("suppressed host outcome", new Error("compaction_not_successful"));
            return;
        }
        if (ctx.mode !== "json") {
            boundedError("suppressed host mode", new Error("host_mode"));
            return;
        }
        if (!locallyQuiescent(ctx)) {
            boundedError("suppressed host window", new Error("not_compaction_quiescent"));
            return;
        }
        if (ctx.hasPendingMessages()) {
            boundedError("suppressed queue", new Error("queue_nonempty_before_authorize"));
            return;
        }
        const session = sessionIdentity(ctx);
        const model = modelIdentity(ctx);
        if (!session || !model) {
            boundedError("suppressed identity", new Error("identity_missing"));
            return;
        }
        if (session.sessionLeaf.type !== "compaction" || !session.sessionLeaf.parentId) {
            boundedError("suppressed session", new Error("compaction_leaf_mismatch"));
            return;
        }
        // Pi 0.83 can reuse the previous callback object's compactionEntry while a
        // follow-up recovery turn compacts again in the same outer run. The
        // persisted SessionManager append tail identifies the new occurrence; the
        // broker independently selects and hashes that exact journal entry.
        const compactionEntryId = session.sessionLeafId;
        const compactionParentId = session.sessionLeaf.parentId;
        if (occurrenceEntries.has(compactionEntryId))
            return;
        occurrenceEntries.add(compactionEntryId);
        let action;
        try {
            action = await backend.compactionKickAuthorize({
                reason: "threshold",
                willRetry: false,
                compactionEntryId,
                compactionParentId,
                sessionId: session.sessionId,
                sessionFile: session.sessionFile,
                sessionLeafId: session.sessionLeafId,
                pid: process.pid,
                provider: model.provider,
                model: model.model,
                reasoning: env.WG_REASONING,
                pluginCompat: env.WG_PI_PLUGIN_COMPAT_VERSION,
                quiescent: true,
                hostIdle: false,
                queueEmpty: true,
                toolClear: true,
            });
        }
        catch (error) {
            boundedError("authorize held", error);
            return;
        }
        const afterAuthorize = sessionIdentity(ctx);
        if (!locallyQuiescent(ctx) ||
            ctx.hasPendingMessages() ||
            !afterAuthorize ||
            afterAuthorize.sessionId !== session.sessionId ||
            afterAuthorize.sessionFile !== session.sessionFile ||
            afterAuthorize.sessionLeafId !== session.sessionLeafId) {
            await backend
                .compactionKickCancel(action.actionId, ctx.hasPendingMessages()
                ? "queue_nonempty_after_authorize"
                : "identity_changed_after_authorize")
                .catch((error) => boundedError("cancel after authorize held", error));
            return;
        }
        let permit;
        try {
            permit = await backend.compactionKickPermit(action.actionId);
        }
        catch (error) {
            boundedError("permit held", error);
            return;
        }
        if (!permit.freshDeliveryGrant || !permit.prompt)
            return;
        // Establish the action-scoped cancellation channel before touching Pi's
        // queue. A terminal/park that already won suppresses the committed permit;
        // the lifecycle epoch remains charged and is never re-granted.
        try {
            const terminal = await backend.compactionKickTerminalWatch(action.actionId, 0, 0);
            if (terminal.abort || terminal.settled) {
                await backend
                    .compactionKickCancel(action.actionId, "terminal_before_native_send")
                    .catch((error) => boundedError("terminal suppression held", error));
                return;
            }
        }
        catch (error) {
            await backend
                .compactionKickCancel(action.actionId, "terminal_watch_unavailable")
                .catch((cancelError) => boundedError("terminal suppression held", cancelError));
            boundedError("terminal watch held", error);
            return;
        }
        const afterPermit = sessionIdentity(ctx);
        const permitIdentity = {
            actionId: action.actionId,
            prompt: permit.prompt,
            promptVersion: permit.promptVersion,
            promptDigest: permit.promptDigest,
            sessionId: session.sessionId,
            sessionFile: session.sessionFile,
            compactionEntryId,
        };
        if (!locallyQuiescent(ctx) ||
            !afterPermit ||
            afterPermit.sessionId !== permitIdentity.sessionId ||
            afterPermit.sessionFile !== permitIdentity.sessionFile ||
            afterPermit.sessionLeafId !== permitIdentity.compactionEntryId ||
            afterPermit.sessionLeaf.type !== "compaction" ||
            sendAttempted.has(action.actionId)) {
            await backend
                .compactionKickCancel(action.actionId, "identity_changed_after_permit")
                .catch((error) => boundedError("cancel after permit held", error));
            return;
        }
        // Arm the bounded cancellation subscription before touching Pi's queue.
        // Calling this async function starts the broker request synchronously up to
        // its first await; no callback can run until this handler yields.
        const terminalController = new AbortController();
        terminalWatches.set(action.actionId, terminalController);
        void runTerminalWatch(action.actionId, ctx, terminalController);
        // Host-serialized linearization boundary. Do not insert an await, promise,
        // timer, callback, or microtask between this final queue read and the
        // synchronous Pi queue append.
        if (ctx.hasPendingMessages()) {
            stopTerminalWatch(action.actionId);
            await backend
                .compactionKickCancel(action.actionId, "queue_nonempty_after_permit")
                .catch((error) => boundedError("queue suppression held", error));
            return;
        }
        sendAttempted.add(action.actionId);
        permits.set(action.actionId, permitIdentity);
        pi.sendMessage({
            customType: WG_PI_COMPACTION_KICK_CUSTOM_TYPE,
            content: permit.prompt,
            display: false,
            details: {
                actionId: action.actionId,
                promptVersion: permit.promptVersion,
                promptDigest: permit.promptDigest,
            },
        }, { deliverAs: "followUp", triggerTurn: true });
    });
    pi.on("message_start", async (event, ctx) => {
        const message = event.message;
        if (message?.role !== "custom" || message.customType !== WG_PI_COMPACTION_KICK_CUSTOM_TYPE) {
            return;
        }
        const actionId = message.details?.actionId;
        const permit = typeof actionId === "string" ? permits.get(actionId) : undefined;
        if (!permit ||
            message.content !== permit.prompt ||
            message.details?.promptVersion !== permit.promptVersion ||
            message.details?.promptDigest !== permit.promptDigest) {
            return;
        }
        try {
            const ack = await backend.compactionKickAck(actionId, permit.promptVersion, permit.promptDigest);
            acknowledgedActions.add(actionId);
            activeActionId = actionId;
            if (ack.abort) {
                ctx.abort();
                await backend
                    .compactionKickAbortAck(actionId)
                    .catch((error) => boundedError("abort ack held", error));
            }
        }
        catch (error) {
            // Never let provider/effect execution begin from a stale stored ack when
            // the broker cannot refresh current terminal-race truth. Ack-only replay
            // may recover on a matching persisted event; send is never retried.
            ctx.abort();
            boundedError("ack held", error);
        }
    });
    // Final effect gate. Since this module is installed after every other
    // embedded component and ambient extensions are disabled, a successful
    // lifecycle CAS is required before a recovery tool may execute.
    pi.on("tool_call", async (event) => {
        if (!activeActionId || isTerminalTool(event.toolName))
            return;
        try {
            await backend.compactionKickEffectBegin(activeActionId, event.toolCallId);
            leasedTools.set(event.toolCallId, activeActionId);
        }
        catch (error) {
            boundedError("effect begin blocked", error);
            return { block: true, reason: "WG terminal/effect interlock refused this tool" };
        }
    });
    pi.on("tool_result", async (event, ctx) => {
        if (activeActionId && isTerminalTool(event.toolName)) {
            const permit = permits.get(activeActionId);
            if (permit) {
                try {
                    const ack = await backend.compactionKickAck(activeActionId, permit.promptVersion, permit.promptDigest);
                    if (ack.abort) {
                        ctx.abort();
                        await backend
                            .compactionKickAbortAck(activeActionId)
                            .catch((error) => boundedError("abort ack held", error));
                    }
                }
                catch (error) {
                    boundedError("terminal cancellation check held", error);
                }
            }
            return;
        }
        const actionId = leasedTools.get(event.toolCallId);
        if (!actionId)
            return;
        try {
            await backend.compactionKickEffectEnd(actionId, event.toolCallId);
            leasedTools.delete(event.toolCallId);
        }
        catch (error) {
            // An unclosed lease is intentionally unsafe and prevents terminal/kick
            // authority until reconciliation proves the exact end.
            boundedError("effect end held", error);
        }
    });
    pi.on("agent_settled", async () => {
        sawAgentSettled = true;
        for (const actionId of [...terminalWatches.keys()])
            stopTerminalWatch(actionId);
        if (!activeActionId)
            return;
        const actionId = activeActionId;
        activeActionId = undefined;
        await backend
            .compactionKickSettle(actionId)
            .catch((error) => boundedError("settle held", error));
    });
    pi.on("session_shutdown", () => {
        for (const actionId of [...terminalWatches.keys()])
            stopTerminalWatch(actionId);
    });
}
//# sourceMappingURL=continuation.js.map