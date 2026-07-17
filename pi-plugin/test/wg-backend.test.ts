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
 * Tests run against the built artifact (dist/) — `npm test` builds first.
 */

import { describe, it, expect, vi } from "vitest";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { canonicalChatId, readWgEnv, WgBackend } from "../dist/index.js";

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
