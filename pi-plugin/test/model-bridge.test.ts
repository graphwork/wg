/**
 * Verifies the model bridge maps a pi `model_select` event to a
 * CoordinatorState.model_override write, mapping the pi model to a WG spec and
 * calling the (mocked) backend. Also covers the provider-registration and
 * restore-skip branches.
 */

import { describe, it, expect, vi } from "vitest";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import { installModelBridge, wgSpecFromModel, buildProviderConfig } from "../dist/index.js";

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

function fakeBackend() {
  return {
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

  it("does not throw if the backend write fails (verb/chat may be absent)", async () => {
    const { pi, fire } = fakePi();
    const backend = {
      setModelOverride: vi.fn(async () => {
        throw new Error("no chat id");
      }),
    };
    installModelBridge(pi, backend, {});
    await expect(
      fire({ type: "model_select", model: { provider: "claude", id: "opus" }, source: "set" }),
    ).resolves.not.toThrow();
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
