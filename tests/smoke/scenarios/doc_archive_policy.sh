#!/usr/bin/env bash
# Pins the documentation archive policy (docs/archive/POLICY.md) and its two
# mechanical gates, so the policy cannot rot back into a habit:
#
#   1. scripts/gen-key-docs.py --check   — docs/KEY_DOCS.md is not stale.
#   2. scripts/doc-archive-audit.sh      — a code-cited doc is BLOCKed (exit 1),
#                                          an unreferenced doc is CLEAR (exit 0).
#   3. the repo root stays a three-file markdown surface (POLICY.md §4).
#   4. every doc moved into docs/archive/ is still tracked (git mv, never rm).
#
# Credential-free, daemon-free, network-free.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"

REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
cd "$REPO_ROOT"

command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "gen-key-docs.py needs python3"

fail() { loud_fail "$*"; }

# ── 0. the policy and its gates exist ───────────────────────────────
for f in docs/archive/POLICY.md scripts/doc-archive-audit.sh scripts/gen-key-docs.py docs/key-docs.tsv docs/KEY_DOCS.md; do
  [[ -f "$f" ]] || fail "policy artefact missing: $f"
done
[[ -x scripts/doc-archive-audit.sh ]] || fail "scripts/doc-archive-audit.sh is not executable"
[[ -x scripts/gen-key-docs.py ]] || fail "scripts/gen-key-docs.py is not executable"

# ── 1. generated index is current (drift gate) ──────────────────────
if ! ./scripts/gen-key-docs.py --check >/tmp/doc_archive_policy.check.$$ 2>&1; then
  cat /tmp/doc_archive_policy.check.$$
  rm -f /tmp/doc_archive_policy.check.$$
  fail "docs/KEY_DOCS.md is stale — run scripts/gen-key-docs.py"
fi
rm -f /tmp/doc_archive_policy.check.$$

# ── 2a. gate BLOCKs a doc that source cites ─────────────────────────
block_rc=0
block_out="$(./scripts/doc-archive-audit.sh docs/ops/runbook.md 2>&1)" || block_rc=$?
[[ $block_rc -eq 1 ]] || fail "audit gate did not BLOCK docs/ops/runbook.md (exit $block_rc): $block_out"
case "$block_out" in
*BLOCK*) : ;;
*) fail "audit gate exit 1 but no BLOCK verdict: $block_out" ;;
esac

# ── 2b. gate CLEARs an unreferenced doc (and does not cry wolf) ─────
canary="docs/archive/.smoke-canary-$$.md"
add_cleanup_hook "rm -f '$REPO_ROOT/$canary'"
printf '# canary\n\nNo other file names %s.\n' "$(basename "$canary")" >"$canary"
clear_rc=0
clear_out="$(./scripts/doc-archive-audit.sh "$canary" 2>&1)" || clear_rc=$?
rm -f "$canary"
[[ $clear_rc -eq 0 ]] || fail "audit gate BLOCKed an unreferenced file (exit $clear_rc): $clear_out"
case "$clear_out" in
*CLEAR*) : ;;
*) fail "audit gate exit 0 but no CLEAR verdict: $clear_out" ;;
esac

# ── 3. the root is a small surface, not a report dump ───────────────
root_md=$(ls -1 ./*.md 2>/dev/null | wc -l | tr -d ' ')
((root_md <= 3)) || fail "repo root has $root_md markdown files; POLICY.md §4 caps it at 3 (write reports under docs/)"
for required in README.md AGENTS.md CLAUDE.md; do
  [[ -f "$required" ]] || fail "root surface lost $required"
done

# ── 4. archived docs are moved, not deleted ─────────────────────────
untracked=$(git ls-files --others --exclude-standard -- docs/archive | head -5)
[[ -z "$untracked" ]] || fail "untracked file(s) under docs/archive — archive moves must be 'git mv': $untracked"

# every archive bucket named in POLICY.md §7 that exists is non-empty
for bucket in root-reports research reports audit; do
  d="docs/archive/$bucket"
  [[ -d "$d" ]] || continue
  n=$(git ls-files -- "$d" | wc -l | tr -d ' ')
  ((n > 0)) || fail "docs/archive/$bucket exists but tracks no files"
done

echo "PASS: archive policy gates hold (KEY_DOCS current, BLOCK/CLEAR verdicts, root=$root_md md, archive tracked)"
exit 0
