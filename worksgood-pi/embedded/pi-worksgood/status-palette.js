/**
 * status-palette.ts — WG's canonical status palette, and how the plugin paints
 * it.
 *
 * The graph/fleet view must match WG's own TUI colours **exactly**. WG's palette
 * is an explicit-RGB table (`src/status_palette.rs`, canonicalised from
 * `tui::viz_viewer::state::flash_color_for_status`). WG exports that table over
 * `wg viz --json` and the daemon `GetFleet.tree` payload as `palette`
 * (`status -> [r, g, b]`). On the default path this module renders *those exact
 * bytes* as ANSI truecolour / 256-colour foreground — never a pi theme role,
 * whose value is whatever the active theme happens to define.
 *
 * Role → RGB is therefore data, not a semantic guess. Only when WG's palette is
 * unavailable (an older daemon/CLI, a malformed payload) do we fall back to the
 * semantic pi theme roles in `fleet-readmodel.ts::taskColor`, and that fallback
 * is surfaced in the UI (`paletteNotice`) rather than silently mismatching.
 */
/**
 * The local mirror of WG's graph-view palette.
 *
 * **Fallback only** — used when WG did not export a palette (older daemon/CLI or
 * a malformed payload). It is the same table as `src/status_palette.rs` and is
 * pinned to `test/fixtures/status-palette.json` by `status-palette.test.ts`, so
 * it can never silently drift from WG. When this fallback is in play the panel
 * says so (`paletteNotice`), because a *semantic* role paint is not a colour
 * guarantee.
 */
export const FALLBACK_STATUS_PALETTE = {
    open: [200, 200, 80],
    "in-progress": [60, 200, 220],
    waiting: [60, 160, 220],
    done: [80, 220, 100],
    blocked: [180, 120, 60],
    failed: [220, 60, 60],
    abandoned: [140, 100, 160],
    "pending-validation": [60, 160, 220],
    "pending-eval": [140, 230, 80],
    "failed-pending-eval": [210, 130, 70],
    incomplete: [255, 165, 0],
};
/** Is `value` a well-formed `[r, g, b]` byte triple? */
function isRgb(value) {
    return (Array.isArray(value) &&
        value.length === 3 &&
        value.every((n) => typeof n === "number" && Number.isInteger(n) && n >= 0 && n <= 255));
}
/**
 * Normalize a raw `palette` payload (from `wg viz --json` / `GetFleet.tree`) to
 * a `status -> RGB` map, dropping any malformed entries. Returns `{}` for a
 * missing/garbage payload, which {@link resolveStatusPalette} treats as
 * "unavailable".
 */
export function normalizeStatusPalette(raw) {
    if (typeof raw !== "object" || raw === null || Array.isArray(raw))
        return {};
    const out = {};
    for (const [status, rgb] of Object.entries(raw)) {
        if (isRgb(rgb))
            out[status] = [rgb[0], rgb[1], rgb[2]];
    }
    return out;
}
/**
 * Resolve the palette to paint with: WG's exported table when present, else the
 * local fallback (flagged `fromWg: false` so the caller can surface it).
 */
export function resolveStatusPalette(exported) {
    const rgb = normalizeStatusPalette(exported);
    if (Object.keys(rgb).length > 0)
        return { rgb, fromWg: true };
    return { rgb: { ...FALLBACK_STATUS_PALETTE }, fromWg: false };
}
/** RGB for a status name, or `undefined` when the palette has no entry. */
export function statusRgb(status, palette) {
    return palette.rgb[status];
}
/** Nearest xterm-256 colour index for an RGB triple (6×6×6 cube + greys). */
export function rgbToAnsi256([r, g, b]) {
    const toCube = (v) => Math.round((v / 255) * 5);
    const cr = toCube(r);
    const cg = toCube(g);
    const cb = toCube(b);
    const cubeIndex = 16 + 36 * cr + 6 * cg + cb;
    // Grey ramp: 24 shades from 8 to 238. Use it when it is closer than the cube.
    const grey = Math.round(((r + g + b) / 3 - 8) / 10);
    if (grey < 0)
        return cubeIndex;
    if (grey > 23)
        return 231;
    const greyValue = 8 + grey * 10;
    const cubeRgb = [cr * 51, cg * 51, cb * 51];
    const cubeDist = (cubeRgb[0] - r) ** 2 + (cubeRgb[1] - g) ** 2 + (cubeRgb[2] - b) ** 2;
    const greyDist = (greyValue - r) ** 2 + (greyValue - g) ** 2 + (greyValue - b) ** 2;
    return greyDist < cubeDist ? 232 + grey : cubeIndex;
}
/**
 * The foreground SGR prefix for an RGB triple in the given terminal mode.
 * `truecolor` emits `38;2;r;g;b`; `256color` emits the nearest `38;5;N`.
 */
export function fgAnsi(rgb, mode = "truecolor") {
    return mode === "256color"
        ? `\x1b[38;5;${rgbToAnsi256(rgb)}m`
        : `\x1b[38;2;${rgb[0]};${rgb[1]};${rgb[2]}m`;
}
/** Paint `text` in an exact RGB colour (default foreground reset with `39`). */
export function paintRgb(text, rgb, mode = "truecolor") {
    return `${fgAnsi(rgb, mode)}${text}\x1b[39m`;
}
/** Read the theme's colour mode defensively (defaults to truecolour). */
export function colorModeOf(theme) {
    try {
        const mode = theme?.getColorMode?.();
        return mode === "256color" ? "256color" : "truecolor";
    }
    catch {
        return "truecolor";
    }
}
/**
 * Paint one task-status line.
 *
 * Default path: WG's exported/fallback RGB, rendered as exact truecolour or
 * 256-colour ANSI. Only when the palette has no entry for `status` does this
 * fall back to the semantic role from `taskColor`; the caller uses
 * {@link paletteNotice} to make that visible.
 */
export function paintStatusText(theme, text, status, palette, semanticRole) {
    if (!theme)
        return text;
    const rgb = statusRgb(status, palette);
    if (rgb) {
        try {
            return paintRgb(text, rgb, colorModeOf(theme));
        }
        catch {
            /* fall through to the semantic role */
        }
    }
    try {
        return theme.fg(semanticRole, text);
    }
    catch {
        return text;
    }
}
/**
 * A short, human-visible notice when the real RGB palette is not in play: WG's
 * palette was unavailable, or some rendered status had no palette entry. Returns
 * `null` when every status is painted from WG's exported RGB.
 */
export function paletteNotice(palette, statuses) {
    if (!palette.fromWg)
        return "colour: pi theme fallback (WG palette unavailable)";
    const missing = new Set();
    for (const status of statuses) {
        if (statusRgb(status, palette) === undefined)
            missing.add(status);
    }
    if (missing.size === 0)
        return null;
    return `colour: theme fallback for ${[...missing].sort().join(", ")}`;
}
//# sourceMappingURL=status-palette.js.map