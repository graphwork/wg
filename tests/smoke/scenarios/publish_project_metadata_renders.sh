#!/usr/bin/env bash
# Scenario: publish_project_metadata_renders
#
# Pins wg-html-publish-2: `wg html publish` supports project metadata
# (title / byline / abstract markdown) at the top of the rendered page,
# resolved through a cascade:
#   1. Per-deployment override (--title / --byline / --abstract on `add`,
#      stored in html-publish.toml)
#   2. Project-level [project] section in <wg_dir>/config.toml
#   3. <wg_dir>/about.md for the abstract (auto-discovered)
#   4. Empty everywhere → project-header is OMITTED and the minimal
#      browser/visible title falls back to hostname:/repo/path
#
# This scenario walks the cascade end-to-end against a local rsync target,
# verifying:
#   * `--title` / `--byline` / `--abstract` flags are advertised in --help
#     and persist into html-publish.toml round-trip
#   * The rendered index.html contains <header class="project-header"> with
#     the resolved title, byline, and a markdown-rendered abstract
#   * Per-deployment values win over project-level config
#   * Project-level config + about.md are picked up when no per-deployment
#     override is set
#   * When no metadata is configured anywhere, the project-header block is
#     OMITTED and <title>/<h1> identify the source as hostname:/repo/path
#   * A configured title is used instead of the path source label, which is
#     the portable/public-export fallback for users who do not want to expose
#     a local absolute path

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v rsync >/dev/null 2>&1; then
    loud_skip "rsync_missing" "rsync is not installed in PATH"
    exit 77
fi

scratch=$(make_scratch)
cd "$scratch"

if ! wg init --route local >init.log 2>&1; then
    loud_fail "wg init --route local failed: $(tail -5 init.log)"
fi

if ! wg add "Sample task" --id sample >add.log 2>&1; then
    loud_fail "wg add failed: $(tail -5 add.log)"
fi

# (1) --help must advertise the new flags
help_out=$(wg html publish add --help 2>&1)
for flag in --title --byline --abstract; do
    if ! echo "$help_out" | grep -qE "^\s+${flag}"; then
        loud_fail "wg html publish add --help must advertise ${flag}; got:\n$help_out"
    fi
done

dest="$scratch/dest-override"
mkdir -p "$dest"

# (2) per-deployment override: --title / --byline persist + render
if ! wg html publish add override --rsync "$dest/" \
        --title 'Override Title' --byline 'override byline' >override-add.log 2>&1; then
    loud_fail "publish add with --title/--byline failed: $(tail -10 override-add.log)"
fi
show_out=$(wg html publish run override 2>&1) || loud_fail "publish run override failed: $show_out"

if [[ ! -f "$dest/index.html" ]]; then
    loud_fail "expected $dest/index.html after run; got: $(ls -la "$dest")"
fi

if ! grep -q '<header class="project-header">' "$dest/index.html"; then
    loud_fail "rendered html missing <header class=\"project-header\"> block"
fi
if ! grep -q '<h1 class="project-title">Override Title</h1>' "$dest/index.html"; then
    loud_fail "rendered html missing per-deployment title 'Override Title'"
fi
if ! grep -q '<p class="project-byline">override byline</p>' "$dest/index.html"; then
    loud_fail "rendered html missing per-deployment byline 'override byline'"
fi
if ! grep -q '<title>Override Title — all tasks</title>' "$dest/index.html"; then
    loud_fail "configured title should drive browser title instead of generic workgraph or host path"
fi
if ! grep -q '<h1>Override Title</h1>' "$dest/index.html"; then
    loud_fail "configured title should replace the visible minimal workgraph header"
fi

# (3) Project-level cascade: config.toml [project] + about.md
# Find the workgraph dir (.wg or .wg) so we can append to config.toml
# and write about.md.
wg_dir=""
for candidate in .wg .wg; do
    if [[ -d "$scratch/$candidate" ]]; then
        wg_dir="$scratch/$candidate"
        break
    fi
done
if [[ -z "$wg_dir" ]]; then
    loud_fail "could not find workgraph dir under $scratch (.wg or .wg)"
fi

# Replace the empty [project] block with one that has title + byline.
# init creates an empty `[project]` line; we substitute it.
python3 - "$wg_dir/config.toml" <<'PY'
import sys, re, pathlib
p = pathlib.Path(sys.argv[1])
text = p.read_text()
# Replace the first `[project]` (with optional empty body) with a populated block.
# Match `[project]` line, optionally followed by blank/whitespace lines until next `[`.
new_block = (
    '[project]\n'
    'title = "Project Config Title"\n'
    'byline = "project config byline"\n'
)
text2, n = re.subn(r'\[project\][^\[]*?(?=\n\[)', new_block, text, count=1, flags=re.S)
if n == 0:
    # No existing [project] block: append.
    text2 = text + '\n' + new_block
p.write_text(text2)
PY

cat > "$wg_dir/about.md" <<'EOF'
## Focus

This is the **abstract**. It supports markdown:

- bullet one
- bullet two
EOF

dest_cascade="$scratch/dest-cascade"
mkdir -p "$dest_cascade"

# Deployment with no per-deployment overrides → must resolve to project config.
if ! wg html publish add cascade --rsync "$dest_cascade/" >cascade-add.log 2>&1; then
    loud_fail "publish add (cascade) failed: $(tail -10 cascade-add.log)"
fi
if ! wg html publish run cascade >cascade-run.log 2>&1; then
    loud_fail "publish run cascade failed: $(tail -20 cascade-run.log)"
fi

if ! grep -q '<h1 class="project-title">Project Config Title</h1>' "$dest_cascade/index.html"; then
    loud_fail "cascade case must render project-config title; got: $(grep -A 1 project-title "$dest_cascade/index.html" | head)"
fi
if ! grep -q '<p class="project-byline">project config byline</p>' "$dest_cascade/index.html"; then
    loud_fail "cascade case must render project-config byline"
fi
# about.md must be markdown-rendered (h2 + ul + li).
if ! grep -q '<h2>Focus</h2>' "$dest_cascade/index.html"; then
    loud_fail "abstract from about.md must render markdown (h2 missing)"
fi
if ! grep -q '<li>bullet one</li>' "$dest_cascade/index.html"; then
    loud_fail "abstract from about.md must render markdown (li missing)"
fi

# (4) Empty case — fresh repo with NO metadata anywhere → project-header
# omitted, browser title + minimal visible header identify the source repo.
empty_scratch=$(make_scratch)
cd "$empty_scratch"
if ! wg init --route local >empty-init.log 2>&1; then
    loud_fail "wg init for empty case failed: $(tail -5 empty-init.log)"
fi
if ! wg add "Empty task" --id empty-sample >empty-add.log 2>&1; then
    loud_fail "wg add for empty case failed: $(tail -5 empty-add.log)"
fi
empty_dest="$empty_scratch/empty-dest"
mkdir -p "$empty_dest"
if ! wg html publish add empty --rsync "$empty_dest/" >empty-pub.log 2>&1; then
    loud_fail "publish add for empty case failed: $(tail -10 empty-pub.log)"
fi
if ! wg html publish run empty >empty-run.log 2>&1; then
    loud_fail "publish run for empty case failed: $(tail -20 empty-run.log)"
fi
if grep -q 'class="project-header"' "$empty_dest/index.html"; then
    loud_fail "empty-meta case must OMIT the project-header block; instead got it in $empty_dest/index.html"
fi
# Sanity: minimal page-header is still present, but no longer says only
# "workgraph"; it uses the host plus repository working directory.
if ! grep -q 'class="page-header"' "$empty_dest/index.html"; then
    loud_fail "empty-meta case must STILL render the minimal page-header"
fi
host="$(hostname 2>/dev/null || true)"
if [[ -z "$host" ]]; then
    host="unknown-host"
fi
expected_source="${host}:${empty_scratch}"
if ! grep -Fq "<title>${expected_source} — all tasks</title>" "$empty_dest/index.html"; then
    loud_fail "empty-meta browser title should be '${expected_source} — all tasks'; got: $(grep '<title>' "$empty_dest/index.html" | head -1)"
fi
if ! grep -Fq "<h1>${expected_source}</h1>" "$empty_dest/index.html"; then
    loud_fail "empty-meta visible h1 should be '${expected_source}'; got: $(grep '<h1>' "$empty_dest/index.html" | head -1)"
fi
if grep -q '<title>workgraph' "$empty_dest/index.html"; then
    loud_fail "empty-meta browser title must not fall back to generic workgraph"
fi

echo "PASS: --title/--byline/--abstract advertised + per-deployment override + project-config cascade + about.md markdown abstract + empty-meta source title/header"
exit 0
