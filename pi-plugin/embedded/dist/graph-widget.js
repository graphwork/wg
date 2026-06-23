/**
 * Legacy graph-widget compatibility exports.
 *
 * The plugin used to install a passive "ready tasks" widget/status footer in
 * every Pi session. That duplicated the WG TUI and created chat cruft, so the
 * current contract is explicit tools/commands only. Keep these exports as no-op
 * compatibility shims for older imports.
 */
/** Parse `wg ready --json` stdout into a typed list, tolerating junk. */
export function parseReady(stdout) {
    const out = stdout.trim();
    if (!out)
        return [];
    try {
        const parsed = JSON.parse(out);
        return Array.isArray(parsed) ? parsed : [];
    }
    catch {
        return [];
    }
}
/** Deprecated no-op: passive ready-task UI is intentionally disabled. */
export function renderWidget(_ready) {
    return [];
}
/** Deprecated no-op: do not subscribe to session lifecycle UI refresh hooks. */
export function installGraphWidget(_pi, _backend) {
    // Intentionally empty.
}
//# sourceMappingURL=graph-widget.js.map