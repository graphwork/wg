/**
 * commands.ts — human-facing slash commands.
 *
 *   /wg            init | ready | graph | show <id> | run <id> | add <title> | done <id> | fail <id> <reason>
 *                  on | off | status
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
 *
 * `/wg on | off | status` are the SESSION-SCOPED ACTIVATION verbs (dormant by
 * default — see `activation.ts`). They are UI affordances, not model-visible
 * tools, so they exist in every state (`/wg on|off` can never lock you out):
 *
 *   /wg on      activate the wg_* tool family in THIS session via
 *               `pi.setActiveTools` and mark the project active (a marker file
 *               inside an existing `.wg/`), so new sessions of this project
 *               start active.
 *   /wg off     deactivate the wg_* tools, unmount the surfaces, and clear the
 *               project marker. It NEVER touches settings.json, NEVER removes
 *               the extension, and NEVER touches `.wg/` or the daemon —
 *               `wg pi-plugin disable` stays the CLI-only uninstall.
 *   /wg status  one compact truth line: session · project · tools · daemon.
 */

import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { existsSync } from "node:fs";
import { join } from "node:path";
import type { WgBackend } from "./wg-backend.js";
import {
  formatActivationStatus,
  type ActivationController,
  type ActivationOutcome,
} from "./activation.js";

export const WG_SUBCOMMANDS = [
  "init",
  "ready",
  "graph",
  "show",
  "run",
  "add",
  "done",
  "fail",
  "on",
  "off",
  "status",
] as const;

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

/** One-word plugin health derived from `wg pi-plugin status`. */
export type PluginState = "current" | "stale" | "missing" | "unknown";

/**
 * Parse `wg pi-plugin status` stdout into the compact plugin state. The Rust
 * side renders a `cache state:` line whose first token is `current`,
 * `MISSING`, or `DRIFTED`; anything else (including a failed / empty run) is
 * honestly reported as `unknown` rather than guessed.
 */
export function parsePluginState(stdout: string): PluginState {
  const match = /cache state:\s*([A-Za-z]+)/.exec(stdout ?? "");
  if (!match) return "unknown";
  const value = (match[1] ?? "").toLowerCase();
  if (value.startsWith("current")) return "current";
  if (value.startsWith("drift")) return "stale";
  if (value.startsWith("missing")) return "missing";
  return "unknown";
}

/** The slice of `wg status --json` `/wg status` renders. */
export interface WgStatusJson {
  service?: { running?: boolean };
  agents?: { alive?: number };
  tasks?: { in_progress?: number; ready?: number; done_total?: number };
}

/**
 * One compact, truthful `/wg status` line: plugin health + graph presence +
 * fleet availability. Read-only.
 */
export function formatStatusLine(input: {
  plugin: PluginState;
  graph: boolean;
  status: WgStatusJson | null;
}): string {
  const tasks = input.status?.tasks;
  const graphText = input.graph
    ? `present (${tasks?.in_progress ?? 0} in-progress, ${tasks?.ready ?? 0} ready, ${tasks?.done_total ?? 0} done)`
    : "absent";
  const alive = input.status?.agents?.alive ?? 0;
  const fleetText = input.status?.service?.running
    ? `service up (${alive} agent${alive === 1 ? "" : "s"})`
    : "daemon down";
  return `WG · plugin: ${input.plugin} · graph: ${graphText} · fleet: ${fleetText}`;
}

/** `/wg on|off|status` without an activation controller (older embedders). */
function notWired(sub: string): string {
  return `/wg ${sub}: session activation is not wired in this build — reinstall the plugin with \`wg pi-plugin install\`.`;
}

/**
 * The `/wg on` confirmation: one line that says exactly what changed, what was
 * NOT touched, and the one-time context-repricing cost of a mid-session
 * tool-set change (pi appends the new tool set before the next model request,
 * which can invalidate the provider's cached prompt prefix).
 */
export function formatOnLine(outcome: ActivationOutcome): string {
  const scope = outcome.project
    ? `project marked active (${outcome.markerPath})`
    : "session-only (no .wg/ here — nothing persisted)";
  return (
    `WG active in this session · ${outcome.visibleTools} wg tools visible · ${scope} · context re-priced`
  );
}

/**
 * The `/wg off` confirmation: dormant in this session, project marker cleared,
 * and the things it deliberately did NOT touch — settings.json, the extension
 * entry, `.wg/`, and the daemon. Running agents are separate processes.
 */
export function formatOffLine(outcome: ActivationOutcome): string {
  return (
    `WG dormant in this session · ${outcome.visibleTools} wg tools visible · ` +
    `project marker ${outcome.markerPath ? "cleared" : "absent"} · ` +
    "settings.json, the extension entry, .wg/ and the daemon untouched — " +
    "running agents are separate processes and keep running · context re-priced"
  );
}

export function registerWgCommands(
  pi: ExtensionAPI,
  backend: WgBackend,
  activation?: ActivationController,
): void {
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
      "WG task graph: /wg init | ready | graph | show <id> | run <id> | add <title> | done <id> | fail <id> <reason> | on | off | status (on/off activate this session's wg tools; they never touch settings.json)",
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
        case "on": {
          if (!activation) return emit(ctx, notWired("on"), "warning");
          const outcome = await activation.activate(ctx);
          emit(ctx, formatOnLine(outcome), "info");
          return;
        }
        case "off": {
          if (!activation) return emit(ctx, notWired("off"), "warning");
          const outcome = await activation.deactivate(ctx);
          emit(ctx, formatOffLine(outcome), "info");
          return;
        }
        case "status": {
          if (!activation) return emit(ctx, notWired("status"), "warning");
          const state = await activation.status(ctx);
          emit(ctx, formatActivationStatus(state), "info");
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
