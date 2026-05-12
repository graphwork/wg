#!/bin/bash
# sync-docs.sh — Convert typst source docs to markdown and optionally sync to website
#
# Usage: ./scripts/sync-docs.sh [--website /path/to/graphwork.github.io]
#
# Typst is the source of truth. This script generates markdown derivatives.

set -e

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WEBSITE_DIR=""

if [[ "$1" == "--website" && -n "$2" ]]; then
    WEBSITE_DIR="$2"
fi

# Preprocess typst to strip constructs pandoc can't handle
preprocess_typst() {
    python3 -c '
import re, sys

content = sys.stdin.read()

# Replace #figure(raw(block: true, ...)) with fenced code blocks
# These are multi-line raw strings that pandoc cannot parse
def replace_figure_raw(m):
    body = m.group(1)
    # Strip leading/trailing quotes and whitespace
    body = body.strip()
    if body.startswith("\"") and body.endswith("\""):
        body = body[1:-1]
    return "```\n" + body + "\n```"

# Match #figure(\n  raw(block: true, lang: ...,\n"..."),\n  caption: ...\n)
content = re.sub(
    r"#figure\(\s*raw\(block:\s*true,\s*lang:\s*\w+,\s*\"(.*?)\"\s*\)\s*(?:,\s*caption:\s*\[.*?\])?\s*\)",
    replace_figure_raw,
    content,
    flags=re.DOTALL
)

# Also match standalone raw() without figure wrapper
content = re.sub(
    r"raw\(block:\s*true,\s*lang:\s*(?:none|\"\w+\"),\s*\"(.*?)\"\s*\)",
    replace_figure_raw,
    content,
    flags=re.DOTALL
)

# Replace #table(...) with simple text (strip table markup, keep cell content)
def replace_table(m):
    body = m.group(0)
    # Extract cell contents: text in [...] brackets
    cells = re.findall(r"\[([^\]]*)\]", body)
    if not cells:
        return ""
    # Try to pair them as term/definition
    lines = []
    for i in range(0, len(cells), 2):
        if i + 1 < len(cells):
            lines.append(f"**{cells[i].strip()}**: {cells[i+1].strip()}")
        else:
            lines.append(cells[i].strip())
    return "\n".join(lines) + "\n"

content = re.sub(
    r"#table\(.*?\n(?:.*?\n)*?.*?\)\s*$",
    replace_table,
    content,
    flags=re.MULTILINE
)

sys.stdout.write(content)
'
}

convert_typ_to_md() {
    local src="$1"
    local dst="$2"
    echo "  $(basename "$src") → $(basename "$dst")"

    # Try direct pandoc conversion first
    if pandoc -f typst -t gfm --wrap=none "$src" -o "$dst" 2>/dev/null; then
        return 0
    fi

    # Fallback: preprocess then convert
    local tmp=$(mktemp --suffix=.typ)
    preprocess_typst < "$src" > "$tmp"

    if pandoc -f typst -t gfm --wrap=none "$tmp" -o "$dst" 2>/dev/null; then
        rm "$tmp"
        return 0
    fi

    # Last resort: just copy as-is (typst is readable enough)
    echo "    Warning: pandoc failed, copying raw typst"
    cp "$src" "$dst"
    rm "$tmp"
}

echo "Converting typst docs to markdown..."

# Manual chapters
for chapter in "$REPO_ROOT"/docs/manual/0[1-5]-*.typ; do
    basename=$(basename "$chapter" .typ)
    convert_typ_to_md "$chapter" "$REPO_ROOT/docs/manual/$basename.md"
done

# Concatenate chapters into full manual markdown
echo "  Assembling full manual..."
cat "$REPO_ROOT"/docs/manual/0[1-5]-*.md > "$REPO_ROOT/docs/manual/wg-manual.md"

# Organizational patterns
if [[ -f "$REPO_ROOT/docs/research/organizational-patterns.typ" ]]; then
    convert_typ_to_md \
        "$REPO_ROOT/docs/research/organizational-patterns.typ" \
        "$REPO_ROOT/docs/research/organizational-patterns.md"
fi

echo "Done. Markdown files generated."

# Sync to website if path provided
if [[ -n "$WEBSITE_DIR" && -d "$WEBSITE_DIR" ]]; then
    echo ""
    echo "Syncing to website: $WEBSITE_DIR"
    WEBSITE_PUBLIC="$WEBSITE_DIR/public"
    if [[ ! -d "$WEBSITE_PUBLIC" ]]; then
        WEBSITE_PUBLIC="$WEBSITE_DIR"
    fi
    cp "$REPO_ROOT/docs/manual/wg-manual.md" "$WEBSITE_PUBLIC/"
    cp "$REPO_ROOT/docs/research/organizational-patterns.md" "$WEBSITE_PUBLIC/" 2>/dev/null || true
    echo "Website markdown updated."
fi
