# Fix: Android Firefox font fallback in `wg html`

Task: `fix-android-firefox-wg-html-font-fallback`
Related context: `bug-wg-html-box-drawing-glyph-alignment.md`,
prior CSS-only attempt `fix-wg-html-box-drawing-alignment` (commit `3dea8bb`)

## Symptom

On Android Firefox the dependency-graph viz inside `<pre class="viz-pre">`
shows long Unicode connector runs (`─` strings ≥4 cells) drifting out of
column with the corner / arrow glyphs they're supposed to land on.
Linux Firefox and Chrome render correctly. Long cross-graph edges become
unfollowable.

## Root cause

The previous fix (`fix-wg-html-box-drawing-alignment`) added typography
invariants — disabled ligatures, kerning, font-synthesis, and zeroed the
edge-span box metrics. Those invariants are necessary but not sufficient
on Android Firefox.

The remaining failure is **per-glyph font fallback inside a single line**:

1. The `.viz-pre` font stack started with
   `'JetBrains Mono', ui-monospace, 'Cascadia Code', 'Source Code Pro',
   Menlo, Consolas, monospace`. None of these are present on stock
   Android (Apple, Microsoft, JetBrains-only fonts).
2. Android Firefox falls through to the generic `monospace`, which on
   stock Android resolves to **Droid Sans Mono** (older builds) or
   **Noto Sans Mono** (newer). Neither has complete coverage of
   U+2500–257F (box-drawing) on every Android version — Droid Sans Mono
   is the bigger offender; many of `─`, `│`, `┐`, `┘`, `├`, `└`, `┤`,
   `┬`, `┴`, `┼` are missing or only partially mapped.
3. When a glyph is missing, Firefox does **per-character font fallback**
   to the next system font that has it (often a sans-serif Noto fallback
   or Roboto). That fallback font is **not the same monospace metric**
   as Droid/Noto Sans Mono — its `─` advance width differs from the
   surrounding ASCII glyphs by a fraction of a pixel.
4. Per-pixel rounding accumulates across long `─` runs, so a 30-cell
   run that's supposed to land on column 47 actually lands one or more
   columns short or long. The corner glyph drifts off the connector.

This is an Android-Firefox-specific bug class because:
- Linux Firefox uses DejaVu Sans Mono / Liberation Mono → has full
  box-drawing.
- Chrome on Android also lacks the fonts but uses a slightly different
  fallback heuristic (sometimes lands on Roboto Mono, which has
  box-drawing).
- iOS Safari uses SF Mono → has box-drawing.

## Fix

**Self-host a JetBrains Mono Regular subset** containing only the
glyphs `wg viz` actually emits. Reference it via `@font-face` with a
`unicode-range` that includes U+2500–257F. This guarantees that every
browser/platform — including Android Firefox — finds box-drawing glyphs
in a single, monospace font with stable advance widths.

### What landed

| Change                                          | File                                                    |
|-------------------------------------------------|---------------------------------------------------------|
| 9.2KB WOFF2 subset (ASCII + box-drawing + arrows + geometric symbols) | `src/html_assets/JetBrainsMono-viz.woff2` |
| SIL OFL 1.1 attribution                         | `src/html_assets/JetBrainsMono-OFL.txt`                 |
| `include_bytes!` + write to out_dir             | `src/html.rs`                                           |
| `@font-face` declaration with `unicode-range`   | `src/html_assets/style.css`                             |
| Expanded `.viz-pre` font stack                  | `src/html_assets/style.css`                             |
| `text-size-adjust: 100%` (with `-webkit-` prefix) | `src/html_assets/style.css`                           |
| `-webkit-font-smoothing: antialiased`, `-moz-osx-font-smoothing: grayscale` | `src/html_assets/style.css`             |
| Integration test                                | `tests/integration_html.rs::bundles_jetbrains_mono_webfont_for_box_drawing_alignment` |
| Smoke scenario                                  | `tests/smoke/scenarios/wg_html_font_bundle.sh`          |
| Smoke manifest entry                            | `tests/smoke/manifest.toml`                             |

### Subset details

Generated via `pyftsubset` from upstream JetBrainsMono-Regular.woff2:

```
pyftsubset jbm-regular.woff2 \
    --output-file=jbm-viz.woff2 \
    --flavor=woff2 \
    --unicodes="U+0020-007E,U+00A0,U+2010-2027,U+2190-21FF,U+2500-257F,U+2580-259F,U+25A0-25FF,U+2600-26FF" \
    --no-hinting \
    --desubroutinize \
    --layout-features='' \
    --notdef-outline \
    --recommended-glyphs
```

- Source: `JetBrainsMono-Regular.woff2` from
  https://github.com/JetBrains/JetBrainsMono (Apache 2 / OFL 1.1)
- Verified: 356 glyphs, all advance widths = 600 units (pure monospace)
- Verified: full coverage of `─ │ ┐ ┘ ├ └ ┤ ┬ ┴ ┼ ╗ ╝ ═ ║ ← → ↑ ↓ ● ◇`

### Font stack rationale

```
font-family: 'JetBrains Mono',
             ui-monospace,
             'Cascadia Code',
             'Source Code Pro',
             'Roboto Mono',         /* Android safety net */
             'Noto Sans Mono',      /* newer Android default */
             'Droid Sans Mono',     /* older Android default */
             Menlo,
             Consolas,
             monospace;
```

The Android-friendly fallbacks (`'Roboto Mono'`, `'Noto Sans Mono'`)
defend against the @font-face load failing — e.g., if the page is
served behind a strict CSP that drops the WOFF2 request, or if a user
clicks a broken offline mirror.

The cascade still applies per-glyph for characters outside the
@font-face `unicode-range`, so the rest of the chain still matters.

### Mobile hardening

```css
-webkit-text-size-adjust: 100%;
text-size-adjust: 100%;
```

Android Firefox / Chrome auto-enlarge text in `<pre>` blocks via "text
inflation" heuristics that adjust per-element font size based on
content width. This desynchronizes box-drawing advance widths from
surrounding glyphs (the fallback font and the primary font are scaled
by different amounts before subpixel rounding). Pinning
`text-size-adjust: 100%` disables the heuristic so every cell shares a
single computed font size.

```css
-webkit-font-smoothing: antialiased;
-moz-osx-font-smoothing: grayscale;
```

Forces grayscale anti-aliasing, which is metric-stable across glyph
classes. Subpixel AA can produce slightly different effective metrics
for box-drawing vs ASCII glyphs depending on subpixel offset.

## Why bundle, not Google Fonts CDN

- **Offline capability**: `wg html` outputs a static directory meant to
  be served via `wg publish` (rsync). The output should work on a
  laptop with no internet, on a server with no outbound network, and
  behind an air-gapped intranet.
- **Performance**: 9.2KB local file vs ~80KB Google Fonts WOFF2 +
  uncached DNS + TLS handshake.
- **Privacy**: no fingerprinting via Google Fonts CDN.
- **Reproducibility**: the font shipped with `wg` is the font the
  user sees — no version drift from Google Fonts API changes.
- **CSP-friendly**: works under `style-src 'self'; font-src 'self'`.

## Why subset, not full

- 9.2KB vs 92KB (10× smaller).
- The viz only emits ASCII, box-drawing, arrows, block elements,
  geometric shapes, and miscellaneous symbols. Anything else (CJK,
  Cyrillic, etc.) falls through to the rest of the cascade via
  `unicode-range`.

## Verification

### Local

- `cargo build --release` ✓
- `cargo test --release --test integration_html bundles_jetbrains_mono_webfont_for_box_drawing_alignment` ✓
- `cargo install --path .` ✓
- `wg html --out /tmp/test` emits `JetBrainsMono-viz.woff2` (9284 B,
  intact `wOF2` magic bytes) and `JetBrainsMono-OFL.txt` ✓
- `bash tests/smoke/scenarios/wg_html_font_bundle.sh` PASS ✓
- `bash tests/smoke/scenarios/wg_html_box_drawing_alignment.sh` still PASS ✓
- `bash tests/smoke/scenarios/wg_html_ascii_viz.sh` still PASS ✓

### Acceptance criteria mapped

From the bug report's "Acceptance criteria":

| Criterion | Status |
|-----------|--------|
| Browser screenshot shows long connector runs lining up | Cannot run Android Firefox here — but the structural fix (font with full box-drawing in same monospace metric, in a single `@font-face`) makes per-glyph fallback impossible for the entire box-drawing range. The bug class is closed by construction. |
| Selecting a node still highlights upstream/downstream/cycle edges | Untouched — `<span class="edge" data-edges="from>to">` markup unchanged; `wg_html_box_drawing_alignment.sh` smoke verifies. |
| Fix works without iframe scaling hacks | Yes — pure CSS + bundled font; no parent-page contract. |
| Smoke / visual regression test protects long-range connectors | `wg_html_font_bundle.sh` + the existing `wg_html_box_drawing_alignment.sh` together pin: (a) WOFF2 file is bundled with intact magic bytes, (b) `@font-face` is declared and references the local file, (c) `unicode-range` covers box-drawing, (d) font stack lists `'JetBrains Mono'` first + Android-friendly fallbacks, (e) `text-size-adjust` mobile hardening present, (f) long `─` runs survive HTML emission byte-for-byte, (g) every dash cell is wrapped in a single edge span. |

### Manual / on-device verification (recommended after this lands)

The structural fix is high-confidence, but a real Android Firefox
verification step on the published mirror
(https://ulivo.poietic.life/wg/feeds/workgraph-itself/) would close
the loop:

1. Open the page on Android Firefox.
2. Scroll to a long-edge case (e.g., a coordinator → many-leaf fan-out).
3. Verify that `─` runs land on the corner glyph (`┐`/`┘`/`├`/`└`).
4. Open Firefox Devtools → Inspector → Computed → `font-family` on a
   `.viz-pre` cell. Confirm the resolved font is `JetBrains Mono`.
5. Network tab: confirm `JetBrainsMono-viz.woff2` was fetched 200 OK.

If alignment is still off after the @font-face load, the residual
cause is not font fallback (because every glyph now comes from the
same monospace font) — it's likely subpixel rounding on the device's
specific DPR. In that case the next step is to switch to a CSS-grid
cell layout (each character in its own grid cell), which is item 5 in
the original bug's "Suggested fixes" — left as a follow-up if needed.

## Found while working on this task

The integration test `tests/integration_html.rs::description_html_is_escaped`
fails on `main` (verified via fresh clone, unrelated to this task):

```
thread 'description_html_is_escaped' panicked at tests/integration_html.rs:262:
raw <script> tag leaked: <!DOCTYPE html>...
<div id="desc-pretty" class="description-rendered"><script>alert('pwn')</script></div>
```

Root cause: `polish-wg-html` introduced a `description_html` field that
runs the description through `pulldown-cmark` and writes the result raw,
which lets `<script>` HTML through. The `<pre id="desc-raw">` correctly
escapes — only the new `desc-pretty` path is vulnerable.

Logged separately as a follow-up XSS bug task — out of scope for the
font-fallback fix.
