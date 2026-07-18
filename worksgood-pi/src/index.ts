/**
 * @worksgood/pi — connect Pi agents to WorksGood graphs, tools, and context.
 *
 * One artifact, loaded the same way whether a human launched pi (Topology C,
 * auto-discovered) or WG spawned it (Topology A `pi --mode rpc`, or Topology B
 * the SDK Node host in host/wg-pi-host.mjs). Registers the wg tool family, the
 * /wg and /wg-model commands, and the model bridge — natively, inside pi's
 * lifecycle (integration-plan-v2.md §2).
 *
 * WG context (which task/chat this session is bound to, the project dir, the
 * daemon socket) rides in via environment variables WG already exports to its
 * handlers; we read them here, inside the factory. Long-lived resources are
 * deferred to `session_start` and torn down in `session_shutdown`.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { readWgEnv, WgBackend } from "./wg-backend.js";
import { registerWgTools } from "./tools.js";
import { registerWgCommands } from "./commands.js";
import { installModelBridge } from "./model-bridge.js";
import { WG_PI_PLUGIN_COMPAT_VERSION as EMBEDDED_COMPAT } from "./version.js";

/**
 * Compat tripwire (mirrors WG_AGENCY_COMPAT_VERSION): a version-skewed plugin
 * silently sends the wrong flags to whatever `wg` is on PATH, so we refuse to
 * load when the build does not match the `wg` binary that spawned us.
 *
 * Two directions, two signals:
 *
 *  - **wg → pi (hermetic):** `wg pi-handler` injects
 *    `WG_PI_PLUGIN_COMPAT_VERSION` into the child env at spawn. We compare it
 *    SYNCHRONOUSLY here and `throw` on mismatch — the SDK collects a thrown
 *    factory error as an extension load error (surfaced in
 *    `extensionsResult.errors` and in attended pi's UI). This is the loud,
 *    testable fail path the host `--selftest --force-compat-mismatch` exercises.
 *
 *  - **pi → wg (human console):** the env is absent, so we ask the `wg` actually
 *    on PATH (`wg pi-plugin compat-version`) ASYNCHRONOUSLY and complain loudly
 *    on mismatch. A factory cannot block on a child process, so this path warns
 *    on stderr / via pi notifications rather than throwing at load.
 */
function assertCompatVersionSync(): void {
  const expected = process.env.WG_PI_PLUGIN_COMPAT_VERSION?.trim();
  if (expected && expected !== EMBEDDED_COMPAT) {
    throw new Error(
      `WorksGood Pi integration compat mismatch: extension=${EMBEDDED_COMPAT} wg=${expected}. ` +
        `The loaded WorksGood Pi integration build does not match the wg binary that spawned it; ` +
        "run `wg pi-plugin install` to repair (or rebuild + re-embed in dev).",
    );
  }
}

/** pi → wg drift catcher: ask the on-PATH `wg` for its compat version. */
async function assertCompatVersionAsync(backend: WgBackend): Promise<void> {
  // Only meaningful when wg did NOT inject the env (i.e. the human-console
  // direction); the sync check already covered the wg→pi spawn.
  if (process.env.WG_PI_PLUGIN_COMPAT_VERSION) return;
  let found: string | undefined;
  try {
    const r = await backend.run(["pi-plugin", "compat-version"]);
    if (r.code === 0) found = r.stdout.trim();
  } catch {
    return; // no `wg` on PATH / older wg without the verb — nothing to assert.
  }
  if (found && found !== EMBEDDED_COMPAT) {
    const msg =
      `WorksGood Pi integration compat mismatch: extension=${EMBEDDED_COMPAT} wg=${found}. ` +
      "Reinstall the matching plugin with `wg pi-plugin install`.";
    // A factory cannot turn an async result into a load-time throw, so the
    // guaranteed signal for the console direction is a loud stderr line.
    console.error(`[pi-worksgood] ${msg}`);
  }
}

export default function worksgoodPi(pi: ExtensionAPI): void {
  assertCompatVersionSync(); // wg→pi: throw → extension load error (loud, testable)
  const env = readWgEnv();
  const backend = new WgBackend(pi, env);
  void assertCompatVersionAsync(backend); // pi→wg: best-effort drift catcher

  registerWgTools(pi, backend); // wg_ready / wg_show / wg_add / wg_done / wg_fail / wg_msg_* / wg_run
  registerWgCommands(pi, backend); // /wg, /wg-model (+ autocomplete)
  installModelBridge(pi, backend, process.env); // registerProvider + model_select → CoordinatorState

  // Tear down any session-scoped resources (the future daemon-IPC socket /
  // graph watcher live here once wg-backend upgrades from exec to IPC).
  pi.on("session_shutdown", async () => {
    /* no long-lived resources yet; placeholder for the daemon-IPC client */
  });
}

// Re-export the building blocks so the SDK host and tests can use them directly.
export { WgBackend, readWgEnv, canonicalChatId } from "./wg-backend.js";
export type { WgEnv, ExecHost } from "./wg-backend.js";
export { registerWgTools } from "./tools.js";
export { registerWgCommands, parseModelSpec } from "./commands.js";
export { installGraphWidget, parseReady, renderWidget } from "./graph-widget.js";
export { installModelBridge, wgSpecFromModel, buildProviderConfig } from "./model-bridge.js";
export { WG_PI_PLUGIN_COMPAT_VERSION } from "./version.js";
