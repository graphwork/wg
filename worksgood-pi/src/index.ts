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
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { readWgEnv, WgBackend } from "./wg-backend.js";
import { registerWgTools } from "./tools.js";
import { registerWgCommands } from "./commands.js";
import { installModelBridge } from "./model-bridge.js";
import { installVizPanel } from "./viz-panel.js";
import { installFleetView } from "./fleet-view.js";
import { installCompletionWatcher } from "./completion-watcher.js";
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
 *    on PATH ASYNCHRONOUSLY and complain loudly on mismatch. A factory cannot
 *    block on a child process, so this path warns on stderr rather than throwing
 *    at load.
 *
 * The console path is also made **self-healing**, matching what worker spawns
 * already get from the Hermetic `ensure-pi-plugin`: the running binary's cache
 * may have been materialized by an OLDER binary (the operator-observed
 * "silent-old console" gap — you `npm update -g`, open `pi`, and get whatever
 * bytes the last touchpoint froze). Two cheap signals at load:
 *
 *  1. `wg pi-plugin compat-version` itself self-heals a stale console cache and
 *     rewires `~/.pi/agent/settings.json` (the Rust side; loud on stderr).
 *  2. the digest check below compares this build's `.wg-embed-digest` stamp
 *     against `wg pi-plugin digest` (the binary's current embed) and, on
 *     mismatch, warns **and** runs `wg pi-plugin install` so the next launch is
 *     fixed — detecting an embed change even under an unchanged compat version.
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

/**
 * Read the content digest this build's cache was materialized with.
 *
 * Only a binary-materialized cache dir carries the `.wg-embed-digest` companion
 * stamp (written beside the version dir by `ensure-pi-plugin`). A dev tree or an
 * npm `node_modules` install has no stamp — return `undefined` so we never
 * nag about a build `wg` does not own.
 */
export function readEmbedDigestAt(versionDir: string): string | undefined {
  try {
    const text = readFileSync(path.join(versionDir, ".wg-embed-digest"), "utf8").trim();
    return text || undefined;
  } catch {
    return undefined;
  }
}

/** The version dir this copy of the plugin was loaded from (parent of `pi-worksgood/`). */
function ownVersionDir(): string {
  return path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
}

/**
 * pi → wg console drift catcher. Best-effort: never throws at load.
 *
 * @param ownDigest override for tests (defaults to this build's stamp).
 */
export async function assertConsolePluginCurrent(
  backend: WgBackend,
  ownDigest?: string,
): Promise<void> {
  // Only meaningful when wg did NOT inject the env (i.e. the human-console
  // direction); the sync check already covered the wg→pi spawn.
  if (process.env.WG_PI_PLUGIN_COMPAT_VERSION) return;
  // Capture our own stamp BEFORE the (self-healing) wg calls below can rewrite
  // the cache under us. `undefined` for dev / npm installs.
  const loadedDigest = ownDigest === undefined ? readEmbedDigestAt(ownVersionDir()) : ownDigest;

  // (1) compat handshake + forward the Rust self-heal warning (if any).
  let found: string | undefined;
  try {
    const r = await backend.run(["pi-plugin", "compat-version"]);
    if (r.code === 0) found = r.stdout.trim();
    if (r.stderr.trim()) console.error(`[pi-worksgood] ${r.stderr.trim()}`);
  } catch {
    return; // no `wg` on PATH / older wg without the verb — nothing to assert.
  }
  if (found && found !== EMBEDDED_COMPAT) {
    console.error(
      `[pi-worksgood] WorksGood Pi integration compat mismatch: extension=${EMBEDDED_COMPAT} wg=${found}. ` +
        "Reinstall the matching plugin with `wg pi-plugin install`.",
    );
  }

  // (2) content-digest check — catches a stale cache even under an unchanged
  // compat version, which the compat handshake alone cannot see.
  let wgDigest: string | undefined;
  try {
    const r = await backend.run(["pi-plugin", "digest"]);
    if (r.code === 0) wgDigest = r.stdout.trim();
  } catch {
    return; // older wg without `digest` — compat check above is the fallback.
  }
  if (!loadedDigest || !wgDigest || loadedDigest === wgDigest) return;

  console.error(
    `[pi-worksgood] stale plugin cache: loaded ${loadedDigest} but this wg embeds ${wgDigest}. ` +
      "Refreshing with `wg pi-plugin install` — restart pi to load the updated /wg-fleet and wg tools.",
  );
  try {
    const heal = await backend.run(["pi-plugin", "install"]);
    if (heal.stderr.trim()) console.error(`[pi-worksgood] ${heal.stderr.trim()}`);
    if (heal.code !== 0) {
      console.error(
        `[pi-worksgood] self-heal failed (exit ${heal.code}); run \`wg pi-plugin install\` manually.`,
      );
    }
  } catch {
    /* best-effort: the warning above is the guaranteed signal */
  }
}

export default function worksgoodPi(pi: ExtensionAPI): void {
  assertCompatVersionSync(); // wg→pi: throw → extension load error (loud, testable)
  const env = readWgEnv();
  const backend = new WgBackend(pi, env);
  void assertConsolePluginCurrent(backend); // pi→wg: best-effort self-heal + drift catcher

  registerWgTools(pi, backend); // wg_capabilities / wg_ready / wg_show / wg_add / wg_publish / wg_done / wg_fail / wg_msg_* / wg_run
  registerWgCommands(pi, backend); // /wg, /wg-model (+ autocomplete)
  installModelBridge(pi, backend, process.env); // registerProvider + model_select → CoordinatorState
  installVizPanel(pi, backend, env); // /wg-viz panel + live widget (TUI mode only, read-only)
  installFleetView(pi, backend, env); // /wg-fleet bottom panel (config-gated, TUI mode only, read-only)
  installCompletionWatcher(pi, backend, env); // /wg-wake + polling watcher: tell the session when tasks finish

  // Tear down any session-scoped resources (the future daemon-IPC socket /
  // graph watcher live here once wg-backend upgrades from exec to IPC).
  pi.on("session_shutdown", async () => {
    /* no long-lived resources yet; placeholder for the daemon-IPC client */
  });
}

// Re-export the building blocks so the SDK host and tests can use them directly.
export { WgBackend, readWgEnv, canonicalChatId, normalizeGetFleet, buildCliFleet, parseVizJson, normalizeAgentUsage } from "./wg-backend.js";
export type {
  AgentUsage,
  GetFleetAgentRow,
  GetFleetCounts,
  GetFleetOptions,
  GetFleetSnapshot,
  GetFleetTaskRow,
  GetFleetTokenSummary,
  GetFleetTree,
} from "./wg-backend.js";
export type { WgEnv, ExecHost } from "./wg-backend.js";
export { registerWgTools } from "./tools.js";
export { registerWgCommands, parseModelSpec } from "./commands.js";
export { installGraphWidget, parseReady, renderWidget } from "./graph-widget.js";
export { installVizPanel, openVizPanel, VizPanelComponent, VIZ_WIDGET_KEY, VIZ_WIDGET_POLL_MS } from "./viz-panel.js";
export {
  installFleetView,
  FLEET_WIDGET_KEY,
  FLEET_POLL_MS,
} from "./fleet-view.js";
export type { FleetViewController, InstallFleetOptions } from "./fleet-view.js";
export {
  FleetPanelComponent,
  openFleetPanel,
  makeFleetFetcher,
  makeFleetDetailFetcher,
  FLEET_PANEL_POLL_MS,
  DETAIL_ENTER_GUARD_MS,
} from "./fleet-panel.js";
export type { FleetPanelOptions, FleetTui, OpenFleetPanelOptions, FleetSnapshotFetcher, FleetDetailFetcher } from "./fleet-panel.js";
export {
  DEFAULT_TRANSCRIPT_LINES,
  FLEET_PANEL_DEFAULT_HEIGHT,
  FLEET_PANEL_MIN_HEIGHT,
  MAX_TRANSCRIPT_BODY_BYTES,
  MAX_TRANSCRIPT_LINES,
  MAX_TRANSCRIPT_LINE_CHARS,
  TRANSCRIPT_TAIL_BYTES,
  agentActivityLabel,
  agentForTask,
  agentStreamCandidates,
  agentUsageDetailLabel,
  agentUsageLabel,
  boundTranscriptBody,
  buildFleetTree,
  clampScroll,
  fleetCountsHeader,
  getFleetRowToVizTask,
  getFleetToVizSnapshot,
  maxScroll,
  pageScroll,
  readAgentStreamTail,
  readTextTail,
  renderWgTree,
  safeTailText,
  scrollToKeepVisible,
  transcriptLineLimit,
  wgTreeLineDepth,
} from "./fleet-panel-model.js";
export type { TextTail, WgTreeLine, WgTreeRender } from "./fleet-panel-model.js";
export {
  DEFAULT_FLEET_VIEW_CONFIG,
  FLEET_CONFIG_DIRS,
  fleetConfigPaths,
  parseFleetViewConfig,
  readFleetViewConfig,
  resolveAgentDir,
} from "./fleet-config.js";
export type { FleetViewConfig, FleetPlacement, FleetConfigIO } from "./fleet-config.js";
export {
  agentColor,
  agentElapsed,
  agentGlyph,
  agentLine,
  agentModel,
  agentUsageCompact,
  agentUsageFull,
  compactCount,
  fleetCounts,
  fleetHeaderLine,
  fleetSummaryLine,
  isAgentAlive,
  renderFleetLines,
  taskColor,
  taskGlyph,
} from "./fleet-readmodel.js";
export type {
  AgentUsage as FleetAgentUsage,
  FleetAgent,
  FleetColor,
  FleetCounts,
  FleetLine,
  FleetRenderOptions,
  FleetSnapshot,
} from "./fleet-readmodel.js";
export {
  FALLBACK_STATUS_PALETTE,
  colorModeOf,
  fgAnsi,
  normalizeStatusPalette,
  paintRgb,
  paintStatusText,
  paletteNotice,
  resolveStatusPalette,
  rgbToAnsi256,
  statusRgb,
} from "./status-palette.js";
export type {
  PaintTheme,
  StatusColorMode,
  StatusPalette,
  StatusRgb,
} from "./status-palette.js";
export {
  FleetPoller,
  fetchAgents,
  fetchFleetOverSocket,
  fleetSnapshotWithFallback,
  normalizeAgent,
} from "./fleet-snapshot.js";
export type { FleetFetchOptions } from "./fleet-snapshot.js";
export {
  fetchVizSnapshot,
  resolveSocketPath,
  socketCandidates,
  vizSnapshotWithFallback,
  VizPoller,
} from "./viz-snapshot.js";
export type { VizSnapshot, VizTask } from "./viz-snapshot.js";
export {
  buildTree,
  detailLines,
  lineTaskMap,
  orderTasks,
  statusGlyph,
  taskCounts,
  treeText,
  widgetLine,
  ageOf,
  tokenDisplay,
} from "./viz-readmodel.js";
export type { VizCounts, TreeRender, DetailLines } from "./viz-readmodel.js";
export { installModelBridge, wgSpecFromModel, buildProviderConfig } from "./model-bridge.js";
export {
  DEFAULT_COMPLETION_WAKE_CONFIG,
  WAKE_PRESENTATION,
  CompletionWatcher,
  MemoryCursorStore,
  fileCursorStore,
  formatWakeMessage,
  installCompletionWatcher,
  isInternalTask,
  isTopLevelTask,
  kindOf,
  planWakes,
  readCompletionWakeConfig,
  readTaskDetail,
  wakeNotifyLevel,
  wakePresentation,
} from "./completion-watcher.js";
export type {
  CompletionCursor,
  CompletionWakeConfig,
  CursorStore,
  GraphTask,
  TaskDetail,
  WakeKind,
  WakePresentation,
  WakeScope,
  WakeTransition,
} from "./completion-watcher.js";
export { WG_PI_PLUGIN_COMPAT_VERSION } from "./version.js";
