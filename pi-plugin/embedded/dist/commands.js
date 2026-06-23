/**
 * commands.ts — human-facing slash commands.
 *
 *   /wg            ready | graph | show <id> | run <id> | add <title> | done <id> | fail <id> <reason>
 *   /wg-model      <provider:id>   warm in-session model swap via pi's native setModel
 *
 * `/wg run <id>` follows plugin-research.md §1.1: it loads the task and injects
 * it as a user message so pi's own agent loop works the task with the wg tools
 * in context. `/wg-model` resolves through `ctx.modelRegistry` and calls
 * `pi.setModel`; the resulting `model_select` event is what model-bridge.ts
 * writes back to WG (plugin-research.md §1.3).
 */
const WG_SUBCOMMANDS = ["ready", "graph", "show", "run", "add", "done", "fail"];
/** Split "provider:rest" into [provider, rest]; tolerates "provider/model" too. */
export function parseModelSpec(spec) {
    const s = spec.trim();
    if (!s)
        return null;
    const colon = s.indexOf(":");
    if (colon > 0) {
        return { provider: s.slice(0, colon), id: s.slice(colon + 1) };
    }
    const slash = s.indexOf("/");
    if (slash > 0) {
        return { provider: s.slice(0, slash), id: s.slice(slash + 1) };
    }
    return null;
}
/** First whitespace-delimited token + the remainder. */
function splitFirst(args) {
    const trimmed = args.trim();
    const m = trimmed.match(/^(\S+)\s*([\s\S]*)$/);
    if (!m)
        return ["", ""];
    return [m[1] ?? "", (m[2] ?? "").trim()];
}
/** Emit text in any mode: notify when a UI is present, else stdout. */
function emit(ctx, text, type = "info") {
    if (ctx.hasUI) {
        ctx.ui.notify(text, type);
    }
    else {
        (type === "error" ? console.error : console.log)(text);
    }
}
export function registerWgCommands(pi, backend) {
    // Stash the model registry from the live context so /wg-model autocomplete
    // can list available models (getArgumentCompletions has no ctx of its own).
    let modelSpecs = [];
    pi.on("session_start", (_event, ctx) => {
        try {
            modelSpecs = ctx.modelRegistry.getAvailable().map((m) => `${m.provider}:${m.id}`);
        }
        catch {
            modelSpecs = [];
        }
    });
    pi.registerCommand("wg", {
        description: "WG task graph: /wg ready | graph | show <id> | run <id> | add <title> | done <id> | fail <id> <reason>",
        getArgumentCompletions: (prefix) => {
            const [sub, rest] = splitFirst(prefix);
            // Completing the subcommand itself (no trailing space yet).
            if (!prefix.includes(" ")) {
                const matches = WG_SUBCOMMANDS.filter((s) => s.startsWith(sub));
                return matches.length ? matches.map((s) => ({ value: s, label: s })) : null;
            }
            void rest;
            return null;
        },
        handler: async (args, ctx) => {
            const [sub, rest] = splitFirst(args);
            switch (sub) {
                case "":
                case "ready":
                case "graph": {
                    const r = await backend.ready({ signal: ctx.signal });
                    emit(ctx, r.stdout.trim() || "No ready tasks.", r.code === 0 ? "info" : "error");
                    return;
                }
                case "show": {
                    if (!rest)
                        return emit(ctx, "usage: /wg show <id>", "warning");
                    const r = await backend.show(rest, { signal: ctx.signal });
                    emit(ctx, r.stdout.trim() || r.stderr.trim(), r.code === 0 ? "info" : "error");
                    return;
                }
                case "run": {
                    if (!rest)
                        return emit(ctx, "usage: /wg run <id>", "warning");
                    const r = await backend.show(rest, { signal: ctx.signal });
                    if (r.code !== 0) {
                        return emit(ctx, r.stderr.trim() || `wg show ${rest} failed`, "error");
                    }
                    pi.sendUserMessage(`Work this WG task. Use the wg_* tools to log progress, validate, and mark it done when complete.\n\n${r.stdout}`);
                    emit(ctx, `Loaded WG task ${rest} into the session.`);
                    return;
                }
                case "add": {
                    if (!rest)
                        return emit(ctx, "usage: /wg add <title>", "warning");
                    const r = await backend.add(rest, [], { signal: ctx.signal });
                    emit(ctx, r.stdout.trim() || r.stderr.trim(), r.code === 0 ? "info" : "error");
                    return;
                }
                case "done": {
                    if (!rest)
                        return emit(ctx, "usage: /wg done <id>", "warning");
                    const r = await backend.done(rest, { signal: ctx.signal });
                    emit(ctx, r.stdout.trim() || r.stderr.trim(), r.code === 0 ? "info" : "error");
                    return;
                }
                case "fail": {
                    const [id, reason] = splitFirst(rest);
                    if (!id || !reason)
                        return emit(ctx, "usage: /wg fail <id> <reason>", "warning");
                    const r = await backend.fail(id, reason, { signal: ctx.signal });
                    emit(ctx, r.stdout.trim() || r.stderr.trim(), r.code === 0 ? "info" : "error");
                    return;
                }
                default:
                    emit(ctx, `unknown /wg subcommand: ${sub}`, "warning");
            }
        },
    });
    pi.registerCommand("wg-model", {
        description: "Warm-swap the session model: /wg-model <provider:id> (round-trips into WG's CoordinatorState)",
        getArgumentCompletions: (prefix) => {
            const p = prefix.trim();
            const matches = modelSpecs.filter((s) => s.startsWith(p));
            return matches.length ? matches.map((s) => ({ value: s, label: s })) : null;
        },
        handler: async (args, ctx) => {
            const parsed = parseModelSpec(args);
            if (!parsed) {
                return emit(ctx, "usage: /wg-model <provider:id> (e.g. claude:opus)", "warning");
            }
            const model = ctx.modelRegistry.find(parsed.provider, parsed.id);
            if (!model) {
                return emit(ctx, `unknown model: ${parsed.provider}:${parsed.id}`, "error");
            }
            const ok = await pi.setModel(model);
            emit(ctx, ok ? `Model set to ${model.provider}:${model.id}` : `No API key for ${model.provider}:${model.id}`, ok ? "info" : "error");
        },
    });
}
//# sourceMappingURL=commands.js.map