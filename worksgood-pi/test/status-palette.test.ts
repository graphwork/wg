/**
 * status-palette tests — the parity pin between WG's exported status palette
 * and what the plugin paints.
 *
 * WG's canonical palette lives in Rust (`src/status_palette.rs`, canonicalised
 * from `tui::viz_viewer::state::flash_color_for_status`) and is exported over
 * `wg viz --json` / `GetFleet.tree`. A checked-in fixture
 * (`test/fixtures/status-palette.json`) is asserted against that Rust table by
 * `src/status_palette.rs::checked_in_fixture_matches_exported_palette`. This
 * test asserts the plugin side against the SAME fixture, covering **every**
 * status in the state.rs table:
 *
 *   - the plugin's local fallback table equals the exported palette;
 *   - `resolveStatusPalette` keeps WG's values (no semantic indirection) and
 *     flags the fallback when the palette is missing;
 *   - `paintRgb` emits the exact `38;2;r;g;b` bytes (and a nearest 256 index);
 *   - `paintStatusText` uses RGB on the default path and falls back to a
 *     semantic role only when a status is absent — surfaced by `paletteNotice`.
 *
 * Tests run against the built `pi-worksgood/` artifact — `npm test` builds first.
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { describe, expect, it } from "vitest";
// @ts-expect-error — built ESM artifact has no co-located .d.ts on this path during dev
import {
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
} from "../pi-worksgood/index.js";

const here = dirname(fileURLToPath(import.meta.url));

/** WG's exported palette, pinned by the Rust test that reads the same file. */
const FIXTURE: Record<string, [number, number, number]> = JSON.parse(
  readFileSync(join(here, "fixtures", "status-palette.json"), "utf8"),
);

/** Every status in the state.rs table (Waiting and PendingValidation share a colour). */
const STATUSES = Object.keys(FIXTURE);

describe("status palette parity with WG", () => {
  it("covers every status in the state.rs table", () => {
    expect(STATUSES.sort()).toEqual(
      [
        "abandoned",
        "blocked",
        "done",
        "failed",
        "failed-pending-eval",
        "in-progress",
        "incomplete",
        "open",
        "pending-eval",
        "pending-validation",
        "waiting",
      ].sort(),
    );
  });

  it("the local fallback mirror equals WG's exported palette", () => {
    expect(FALLBACK_STATUS_PALETTE).toEqual(FIXTURE);
  });

  it("resolveStatusPalette keeps WG's exact values and marks them fromWg", () => {
    const palette = resolveStatusPalette(FIXTURE);
    expect(palette.fromWg).toBe(true);
    expect(palette.rgb).toEqual(FIXTURE);
    for (const status of STATUSES) {
      expect(statusRgb(status, palette)).toEqual(FIXTURE[status]);
    }
  });

  it("falls back (and is flagged) when WG exports no palette", () => {
    for (const missing of [undefined, null, {}, "nonsense", [], { done: "green" }]) {
      const palette = resolveStatusPalette(missing);
      expect(palette.fromWg).toBe(false);
      expect(palette.rgb).toEqual(FALLBACK_STATUS_PALETTE);
    }
  });

  it("normalizeStatusPalette drops malformed entries", () => {
    expect(
      normalizeStatusPalette({
        done: [80, 220, 100],
        bad: [1, 2],
        worse: "red",
        negative: [-1, 0, 0],
        floatish: [1.5, 2, 3],
      }),
    ).toEqual({ done: [80, 220, 100] });
  });

  it("paints each status's exact RGB as truecolour ANSI", () => {
    const palette = resolveStatusPalette(FIXTURE);
    for (const status of STATUSES) {
      const [r, g, b] = FIXTURE[status]!;
      const painted = paintStatusText(
        { fg: () => "SEMANTIC" },
        `line:${status}`,
        status,
        palette,
        "warning",
      );
      expect(painted).toBe(`\x1b[38;2;${r};${g};${b}mline:${status}\x1b[39m`);
      // The exact bytes are present — not a semantic-role indirection.
      expect(painted).toContain(`38;2;${r};${g};${b}`);
    }
  });

  it("fgAnsi/paintRgb produce exact truecolour and a nearest 256 index", () => {
    expect(fgAnsi([80, 220, 100])).toBe("\x1b[38;2;80;220;100m");
    expect(paintRgb("x", [80, 220, 100])).toBe("\x1b[38;2;80;220;100mx\x1b[39m");
    expect(fgAnsi([80, 220, 100], "256color")).toBe(`\x1b[38;5;${rgbToAnsi256([80, 220, 100])}m`);
    const idx = rgbToAnsi256([80, 220, 100]);
    expect(idx).toBeGreaterThanOrEqual(16);
    expect(idx).toBeLessThanOrEqual(255);
  });

  it("honours the theme's colour mode (256-colour terminals)", () => {
    const palette = resolveStatusPalette(FIXTURE);
    const painted = paintStatusText(
      { fg: () => "SEMANTIC", getColorMode: () => "256color" as const },
      "line",
      "done",
      palette,
      "success",
    );
    expect(painted).toBe(`\x1b[38;5;${rgbToAnsi256([80, 220, 100])}mline\x1b[39m`);
    expect(colorModeOf({ fg: () => "", getColorMode: () => "256color" })).toBe("256color");
    expect(colorModeOf({ fg: () => "" })).toBe("truecolor");
  });

  it("falls back to a semantic role only when the status is missing, visibly", () => {
    // A WG palette that lacks some statuses: RGB where present, role where not.
    const partial = resolveStatusPalette({ done: [80, 220, 100] });
    const semantic = (color: string, text: string) => `ROLE(${color}):${text}`;
    expect(paintStatusText({ fg: semantic as never }, "d", "done", partial, "success")).toBe(
      "\x1b[38;2;80;220;100md\x1b[39m",
    );
    expect(paintStatusText({ fg: semantic as never }, "b", "blocked", partial, "warning")).toBe(
      "ROLE(warning):b",
    );
    // The fallback is visible, never silent.
    expect(paletteNotice(partial, ["done", "blocked"])).toContain("blocked");
    // No notice when every status is painted from WG's RGB.
    expect(paletteNotice(resolveStatusPalette(FIXTURE), STATUSES)).toBeNull();
    // Missing palette → explicit theme-fallback notice.
    expect(paletteNotice(resolveStatusPalette(undefined), STATUSES)).toContain("fallback");
  });
});
