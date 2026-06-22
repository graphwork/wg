/**
 * tools.ts — the wg verb family the LLM (and, via /wg, the human) can call
 * from inside a pi session. Each tool shells to the `wg` binary through the
 * shared {@link WgBackend}. Mirrors examples/extensions/tools.ts and the
 * design in plugin-research.md §1.1.
 */

import { Type } from "@earendil-works/pi-ai";
import type { AgentToolResult, ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { ExecResult } from "@earendil-works/pi-coding-agent";
import type { WgBackend } from "./wg-backend.js";

interface WgToolDetails {
  command: string;
  code: number;
  stderr?: string;
}

/** Build an LLM-facing tool result from a `wg` exec result. */
function fromExec(command: string, r: ExecResult): AgentToolResult<WgToolDetails> {
  const body = r.code === 0 ? r.stdout : `${r.stdout}\n${r.stderr}`.trim();
  const text = body || (r.code === 0 ? "(no output)" : `wg ${command} exited ${r.code}`);
  return {
    content: [{ type: "text", text }],
    details: { command, code: r.code, stderr: r.stderr || undefined },
  };
}

/**
 * Register the wg tool family on a pi extension API.
 *
 * Tool names (the task's contract): `wg_ready`, `wg_show`, `wg_add`,
 * `wg_done`, `wg_fail`, `wg_msg_send`, `wg_msg_read`, `wg_run`.
 */
export function registerWgTools(pi: ExtensionAPI, backend: WgBackend): void {
  pi.registerTool({
    name: "wg_ready",
    label: "WG: ready tasks",
    description: "List WG tasks that are ready to be worked on (no unmet dependencies).",
    parameters: Type.Object({}),
    async execute(_id, _params, signal) {
      return fromExec("ready", await backend.ready({ signal }));
    },
  });

  pi.registerTool({
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

  pi.registerTool({
    name: "wg_add",
    label: "WG: add task",
    description:
      "Create a new WG task. Optionally give a description and a dependency (the new task runs after `after`).",
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

  pi.registerTool({
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

  pi.registerTool({
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

  pi.registerTool({
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

  pi.registerTool({
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

  pi.registerTool({
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
