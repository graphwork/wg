import { describe, expect, it, vi } from "vitest";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { installCompactionContinuation } from "../pi-worksgood/continuation.js";

type Handler = (event: any, ctx: any) => Promise<any> | any;

function harness(
  overrides: Record<string, string | undefined> = {},
  hostVersion = "0.83.0",
) {
  const handlers = new Map<string, Handler[]>();
  const sent: any[] = [];
  const calls: string[] = [];
  const pi = {
    on: vi.fn((name: string, handler: Handler) => {
      handlers.set(name, [...(handlers.get(name) ?? []), handler]);
    }),
    sendMessage: vi.fn((message: any, options: any) => {
      calls.push("send");
      sent.push({ message, options });
    }),
  };
  const backend = {
    env: { taskId: "task-a" },
    compactionKickAuthorize: vi.fn(async (request: any) => {
      calls.push("authorize");
      return {
        actionId: `action-${request.compactionEntryId}`,
        occurrenceId: `occurrence-${request.compactionEntryId}`,
        state: "authorized",
      };
    }),
    compactionKickPermit: vi.fn(async (actionId: string) => {
      calls.push("permit");
      return {
        actionId,
        state: "delivery_permitted",
        freshDeliveryGrant: true,
        prompt: "[WG_PI_COMPACTION_KICK_V1] finish the exact unresolved WG task",
        promptVersion: "WG_PI_COMPACTION_KICK_V1",
        promptDigest: "b3:prompt",
      };
    }),
    compactionKickAck: vi.fn(async (actionId: string) => {
      calls.push(`ack:${actionId}`);
      return { actionId, state: "acknowledged", abort: false };
    }),
    compactionKickTerminalWatch: vi.fn(async (
      actionId: string,
      watchSequence: number,
      waitMs: number,
    ) => {
      calls.push(`terminal-watch:${actionId}:${watchSequence}`);
      return { actionId, abort: false, settled: waitMs > 0, timedOut: false };
    }),
    compactionKickCancel: vi.fn(async (actionId: string, reason: string) => {
      calls.push(`cancel:${actionId}:${reason}`);
      return { actionId, state: "cancelled" };
    }),
    compactionKickSettle: vi.fn(async (actionId: string) => {
      calls.push(`settle:${actionId}`);
      return { actionId, state: "settled_after_kick" };
    }),
    compactionKickAbortAck: vi.fn(async (actionId: string) => {
      calls.push(`abort-ack:${actionId}`);
      return { actionId, state: "terminal_abort_acknowledged" };
    }),
    compactionKickEffectBegin: vi.fn(async (actionId: string, toolCallId: string) => {
      calls.push(`effect-begin:${actionId}:${toolCallId}`);
      return { actionId, state: "running" };
    }),
    compactionKickEffectEnd: vi.fn(async (actionId: string, toolCallId: string) => {
      calls.push(`effect-end:${actionId}:${toolCallId}`);
      return { actionId, state: "running" };
    }),
  };
  const env = {
    WG_TASK_ID: "task-a",
    WG_AGENT_ID: "agent-a",
    WG_WORKER_CAPABILITY: "opaque-capability",
    WG_PI_TASK_WORKER: "1",
    WG_PI_COMPACTION_KICK: "1",
    WG_PI_COMPACTION_KICK_HOST_CONTRACT: "pi-0.83-session-compact-sync-v1",
    WG_PI_PLUGIN_COMPAT_VERSION: "0.3.0",
    ...overrides,
  };
  installCompactionContinuation(pi as any, backend as any, env, hostVersion);
  let pending = false;
  let idle = false;
  let leaf = "entry-1";
  let pendingReads = 0;
  let pendingReadHook: ((count: number) => void) | undefined;
  const ctx = {
    mode: "json",
    isIdle: () => idle,
    hasPendingMessages: () => {
      pendingReads += 1;
      pendingReadHook?.(pendingReads);
      return pending;
    },
    model: { provider: "fake-provider", id: "fake-model" },
    thinkingLevel: "high",
    sessionManager: {
      getSessionId: () => "session-a",
      getSessionFile: () => "/tmp/session-a.jsonl",
      getLeafId: () => leaf,
      getLeafEntry: () => ({
        type: "compaction",
        id: leaf,
        parentId: leaf === "entry-2" ? "assistant-2" : "assistant-1",
      }),
      getEntries: () => [{
        type: "compaction",
        id: leaf,
        parentId: leaf === "entry-2" ? "assistant-2" : "assistant-1",
      }],
    },
    abort: vi.fn(),
  };
  async function emit(name: string, event: any = {}) {
    for (const handler of handlers.get(name) ?? []) await handler({ type: name, ...event }, ctx);
  }
  return {
    pi,
    backend,
    calls,
    sent,
    ctx,
    emit,
    setPending(value: boolean) { pending = value; },
    setIdle(value: boolean) { idle = value; },
    setLeaf(value: string) { leaf = value; },
    onPendingRead(hook: (count: number) => void) { pendingReadHook = hook; },
  };
}

function compaction(id = "entry-1", parentId = "assistant-1") {
  return {
    reason: "threshold",
    willRetry: false,
    compactionEntry: {
      type: "compaction",
      id,
      parentId,
      timestamp: "2026-08-06T00:00:00Z",
      summary: "untrusted summary must not cross the broker",
      firstKeptEntryId: "user-1",
      tokensBefore: 1700,
    },
  };
}

describe("authoritative threshold-compaction continuation", () => {
  it("RED: sends and acknowledges exactly one same-stack followUp for the qualifying empty-queue gap", async () => {
    const h = harness();
    await h.emit("agent_start");
    await h.emit("agent_end");
    h.onPendingRead((count) => {
      if (count === 3) {
        h.calls.push("final-queue-read");
        queueMicrotask(() => h.calls.push("boundary-microtask"));
      }
    });
    await h.emit("session_compact", compaction());

    expect(h.backend.compactionKickAuthorize).toHaveBeenCalledTimes(1);
    expect(h.backend.compactionKickAuthorize.mock.calls[0][0]).not.toHaveProperty("summary");
    expect(h.backend.compactionKickPermit).toHaveBeenCalledTimes(1);
    expect(h.sent).toHaveLength(1);
    expect(h.sent[0]).toEqual({
      message: {
        customType: "wg-pi-compaction-kick",
        content: "[WG_PI_COMPACTION_KICK_V1] finish the exact unresolved WG task",
        display: false,
        details: {
          actionId: "action-entry-1",
          promptVersion: "WG_PI_COMPACTION_KICK_V1",
          promptDigest: "b3:prompt",
        },
      },
      options: { deliverAs: "followUp", triggerTurn: true },
    });
    expect(h.calls.indexOf("terminal-watch:action-entry-1:1"))
      .toBeLessThan(h.calls.indexOf("final-queue-read"));
    expect(h.calls.indexOf("final-queue-read")).toBeLessThan(h.calls.indexOf("send"));
    expect(h.calls.indexOf("send")).toBeLessThan(h.calls.indexOf("boundary-microtask"));

    await h.emit("session_compact", compaction());
    expect(h.sent).toHaveLength(1);

    await h.emit("message_start", {
      message: {
        role: "custom",
        customType: "wg-pi-compaction-kick",
        content: "[WG_PI_COMPACTION_KICK_V1] finish the exact unresolved WG task",
        details: {
          actionId: "action-entry-1",
          promptVersion: "WG_PI_COMPACTION_KICK_V1",
          promptDigest: "b3:prompt",
        },
      },
    });
    expect(h.backend.compactionKickAck).toHaveBeenCalledTimes(1);
  });

  it("permits distinct descendant compactions but deduplicates each occurrence", async () => {
    const h = harness();
    await h.emit("agent_start");
    await h.emit("agent_end");
    await h.emit("session_compact", compaction("entry-1", "assistant-1"));
    await h.emit("message_start", { message: h.sent[0].message });
    await h.emit("agent_start");
    await h.emit("agent_end");
    h.setLeaf("entry-2");
    // Pi 0.83 reuses the prior callback object's compactionEntry for a nested
    // post-followUp compaction; the persisted append tail is nevertheless E2.
    await h.emit("session_compact", compaction("entry-1", "assistant-1"));
    await h.emit("session_compact", compaction("entry-1", "assistant-1"));
    expect(h.sent.map((value) => value.message.details.actionId)).toEqual([
      "action-entry-1",
      "action-entry-2",
    ]);
  });

  it.each([
    ["manual", false, false],
    ["overflow", true, false],
  ])("suppresses %s compaction", async (reason, willRetry) => {
    const h = harness();
    await h.emit("agent_start");
    await h.emit("agent_end");
    await h.emit("session_compact", { ...compaction(), reason, willRetry });
    expect(h.backend.compactionKickAuthorize).not.toHaveBeenCalled();
    expect(h.sent).toHaveLength(0);
  });

  it.each([
    { aborted: true },
    { errorMessage: "Auto-compaction failed: fixture" },
  ])("suppresses explicit failed/aborted host outcome $aborted$errorMessage", async (outcome) => {
    const h = harness();
    await h.emit("agent_start");
    await h.emit("agent_end");
    await h.emit("session_compact", { ...compaction(), ...outcome });
    expect(h.backend.compactionKickAuthorize).not.toHaveBeenCalled();
    expect(h.sent).toHaveLength(0);
  });

  it("normal final-answer settlement without session_compact creates no action", async () => {
    const h = harness();
    await h.emit("agent_start");
    await h.emit("agent_end");
    await h.emit("agent_settled");
    expect(h.backend.compactionKickAuthorize).not.toHaveBeenCalled();
    expect(h.backend.compactionKickPermit).not.toHaveBeenCalled();
    expect(h.sent).toHaveLength(0);
  });

  it("suppresses queued, post-settled, active-tool, interactive, and non-worker cases", async () => {
    for (const setup of [
      async (h: ReturnType<typeof harness>) => { h.setPending(true); await h.emit("agent_start"); await h.emit("agent_end"); },
      async (h: ReturnType<typeof harness>) => { h.setIdle(true); await h.emit("agent_start"); await h.emit("agent_end"); },
      async (h: ReturnType<typeof harness>) => { await h.emit("agent_start"); await h.emit("tool_execution_start", { toolCallId: "t" }); await h.emit("agent_end"); },
    ]) {
      const h = harness();
      await setup(h);
      await h.emit("session_compact", compaction());
      expect(h.sent).toHaveLength(0);
    }
    const settled = harness();
    await settled.emit("agent_start");
    await settled.emit("agent_end");
    await settled.emit("agent_settled");
    await settled.emit("session_compact", compaction());
    expect(settled.backend.compactionKickAuthorize).not.toHaveBeenCalled();

    const interactive = harness();
    interactive.ctx.mode = "interactive";
    await interactive.emit("agent_start");
    await interactive.emit("agent_end");
    await interactive.emit("session_compact", compaction());
    expect(interactive.backend.compactionKickAuthorize).not.toHaveBeenCalled();

    for (const overrides of [
      { WG_WORKER_CAPABILITY: undefined },
      { WG_PI_TASK_WORKER: undefined },
      { WG_TASK_ID: undefined },
    ]) {
      const unmanaged = harness(overrides);
      await unmanaged.emit("agent_start");
      await unmanaged.emit("agent_end");
      await unmanaged.emit("session_compact", compaction());
      expect(unmanaged.backend.compactionKickAuthorize).not.toHaveBeenCalled();
    }

    for (const hostVersion of ["0.84.0", ""]) {
      const unsupported = harness({}, hostVersion);
      await unsupported.emit("agent_start");
      await unsupported.emit("agent_end");
      await unsupported.emit("session_compact", compaction());
      expect(unsupported.backend.compactionKickAuthorize).not.toHaveBeenCalled();
    }
  });

  it.each(["wg_done", "wg_fail", "wg_wait"])(
    "suppresses accepted %s before authorize and before permit",
    async (terminal) => {
      const beforeAuthorize = harness();
      beforeAuthorize.backend.compactionKickAuthorize.mockRejectedValueOnce(
        new Error(`compaction_kick.lifecycle_resolved_or_held:${terminal}`),
      );
      await beforeAuthorize.emit("agent_start");
      await beforeAuthorize.emit("agent_end");
      await beforeAuthorize.emit("session_compact", compaction());
      expect(beforeAuthorize.backend.compactionKickPermit).not.toHaveBeenCalled();
      expect(beforeAuthorize.sent).toHaveLength(0);

      const beforePermit = harness();
      beforePermit.backend.compactionKickPermit.mockRejectedValueOnce(
        new Error(`attempt_already_terminal:${terminal}`),
      );
      await beforePermit.emit("agent_start");
      await beforePermit.emit("agent_end");
      await beforePermit.emit("session_compact", compaction());
      expect(beforePermit.backend.compactionKickAuthorize).toHaveBeenCalledTimes(1);
      expect(beforePermit.sent).toHaveLength(0);
    },
  );

  it("suppresses loudly when the shared continuation budget is exhausted", async () => {
    const h = harness();
    h.backend.compactionKickAuthorize.mockRejectedValueOnce(
      new Error("continuation_budget_exhausted"),
    );
    await h.emit("agent_start");
    await h.emit("agent_end");
    await h.emit("session_compact", compaction());
    expect(h.backend.compactionKickPermit).not.toHaveBeenCalled();
    expect(h.sent).toHaveLength(0);
  });

  it("gates and closes every non-terminal recovery effect", async () => {
    const h = harness();
    await h.emit("agent_start");
    await h.emit("agent_end");
    await h.emit("session_compact", compaction());
    await h.emit("message_start", {
      message: { role: "custom", ...h.sent[0].message },
    });

    const handler = (h.pi.on.mock.calls as any[])
      .find(([name]) => name === "tool_call")?.[1];
    const allowed = await handler(
      { type: "tool_call", toolName: "bash", toolCallId: "tool-1" }, h.ctx,
    );
    expect(allowed).toBeUndefined();
    expect(h.backend.compactionKickEffectBegin).toHaveBeenCalledWith(
      "action-entry-1", "tool-1",
    );
    await h.emit("tool_result", { toolName: "bash", toolCallId: "tool-1" });
    expect(h.backend.compactionKickEffectEnd).toHaveBeenCalledWith(
      "action-entry-1", "tool-1",
    );
  });

  it("aborts fail-closed when acknowledgement truth cannot be refreshed", async () => {
    const h = harness();
    await h.emit("agent_start");
    await h.emit("agent_end");
    await h.emit("session_compact", compaction());
    h.backend.compactionKickAck.mockRejectedValueOnce(
      new Error("worker_control.compaction_ack_replay_held"),
    );
    await h.emit("message_start", {
      message: { role: "custom", ...h.sent[0].message },
    });
    expect(h.ctx.abort).toHaveBeenCalledTimes(1);
    expect(h.backend.compactionKickEffectBegin).not.toHaveBeenCalled();
  });

  it("opens cancellation watch before send and aborts an externally accepted terminal", async () => {
    const h = harness();
    h.backend.compactionKickTerminalWatch
      .mockResolvedValueOnce({
        actionId: "action-entry-1", abort: false, settled: false, timedOut: false,
      })
      .mockResolvedValueOnce({
        actionId: "action-entry-1", abort: true, settled: false, timedOut: false,
      });
    await h.emit("agent_start");
    await h.emit("agent_end");
    await h.emit("session_compact", compaction());
    await Promise.resolve();
    expect(h.calls.indexOf("terminal-watch:action-entry-1:0"))
      .toBeLessThan(h.calls.indexOf("send"));
    expect(h.ctx.abort).toHaveBeenCalledTimes(1);
    expect(h.backend.compactionKickCancel).toHaveBeenCalledWith(
      "action-entry-1", "terminal_watch_before_ack",
    );
  });

  it("aborts and acknowledges an accepted terminal observed after delivery ack", async () => {
    const h = harness();
    let releaseWatch!: (status: any) => void;
    h.backend.compactionKickTerminalWatch
      .mockResolvedValueOnce({
        actionId: "action-entry-1", abort: false, settled: false, timedOut: false,
      })
      .mockImplementationOnce(async () => new Promise((resolve) => {
        releaseWatch = resolve;
      }));
    await h.emit("agent_start");
    await h.emit("agent_end");
    await h.emit("session_compact", compaction());
    await h.emit("message_start", {
      message: { role: "custom", ...h.sent[0].message },
    });
    releaseWatch({
      actionId: "action-entry-1", abort: true, settled: false, timedOut: false,
    });
    await new Promise((resolve) => setImmediate(resolve));
    expect(h.ctx.abort).toHaveBeenCalledTimes(1);
    expect(h.backend.compactionKickAbortAck).toHaveBeenCalledWith("action-entry-1");
    expect(h.backend.compactionKickCancel).not.toHaveBeenCalled();
  });

  it("aborts when terminal won after permit but before acknowledgement", async () => {
    const h = harness();
    await h.emit("agent_start");
    await h.emit("agent_end");
    await h.emit("session_compact", compaction());
    h.backend.compactionKickAck.mockResolvedValueOnce({
      actionId: "action-entry-1",
      state: "acknowledged_terminal_race",
      abort: true,
    });
    await h.emit("message_start", {
      message: { role: "custom", ...h.sent[0].message },
    });
    expect(h.ctx.abort).toHaveBeenCalledTimes(1);
    expect(h.backend.compactionKickAbortAck).toHaveBeenCalledWith("action-entry-1");
  });

  it("blocks a recovery tool when the lifecycle effect lease loses the terminal race", async () => {
    const h = harness();
    await h.emit("agent_start");
    await h.emit("agent_end");
    await h.emit("session_compact", compaction());
    await h.emit("message_start", {
      message: { role: "custom", ...h.sent[0].message },
    });
    h.backend.compactionKickEffectBegin.mockRejectedValueOnce(new Error("kick_action_revoked"));
    const handler = (h.pi.on.mock.calls as any[])
      .find(([name]) => name === "tool_call")?.[1];
    const result = await handler(
      { type: "tool_call", toolName: "write", toolCallId: "tool-race" }, h.ctx,
    );
    expect(result).toEqual({
      block: true,
      reason: "WG terminal/effect interlock refused this tool",
    });
    expect(h.backend.compactionKickEffectEnd).not.toHaveBeenCalled();
  });

  it("suppresses after permit when a real message arrived while the broker was awaited", async () => {
    const h = harness();
    h.backend.compactionKickPermit.mockImplementationOnce(async (actionId: string) => {
      h.setPending(true);
      return {
        actionId,
        state: "delivery_permitted",
        freshDeliveryGrant: true,
        prompt: "prompt",
        promptVersion: "WG_PI_COMPACTION_KICK_V1",
        promptDigest: "b3:prompt",
      };
    });
    await h.emit("agent_start");
    await h.emit("agent_end");
    await h.emit("session_compact", compaction());
    expect(h.sent).toHaveLength(0);
    expect(h.backend.compactionKickCancel).toHaveBeenCalledWith("action-entry-1", "queue_nonempty_after_permit");
  });
});
