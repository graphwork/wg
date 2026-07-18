/**
 * Verifies the model bridge maps a pi `model_select` event to a
 * CoordinatorState.model_override write, mapping the pi model to a WG spec and
 * calling the (mocked) backend. Also covers the provider-registration and
 * restore-skip branches.
 */

import { describe, it, expect, vi } from "vitest";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { installModelBridge, wgSpecFromModel, buildProviderConfig, WgBackend } from "../dist/index.js";

type ModelSelectHandler = (event: unknown) => unknown | Promise<unknown>;

function fakePi() {
  let modelSelectHandler: ModelSelectHandler | undefined;
  const pi = {
    registerProvider: vi.fn(),
    on: vi.fn((event: string, handler: ModelSelectHandler) => {
      if (event === "model_select") modelSelectHandler = handler;
    }),
  };
  return { pi, fire: (event: unknown) => modelSelectHandler!(event) };
}

function fakeBackend(chatId = ".chat-1") {
  return {
    hasChatContext: vi.fn(() => chatId !== ""),
    setModelOverride: vi.fn(async () => ({ stdout: "", stderr: "", code: 0, killed: false })),
  };
}

describe("wgSpecFromModel", () => {
  it("formats provider:id", () => {
    expect(wgSpecFromModel({ provider: "claude", id: "opus" })).toBe("claude:opus");
    expect(wgSpecFromModel({ provider: "openrouter", id: "anthropic/claude-opus-4-7" })).toBe(
      "openrouter:anthropic/claude-opus-4-7",
    );
  });
});

describe("installModelBridge model_select write-back", () => {
  it("writes the selected model into CoordinatorState.model_override via the backend", async () => {
    const { pi, fire } = fakePi();
    const backend = fakeBackend();

    installModelBridge(pi, backend, {}); // no WG_PI_BASE_URL → no provider registration

    expect(pi.registerProvider).not.toHaveBeenCalled();
    expect(pi.on).toHaveBeenCalledWith("model_select", expect.any(Function));

    await fire({
      type: "model_select",
      model: { provider: "openrouter", id: "anthropic/claude-opus-4-7" },
      previousModel: undefined,
      source: "set",
    });

    expect(backend.setModelOverride).toHaveBeenCalledTimes(1);
    expect(backend.setModelOverride).toHaveBeenCalledWith("openrouter:anthropic/claude-opus-4-7");
  });

  it("skips the write-back for a 'restore' select (already persisted)", async () => {
    const { pi, fire } = fakePi();
    const backend = fakeBackend();
    installModelBridge(pi, backend, {});
    await fire({ type: "model_select", model: { provider: "claude", id: "opus" }, source: "restore" });
    expect(backend.setModelOverride).not.toHaveBeenCalled();
  });

  it.each([
    ["OpenRouter", { provider: "openrouter", id: "qwen/qwen3.6-flash" }],
    ["local llama.cpp", { provider: "llamacpp", id: "llama-3.3-local" }],
  ])(
    "lets a standalone Ctrl-P cycle select %s with zero WG calls and zero errors",
    async (_label, model) => {
      const { pi, fire } = fakePi();
      const backend = fakeBackend("");
      const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
      try {
        installModelBridge(pi, backend, {});
        await expect(
          fire({ type: "model_select", model, source: "cycle" }),
        ).resolves.not.toThrow();
        expect(backend.hasChatContext).toHaveBeenCalledTimes(1);
        expect(backend.setModelOverride).not.toHaveBeenCalled();
        expect(errSpy).not.toHaveBeenCalled();
      } finally {
        errSpy.mockRestore();
      }
    },
  );

  it("logs a write-back warning when `wg chat model` exits non-zero (not a silent no-op)", async () => {
    // Regression guard for fix-pi-model: wire a REAL WgBackend over a fake exec
    // host whose `wg chat model` exits non-zero (e.g. the verb is absent). Before
    // the fix the ExecResult was returned as-is and nothing was logged; now the
    // backend rejects and the model_select handler surfaces a console.error.
    const { pi, fire } = fakePi();
    const host = {
      exec: vi.fn(async () => ({
        stdout: "",
        stderr: "error: unrecognized subcommand 'model'",
        code: 2,
        killed: false,
      })),
    };
    const backend = new WgBackend(host, { chatId: ".chat-1" });
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      installModelBridge(pi, backend, {});
      await expect(
        fire({ type: "model_select", model: { provider: "openrouter", id: "openai/gpt-4o" }, source: "set" }),
      ).resolves.not.toThrow();

      // The non-zero exit was NOT swallowed: one concise, actionable line is
      // visible, without passing an Error object (which prints a full stack).
      expect(host.exec).toHaveBeenCalledTimes(1);
      expect(errSpy).toHaveBeenCalledTimes(1);
      expect(errSpy.mock.calls[0]).toHaveLength(1);
      const logged = errSpy.mock.calls[0][0];
      expect(logged).toContain("model write-back failed");
      expect(logged).toContain("openrouter:openai/gpt-4o");
      expect(logged).toContain(".chat-1");
      expect(logged).not.toContain("\n");
      expect(logged).not.toContain("    at ");
    } finally {
      errSpy.mockRestore();
    }
  });
});

describe("buildProviderConfig", () => {
  it("returns null when WG exports no endpoint", () => {
    expect(buildProviderConfig({})).toBeNull();
  });

  it("builds a provider from WG env (endpoint + comma model list)", () => {
    const reg = buildProviderConfig({
      WG_PI_BASE_URL: "https://wg.example/v1",
      WG_PI_PROVIDER: "wg",
      WG_PI_API: "openai-completions",
      WG_PI_API_KEY: "$WG_KEY",
      WG_PI_MODELS: "qwen3-coder-30b, qwen3-coder-7b",
    });
    expect(reg).not.toBeNull();
    expect(reg.name).toBe("wg");
    expect(reg.config.baseUrl).toBe("https://wg.example/v1");
    expect(reg.config.apiKey).toBe("$WG_KEY");
    expect(reg.config.models).toHaveLength(2);
    expect(reg.config.models[0]).toMatchObject({ id: "qwen3-coder-30b", baseUrl: "https://wg.example/v1" });
  });

  it("falls back to the generic WG_* endpoint vars WG already exports", () => {
    const reg = buildProviderConfig({
      WG_ENDPOINT_URL: "https://lambda01.example:30000",
      WG_PROVIDER: "nex",
      WG_API_KEY: "sk-test",
      WG_MODEL: "openrouter:anthropic/claude-opus-4-7",
    });
    expect(reg).not.toBeNull();
    expect(reg.name).toBe("nex");
    expect(reg.config.baseUrl).toBe("https://lambda01.example:30000");
    expect(reg.config.apiKey).toBe("sk-test");
    // WG_MODEL's "provider:" prefix is stripped to the bare model id.
    expect(reg.config.models).toHaveLength(1);
    expect(reg.config.models[0].id).toBe("anthropic/claude-opus-4-7");
  });

  it("registers the WG provider through pi when env supplies an endpoint", () => {
    const { pi } = fakePi();
    const backend = fakeBackend();
    installModelBridge(pi, backend, { WG_PI_BASE_URL: "https://wg.example/v1", WG_PI_PROVIDER: "wg" });
    expect(pi.registerProvider).toHaveBeenCalledWith("wg", expect.objectContaining({ baseUrl: "https://wg.example/v1" }));
  });
});
