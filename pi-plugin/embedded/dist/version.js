/**
 * version.ts — the wg↔pi WIRE-COMPAT stamp (GENERATED — do not edit by hand).
 *
 * Single source of truth is the Rust const `WG_PI_PLUGIN_COMPAT_VERSION` in
 * `src/pi_plugin/mod.rs`. The `make embed-pi-plugin` step rewrites BOTH this
 * file and `pi-plugin/embedded/version.json` from that const so the three can
 * never silently diverge (a Rust unit test asserts const == embedded JSON, and
 * CI re-runs the embed and `git diff --exit-code`s the result).
 *
 * This is a *wire-compat* number, deliberately decoupled from the npm
 * `package.json` `version` of `@worksgood/wg-pi-plugin` — exactly as agency's
 * `WG_AGENCY_COMPAT_VERSION` is decoupled from any package version. Bump it
 * whenever the wg↔plugin flag/contract surface changes.
 *
 * The plugin factory (`src/index.ts`) asserts this value against the wg binary
 * at startup and fails LOUDLY on mismatch.
 */
export const WG_PI_PLUGIN_COMPAT_VERSION = "0.1.1";
//# sourceMappingURL=version.js.map