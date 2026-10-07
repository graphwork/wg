/**
 * Verifies WgBackend.setModelOverride treats a non-zero `wg chat model` exit as
 * an error (regression guard for fix-pi-model).
 *
 * `pi.exec` RESOLVES on a non-zero exit code — it only rejects on spawn failure
 * — so before this fix a missing/erroring `wg chat model` verb made the
 * model-override write-back a SILENT no-op: the ExecResult was returned as-is
 * and the `model_select` handler's catch never fired. setModelOverride now
 * inspects `.code` and rejects on failure so the write-back error is visible.
 *
 * Tests run against the built `pi-worksgood/` artifact — `npm test` builds first.
 */

import { describe, it, expect, vi } from "vitest";
import { createServer } from "node:net";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { canonicalChatId, readWgEnv, WgBackend } from "../pi-worksgood/index.js";

type ExecArgs = { command: string; args: string[] };

/** A fake ExecHost that returns a canned ExecResult and records the call. */
function fakeHost(result: { stdout?: string; stderr?: string; code: number }) {
  const calls: ExecArgs[] = [];
  const host = {
    exec: vi.fn(async (command: string, args: string[]) => {
      calls.push({ command, args });
      return { stdout: result.stdout ?? "", stderr: result.stderr ?? "", code: result.code, killed: false };
    }),
  };
  return { host, calls };
}

describe("WgBackend.setModelOverride exit-code handling", () => {
  it("rejects when `wg chat model` exits non-zero (no longer a silent no-op)", async () => {
    const { host } = fakeHost({ stderr: "error: unrecognized subcommand 'model'", code: 2 });
    const backend = new WgBackend(host, { chatId: ".chat-1", dir: "/proj" });

    await expect(backend.setModelOverride("openrouter:openai/gpt-4o")).rejects.toThrow(
      /model override for \.chat-1 failed \(wg exit 2\)/,
    );
  });

  it("surfaces the wg stderr/stdout in the rejection so the failure is diagnosable", async () => {
    const { host } = fakeHost({ stderr: "error: unrecognized subcommand 'model'", code: 2 });
    const backend = new WgBackend(host, { chatId: ".chat-1" });

    await expect(backend.setModelOverride("claude:opus")).rejects.toThrow(
      /unrecognized subcommand 'model'/,
    );
  });

  it("resolves with the ExecResult when the verb succeeds (exit 0)", async () => {
    const { host, calls } = fakeHost({ stdout: "model set", code: 0 });
    const backend = new WgBackend(host, { chatId: ".chat-1", dir: "/proj" });

    const r = await backend.setModelOverride("claude:opus");
    expect(r?.code).toBe(0);
    // The verb is invoked with the --dir prefix + the canonical chat task id.
    expect(calls[0].command).toBe("wg");
    expect(calls[0].args).toEqual([
      "--dir",
      "/proj",
      "chat",
      "model",
      ".chat-1",
      "claude:opus",
      "--warm-pi-writeback",
    ]);
  });

  it("is a safe no-op when no chat id is available", async () => {
    const { host } = fakeHost({ code: 0 });
    const backend = new WgBackend(host, {}); // no chatId

    await expect(backend.setModelOverride("claude:opus")).resolves.toBeNull();
    expect(backend.hasChatContext()).toBe(false);
    expect(host.exec).not.toHaveBeenCalled();
  });

  it("prefers an explicit canonical chatRef over the env chat id", async () => {
    const { host, calls } = fakeHost({ code: 0 });
    const backend = new WgBackend(host, { chatId: ".chat-1" });

    await backend.setModelOverride("claude:opus", ".chat-8");
    expect(calls[0].args).toEqual([
      "chat",
      "model",
      ".chat-8",
      "claude:opus",
      "--warm-pi-writeback",
    ]);
  });
});

describe("expert backend boundary", () => {
  it("uses the full wg backend, never the limited worksgood concierge", async () => {
    const { host, calls } = fakeHost({ stdout: "[]", code: 0 });
    const backend = new WgBackend(host, { dir: "/proj" });

    await backend.ready();
    await backend.show("task-1");
    await backend.msgRead("task-1", "agent-1");
    expect(calls.map((call) => call.command)).toEqual(["wg", "wg", "wg"]);
    expect(calls.flatMap((call) => call.args)).not.toContain("worksgood");
    expect(calls[0].args).toEqual(["--dir", "/proj", "ready", "--json"]);
  });

  it("binds wait and landing-turn operations to the managed task", async () => {
    const { host, calls } = fakeHost({ code: 0 });
    const backend = new WgBackend(host, { taskId: "task-1" });

    await backend.wait("message", "checkpoint");
    await backend.landingTurn("request", "refs/heads/main", undefined, "ready");
    await backend.landingTurn("status", "refs/heads/main");
    expect(calls.map((call) => call.args)).toEqual([
      ["wait", "task-1", "--until", "message", "--checkpoint", "checkpoint"],
      [
        "landing-turn",
        "request",
        "task-1",
        "--integration-ref",
        "refs/heads/main",
        "--checkpoint",
        "ready",
      ],
      ["landing-turn", "status", "refs/heads/main", "--task", "task-1"],
    ]);
  });

  it("refuses wait and landing-turn calls outside a managed task", async () => {
    const { host } = fakeHost({ code: 0 });
    const backend = new WgBackend(host, {});
    expect(() => backend.wait("message")).toThrow(/managed WG task/);
    expect(() => backend.landingTurn("request", "refs\/heads\/main")).toThrow(
      /managed WG task/,
    );
  });

  it("publishes exactly one staged task", async () => {
    const { host, calls } = fakeHost({ stdout: "published", code: 0 });
    const backend = new WgBackend(host, { dir: "/proj" });

    await backend.publish("visible-draft");
    expect(calls[0]).toEqual({
      command: "wg",
      args: ["--dir", "/proj", "publish", "visible-draft", "--only"],
    });
  });
});

describe("WG chat launch context", () => {
  it("prefers canonical WG_CHAT_ID and accepts WG_CHAT_REF compatibility alias", () => {
    expect(canonicalChatId({ WG_CHAT_ID: ".chat-7", WG_CHAT_REF: "chat-8" })).toBe(".chat-7");
    expect(readWgEnv({ WG_CHAT_REF: "chat-8" }).chatId).toBe(".chat-8");
    expect(readWgEnv({ WG_CHAT_REF: "coordinator-2" }).chatId).toBe(".coordinator-2");
  });

  it("does not invent chat identity from ambient project or task state", () => {
    expect(
      readWgEnv({
        WG_TASK_ID: ".chat-99",
        WG_DIR: "/project/.wg",
        WG_PROJECT_ROOT: "/project",
      }).chatId,
    ).toBeUndefined();
    expect(readWgEnv({ WG_CHAT_ID: "not-a-canonical-chat" }).chatId).toBeUndefined();
  });
});

// ── GetFleet daemon read surface ─────────────────────────────────────────────

/** A fake ExecHost that returns canned stdout keyed by the wg verb. */
function fakeVerbHost(map: Record<string, { stdout?: string; code?: number }>) {
  const calls: { command: string; args: string[] }[] = [];
  const host = {
    exec: vi.fn(async (command: string, args: string[]) => {
      calls.push({ command, args });
      const verb = args.find((a) => ["list", "agents", "ready", "viz", "show"].includes(a)) ?? "";
      const entry = map[verb] ?? { stdout: "[]", code: 0 };
      return { stdout: entry.stdout ?? "", stderr: "", code: entry.code ?? 0, killed: false };
    }),
  };
  return { host, calls };
}

/** A one-request-per-connection fake daemon over a UNIX socket. */
async function fakeDaemon(
  socketPath: string,
  respond: (req: Record<string, unknown>) => { body: string | null; delayMs?: number },
): Promise<{ close: () => Promise<void>; requests: Record<string, unknown>[] }> {
  const requests: Record<string, unknown>[] = [];
  const server = createServer((sock) => {
    let buffer = "";
    sock.on("data", (chunk) => {
      buffer += chunk.toString("utf8");
      const idx = buffer.indexOf("\n");
      if (idx < 0) return;
      const line = buffer.slice(0, idx).trim();
      if (!line) return;
      requests.push(JSON.parse(line) as Record<string, unknown>);
      const { body, delayMs } = respond(JSON.parse(line));
      setTimeout(() => {
        if (body !== null) sock.write(`${body}\n`);
        sock.end();
      }, delayMs ?? 0);
    });
  });
  await new Promise<void>((resolve) => server.listen(socketPath, resolve));
  return {
    requests,
    close: () =>
      new Promise<void>((resolve) => {
        server.close(() => resolve());
      }),
  };
}

const CLI_LIST = JSON.stringify([
  { id: "t1", title: "T1", status: "in-progress", assigned: "agent-7", after: [] },
  { id: "t2", title: "T2", status: "done", after: ["t1"] },
  { id: "t3", title: "T3", status: "open", after: [] },
]);
const CLI_AGENTS = JSON.stringify([
  {
    id: "agent-7",
    task_id: "t1",
    executor: "pi",
    model: "pi:openrouter:anthropic/claude-opus-4-7",
    status: "working",
    started_at: "2026-02-02T09:00:00Z",
    uptime: "12m",
    process_alive: true,
  },
]);
const CLI_READY = JSON.stringify([{ id: "t3", title: "T3", ready: true }]);
const CLI_VIZ_TEXT = "t1  (in-progress)\n└→ t2  (done)";
const CLI_VIZ = JSON.stringify({
  text: CLI_VIZ_TEXT,
  node_lines: { t1: 0, t2: 1 },
  forward_edges: { t1: ["t2"] },
  palette: { "in-progress": [60, 200, 220], done: [80, 220, 100] },
});

describe("WgBackend.getFleet", () => {
  it("reads the bounded snapshot from the daemon over the IPC socket", async () => {
    const dir = mkdtempSync(join(tmpdir(), "wg-fleet-"));
    const socket = join(dir, "daemon.sock");
    const daemon = await fakeDaemon(socket, () => ({
      body: JSON.stringify({
        ok: true,
        revision: "wggraph:v1:abc#deadbeef",
        unchanged: false,
        counts: { in_progress: 1, ready: 2, blocked: 0, done: 3, failed: 0, total: 6 },
        tasks: [
          {
            id: "t1",
            title: "T1",
            status: "in-progress",
            assigned: "agent-7",
            depends_on: ["t0"],
            dependency_count: 1,
            token_usage: { input_tokens: 5, output_tokens: 5, total_tokens: 10, cost_usd: 0.01 },
          },
        ],
        agents: [
          {
            id: "agent-7",
            task_id: "t1",
            executor: "pi",
            status: "working",
            elapsed_ms: 720000,
            activity: "running cargo test",
          },
        ],
      }),
    }));
    try {
      const { host, calls } = fakeVerbHost({});
      const backend = new WgBackend(host, { daemonSocket: socket });

      const snapshot = await backend.getFleet();
      expect(snapshot?.source).toBe("daemon");
      expect(snapshot?.revision).toBe("wggraph:v1:abc#deadbeef");
      expect(snapshot?.counts.in_progress).toBe(1);
      expect(snapshot?.counts.total).toBe(6);
      expect(snapshot?.tasks[0].depends_on).toEqual(["t0"]);
      expect(snapshot?.tasks[0].token_usage?.total_tokens).toBe(10);
      expect(snapshot?.agents[0].activity).toBe("running cargo test");
      // The daemon answered, so the CLI must not be shelled at all.
      expect(calls).toHaveLength(0);
      // The request carries the read-only cmd.
      expect(daemon.requests[0].cmd).toBe("get_fleet");
    } finally {
      await daemon.close();
    }
  });

  it("forwards since_revision for the bounded delta form", async () => {
    const dir = mkdtempSync(join(tmpdir(), "wg-fleet-"));
    const socket = join(dir, "daemon.sock");
    const daemon = await fakeDaemon(socket, () => ({
      body: JSON.stringify({
        ok: true,
        revision: "wggraph:v1:abc#deadbeef",
        unchanged: true,
        counts: { in_progress: 1, ready: 0, blocked: 0, done: 0, total: 1 },
        tasks: [],
        agents: [],
      }),
    }));
    try {
      const { host } = fakeVerbHost({});
      const backend = new WgBackend(host, { daemonSocket: socket });
      const snapshot = await backend.getFleet({ sinceRevision: "wggraph:v1:abc#deadbeef" });
      expect(snapshot?.source).toBe("daemon");
      expect(snapshot?.unchanged).toBe(true);
      expect(snapshot?.tasks).toEqual([]);
      expect(daemon.requests[0].since_revision).toBe("wggraph:v1:abc#deadbeef");
    } finally {
      await daemon.close();
    }
  });

  it("falls back to the CLI path when no daemon socket exists", async () => {
    const dir = mkdtempSync(join(tmpdir(), "wg-fleet-"));
    const { host, calls } = fakeVerbHost({
      list: { stdout: CLI_LIST },
      agents: { stdout: CLI_AGENTS },
      ready: { stdout: CLI_READY },
    });
    const backend = new WgBackend(host, { dir });

    const snapshot = await backend.getFleet();
    expect(snapshot?.source).toBe("cli");
    expect(snapshot?.counts.in_progress).toBe(1);
    expect(snapshot?.counts.done).toBe(1);
    expect(snapshot?.counts.ready).toBe(1);
    expect(snapshot?.counts.active_agents).toBe(1);
    expect(snapshot?.agents[0].id).toBe("agent-7");
    expect(snapshot?.revision).toBe("");
    // All three read-only CLI verbs ran.
    expect(calls.map((c) => c.args.find((a) => ["list", "agents", "ready"].includes(a))).sort()).toEqual([
      "agents",
      "list",
      "ready",
    ]);
  });

  it("falls back to the CLI on a daemon error response", async () => {
    const dir = mkdtempSync(join(tmpdir(), "wg-fleet-"));
    const socket = join(dir, "daemon.sock");
    const daemon = await fakeDaemon(socket, () => ({
      body: JSON.stringify({ ok: false, error: "boom" }),
    }));
    try {
      const { host } = fakeVerbHost({
        list: { stdout: CLI_LIST },
        agents: { stdout: CLI_AGENTS },
        ready: { stdout: CLI_READY },
      });
      const backend = new WgBackend(host, { daemonSocket: socket });
      const snapshot = await backend.getFleet();
      expect(snapshot?.source).toBe("cli");
    } finally {
      await daemon.close();
    }
  });

  it("falls back to the CLI on a protocol mismatch (older daemon shape)", async () => {
    const dir = mkdtempSync(join(tmpdir(), "wg-fleet-"));
    const socket = join(dir, "daemon.sock");
    const daemon = await fakeDaemon(socket, () => ({
      // ok, but missing counts/tasks — an older protocol.
      body: JSON.stringify({ ok: true, agents: [] }),
    }));
    try {
      const { host } = fakeVerbHost({
        list: { stdout: CLI_LIST },
        agents: { stdout: CLI_AGENTS },
        ready: { stdout: CLI_READY },
      });
      const backend = new WgBackend(host, { daemonSocket: socket });
      const snapshot = await backend.getFleet();
      expect(snapshot?.source).toBe("cli");
      expect(snapshot?.counts.total).toBe(3);
    } finally {
      await daemon.close();
    }
  });

  it("falls back to the CLI when the daemon times out", async () => {
    const dir = mkdtempSync(join(tmpdir(), "wg-fleet-"));
    const socket = join(dir, "daemon.sock");
    const daemon = await fakeDaemon(socket, () => ({ body: null, delayMs: 200 }));
    try {
      const { host } = fakeVerbHost({
        list: { stdout: CLI_LIST },
        agents: { stdout: CLI_AGENTS },
        ready: { stdout: CLI_READY },
      });
      const backend = new WgBackend(host, { daemonSocket: socket });
      const snapshot = await backend.getFleet({ timeoutMs: 50 });
      expect(snapshot?.source).toBe("cli");
    } finally {
      await daemon.close();
    }
  });

  it("returns null when neither source produces data", async () => {
    const dir = mkdtempSync(join(tmpdir(), "wg-fleet-"));
    const { host } = fakeVerbHost({
      list: { stdout: "", code: 1 },
      agents: { stdout: "", code: 1 },
      ready: { stdout: "", code: 1 },
    });
    const backend = new WgBackend(host, { dir });
    expect(await backend.getFleet()).toBeNull();
  });

  it("requests WG's rendered tree from the daemon and normalizes it", async () => {
    const dir = mkdtempSync(join(tmpdir(), "wg-fleet-"));
    const socket = join(dir, "daemon.sock");
    const daemon = await fakeDaemon(socket, () => ({
      body: JSON.stringify({
        ok: true,
        revision: "r",
        unchanged: false,
        counts: { in_progress: 1, ready: 0, blocked: 0, done: 1, total: 2 },
        tasks: [{ id: "t1", title: "T1", status: "in-progress", depends_on: [] }],
        agents: [],
        tree: { text: CLI_VIZ_TEXT, node_lines: { t1: 0, t2: 1 }, palette: { "in-progress": [60, 200, 220], done: [80, 220, 100] } },
      }),
    }));
    try {
      const { host, calls } = fakeVerbHost({});
      const backend = new WgBackend(host, { daemonSocket: socket });
      const snapshot = await backend.getFleet({ includeTree: true, treeColumns: 72 });
      expect(snapshot?.source).toBe("daemon");
      expect(snapshot?.tree?.text).toBe(CLI_VIZ_TEXT);
      expect(snapshot?.tree?.node_lines).toEqual({ t1: 0, t2: 1 });
      expect(snapshot?.tree?.palette).toEqual({ "in-progress": [60, 200, 220], done: [80, 220, 100] });
      expect(daemon.requests[0].include_tree).toBe(true);
      expect(daemon.requests[0].tree_columns).toBe(72);
      // A daemon that carried the tree needs no CLI fallback at all.
      expect(calls).toHaveLength(0);
    } finally {
      await daemon.close();
    }
  });

  it("back-fills the tree from `wg viz --json` on the CLI path when requested", async () => {
    const dir = mkdtempSync(join(tmpdir(), "wg-fleet-"));
    const { host, calls } = fakeVerbHost({
      list: { stdout: CLI_LIST },
      agents: { stdout: CLI_AGENTS },
      ready: { stdout: CLI_READY },
      viz: { stdout: CLI_VIZ },
    });
    const backend = new WgBackend(host, { dir });
    const snapshot = await backend.getFleet({ includeTree: true, treeColumns: 80 });
    expect(snapshot?.source).toBe("cli");
    expect(snapshot?.tree?.text).toBe(CLI_VIZ_TEXT);
    expect(snapshot?.tree?.palette).toEqual({ "in-progress": [60, 200, 220], done: [80, 220, 100] });
    const vizCall = calls.find((c) => c.args.includes("viz"));
    expect(vizCall).toBeTruthy();
    expect(vizCall!.args).toContain("--json");
    expect(vizCall!.args).toContain("--columns");
    expect(vizCall!.args).toContain("80");
    // The default path (no includeTree) never shells `wg viz`.
    const { host: host2, calls: calls2 } = fakeVerbHost({
      list: { stdout: CLI_LIST },
      agents: { stdout: CLI_AGENTS },
      ready: { stdout: CLI_READY },
    });
    await new WgBackend(host2, { dir }).getFleet();
    expect(calls2.some((c) => c.args.includes("viz"))).toBe(false);
  });
});

// ── GetTaskDetail daemon read surface ────────────────────────────────────────

const CLI_SHOW_TEXT = ["Task: t1", "Title: T1", "Status: in-progress", "Completion contract: land"].join(
  "\n",
);

describe("WgBackend.getTaskDetail", () => {
  it("reads WG's own `wg show` text from the daemon over the IPC socket", async () => {
    const dir = mkdtempSync(join(tmpdir(), "wg-detail-"));
    const socket = join(dir, "daemon.sock");
    const daemon = await fakeDaemon(socket, () => ({
      body: JSON.stringify({ ok: true, task_id: "t1", text: CLI_SHOW_TEXT }),
    }));
    try {
      const { host, calls } = fakeVerbHost({});
      const backend = new WgBackend(host, { daemonSocket: socket });
      const detail = await backend.getTaskDetail("t1", { columns: 80 });
      expect(detail?.source).toBe("daemon");
      expect(detail?.task_id).toBe("t1");
      expect(detail?.text).toBe(CLI_SHOW_TEXT);
      // The daemon answered, so the CLI must not be shelled at all.
      expect(calls).toHaveLength(0);
      // The request carries the read-only cmd + width.
      expect(daemon.requests[0].cmd).toBe("get_task_detail");
      expect(daemon.requests[0].task_id).toBe("t1");
      expect(daemon.requests[0].columns).toBe(80);
    } finally {
      await daemon.close();
    }
  });

  it("falls back to `wg show <task>` when no daemon socket exists", async () => {
    const dir = mkdtempSync(join(tmpdir(), "wg-detail-"));
    const { host, calls } = fakeVerbHost({ show: { stdout: CLI_SHOW_TEXT } });
    const backend = new WgBackend(host, { dir });
    const detail = await backend.getTaskDetail("t1");
    expect(detail?.source).toBe("cli");
    expect(detail?.text).toBe(CLI_SHOW_TEXT);
    const showCall = calls.find((c) => c.args.includes("show"));
    expect(showCall?.args).toContain("t1");
  });

  it("falls back to the CLI on a daemon protocol mismatch (older daemon)", async () => {
    const dir = mkdtempSync(join(tmpdir(), "wg-detail-"));
    const socket = join(dir, "daemon.sock");
    const daemon = await fakeDaemon(socket, () => ({
      // ok, but no `text` — an older daemon without get_task_detail.
      body: JSON.stringify({ ok: true, task_id: "t1" }),
    }));
    try {
      const { host } = fakeVerbHost({ show: { stdout: CLI_SHOW_TEXT } });
      const backend = new WgBackend(host, { daemonSocket: socket });
      const detail = await backend.getTaskDetail("t1");
      expect(detail?.source).toBe("cli");
      expect(detail?.text).toBe(CLI_SHOW_TEXT);
    } finally {
      await daemon.close();
    }
  });

  it("returns null when an empty id is given", async () => {
    const { host } = fakeVerbHost({});
    const backend = new WgBackend(host, { dir: mkdtempSync(join(tmpdir(), "wg-detail-")) });
    expect(await backend.getTaskDetail("   ")).toBeNull();
  });
});
