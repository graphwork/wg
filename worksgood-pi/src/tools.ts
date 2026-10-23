/**
 * tools.ts — the wg verb family the LLM (and, via /wg, the human) can call
 * from inside a pi session. Each tool shells to the `wg` binary through the
 * shared {@link WgBackend}. Mirrors examples/extensions/tools.ts and the
 * design in plugin-research.md §1.1.
 */

import { mkdtemp, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import {
  DEFAULT_MAX_BYTES,
  DEFAULT_MAX_LINES,
  formatSize,
  truncateTail,
  withFileMutationQueue,
  type TruncationResult,
} from "@earendil-works/pi-coding-agent";
import { Type } from "@earendil-works/pi-ai";
import type { TSchema } from "@earendil-works/pi-ai";
import type { AgentToolResult, ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";
import type { ExecResult } from "@earendil-works/pi-coding-agent";
import type { WgBackend } from "./wg-backend.js";

interface WgToolDetails {
  command: string;
  code: number;
  stderr?: string;
  truncation?: TruncationResult;
  fullOutputPath?: string;
}

/** Build an LLM-facing tool result from a `wg` exec result. */
async function fromExec(command: string, r: ExecResult): Promise<AgentToolResult<WgToolDetails>> {
  const body = r.code === 0 ? r.stdout : `${r.stdout}\n${r.stderr}`.trim();
  const text = body || (r.code === 0 ? "(no output)" : `wg ${command} exited ${r.code}`);
  const truncation = truncateTail(text, {
    maxBytes: DEFAULT_MAX_BYTES,
    maxLines: DEFAULT_MAX_LINES,
  });
  let resultText = truncation.content;
  const details: WgToolDetails = { command, code: r.code, stderr: r.stderr || undefined };

  if (truncation.truncated) {
    const tempDir = await mkdtemp(join(tmpdir(), "wg-pi-tool-"));
    const fullOutputPath = join(tempDir, "output.txt");
    await withFileMutationQueue(fullOutputPath, async () => {
      await writeFile(fullOutputPath, text, "utf8");
    });

    const omittedLines = truncation.totalLines - truncation.outputLines;
    const omittedBytes = truncation.totalBytes - truncation.outputBytes;
    resultText +=
      `\n\n[Output truncated: showing ${truncation.outputLines} of ${truncation.totalLines} lines` +
      ` (${formatSize(truncation.outputBytes)} of ${formatSize(truncation.totalBytes)}).` +
      ` ${omittedLines} lines (${formatSize(omittedBytes)}) omitted.` +
      ` Full output saved to: ${fullOutputPath}]`;

    details.truncation = truncation;
    details.fullOutputPath = fullOutputPath;
  }

  return {
    content: [{ type: "text", text: resultText }],
    details,
  };
}

/**
 * The wg tool family (the task's contract), in registration order:
 * `wg_capabilities`, `wg_ready`, `wg_show`, `wg_add`, `wg_publish`, `wg_done`,
 * `wg_fail`, `wg_wait`, `wg_landing_turn`, `wg_msg_send`, `wg_msg_read`,
 * `wg_run`.
 *
 * This list IS the `/wg on` active-set delta (see `activation.ts`), so it lives
 * next to the registrations that must stay in lock-step with it.
 */
export const WG_TOOL_NAMES: readonly string[] = [
  "wg_capabilities",
  "wg_ready",
  "wg_show",
  "wg_add",
  "wg_publish",
  "wg_done",
  "wg_fail",
  "wg_wait",
  "wg_landing_turn",
  "wg_msg_send",
  "wg_msg_read",
  "wg_run",
];

/**
 * Register ONE wg tool as **registered but inactive**.
 *
 * SESSION-SCOPED ACTIVATION (plugin-research / the operator's dormant-by-default
 * requirement): pi's `hidden` exposure means "registered but unreachable —
 * activating it has no effect", so it cannot be switched on later. `direct` +
 * `defaultActive: false` is the exposure that keeps the tool declarable while
 * leaving it OUT of the active set until `pi.setActiveTools()` names it (/wg on).
 */
function registerInactive<TParams extends TSchema, TDetails>(
  pi: ExtensionAPI,
  tool: ToolDefinition<TParams, TDetails>,
): void {
  // `exposure` / `defaultActive` are part of pi's tool contract (ToolExposure
  // docs, pi >= 1.3) but are not yet in this package's pinned dev type surface,
  // so the two fields ride through one narrow cast rather than polluting every
  // tool literal. On an older pi that ignores them, dormancy is still enforced
  // by `activation.ts` (the first session_start strips the wg tools from the
  // active set), so the failure mode is "starts active, immediately dormant".
  const dormant = { exposure: "direct", defaultActive: false } as Record<string, unknown>;
  (pi.registerTool as (definition: unknown) => void)({ ...tool, ...dormant });
}

/**
 * Register the wg tool family on a pi extension API — every tool INACTIVE.
 * `/wg on` (activation.ts) is what puts them in front of the model.
 */
export function registerWgTools(pi: ExtensionAPI, backend: WgBackend): void {
  registerInactive(pi, {
    name: "wg_capabilities",
    label: "WG: worker capabilities",
    description:
      "Show this worker's effective trusted/scoped/read-only graph-authority mode and important restrictions before attempting coordination.",
    parameters: Type.Object({}),
    async execute(_id, _params, signal) {
      return fromExec("capabilities", await backend.capabilities({ signal }));
    },
  });

  registerInactive(pi, {
    name: "wg_ready",
    label: "WG: ready tasks",
    description: "List WG tasks that are ready to be worked on (no unmet dependencies).",
    parameters: Type.Object({}),
    async execute(_id, _params, signal) {
      return fromExec("ready", await backend.ready({ signal }));
    },
  });

  registerInactive(pi, {
    name: "wg_show",
    label: "WG: show task",
    description: "Show a WG task's details, status, dependencies, artifacts and logs.",
    parameters: Type.Object({
      id: Type.String({ description: "Task id (e.g. 'pi-plugin-impl-package')." }),
    }),
    async execute(_id, params, signal) {
      return fromExec("show", await backend.show(params.id, { signal }));
    },
  });

  registerInactive(pi, {
    name: "wg_add",
    label: "WG: add task",
    description:
      "Create a visible draft WG task. Optionally give a description and a dependency; call wg_publish explicitly to release it for dispatch.",
    parameters: Type.Object({
      title: Type.String({ description: "Task title." }),
      description: Type.Optional(
        Type.String({ description: "Markdown description, ideally with a '## Validation' section." }),
      ),
      after: Type.Optional(
        Type.String({ description: "Comma-separated task id(s) this task depends on." }),
      ),
    }),
    async execute(_id, params, signal) {
      const extra: string[] = [];
      if (params.description) extra.push("-d", params.description);
      if (params.after) extra.push("--after", params.after);
      return fromExec("add", await backend.add(params.title, extra, { signal }));
    },
  });

  registerInactive(pi, {
    name: "wg_publish",
    label: "WG: publish task",
    description: "Release one visible draft for dispatch (`wg publish <id> --only`).",
    parameters: Type.Object({
      id: Type.String({ description: "Draft task id to publish." }),
    }),
    async execute(_id, params, signal) {
      return fromExec("publish", await backend.publish(params.id, { signal }));
    },
  });

  registerInactive(pi, {
    name: "wg_done",
    label: "WG: mark done",
    description: "Mark a WG task complete (runs the smoke gate for owned scenarios).",
    parameters: Type.Object({
      id: Type.String({ description: "Task id to complete." }),
    }),
    async execute(_id, params, signal) {
      return fromExec("done", await backend.done(params.id, { signal }));
    },
  });

  registerInactive(pi, {
    name: "wg_fail",
    label: "WG: mark failed",
    description: "Mark a WG task failed. Use only after a genuine attempt; include what blocked you.",
    parameters: Type.Object({
      id: Type.String({ description: "Task id to fail." }),
      reason: Type.String({ description: "What was tried and what specifically blocked progress." }),
    }),
    async execute(_id, params, signal) {
      return fromExec("fail", await backend.fail(params.id, params.reason, { signal }));
    },
  });

  registerInactive(pi, {
    name: "wg_wait",
    label: "WG: wait / park task",
    description:
      "Atomically park this managed task on a typed wait condition while retaining its checkpoint and resumable session.",
    parameters: Type.Object({
      until: Type.String({
        description:
          "Typed condition such as task:<id>=done, timer:<duration>, message, or landing-turn:<ref>#<ticket>.",
      }),
      checkpoint: Type.Optional(Type.String({ description: "Bounded checkpoint for resumption." })),
    }),
    async execute(_id, params, signal) {
      return fromExec("wait", await backend.wait(params.until, params.checkpoint, { signal }));
    },
  });

  registerInactive(pi, {
    name: "wg_landing_turn",
    label: "WG: landing turn",
    description:
      "Request, inspect, renew, release, or cancel this source task's exact ref-scoped FIFO landing lease. A parked request resumes automatically at the head.",
    parameters: Type.Object({
      action: Type.Union([
        Type.Literal("request"),
        Type.Literal("status"),
        Type.Literal("renew"),
        Type.Literal("release"),
        Type.Literal("cancel"),
      ]),
      integration_ref: Type.Optional(
        Type.String({ description: "Integration ref (default refs/heads/main)." }),
      ),
      progress: Type.Optional(
        Type.String({ description: "Changed proof-of-progress token for lease renewal." }),
      ),
      checkpoint: Type.Optional(
        Type.String({ description: "Checkpoint recorded if request must park." }),
      ),
    }),
    async execute(_id, params, signal) {
      return fromExec(
        `landing-turn ${params.action}`,
        await backend.landingTurn(
          params.action,
          params.integration_ref ?? "refs/heads/main",
          params.progress,
          params.checkpoint,
          { signal },
        ),
      );
    },
  });

  registerInactive(pi, {
    name: "wg_msg_send",
    label: "WG: send message",
    description: "Send a message to a WG task's inbox (coordination / replies to other agents).",
    parameters: Type.Object({
      target: Type.String({ description: "Task id whose inbox receives the message." }),
      message: Type.String({ description: "Message body." }),
    }),
    async execute(_id, params, signal) {
      return fromExec("msg send", await backend.msgSend(params.target, params.message, { signal }));
    },
  });

  registerInactive(pi, {
    name: "wg_msg_read",
    label: "WG: read messages",
    description: "Read messages for a WG task (optionally scoped to a specific agent id).",
    parameters: Type.Object({
      target: Type.String({ description: "Task id whose inbox to read." }),
      agent: Type.Optional(Type.String({ description: "Agent id to scope the read to." })),
    }),
    async execute(_id, params, signal) {
      return fromExec("msg read", await backend.msgRead(params.target, params.agent, { signal }));
    },
  });

  registerInactive(pi, {
    name: "wg_run",
    label: "WG: run task",
    description:
      "Load a WG task into context so the agent can work it: returns the task's full description, validation criteria and logs. Follow up by performing the task and calling wg_done when finished.",
    parameters: Type.Object({
      id: Type.String({ description: "Task id to pick up and work." }),
    }),
    async execute(_id, params, signal) {
      return fromExec("show", await backend.show(params.id, { signal }));
    },
  });
}
