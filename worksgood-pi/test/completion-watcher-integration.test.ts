/**
 * completion-watcher integration — a REAL WG graph with a scratch session.
 *
 * Drives the actual {@link CompletionWatcher} (and {@link readTaskDetail})
 * against a scratch WG project created and transitioned with the real `wg`
 * binary, through a real exec host. No live daemon and no model credential is
 * needed: the wake sink is a fake Pi session that records what
 * `installCompletionWatcher` would have put in the conversation.
 *
 * A true `pi --mode rpc` scratch session needs a model credential, so it is
 * deferred (see docs/design-pi-completion-wakeups.md §7); the plugin's own load
 * path is pinned credential-free by the `pi_vizview_embedded_panel_contract`
 * smoke scenario, and the wake delivery is pinned against the real
 * `sendMessage`-shaped API by the unit suite.
 *
 * Skips (does not fail) when the `wg` binary is not on PATH.
 */

import { describe, expect, it } from "vitest";
import { execFile } from "node:child_process";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { WgBackend } from "../pi-worksgood/index.js";
// @ts-expect-error — built ESM artifact import (see above)
import {
  CompletionWatcher,
  DEFAULT_COMPLETION_WAKE_CONFIG,
  MemoryCursorStore,
  formatWakeMessage,
  readTaskDetail,
  wakeNotifyLevel,
} from "../pi-worksgood/completion-watcher.js";

interface RunResult {
  code: number;
  stdout: string;
  stderr: string;
}

function run(command: string, args: string[], cwd: string, env: NodeJS.ProcessEnv): Promise<RunResult> {
  return new Promise((resolve) => {
    execFile(command, args, { cwd, env, maxBuffer: 32 * 1024 * 1024 }, (err, stdout, stderr) => {
      const code =
        err && typeof (err as { code?: unknown }).code === "number"
          ? (err as { code: number }).code
          : err
            ? 1
            : 0;
      resolve({ code, stdout: stdout ?? "", stderr: stderr ?? "" });
    });
  });
}

let wgOnPath = true;
try {
  const probe = await run("wg", ["--version"], process.cwd(), process.env);
  wgOnPath = probe.code === 0;
} catch {
  wgOnPath = false;
}

/** A worker-capability-free env bound to an isolated HOME + global WG dir. */
function cleanEnv(home: string): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...process.env };
  for (const key of [
    "WG_AGENT_ID",
    "WG_TASK_ID",
    "WG_EXECUTOR_TYPE",
    "WG_MODEL",
    "WG_REASONING",
    "WG_TIER",
    "WG_WORKER_CAPABILITY",
    "WG_WORKER_CONTROL_MODE",
    "WG_DIR",
    "WG_PROJECT_ROOT",
    "WG_WORKTREE_PATH",
    "WG_WORKTREE_ACTIVE",
    "WG_BRANCH",
    "WG_CHAT_ID",
    "WG_CHAT_REF",
    "WG_PI_PLUGIN_COMPAT_VERSION",
  ]) {
    delete env[key];
  }
  env.HOME = home;
  env.WG_GLOBAL_DIR = join(home, ".wg");
  return env;
}

/** The real `wg` binary as an ExecHost, with a fixed cwd (no `--dir` needed). */
function wgHost(cwd: string, env: NodeJS.ProcessEnv) {
  return {
    async exec(command: string, args: string[]) {
      const r = await run(command, args, cwd, env);
      return { stdout: r.stdout, stderr: r.stderr, code: r.code, killed: false };
    },
  };
}

describe.skipIf(!wgOnPath)("completion watcher against a real graph", () => {
  it(
    "produces exactly one actionable wake for a completing task and never re-announces it",
    async () => {
      const project = await mkdtemp(join(tmpdir(), "wg-wake-it-"));
      const home = join(project, "home");
      await mkdir(home, { recursive: true });
      const env = cleanEnv(home);

      // A real WG project needs a git repo and an initialized graph.
      expect((await run("git", ["init", "-q", "-b", "main"], project, env)).code).toBe(0);
      await run("git", ["config", "user.email", "smoke@example.invalid"], project, env);
      await run("git", ["config", "user.name", "WG Smoke"], project, env);
      await writeFile(join(project, "seed.txt"), "seed\n", "utf8");
      await run("git", ["add", "seed.txt"], project, env);
      expect((await run("git", ["commit", "-q", "-m", "seed"], project, env)).code).toBe(0);

      const host = wgHost(project, env);
      const backend = new WgBackend(host as never, {});
      const init = await backend.run(["init"]);
      expect(init.code, init.stderr).toBe(0);
      // Commit the WG scaffolding so a later completion can settle cleanly.
      await run("git", ["add", "-A"], project, env);
      expect((await run("git", ["commit", "-q", "-m", "wg init"], project, env)).code).toBe(0);

      const add = await backend.run(["add", "Integration task", "--id", "int-task"]);
      expect(add.code, add.stderr).toBe(0);

      const delivered: string[] = [];
      const watcher = new CompletionWatcher(
        () => backend.runJson<Array<{ id: string; title?: string; status: string; after?: string[] }>>(["list"]),
        new MemoryCursorStore(),
        (_wake, message) => {
          delivered.push(message);
        },
        { ...DEFAULT_COMPLETION_WAKE_CONFIG, intervalMs: 3_600_000 },
        (id) => readTaskDetail(backend, id),
      );

      // Baseline: the open task emits nothing.
      await watcher.refresh();
      expect(delivered).toEqual([]);

      // Real transition: open → in-progress → done.
      const claim = await backend.run(["claim", "int-task", "--actor", "smoke-agent"]);
      expect(claim.code, claim.stderr).toBe(0);
      const done = await backend.run(["done", "int-task"]);
      expect(done.code, done.stderr).toBe(0);

      const wakes = await watcher.refresh();
      expect(wakes).toHaveLength(1);
      expect(wakes[0]!.taskId).toBe("int-task");
      expect(wakes[0]!.kind).toBe("completed");
      expect(delivered).toHaveLength(1);
      const message = delivered[0]!;
      expect(message).toContain("int-task");
      expect(message).toContain("Integration task");
      expect(message).toContain("completed");
      // Actionable: a receipt reference and the wg-tools detail pointer.
      expect(message).toMatch(/Receipt: b3:|Detail: call wg_show/);
      expect(message).toContain("wg_show");

      // Repeated poll: no re-announce.
      const again = await watcher.refresh();
      expect(again).toEqual([]);
      expect(delivered).toHaveLength(1);
    },
    180_000,
  );

  it(
    "renders a deliberate abandonment as a quiet ⊘ wake, distinguishable from a failure",
    async () => {
      const project = await mkdtemp(join(tmpdir(), "wg-wake-abandon-"));
      const home = join(project, "home");
      await mkdir(home, { recursive: true });
      const env = cleanEnv(home);

      expect((await run("git", ["init", "-q", "-b", "main"], project, env)).code).toBe(0);
      await run("git", ["config", "user.email", "smoke@example.invalid"], project, env);
      await run("git", ["config", "user.name", "WG Smoke"], project, env);
      await writeFile(join(project, "seed.txt"), "seed\n", "utf8");
      await run("git", ["add", "seed.txt"], project, env);
      expect((await run("git", ["commit", "-q", "-m", "seed"], project, env)).code).toBe(0);

      const host = wgHost(project, env);
      const backend = new WgBackend(host as never, {});
      expect((await backend.run(["init"])).code).toBe(0);
      await run("git", ["add", "-A"], project, env);
      expect((await run("git", ["commit", "-q", "-m", "wg init"], project, env)).code).toBe(0);

      const add = await backend.run(["add", "Stale scaffold", "--id", "stale-scaffold"]);
      expect(add.code, add.stderr).toBe(0);

      const delivered: Array<{ message: string; kind: string; level: string; text: string }> = [];
      const watcher = new CompletionWatcher(
        () =>
          backend.runJson<Array<{ id: string; title?: string; status: string; after?: string[] }>>(
            ["list"],
          ),
        new MemoryCursorStore(),
        (wake, message, detail) => {
          delivered.push({
            message,
            kind: wake.kind,
            level: wakeNotifyLevel(wake.kind),
            text: formatWakeMessage(wake, detail),
          });
        },
        { ...DEFAULT_COMPLETION_WAKE_CONFIG, intervalMs: 3_600_000 },
        (id) => readTaskDetail(backend, id),
      );

      await watcher.refresh(); // baseline: the open task emits nothing
      expect(delivered).toEqual([]);

      // Real transition: open → abandoned (operator triage).
      const abandon = await backend.run([
        "abandon",
        "stale-scaffold",
        "--reason",
        "superseded by the new design",
      ]);
      expect(abandon.code, abandon.stderr).toBe(0);

      const wakes = await watcher.refresh();
      expect(wakes).toHaveLength(1);
      expect(wakes[0]!.kind).toBe("abandoned");
      expect(wakes[0]!.kind).not.toBe("failed");
      expect(delivered).toHaveLength(1);
      expect(delivered[0]!.message).toBe(delivered[0]!.text);

      const message = delivered[0]!.message;
      expect(message).toContain("⊘ stale-scaffold abandoned (open → abandoned)");
      expect(message).not.toContain("✗");
      expect(message).not.toContain("stale-scaffold failed");
      expect(message).toContain("Note: superseded by the new design");
      expect(message).toContain("wg_show");
      expect(delivered[0]!.level).toBe("info");

      // Repeated poll: no re-announce.
      await watcher.refresh();
      expect(delivered).toHaveLength(1);
    },
    180_000,
  );
});
