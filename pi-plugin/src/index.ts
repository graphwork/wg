/**
 * @worksgood/wg-pi-plugin — the WG integration channel for the pi coding agent.
 *
 * One artifact, loaded the same way whether a human launched pi (Topology C,
 * auto-discovered) or WG spawned it (Topology A `pi --mode rpc`, or Topology B
 * the SDK Node host in host/wg-pi-host.mjs). Registers the wg tool family, the
 * /wg and /wg-model commands, an in-session task-graph widget, and the model
 * bridge — natively, inside pi's lifecycle (integration-plan-v2.md §2).
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
import { installGraphWidget } from "./graph-widget.js";
import { installModelBridge } from "./model-bridge.js";

export default function wgPlugin(pi: ExtensionAPI): void {
  const env = readWgEnv();
  const backend = new WgBackend(pi, env);

  registerWgTools(pi, backend); // wg_ready / wg_show / wg_add / wg_done / wg_fail / wg_msg_* / wg_run
  registerWgCommands(pi, backend); // /wg, /wg-model (+ autocomplete)
  installGraphWidget(pi, backend); // setWidget/setStatus on session_start + turn_end
  installModelBridge(pi, backend, process.env); // registerProvider + model_select → CoordinatorState

  // Tear down any session-scoped resources (the future daemon-IPC socket /
  // graph watcher live here once wg-backend upgrades from exec to IPC).
  pi.on("session_shutdown", async () => {
    /* no long-lived resources yet; placeholder for the daemon-IPC client */
  });
}

// Re-export the building blocks so the SDK host and tests can use them directly.
export { WgBackend, readWgEnv } from "./wg-backend.js";
export type { WgEnv, ExecHost } from "./wg-backend.js";
export { registerWgTools } from "./tools.js";
export { registerWgCommands, parseModelSpec } from "./commands.js";
export { installGraphWidget, parseReady, renderWidget } from "./graph-widget.js";
export { installModelBridge, wgSpecFromModel, buildProviderConfig } from "./model-bridge.js";
