/**
 * activation.ts — SESSION-SCOPED WG ACTIVATION (dormant by default).
 *
 * The plugin used to register ~13 model-visible `wg_*` tools in EVERY session,
 * in EVERY directory, unconditionally: the agent permanently "perceived" WG
 * whether invited or not. Every earlier off-switch made it worse — `/wg off`
 * used to *remove the settings entry* (catch-22: the next session loses `/wg`
 * itself, recovery only via the CLI), the HUD needed config-JSON edits, nothing
 * was session-scoped, and mutating `.wg/` would have broken running agents.
 *
 * The mechanism pi 1.3.0 documents ("Activate tools dynamically",
 * `docs/extensions.md`): register every tool first, keep the optional tools
 * INACTIVE, and use `pi.setActiveTools()` to select the active set. Commands
 * (`/wg`) are UI affordances, not model-visible tools, so they stay available
 * in every state.
 *
 * **The registration choice.** pi's `ToolExposure` has five values, and
 * `hidden` means *"registered but unreachable — activating it has no effect"*,
 * so `hidden` CANNOT be flipped on later. The clean API is therefore
 * `exposure: "direct"` + `defaultActive: false`: the tool is registered,
 * declarable, and activated only by `setActiveTools()` (or an explicit
 * `--tools` / `defaultTools` opt-in). `tools.ts` registers the whole family that
 * way; this module owns the toggling.
 *
 * **Persistence (small, honest).** A per-project marker file
 * `<project>/.wg/session-active` holding the plain text `active\n`: a project you
 * booted with `/wg on` stays active in NEW sessions of that project, a project
 * never booted stays dormant, and `/wg off` clears the marker (so the project is
 * dormant for future sessions too — session teardown AND persistence in one
 * act). The marker lives INSIDE an existing `.wg/`: it never creates or removes
 * `.wg/` itself, and running agents are separate processes that are unaffected by
 * an extra file.
 */
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { WG_TOOL_NAMES } from "./tools.js";
/** The WG state directory name (never created/torn down by the plugin). */
export const ACTIVATION_STATE_DIR = ".wg";
/** Marker file inside `.wg/` whose presence means "booted in this project". */
export const ACTIVATION_MARKER_FILE = "session-active";
/**
 * Marker file contents. Plain text, not JSON: the file's *existence* is the
 * whole signal, so a human can create/remove it with `touch`/`rm` and a diff
 * review sees one obvious line. `active\n`.
 */
export const ACTIVATION_MARKER_CONTENT = "active\n";
/** The absolute marker path for a project root. */
export function activationMarkerPath(projectRoot) {
    return join(projectRoot, ACTIVATION_STATE_DIR, ACTIVATION_MARKER_FILE);
}
/** Default IO: synchronous node:fs (the file is tiny and read once per session). */
export const DEFAULT_ACTIVATION_IO = {
    exists: (path) => existsSync(path),
    read: (path) => {
        try {
            return readFileSync(path, "utf8");
        }
        catch {
            return null;
        }
    },
    write: (path, contents) => {
        mkdirSync(dirname(path), { recursive: true });
        writeFileSync(path, contents, { encoding: "utf8", mode: 0o600 });
    },
    remove: (path) => {
        try {
            rmSync(path, { force: true });
        }
        catch {
            /* already gone */
        }
    },
};
/**
 * Normalize a WG `--dir` into the OWNING project root.
 *
 * `WG_DIR` is the STATE directory (`<project>/.wg`), not the project root, so
 * blindly appending `.wg` to it would look for `<project>/.wg/.wg` and silently
 * refuse to persist. A `--dir` that IS a `.wg` directory resolves to its parent;
 * anything else is already a project root.
 */
export function projectRootFromDir(dir) {
    const trimmed = dir.trim().replace(/[\\/]+$/, "");
    if (!trimmed)
        return trimmed;
    return basename(trimmed) === ACTIVATION_STATE_DIR ? dirname(trimmed) : trimmed;
}
/**
 * The project root this session belongs to: WG's explicit `--dir` (WG_DIR /
 * WG_PROJECT_DIR / WG_PROJECT_ROOT) when present, else the pi session cwd.
 */
export function resolveProjectRoot(env, cwd) {
    const explicit = env?.dir?.trim();
    if (explicit)
        return projectRootFromDir(explicit);
    const sessionCwd = cwd?.trim();
    return sessionCwd ? projectRootFromDir(sessionCwd) : undefined;
}
/** Is this project marked active (i.e. booted by `/wg on` in some session)? */
export function isProjectMarkedActive(projectRoot, io = DEFAULT_ACTIVATION_IO) {
    if (!projectRoot)
        return false;
    const path = activationMarkerPath(projectRoot);
    if (!io.exists(path))
        return false;
    // Existence is the signal; a legacy/unknown body still means "booted".
    return true;
}
/**
 * The project marker can only be written when the project actually has a
 * `.wg/` directory. Creating `.wg/` here would *initialize* WG presence, which
 * is exactly what this design refuses to do, so a non-WG project gets a
 * session-only activation and an honest warning.
 */
export function canPersistProjectMarker(projectRoot, io = DEFAULT_ACTIVATION_IO) {
    if (!projectRoot)
        return false;
    return io.exists(join(projectRoot, ACTIVATION_STATE_DIR));
}
export function markProjectActive(projectRoot, io = DEFAULT_ACTIVATION_IO) {
    if (!canPersistProjectMarker(projectRoot, io))
        return undefined;
    const path = activationMarkerPath(projectRoot);
    io.write(path, ACTIVATION_MARKER_CONTENT);
    return path;
}
export function clearProjectMarker(projectRoot, io = DEFAULT_ACTIVATION_IO) {
    if (!projectRoot)
        return false;
    const path = activationMarkerPath(projectRoot);
    if (!io.exists(path))
        return false;
    io.remove(path);
    return true;
}
/** Every wg tool name as a Set (the exact active-set delta `/wg on` adds). */
export const WG_TOOL_NAME_SET = new Set(WG_TOOL_NAMES);
/** Split a tool-name list into [wg tools, everything else], preserving order. */
export function partitionToolNames(names) {
    const wg = [];
    const other = [];
    for (const name of names) {
        (WG_TOOL_NAME_SET.has(name) ? wg : other).push(name);
    }
    return { wg, other };
}
/** The active set for an ACTIVE session: everything else, then the wg family. */
export function activeToolSet(current) {
    const { other } = partitionToolNames(current);
    return [...other, ...WG_TOOL_NAMES];
}
/** The active set for a DORMANT session: everything except the wg family. */
export function dormantToolSet(current) {
    return current.filter((name) => !WG_TOOL_NAME_SET.has(name));
}
/** How many wg tools are currently declared to the model. */
export function visibleWgToolCount(getActiveTools) {
    if (!getActiveTools)
        return 0;
    try {
        return partitionToolNames(getActiveTools()).wg.length;
    }
    catch {
        return 0;
    }
}
/** One compact truth line: session · project · tools · daemon. */
export function formatActivationStatus(input) {
    return (`session: ${input.session ? "active" : "dormant"}` +
        ` · project: ${input.project ? "marked active" : "dormant"}` +
        ` · tools: ${input.visibleTools} wg tool${input.visibleTools === 1 ? "" : "s"} visible` +
        ` · daemon: ${input.daemon}`);
}
/** Probe `wg status --json` for the daemon's real state; never throws. */
async function probeDaemon(backend) {
    try {
        const status = await backend.statusJson();
        if (!status)
            return "unknown";
        return status.service?.running === true ? "running" : "stopped";
    }
    catch {
        return "unknown";
    }
}
/**
 * Install session-scoped activation.
 *
 * Load-time behavior (every session, every directory): the wg tools are already
 * registered-but-inactive (`tools.ts`, `defaultActive: false`), and the first
 * `session_start` enforces dormancy unless the project marker says otherwise.
 * `/wg` and `/wg-model` are commands, so they exist in every state — that is
 * what makes the toggle self-referential-safe.
 */
export function installSessionActivation(pi, backend, env, options = {}) {
    const io = options.io ?? DEFAULT_ACTIVATION_IO;
    const daemonRunning = options.daemonRunning ?? (() => probeDaemon(backend));
    let sessionActive = false;
    let lastCwd;
    let hooks = {};
    const projectRoot = (ctx) => resolveProjectRoot(env, ctx?.cwd ?? lastCwd);
    const readActiveTools = () => {
        try {
            return pi.getActiveTools();
        }
        catch {
            return [];
        }
    };
    const applyActiveSet = (names) => {
        try {
            pi.setActiveTools(names);
        }
        catch {
            /* no live session bound yet — the next session_start re-applies */
        }
    };
    const activate = async (ctx) => {
        applyActiveSet(activeToolSet(readActiveTools()));
        sessionActive = true;
        const root = projectRoot(ctx);
        const markerPath = markProjectActive(root, io);
        try {
            await hooks.mount?.(ctx);
        }
        catch {
            /* a surface must never throw into the chat */
        }
        return {
            session: true,
            project: markerPath !== undefined,
            markerPath,
            visibleTools: visibleWgToolCount(() => readActiveTools()),
            sessionOnly: markerPath === undefined,
        };
    };
    const deactivate = async (ctx) => {
        applyActiveSet(dormantToolSet(readActiveTools()));
        sessionActive = false;
        const root = projectRoot(ctx);
        const hadMarker = clearProjectMarker(root, io);
        try {
            await hooks.unmount?.(ctx);
        }
        catch {
            /* a surface must never throw into the chat */
        }
        return {
            session: false,
            project: false,
            markerPath: hadMarker ? activationMarkerPath(root) : undefined,
            visibleTools: visibleWgToolCount(() => readActiveTools()),
            sessionOnly: !canPersistProjectMarker(root, io),
        };
    };
    const status = async (ctx) => {
        const root = projectRoot(ctx);
        return {
            session: sessionActive,
            project: isProjectMarkedActive(root, io),
            visibleTools: visibleWgToolCount(() => readActiveTools()),
            daemon: await daemonRunning(),
            projectRoot: root,
        };
    };
    const applyForSession = async (ctx) => {
        if (ctx?.cwd)
            lastCwd = ctx.cwd;
        const root = projectRoot(ctx);
        if (isProjectMarkedActive(root, io))
            return activate(ctx);
        // Dormant: enforce it, so an inherited `--tools` / `defaultTools` list (or a
        // transcript restored from an active session) cannot leave the model
        // perceiving WG in a session that was never booted.
        return deactivate(ctx);
    };
    pi.on("session_start", (_event, ctx) => {
        void applyForSession(ctx);
    });
    return {
        isActive: () => sessionActive,
        isProjectActive: () => isProjectMarkedActive(projectRoot(), io),
        projectRoot,
        setHooks: (next) => {
            hooks = next;
        },
        activate,
        deactivate,
        status,
        applyForSession,
    };
}
/** Parent-directory-safe helper used by the `/wg` command's usage text. */
export function activationMarkerDir(projectRoot) {
    return join(projectRoot, ACTIVATION_STATE_DIR);
}
//# sourceMappingURL=activation.js.map