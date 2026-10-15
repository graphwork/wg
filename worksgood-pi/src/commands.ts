/**
 * commands.ts — human-facing slash commands.
 *
 *   /wg            init | ready | graph | show <id> | run <id> | add <title> | done <id> | fail <id> <reason>
 *   /wg-model      <provider:id>   warm in-session model swap via pi's native setModel
 *
 * `/wg run <id>` follows plugin-research.md §1.1: it loads the task and injects
 * it as a user message so pi's own agent loop works the task with the wg tools
 * in context. `/wg init` is the conversational bootstrap: it injects ONE short
 * user message telling the agent to inspect state, then run `wg init` / `wg
 * setup` / `wg pi-plugin install` with its own tools — the plugin itself stays
 * quiet until summoned (no load-time work here). `/wg-model` resolves through
 * `ctx.modelRegistry` and calls `pi.setModel`; the resulting `model_select`
 * event is what model-bridge.ts writes back to WG (plugin-research.md §1.3).
 */

import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { existsSync } from "node:fs";
import { join } from "node:path";
import type { WgBackend } from "./wg-backend.js";

const WG_SUBCOMMANDS = ["init", "ready", "graph", "show", "run", "add", "done", "fail"] as const;

/**
 * The compact bootstrap prompt `/wg init` injects. One paragraph: the agent
 * does the setup with its own tools, asking the operator only what it cannot
 * infer, and reports a short summary. Exported so the injected wording is
 * pinned by tests and shared with any future bootstrap surface.
 */
export const WG_INIT_BOOTSTRAP_PROMPT =
  "Bootstrap the WorksGood (WG) environment for this project using your tools and the `wg` verbs. " +
  "First REPORT state without changing anything: does `.wg` exist (run `wg status`), and is the plugin current (run `wg pi-plugin status`). " +
  "If WG is not set up here, run `wg init`; then run `wg setup` to choose the execution route, asking me ONLY what you cannot infer (e.g. which model route/provider) \u2014 if I say \"default\" or \"you choose\", take the Pi defaults via `wg setup --route pi --yes`. " +
  "Then run `wg pi-plugin install` and verify with `wg status` + `wg pi-plugin status`. " +
  "Ask before creating anything surprising. " +
  "Finish with a 3-5 line summary: the chosen route, the daemon state, how to open the fleet (`/wg-fleet`), and how to quiet it back down (`/wg-wake off` mutes completion wakeups; `wg service stop` stops the daemon).";

/** True when `dir` looks like an initialized WG project (`.wg/` or `worksgood.toml`). */
function isWgProject(dir: string): boolean {
  return existsSync(join(dir, ".wg")) || existsSync(join(dir, "worksgood.toml"));
}

/** Split "provider:rest" into [provider, rest]; tolerates "provider/model" too. */
export function parseModelSpec(spec: string): { provider: string; id: string } | null {
  const s = spec.trim();
  if (!s) return null;
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
function splitFirst(args: string): [string, string] {
  const trimmed = args.trim();
  const m = trimmed.match(/^(\S+)\s*([\s\S]*)$/);
  if (!m) return ["", ""];
  return [m[1] ?? "", (m[2] ?? "").trim()];
}

/** Emit text in any mode: notify when a UI is present, else stdout. */
function emit(ctx: ExtensionCommandContext, text: string, type: "info" | "warning" | "error" = "info") {
  if (ctx.hasUI) {
    ctx.ui.notify(text, type);
  } else {
    (type === "error" ? console.error : console.log)(text);
  }
}

export function registerWgCommands(pi: ExtensionAPI, backend: WgBackend): void {
  // Stash the model registry from the live context so /wg-model autocomplete
  // can list available models (getArgumentCompletions has no ctx of its own).
  let modelSpecs: string[] = [];
  pi.on("session_start", (_event, ctx) => {
    try {
      modelSpecs = ctx.modelRegistry.getAvailable().map((m) => `${m.provider}:${m.id}`);
    } catch {
      modelSpecs = [];
    }
  });

  pi.registerCommand("wg", {
    description:
      "WG task graph: /wg init | ready | graph | show <id> | run <id> | add <title> | done <id> | fail <id> <reason>",
    getArgumentCompletions: (prefix: string) => {
      const [sub, rest] = splitFirst(prefix);
      // Completing the subcommand itself (no trailing space yet).
      if (!prefix.includes(" ")) {
        const matches = WG_SUBCOMMANDS.filter((s) => s.startsWith(sub));
        return matches.length ? matches.map((s) => ({ value: s, label: s })) : null;
      }
      void rest;
      return null;
    },
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const [sub, rest] = splitFirst(args);
      switch (sub) {
        case "init": {
          pi.sendUserMessage(WG_INIT_BOOTSTRAP_PROMPT);
          emit(ctx, "Starting WG setup \u2014 the agent will report state, then init, route, and install the plugin.");
          return;
        }
        case "": {
          // Quiet nicety: when summoned bare in a project with no WG state,
          // point at the setup command instead of running a doomed `wg ready`.
          const projectDir = backend.env.dir?.trim() || ctx.cwd?.trim();
          if (projectDir && !isWgProject(projectDir)) {
            return emit(ctx, "WG isn't set up in this project yet \u2014 run /wg init to set up WG here.", "warning");
          }
          // fall through to the default ready view
        }
        case "ready":
        case "graph": {
          const r = await backend.ready({ signal: ctx.signal });
          emit(ctx, r.stdout.trim() || "No ready tasks.", r.code === 0 ? "info" : "error");
          return;
        }
        case "show": {
          if (!rest) return emit(ctx, "usage: /wg show <id>", "warning");
          const r = await backend.show(rest, { signal: ctx.signal });
          emit(ctx, r.stdout.trim() || r.stderr.trim(), r.code === 0 ? "info" : "error");
          return;
        }
        case "run": {
          if (!rest) return emit(ctx, "usage: /wg run <id>", "warning");
          const r = await backend.show(rest, { signal: ctx.signal });
          if (r.code !== 0) {
            return emit(ctx, r.stderr.trim() || `wg show ${rest} failed`, "error");
          }
          pi.sendUserMessage(
            `Work this WG task. Use the wg_* tools to log progress, validate, and mark it done when complete.\n\n${r.stdout}`,
          );
          emit(ctx, `Loaded WG task ${rest} into the session.`);
          return;
        }
        case "add": {
          if (!rest) return emit(ctx, "usage: /wg add <title>", "warning");
          const r = await backend.add(rest, [], { signal: ctx.signal });
          emit(ctx, r.stdout.trim() || r.stderr.trim(), r.code === 0 ? "info" : "error");
          return;
        }
        case "publish": {
          if (!rest) return emit(ctx, "usage: /wg publish <id>", "warning");
          const r = await backend.publish(rest, { signal: ctx.signal });
          emit(ctx, r.stdout.trim() || r.stderr.trim(), r.code === 0 ? "info" : "error");
          return;
        }
        case "done": {
          if (!rest) return emit(ctx, "usage: /wg done <id>", "warning");
          const r = await backend.done(rest, { signal: ctx.signal });
          emit(ctx, r.stdout.trim() || r.stderr.trim(), r.code === 0 ? "info" : "error");
          return;
        }
        case "fail": {
          const [id, reason] = splitFirst(rest);
          if (!id || !reason) return emit(ctx, "usage: /wg fail <id> <reason>", "warning");
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
    getArgumentCompletions: (prefix: string) => {
      const p = prefix.trim();
      const matches = modelSpecs.filter((s) => s.startsWith(p));
      return matches.length ? matches.map((s) => ({ value: s, label: s })) : null;
    },
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const parsed = parseModelSpec(args);
      if (!parsed) {
        return emit(ctx, "usage: /wg-model <provider:id> (e.g. claude:opus)", "warning");
      }
      const model = ctx.modelRegistry.find(parsed.provider, parsed.id);
      if (!model) {
        return emit(ctx, `unknown model: ${parsed.provider}:${parsed.id}`, "error");
      }
      const ok = await pi.setModel(model);
      emit(
        ctx,
        ok ? `Model set to ${model.provider}:${model.id}` : `No API key for ${model.provider}:${model.id}`,
        ok ? "info" : "error",
      );
    },
  });
}
