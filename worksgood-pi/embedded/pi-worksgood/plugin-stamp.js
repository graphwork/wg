/**
 * plugin-stamp.ts — this plugin build's own identity, and its comparison
 * against the running `wg` binary's embedded build.
 *
 * The operator-reported fleet-panel mystery (wrong colours) had one root cause:
 * the session was running a STALE stack — an old daemon payload (no RGB palette)
 * with a plugin cache that no longer matched the binary. Neither fact was
 * visible from inside the session; only `wg dev-check` / `wg pi-plugin status`
 * (out-of-session CLI) could show it.
 *
 * This module is the in-session answer: it reads this build's
 * `.wg-embed-digest` stamp (the content digest `ensure-pi-plugin` materialized
 * the cache with), and {@link compareBuildStamps} turns the pair
 * (this build, the running binary's embed digest) into a nameable skew.
 *
 * Only a binary-materialized cache dir carries the stamp; a dev tree or an npm
 * `node_modules` install has none, so the comparison honestly reports
 * "unknown" rather than guessing.
 */
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
/**
 * Read the content digest a plugin cache dir was materialized with.
 *
 * `versionDir` is the parent of `pi-worksgood/`. Returns `undefined` when the
 * companion `.wg-embed-digest` stamp is absent (dev/npm installs).
 */
export function readEmbedDigestAt(versionDir) {
    try {
        const text = readFileSync(path.join(versionDir, ".wg-embed-digest"), "utf8").trim();
        return text || undefined;
    }
    catch {
        return undefined;
    }
}
/** The version dir THIS build was loaded from (parent of `pi-worksgood/`). */
export function ownVersionDir(moduleUrl = import.meta.url) {
    return path.resolve(path.dirname(fileURLToPath(moduleUrl)), "..");
}
/** This build's materialization stamp, or `undefined` for dev/npm installs. */
export function ownBuildDigest(moduleUrl) {
    return readEmbedDigestAt(ownVersionDir(moduleUrl));
}
/**
 * True when this pi process is a WG-managed worker/agent, not a human console.
 *
 * The daemon exports `WG_AGENT_ID` for every spawned task agent, and an
 * attempt-scoped sandbox holds `WG_WORKER_CAPABILITY`. Either signal means the
 * session is administered by WG, so admin probes (`wg pi-plugin …`) are refused
 * by the worker gate and must not be attempted (see `stack-status.ts`).
 */
export function isWorkerSession(env = process.env) {
    const nonEmpty = (value) => typeof value === "string" && value.trim().length > 0;
    return nonEmpty(env.WG_AGENT_ID) || nonEmpty(env.WG_WORKER_CAPABILITY);
}
/**
 * Compare this plugin build's stamp against the running binary's embed digest.
 *
 * A digest only proves SAME-or-DIFFERENT; it never proves which side is newer.
 * So the wording is "the daemon binary embeds a different plugin build", never a
 * confident "the daemon is older" — except where the config/`wg dev-sync` path
 * legitimately says so, because there the operator's cache is the thing that
 * was materialized by the installed binary.
 */
export function compareBuildStamps(pluginDigest, binaryEmbedDigest) {
    if (!pluginDigest || !binaryEmbedDigest)
        return "unknown";
    return pluginDigest === binaryEmbedDigest ? "match" : "skew";
}
/** A short, copy-pasteable digest (full digests are long and identical-prefixed). */
export function shortDigest(digest) {
    if (!digest)
        return "unknown";
    const bare = digest.replace(/^b3:/, "");
    return bare.length <= 10 ? digest : `b3:${bare.slice(0, 8)}…`;
}
//# sourceMappingURL=plugin-stamp.js.map