#!/usr/bin/env bash
# Smoke: wg html bundles a self-hosted JetBrains Mono webfont so the viz
# <pre> renders box-drawing glyphs (`─ │ ┐ ┘ ├ └ ┤ ┬ ┴ ┼`) on a stable
# monospace cell grid on EVERY platform — including Android Firefox.
#
# Bug shape (fix-android-firefox-wg-html-font-fallback): on Android,
# Firefox's `monospace` generic resolves to Droid Sans Mono / Noto Sans
# Mono (depending on Android version), which lacks complete box-drawing
# coverage. Without a self-hosted webfont declaring `unicode-range:
# U+2500-257F`, Firefox does per-glyph fallback for `─`/`│`/`┐`/`┘` from
# a different (proportional-metrics) font and long connector runs visually
# drift out of column. The fix bundles a JetBrains Mono Regular subset
# (~9KB WOFF2) and references it via @font-face; the font has full
# box-drawing coverage and pure-monospace advance widths (all glyphs
# width=600).
#
# What this scenario pins:
#   1. The WOFF2 font binary is emitted alongside style.css / panel.js
#      with the correct WOFF2 magic bytes (include_bytes! pipeline did
#      not corrupt the binary).
#   2. The SIL OFL 1.1 license file is co-bundled (license requires the
#      attribution travel with the font binary).
#   3. style.css declares an `@font-face` rule referencing the LOCAL
#      WOFF2 file (NOT a Google Fonts CDN URL — keeping the page
#      offline-capable).
#   4. The @font-face declares a `unicode-range` covering box-drawing
#      (U+2500-257F).
#   5. The .viz-pre font-family stack lists 'JetBrains Mono' first AND
#      includes Android-friendly fallbacks ('Roboto Mono' / 'Noto Sans
#      Mono') as a safety net if @font-face fails to load.
#   6. The .viz-pre block declares text-size-adjust: 100% (with -webkit-
#      prefix) so Android's mobile text-inflation heuristic does not
#      desynchronize advance widths between primary and fallback fonts.
#
# This is a non-LLM, no-daemon assertion: pure render + parse.

set -euo pipefail

OUTDIR=$(mktemp -d)
trap 'rm -rf "$OUTDIR"' EXIT

# Generate the HTML against the current WG project (this repo). The asset
# emission path is identical regardless of graph contents.
wg html --out "$OUTDIR" >/dev/null 2>&1

INDEX="$OUTDIR/index.html"
CSS="$OUTDIR/style.css"
WOFF2="$OUTDIR/JetBrainsMono-viz.woff2"
LICENSE="$OUTDIR/JetBrainsMono-OFL.txt"

[ -f "$INDEX" ]   || { echo "FAIL: index.html not created"; exit 1; }
[ -f "$CSS" ]     || { echo "FAIL: style.css missing";    exit 1; }
[ -f "$WOFF2" ]   || { echo "FAIL: JetBrainsMono-viz.woff2 not emitted"; exit 1; }
[ -f "$LICENSE" ] || { echo "FAIL: JetBrainsMono-OFL.txt not emitted (SIL OFL 1.1 attribution required)"; exit 1; }

# (1) WOFF2 magic bytes intact.
MAGIC=$(head -c 4 "$WOFF2" | od -An -tx1 | tr -d ' \n')
if [ "$MAGIC" != "774f4632" ]; then
    echo "FAIL: WOFF2 magic bytes corrupted: got $MAGIC, expected 774f4632 (wOF2)"
    exit 1
fi

# (1b) Reasonable file size — corruption / accidental empty file guard.
WOFF2_BYTES=$(wc -c < "$WOFF2")
if [ "$WOFF2_BYTES" -lt 4096 ]; then
    echo "FAIL: WOFF2 unexpectedly small ($WOFF2_BYTES bytes) — include_bytes! broken?"
    exit 1
fi
if [ "$WOFF2_BYTES" -gt 200000 ]; then
    echo "FAIL: WOFF2 unexpectedly large ($WOFF2_BYTES bytes) — accidentally bundling unsubsetted font?"
    exit 1
fi

# (2) License file has the SIL OFL header.
grep -q "SIL Open Font License" "$LICENSE" \
    || { echo "FAIL: JetBrainsMono-OFL.txt missing SIL OFL header"; exit 1; }

# (3) @font-face references LOCAL file, not a remote CDN.
grep -q '@font-face' "$CSS" \
    || { echo "FAIL: style.css missing @font-face declaration"; exit 1; }
grep -qE "url\(['\"]?JetBrainsMono-viz\.woff2['\"]?\)" "$CSS" \
    || { echo "FAIL: @font-face must reference local 'JetBrainsMono-viz.woff2' (offline-capable)"; exit 1; }
if grep -qE 'fonts\.(googleapis|gstatic)\.com' "$CSS"; then
    echo "FAIL: style.css must NOT pull from Google Fonts — page must be offline-capable"
    exit 1
fi
grep -qE "format\(['\"]woff2['\"]\)" "$CSS" \
    || { echo "FAIL: @font-face src missing format('woff2')"; exit 1; }

# (4) unicode-range covers box-drawing.
grep -qE 'U\+2500-25[7F]F' "$CSS" \
    || { echo "FAIL: @font-face unicode-range must include U+2500-257F (box-drawing)"; exit 1; }

# (5) .viz-pre font stack assertions, scoped to inside the .viz-pre block.
python3 - "$CSS" <<'PYEOF'
import re, sys
css = open(sys.argv[1], encoding='utf-8').read()

# Extract the .viz-pre { ... } block.
m = re.search(r'\.viz-pre\s*\{(.*?)\}', css, re.DOTALL)
if not m:
    print("FAIL: missing .viz-pre block in style.css")
    sys.exit(1)
block = m.group(1)

# font-family stack must put 'JetBrains Mono' first and end in 'monospace'.
ff = re.search(r"font-family:\s*([^;]+);", block, re.DOTALL)
if not ff:
    print("FAIL: .viz-pre missing font-family declaration")
    sys.exit(1)
stack = ff.group(1).strip()

if "'JetBrains Mono'" not in stack:
    print(f"FAIL: .viz-pre font-family must list 'JetBrains Mono' first: got {stack!r}")
    sys.exit(1)

jbm_idx = stack.find("'JetBrains Mono'")
mono_idx = stack.rfind("monospace")
if mono_idx == -1 or jbm_idx >= mono_idx:
    print(f"FAIL: 'JetBrains Mono' must precede generic 'monospace' in stack: {stack!r}")
    sys.exit(1)

if "'Roboto Mono'" not in stack and "'Noto Sans Mono'" not in stack:
    print(f"FAIL: .viz-pre font-family must include an Android-friendly fallback "
          f"('Roboto Mono' or 'Noto Sans Mono'): got {stack!r}")
    sys.exit(1)

# (6) Mobile text-size-adjust hardening.
if "text-size-adjust: 100%" not in block:
    print("FAIL: .viz-pre missing 'text-size-adjust: 100%' "
          "(Android mobile text-inflation breaks box-drawing alignment without it)")
    sys.exit(1)
if "-webkit-text-size-adjust: 100%" not in block:
    print("FAIL: .viz-pre missing '-webkit-text-size-adjust: 100%' "
          "(Chrome/WebKit-derived Android browsers need the prefix)")
    sys.exit(1)

print("OK: .viz-pre font stack + mobile hardening intact")
PYEOF

echo "PASS: wg html bundles JetBrains Mono webfont with @font-face + Android-safe fallbacks"
