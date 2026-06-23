#!/usr/bin/env node
/**
 * wg-pi-host.mjs — Topology B: WG embeds pi as a library.
 *
 * Instead of spawning the `pi` binary, WG's ExecutorKind::Pi can spawn
 * `node wg-pi-host.mjs`. This host loads @worksgood/wg-pi-plugin as an
 * in-process JS object via DefaultResourceLoader({extensionFactories:[wgPlugin],
 * eventBus}) and bridges the shared event bus to WG over stdio. No terminal is
 * ever grabbed (headless by construction), so the Axis-2 takeover never occurs
 * (integration-plan-v2.md §2.1 / plugin-research.md §4.2).
 *
 * Modes:
 *   --selftest   Load the plugin against an auth-less faux registry, assert it
 *                registered the wg tools + /wg commands, then exit 0. Used by
 *                the smoke gate — no network, no credentials, no terminal.
 *   (default)    Start a session and bridge: stdin JSON lines {type:"prompt",
 *                text} drive the agent; assistant text deltas + plugin event-bus
 *                messages are emitted as JSON lines on stdout. The downstream
 *                pi handler task fills in the full WG IPC framing.
 */

import { pathToFileURL } from "node:url";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { createInterface } from "node:readline";

const __dirname = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const SELFTEST = args.includes("--selftest");
// Force a deliberate compat-version mismatch to prove the loud-fail tripwire:
// the plugin factory must throw, and the SDK must surface it as an
// `extensionsResult.errors` entry naming expected-vs-found versions.
const FORCE_COMPAT_MISMATCH = args.includes("--force-compat-mismatch");

const REQUIRED_TOOLS = ["wg_ready", "wg_show", "wg_add", "wg_done", "wg_fail", "wg_msg_send", "wg_msg_read", "wg_run"];
const REQUIRED_COMMANDS = ["wg", "wg-model"];

function die(msg, code = 1) {
  process.stderr.write(`wg-pi-host: ${msg}\n`);
  process.exit(code);
}

/** Dynamically import the built plugin, with a clear error if dist/ is missing. */
async function loadPlugin() {
  const builtUrl = pathToFileURL(resolve(__dirname, "..", "dist", "index.js")).href;
  try {
    const mod = await import(builtUrl);
    return mod.default ?? mod.wgPlugin;
  } catch (err) {
    die(`could not load built plugin at ${builtUrl} — run \`npm run build\` first.\n${err?.stack || err}`);
  }
}

/** Resolve the pi SDK exports (peer dependency, provided by pi at runtime). */
async function loadPi() {
  try {
    const core = await import("@earendil-works/pi-coding-agent");
    const ai = await import("@earendil-works/pi-ai");
    return { core, ai };
  } catch (err) {
    die(`@earendil-works/pi-coding-agent not resolvable — install peer deps.\n${err?.stack || err}`);
  }
}

/**
 * Build an embedded pi session with the wg plugin loaded.
 * Uses an in-memory, auth-less registry and a faux model so the host can start
 * with no credentials (real auth/model is injected by the WG handler in prod).
 */
async function buildSession({ hermetic }) {
  const wgPlugin = await loadPlugin();
  if (typeof wgPlugin !== "function") die("plugin default export is not a function");

  const { core, ai } = await loadPi();
  const { createAgentSession, DefaultResourceLoader, createEventBus, AuthStorage, ModelRegistry, SessionManager } = core;
  const { registerFauxProvider } = ai;

  // Isolated, throwaway agent dir + cwd so no global ~/.pi or project .pi
  // extensions load and the plugin's `wg --dir` never touches a real project.
  const agentDir = mkdtempSync(resolve(tmpdir(), "wg-pi-host-"));
  const cwd = mkdtempSync(resolve(tmpdir(), "wg-pi-cwd-"));
  if (hermetic) {
    // Point the plugin's backend at the empty cwd so any `wg` call is a fast,
    // side-effect-free miss rather than hitting a real/global WG daemon.
    process.env.WG_PROJECT_DIR = cwd;
  }

  const eventBus = createEventBus();
  const resourceLoader = new DefaultResourceLoader({
    cwd,
    agentDir,
    eventBus,
    extensionFactories: [wgPlugin],
    noContextFiles: true,
  });
  await resourceLoader.reload();

  const authStorage = AuthStorage.inMemory();
  const modelRegistry = ModelRegistry.inMemory(authStorage);
  const faux = registerFauxProvider({ provider: "wg-selftest", models: [{ id: "selftest" }] });

  const { session, extensionsResult } = await createAgentSession({
    cwd,
    agentDir,
    model: faux.getModel(),
    thinkingLevel: "off",
    authStorage,
    modelRegistry,
    resourceLoader,
    sessionManager: SessionManager.inMemory(cwd),
  });

  return { session, extensionsResult, eventBus, faux };
}

/** Collect registered tool/command names across all loaded extensions. */
function collectRegistrations(extensionsResult) {
  const tools = new Set();
  const commands = new Set();
  for (const ext of extensionsResult.extensions ?? []) {
    for (const name of ext.tools?.keys?.() ?? []) tools.add(name);
    for (const name of ext.commands?.keys?.() ?? []) commands.add(name);
  }
  return { tools, commands, errors: extensionsResult.errors ?? [] };
}

/**
 * Forced-mismatch selftest: set a bogus WG_PI_PLUGIN_COMPAT_VERSION so the
 * plugin factory throws, and assert the SDK collected it as a load error whose
 * message names both the embedded (plugin) and expected (wg) compat versions.
 * Exit 0 when the tripwire fired correctly; exit 1 if the plugin loaded anyway.
 */
async function runCompatMismatchSelftest() {
  const bogus = "9.9.9-deliberate-mismatch";
  process.env.WG_PI_PLUGIN_COMPAT_VERSION = bogus;
  const { session, extensionsResult } = await buildSession({ hermetic: true });
  try {
    const { tools, errors } = collectRegistrations(extensionsResult);
    const text = errors.map((e) => String(e.error ?? e)).join(" | ");
    const fired = errors.length > 0 && /compat mismatch/i.test(text) && text.includes(bogus);
    if (!fired) {
      die(
        "compat-mismatch tripwire did NOT fire: the plugin loaded despite a " +
          `version skew (expected a load error naming ${bogus}).\n` +
          `  errors: ${text || "(none)"}\n` +
          `  tools: ${[...tools].join(", ") || "(none — plugin failed to register, as expected)"}`,
      );
    }
    process.stdout.write(
      `wg-pi-host compat-mismatch selftest OK: tripwire fired loudly — ${text}\n`,
    );
  } finally {
    session.dispose?.();
  }
  process.exit(0);
}

async function runSelftest() {
  const { session, extensionsResult } = await buildSession({ hermetic: true });
  try {
    const { tools, commands, errors } = collectRegistrations(extensionsResult);

    if (errors.length) {
      die(`extension load errors: ${errors.map((e) => `${e.path}: ${e.error}`).join("; ")}`);
    }
    const missingTools = REQUIRED_TOOLS.filter((t) => !tools.has(t));
    const missingCommands = REQUIRED_COMMANDS.filter((c) => !commands.has(c));
    if (missingTools.length || missingCommands.length) {
      die(
        `plugin did not register expected items.\n` +
          `  missing tools: ${missingTools.join(", ") || "(none)"}\n` +
          `  missing commands: ${missingCommands.join(", ") || "(none)"}\n` +
          `  got tools: ${[...tools].join(", ")}\n` +
          `  got commands: ${[...commands].join(", ")}`,
      );
    }

    process.stdout.write(
      `wg-pi-host selftest OK: ${tools.size} tools, ${commands.size} commands ` +
        `(wg tools: ${REQUIRED_TOOLS.length}, /wg + /wg-model present)\n`,
    );
  } finally {
    session.dispose?.();
  }
  process.exit(0);
}

async function runHost() {
  const { session, eventBus } = await buildSession({ hermetic: false });

  // Forward plugin event-bus traffic to WG as JSON lines (the handler task
  // replaces this with the real WG IPC transport).
  eventBus.on("wg:event", (data) => {
    process.stdout.write(`${JSON.stringify({ type: "wg:event", data })}\n`);
  });

  session.subscribe?.((event) => {
    if (event.type === "message_update" && event.assistantMessageEvent?.type === "text_delta") {
      process.stdout.write(`${JSON.stringify({ type: "delta", text: event.assistantMessageEvent.delta })}\n`);
    }
  });

  process.stdout.write(`${JSON.stringify({ type: "ready" })}\n`);

  const rl = createInterface({ input: process.stdin });
  rl.on("line", async (line) => {
    const text = line.trim();
    if (!text) return;
    let msg;
    try {
      msg = JSON.parse(text);
    } catch {
      msg = { type: "prompt", text };
    }
    if (msg.type === "prompt" && msg.text) {
      try {
        await session.prompt(msg.text);
      } catch (err) {
        process.stdout.write(`${JSON.stringify({ type: "error", error: String(err?.message || err) })}\n`);
      }
    }
  });

  const shutdown = () => {
    try {
      session.dispose?.();
    } finally {
      process.exit(0);
    }
  };
  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
  rl.on("close", shutdown);
}

const entry = FORCE_COMPAT_MISMATCH ? runCompatMismatchSelftest() : SELFTEST ? runSelftest() : runHost();
entry.catch((err) => die(err?.stack || String(err)));
