/**
 * completion-watcher tests — transition detection, dedup/cursor persistence,
 * config gating, and the install wiring.
 *
 * All unit tests use fake graph snapshots — no live daemon, no `wg` binary.
 * The real-graph integration lives in `completion-watcher-integration.test.ts`.
 *
 * Tests run against the built `pi-worksgood/` artifact — `npm test` builds first.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import {
  CompletionWatcher,
  DEFAULT_COMPLETION_WAKE_CONFIG,
  MemoryCursorStore,
  fileCursorStore,
  formatWakeMessage,
  installCompletionWatcher,
  isInternalTask,
  isTopLevelTask,
  kindOf,
  planWakes,
  readCompletionWakeConfig,
  wakeNotifyLevel,
  wakePresentation,
} from "../pi-worksgood/index.js";

const TASKS = {
  root: { id: "root", title: "Root task", status: "open", after: [] },
  dep: { id: "dep", title: "Dependency", status: "open", after: [] },
  child: { id: "child", title: "Child task", status: "open", after: ["dep"] },
  internal: { id: ".evaluate-root", title: "Agency", status: "open", after: [] },
};

function ids(wakes: Array<{ taskId: string; kind: string }>) {
  return wakes.map((w) => `${w.taskId}:${w.kind}`);
}

describe("planWakes — transition detection + scope gating", () => {
  const cfg = () => ({ ...DEFAULT_COMPLETION_WAKE_CONFIG });

  it("establishes a silent baseline on the first read (never replays history)", () => {
    const tasks = [
      { ...TASKS.root, status: "done" },
      { ...TASKS.dep, status: "failed" },
    ];
    const plan = planWakes(null, tasks, cfg());
    expect(plan.wakes).toEqual([]);
    expect(plan.nextStatuses).toEqual({ root: "done", dep: "failed" });
  });

  it("wakes on a top-level completion by default", () => {
    const prev = { root: "open" };
    const plan = planWakes(prev, [{ ...TASKS.root, status: "done" }], cfg());
    expect(ids(plan.wakes)).toEqual(["root:completed"]);
    expect(plan.wakes[0]!.from).toBe("open");
    expect(plan.wakes[0]!.to).toBe("done");
    expect(plan.wakes[0]!.topLevel).toBe(true);
  });

  it("keeps a dependent completion quiet by default and wakes it under 'all'", () => {
    const tasks = [
      { ...TASKS.dep, status: "done" },
      { ...TASKS.child, status: "done" },
    ];
    const quiet = planWakes({ dep: "open", child: "open" }, tasks, cfg());
    expect(ids(quiet.wakes)).toEqual(["dep:completed"]);
    const loud = planWakes({ dep: "open", child: "open" }, tasks, {
      ...cfg(),
      completions: "all",
    });
    expect(ids(loud.wakes).sort()).toEqual(["child:completed", "dep:completed"]);
  });

  it("always surfaces a failure, even for a dependent task", () => {
    const tasks = [
      { ...TASKS.dep, status: "done" },
      { ...TASKS.child, status: "failed" },
    ];
    const plan = planWakes({ dep: "done", child: "in-progress" }, tasks, cfg());
    expect(ids(plan.wakes)).toEqual(["child:failed"]);
    // ... unless the per-session mute is on.
    const muted = planWakes({ child: "in-progress" }, tasks, { ...cfg(), quiet: true });
    expect(muted.wakes).toEqual([]);
  });

  it("wakes blocked/waiting top-level by default, off when attention=off", () => {
    const tasks = [{ ...TASKS.root, status: "blocked" }];
    expect(ids(planWakes({ root: "open" }, tasks, cfg()).wakes)).toEqual(["root:attention"]);
    expect(planWakes({ root: "open" }, tasks, { ...cfg(), attention: "off" }).wakes).toEqual([]);
  });

  it("wakes a deliberate abandonment as its own quiet kind, not a failure", () => {
    const tasks = [{ ...TASKS.root, status: "abandoned" }];
    const plan = planWakes({ root: "open" }, tasks, cfg());
    expect(ids(plan.wakes)).toEqual(["root:abandoned"]);
    expect(plan.wakes[0]!.kind).not.toBe("failed");
    // Scope-filtered like completions (top-level default), not always-on.
    const child = planWakes(
      { dep: "done", child: "open" },
      [{ ...TASKS.dep, status: "done" }, { ...TASKS.child, status: "abandoned" }],
      cfg(),
    );
    expect(child.wakes).toEqual([]);
    // ... and independently disable-able without touching failures.
    expect(planWakes({ root: "open" }, tasks, { ...cfg(), abandoned: "off" }).wakes).toEqual([]);
    // `failures: false` no longer silences abandonment; only `failed` is gated by it.
    expect(ids(planWakes({ root: "open" }, tasks, { ...cfg(), failures: false }).wakes)).toEqual([
      "root:abandoned",
    ]);
  });

  it("never wakes internal ids and never wakes in-flight transitions", () => {
    const tasks = [
      { ...TASKS.internal, status: "done" },
      { ...TASKS.root, status: "in-progress" },
    ];
    const plan = planWakes({ ".evaluate-root": "open", root: "open" }, tasks, cfg());
    expect(plan.wakes).toEqual([]);
    // internal ids are not even tracked in the cursor.
    expect(plan.nextStatuses[".evaluate-root"]).toBeUndefined();
    expect(plan.nextStatuses.root).toBe("in-progress");
  });

  it("wakes a task first seen already-terminal after the baseline", () => {
    const plan = planWakes({}, [{ ...TASKS.root, status: "done" }], cfg());
    expect(ids(plan.wakes)).toEqual(["root:completed"]);
    expect(plan.wakes[0]!.from).toBe("(new)");
  });

  it("does not wake when the master switch is off", () => {
    const plan = planWakes({ root: "open" }, [{ ...TASKS.root, status: "done" }], {
      ...cfg(),
      enabled: false,
    });
    expect(plan.wakes).toEqual([]);
    expect(plan.nextStatuses.root).toBe("done");
  });

  it("classifies top-level and internal tasks", () => {
    const byId = new Map([
      [TASKS.dep.id, TASKS.dep],
      [TASKS.child.id, TASKS.child],
    ]);
    expect(isTopLevelTask(TASKS.dep, byId)).toBe(true);
    expect(isTopLevelTask(TASKS.child, byId)).toBe(false);
    expect(isInternalTask(".chat-3")).toBe(true);
    expect(isInternalTask("normal-task")).toBe(false);
  });
});

describe("kindOf — status → wake kind mapping", () => {
  it("maps terminal/needs-attention statuses and rejects in-flight ones", () => {
    expect(kindOf("failed")).toBe("failed");
    expect(kindOf("abandoned")).toBe("abandoned");
    expect(kindOf("done")).toBe("completed");
    expect(kindOf("blocked")).toBe("attention");
    expect(kindOf("waiting")).toBe("attention");
    expect(kindOf("incomplete")).toBe("attention");
    for (const inFlight of ["open", "in-progress", "pending-validation", "whatever"]) {
      expect(kindOf(inFlight)).toBeNull();
    }
  });

  it("never folds abandonment into the failure kind", () => {
    expect(kindOf("abandoned")).not.toBe(kindOf("failed"));
  });
});

describe("wake presentation — glyph, label, notify level per kind", () => {
  it("gives every kind a distinct glyph and an honest label", () => {
    const failed = wakePresentation("failed");
    const abandoned = wakePresentation("abandoned");
    const completed = wakePresentation("completed");
    const attention = wakePresentation("attention");

    expect(failed).toEqual({ glyph: "✗", label: "failed", notify: "warning" });
    expect(abandoned).toEqual({ glyph: "⊘", label: "abandoned", notify: "info" });
    expect(completed.glyph).toBe("✓");
    expect(completed.notify).toBe("info");
    expect(attention.glyph).toBe("⏸");
    expect(attention.label).toBe("needs attention");
    expect(attention.notify).toBe("info");
    // Distinct glyphs so abandonment can never read as a failure.
    expect(abandoned.glyph).not.toBe(failed.glyph);
    expect(new Set([failed.glyph, abandoned.glyph, completed.glyph, attention.glyph]).size).toBe(4);
  });

  it("only a genuine failure alarms", () => {
    expect(wakeNotifyLevel("failed")).toBe("warning");
    expect(wakeNotifyLevel("abandoned")).toBe("info");
    expect(wakeNotifyLevel("completed")).toBe("info");
    expect(wakeNotifyLevel("attention")).toBe("info");
  });

  it("formats abandoned distinctly from failed in both message and level", () => {
    const base = {
      taskId: "stale-scaffold",
      title: "Stale scaffold",
      from: "open",
      to: "abandoned",
      topLevel: true,
    };
    const abandonedWake = { ...base, kind: "abandoned" as const };
    const failedWake = { ...base, kind: "failed" as const, to: "failed" };
    const detail = { failure_reason: "operator triaged the stale scaffold" };

    const abandonedMsg = formatWakeMessage(abandonedWake, detail);
    const failedMsg = formatWakeMessage(failedWake, detail);

    // Own glyph + label; no ✗ alarm and no "failed" label on abandonment.
    expect(abandonedMsg).toContain("⊘ stale-scaffold abandoned (open → abandoned)");
    expect(abandonedMsg).not.toContain("✗");
    expect(abandonedMsg).not.toContain("stale-scaffold failed");
    expect(abandonedMsg).toContain("Note: operator triaged the stale scaffold");

    expect(failedMsg).toContain("✗ stale-scaffold failed (open → failed)");
    expect(failedMsg).toContain("Reason: operator triaged the stale scaffold");

    expect(failedMsg).not.toEqual(abandonedMsg);
    expect(wakeNotifyLevel("abandoned")).not.toBe(wakeNotifyLevel("failed"));
    expect(wakeNotifyLevel("abandoned")).toBe("info");
    expect(wakeNotifyLevel("failed")).toBe("warning");
  });

  it("falls back to the last log for an abandonment with no recorded reason", () => {
    const msg = formatWakeMessage(
      { taskId: "t", title: "T", kind: "abandoned", from: "open", to: "abandoned", topLevel: true },
      { last_log: "Task abandoned" },
    );
    expect(msg).toContain("Summary: Task abandoned");
    expect(msg).not.toContain("Reason:");
  });
});

describe("formatWakeMessage", () => {
  const wake = {
    taskId: "int-task",
    title: "Integration task",
    kind: "completed" as const,
    from: "in-progress",
    to: "done",
    topLevel: true,
  };

  it("is actionable: id, title, status, receipt, and the wg-tools detail pointer", () => {
    const msg = formatWakeMessage(wake, {
      completion_receipt: "b3:deadbeef",
      completion_disposition: "landed",
      actual_model: "pi:lunaroute:glm",
    });
    expect(msg).toContain("int-task");
    expect(msg).toContain("Integration task");
    expect(msg).toContain("done");
    expect(msg).toContain("b3:deadbeef");
    expect(msg).toContain("wg_show");
    expect(msg).toContain("/wg graph");
  });

  it("cites the failure reason for a failure", () => {
    const msg = formatWakeMessage(
      { ...wake, kind: "failed", to: "failed" },
      { failure_reason: "Provider failure detected after streaming: timeout" },
    );
    expect(msg).toContain("failed");
    expect(msg).toContain("timeout");
  });

  it("still carries the detail pointer with no enrichment", () => {
    const msg = formatWakeMessage(wake, null);
    expect(msg).toContain("wg_show");
  });
});

describe("readCompletionWakeConfig", () => {
  it("returns the safe defaults for an empty environment", () => {
    expect(readCompletionWakeConfig({})).toEqual(DEFAULT_COMPLETION_WAKE_CONFIG);
  });

  it("honors explicit overrides", () => {
    const cfg = readCompletionWakeConfig({
      WG_PI_COMPLETION_WAKES: "off",
      WG_PI_COMPLETION_FAILURES: "off",
      WG_PI_COMPLETION_ABANDONED: "off",
      WG_PI_COMPLETION_COMPLETIONS: "all",
      WG_PI_COMPLETION_ATTENTION: "off",
      WG_PI_COMPLETION_QUIET: "on",
      WG_PI_COMPLETION_INTERVAL_MS: "5000",
    });
    expect(cfg).toEqual({
      enabled: false,
      failures: false,
      abandoned: "off",
      completions: "all",
      attention: "off",
      quiet: true,
      intervalMs: 5000,
    });
  });

  it("falls back to defaults on invalid values (never throws)", () => {
    const cfg = readCompletionWakeConfig({
      WG_PI_COMPLETION_COMPLETIONS: "everythings",
      WG_PI_COMPLETION_INTERVAL_MS: "not-a-number",
    });
    expect(cfg.completions).toBe("top-level");
    expect(cfg.intervalMs).toBe(DEFAULT_COMPLETION_WAKE_CONFIG.intervalMs);
  });
});

describe("cursor stores", () => {
  it("fileCursorStore round-trips atomically and tolerates missing/corrupt files", async () => {
    const dir = await mkdtemp(join(tmpdir(), "wg-wake-cursor-"));
    const path = join(dir, "cursor.json");
    const store = fileCursorStore(path);
    expect(await store.load()).toBeNull(); // missing

    await store.save({ version: 1, initialized: true, statuses: { a: "done" } });
    expect(await store.load()).toEqual({ version: 1, initialized: true, statuses: { a: "done" } });
    expect(JSON.parse(await readFile(path, "utf8"))).toEqual({
      version: 1,
      initialized: true,
      statuses: { a: "done" },
    });
    expect(existsSync(`${path}.tmp`)).toBe(false);

    await writeFile(path, "{ not json", "utf8");
    expect(await store.load()).toBeNull(); // corrupt
  });
});

describe("CompletionWatcher — dedup, persistence, and bounds", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  const cfg = (over: object = {}) => ({ ...DEFAULT_COMPLETION_WAKE_CONFIG, ...over });

  it("announces a transition exactly once; a repeated read never re-announces", async () => {
    let tasks = [{ ...TASKS.root, status: "open" }];
    const delivered: string[] = [];
    const store = new MemoryCursorStore();
    const watcher = new CompletionWatcher(
      async () => tasks,
      store,
      (_wake, message) => {
        delivered.push(message);
      },
      cfg(),
    );
    await watcher.refresh(); // baseline
    expect(delivered).toEqual([]);

    tasks = [{ ...TASKS.root, status: "done" }];
    await watcher.refresh();
    expect(delivered).toHaveLength(1);
    await watcher.refresh();
    await watcher.refresh();
    expect(delivered).toHaveLength(1);
    expect((await store.load())?.statuses.root).toBe("done");
  });

  it("re-announces after a re-open then a second completion", async () => {
    let tasks = [{ ...TASKS.root, status: "done" }];
    const delivered: string[] = [];
    const watcher = new CompletionWatcher(
      async () => tasks,
      new MemoryCursorStore(),
      (_wake, message) => {
        delivered.push(message);
      },
      cfg(),
    );
    await watcher.refresh(); // baseline = done
    tasks = [{ ...TASKS.root, status: "open" }];
    await watcher.refresh(); // reopen: no wake
    expect(delivered).toEqual([]);
    tasks = [{ ...TASKS.root, status: "done" }];
    await watcher.refresh(); // second completion: wake
    expect(delivered).toHaveLength(1);
  });

  it("persists across a restart via the cursor store (no re-announce)", async () => {
    const store = new MemoryCursorStore();
    let tasks = [{ ...TASKS.root, status: "open" }];
    const first = new CompletionWatcher(async () => tasks, store, () => undefined, cfg());
    await first.refresh(); // baseline open
    tasks = [{ ...TASKS.root, status: "done" }];
    await first.refresh(); // announce + persist done

    const delivered: string[] = [];
    const second = new CompletionWatcher(
      async () => tasks,
      store,
      (_wake, message) => {
        delivered.push(message);
      },
      cfg(),
    );
    await second.start();
    second.stop();
    expect(delivered).toEqual([]);
  });

  it("bounds reads: a refresh while one is in flight is a no-op", async () => {
    let resolveRead: ((v: unknown[]) => void) | null = null;
    const readTasks = vi.fn(
      () =>
        new Promise<unknown[]>((resolve) => {
          resolveRead = resolve as (v: unknown[]) => void;
        }),
    );
    const watcher = new CompletionWatcher(
      readTasks as never,
      new MemoryCursorStore(),
      () => undefined,
      cfg(),
    );
    const a = watcher.refresh();
    const b = watcher.refresh();
    expect(readTasks).toHaveBeenCalledTimes(1);
    resolveRead!([]);
    await Promise.all([a, b]);
  });

  it("survives a failing read without losing the cursor", async () => {
    let failing = false;
    let tasks = [{ ...TASKS.root, status: "open" }];
    const delivered: string[] = [];
    const watcher = new CompletionWatcher(
      async () => {
        if (failing) throw new Error("offline");
        return tasks;
      },
      new MemoryCursorStore(),
      (_wake, message) => {
        delivered.push(message);
      },
      cfg(),
    );
    await watcher.refresh(); // baseline open
    failing = true;
    await watcher.refresh(); // read fails: nothing emitted, cursor intact
    failing = false;
    tasks = [{ ...TASKS.root, status: "done" }];
    await watcher.refresh();
    expect(delivered).toHaveLength(1);
  });
});

describe("installCompletionWatcher", () => {
  function makeFakePi() {
    const handlers = new Map<string, (event: unknown, ctx: unknown) => void>();
    const commands = new Map<string, { handler: (args: string, ctx: unknown) => Promise<void> }>();
    const sent: Array<{ msg: Record<string, unknown>; opts: unknown }> = [];
    const pi = {
      registerCommand: vi.fn((name: string, def: never) => commands.set(name, def)),
      on: vi.fn((event: string, handler: (event: unknown, ctx: unknown) => void) =>
        handlers.set(event, handler),
      ),
      sendMessage: vi.fn((msg: Record<string, unknown>, opts: unknown) => sent.push({ msg, opts })),
    };
    return { pi, handlers, commands, sent };
  }

  const ctx = {
    hasUI: true,
    ui: { notify: vi.fn() },
    isIdle: () => true,
    sessionManager: undefined,
  };

  it("registers /wg-wake and the session lifecycle", () => {
    const { pi, commands } = makeFakePi();
    installCompletionWatcher(pi as never, { runJson: async () => null } as never, {} as never);
    expect(commands.has("wg-wake")).toBe(true);
    expect(pi.on).toHaveBeenCalledWith("session_start", expect.any(Function));
    expect(pi.on).toHaveBeenCalledWith("session_shutdown", expect.any(Function));
  });

  it("delivers exactly one wake message for a top-level completion, none on repeat, and honors quiet", async () => {
    vi.useFakeTimers();
    try {
      const { pi, handlers, commands, sent } = makeFakePi();
      let tasks: unknown[] = [{ ...TASKS.root, status: "open" }];
      const backend = {
        runJson: vi.fn(async (args: string[]) => {
          if (args[0] === "list") return tasks;
          if (args[0] === "show") {
            return { completion_receipt: "b3:abc123", completion_disposition: "landed", log: [] };
          }
          return null;
        }),
      };
      installCompletionWatcher(pi as never, backend as never, {} as never, {
        config: { ...DEFAULT_COMPLETION_WAKE_CONFIG, intervalMs: 10 },
      });
      handlers.get("session_start")!({}, ctx);
      await vi.advanceTimersByTimeAsync(30); // baseline
      expect(sent).toHaveLength(0);

      tasks = [{ ...TASKS.root, status: "done" }];
      await vi.advanceTimersByTimeAsync(30);
      expect(sent).toHaveLength(1);
      expect(sent[0]!.msg.customType).toBe("wg-completion");
      expect(sent[0]!.msg.content).toContain("b3:abc123");
      expect(sent[0]!.opts).toEqual({ triggerTurn: true });

      await vi.advanceTimersByTimeAsync(30); // repeat: no re-announce
      expect(sent).toHaveLength(1);

      // Per-session quiet toggle mutes the next transition.
      tasks = [{ ...TASKS.root, status: "open" }];
      await vi.advanceTimersByTimeAsync(30);
      tasks = [{ ...TASKS.root, status: "done" }];
      await commands.get("wg-wake")!.handler("off", ctx);
      await vi.advanceTimersByTimeAsync(30);
      expect(sent).toHaveLength(1);

      handlers.get("session_shutdown")!({}, ctx);
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not poll in json/print worker modes", async () => {
    vi.useFakeTimers();
    try {
      const { pi, handlers } = makeFakePi();
      const backend = { runJson: vi.fn(async () => []) };
      installCompletionWatcher(pi as never, backend as never, {} as never, {
        config: { ...DEFAULT_COMPLETION_WAKE_CONFIG, intervalMs: 10 },
      });
      handlers.get("session_start")!({}, { ...ctx, mode: "json" });
      await vi.advanceTimersByTimeAsync(30);
      expect(backend.runJson).not.toHaveBeenCalled();
      handlers.get("session_shutdown")!({}, ctx);
    } finally {
      vi.useRealTimers();
    }
  });

  it("uses deliverAs followUp when the session is busy (does not force a turn)", async () => {
    vi.useFakeTimers();
    try {
      const { pi, handlers, sent } = makeFakePi();
      const busyCtx = { ...ctx, isIdle: () => false };
      let tasks: unknown[] = [{ ...TASKS.root, status: "open" }];
      const backend = {
        runJson: vi.fn(async (args: string[]) => (args[0] === "list" ? tasks : null)),
      };
      installCompletionWatcher(pi as never, backend as never, {} as never, {
        config: { ...DEFAULT_COMPLETION_WAKE_CONFIG, intervalMs: 10 },
      });
      handlers.get("session_start")!({}, busyCtx);
      await vi.advanceTimersByTimeAsync(20);
      tasks = [{ ...TASKS.root, status: "done" }];
      await vi.advanceTimersByTimeAsync(20);
      expect(sent).toHaveLength(1);
      expect(sent[0]!.opts).toEqual({ deliverAs: "followUp" });
      handlers.get("session_shutdown")!({}, busyCtx);
    } finally {
      vi.useRealTimers();
    }
  });
});
