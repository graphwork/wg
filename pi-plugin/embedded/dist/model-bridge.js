/**
 * model-bridge.ts — native model management through the plugin.
 *
 * Two halves (plugin-research.md §1.3, integration-plan-v2.md §2.2):
 *
 *  1. registerProvider — inject WG's configured endpoint/key/models into pi's
 *     model registry so pi's native /model picker lists exactly WG's models.
 *     Driven by env WG already resolves from `wg secret` / the active profile.
 *
 *  2. write-back — subscribe to `model_select` (fired by /model, /wg-model,
 *     Ctrl+P cycle, or restore) and persist the choice into the chat's
 *     `CoordinatorState.model_override` via the backend, so a warm in-process
 *     pi `setModel` survives a WG-side respawn (the identity bridge).
 *
 * This module is the pi half of the old P3 warm-swap task: warm `setModel` is
 * now the normal path, not a deferred optimization.
 */
/** Map a pi model to the WG model spec string ("provider:id"). */
export function wgSpecFromModel(model) {
    return `${model.provider}:${model.id}`;
}
/** Strip a "provider:" prefix off a WG model spec, returning the bare model id. */
function modelIdFromSpec(spec) {
    const colon = spec.indexOf(":");
    return colon > 0 ? spec.slice(colon + 1) : spec;
}
/**
 * Build a pi {@link ProviderConfig} from WG's environment, or null when WG has
 * not exported an endpoint (e.g. the default Anthropic route needs no bridge).
 *
 * The pi-specific `WG_PI_*` vars take precedence; they fall back to the generic
 * endpoint vars WG already exports to every handler, so the provider bridge
 * works today without the handler task wiring new env:
 *
 *   base URL   WG_PI_BASE_URL  → WG_ENDPOINT_URL
 *   provider   WG_PI_PROVIDER  → WG_PROVIDER          (default "wg")
 *   api        WG_PI_API
 *   api key    WG_PI_API_KEY   → WG_API_KEY
 *   models     WG_PI_MODELS (JSON array | comma list of ids) → WG_MODEL (one id)
 */
export function buildProviderConfig(env = process.env) {
    const baseUrl = (env.WG_PI_BASE_URL ?? env.WG_ENDPOINT_URL)?.trim();
    if (!baseUrl)
        return null;
    const name = (env.WG_PI_PROVIDER ?? env.WG_PROVIDER)?.trim() || "wg";
    const api = env.WG_PI_API?.trim();
    const apiKey = (env.WG_PI_API_KEY ?? env.WG_API_KEY)?.trim();
    // Explicit model list wins; otherwise fall back to the single WG_MODEL route.
    const rawModels = env.WG_PI_MODELS?.trim() || modelSpecToList(env.WG_MODEL);
    const models = parseModelList(rawModels, baseUrl, api);
    const config = { name, baseUrl };
    if (api)
        config.api = api;
    if (apiKey)
        config.apiKey = apiKey;
    if (models.length)
        config.models = models;
    return { name, config };
}
/** Turn a single WG_MODEL spec ("provider:id") into a one-element id list. */
function modelSpecToList(spec) {
    const s = spec?.trim();
    if (!s)
        return undefined;
    return modelIdFromSpec(s);
}
/** Parse WG_PI_MODELS as JSON (array of model configs) or a comma list of ids. */
function parseModelList(raw, baseUrl, api) {
    const val = raw?.trim();
    if (!val)
        return [];
    const defaults = {
        reasoning: false,
        input: ["text"],
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        contextWindow: 200_000,
        maxTokens: 16_384,
    };
    if (val.startsWith("[")) {
        try {
            const arr = JSON.parse(val);
            return arr.map((m) => ({ ...defaults, name: m.id, ...m, baseUrl: m.baseUrl ?? baseUrl }));
        }
        catch {
            return [];
        }
    }
    return val
        .split(",")
        .map((id) => id.trim())
        .filter(Boolean)
        .map((id) => ({
        id,
        name: id,
        baseUrl,
        ...(api ? { api: api } : {}),
        ...defaults,
    }));
}
/**
 * Install the model bridge: register WG's provider (if configured) and wire
 * the `model_select` → `CoordinatorState.model_override` write-back.
 */
export function installModelBridge(pi, backend, env = process.env) {
    const reg = buildProviderConfig(env);
    if (reg) {
        pi.registerProvider(reg.name, reg.config);
    }
    pi.on("model_select", async (event) => {
        // A "restore" select just re-applies the already-persisted model on
        // session load — nothing new to write back.
        if (event.source === "restore")
            return;
        const spec = wgSpecFromModel(event.model);
        try {
            await backend.setModelOverride(spec);
        }
        catch (err) {
            // The `wg chat model` verb (pi-plugin-impl-chat-model-verb) or a chat id
            // may be absent; keep the warm pi swap working regardless.
            console.error(`wg-pi-plugin: model write-back failed for ${spec}:`, err);
        }
    });
}
//# sourceMappingURL=model-bridge.js.map