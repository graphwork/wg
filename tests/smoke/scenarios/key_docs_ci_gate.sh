#!/usr/bin/env bash
# Scenario: key_docs_ci_gate
#
# pins the CI anti-drift gate for the generated documentation index
# (docs/KEY_DOCS.md <- docs/key-docs.tsv via scripts/gen-key-docs.py).
#
# WHY: the index is *generated* because the hand-maintained predecessor went
# stale on 2026-04-29 and stayed stale through an entire feature wave
# (federation / review / exec). docs/archive/POLICY.md §9 makes
# `scripts/gen-key-docs.py --check` the mechanical gate; this scenario pins that
# the gate is actually wired into CI and that it really is red-on-drift /
# green-on-clean, so the wiring cannot be deleted (or quietly turned into a
# no-op) without turning this smoke gate red.
#
# Asserts:
#   (a) .github/workflows/ci.yml declares the `docs-key-docs` job and its step
#       runs `scripts/gen-key-docs.py --check`;
#   (b) the EXACT run block extracted from the workflow is green on a current
#       tree (exit 0, "is current");
#   (c) the same block is red (exit 1) when docs/KEY_DOCS.md is tampered, and
#       its output carries the drift diff naming the tampered line;
#   (d) restoring the file makes it green again, and running the documented
#       remedy (`scripts/gen-key-docs.py`, same commit as the doc move) also
#       returns the tree to the committed bytes;
#   (e) the rest of CI is untouched: every pre-existing job id is still present.
#
# Credential-free, daemon-free, network-free. Needs git + python3 (loud SKIP).
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=tests/smoke/scenarios/_helpers.sh
. "$HERE/_helpers.sh"

REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
cd "$REPO_ROOT" || loud_fail "cannot cd to $REPO_ROOT"

fail() { loud_fail "$*"; }

command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "the KEY_DOCS gate renders from \`git ls-files\`"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "scripts/gen-key-docs.py is a python3 script"

CI=".github/workflows/ci.yml"
GEN="scripts/gen-key-docs.py"
TSV="docs/key-docs.tsv"
OUT="docs/KEY_DOCS.md"

# ── 0. the gate's artefacts exist ───────────────────────────────────
for f in "$CI" "$GEN" "$TSV" "$OUT"; do
  [[ -f "$f" ]] || fail "gate artefact missing: $f"
done
[[ -x "$GEN" ]] || fail "$GEN is not executable"

# ── 1. the workflow wires the gate (and keeps the other jobs) ───────
grep -q '^  docs-key-docs:' "$CI" \
  || fail "$CI has no \`docs-key-docs\` job — the KEY_DOCS anti-drift gate was removed"

# The gate must not cost another lane: every job that existed before it must
# still be declared. (Grow-only: adding jobs is fine, deleting one is not.)
for job in check build lifecycle-formal integration installer-windows worksgood-pi nightly; do
  grep -q "^  ${job}:" "$CI" || fail "$CI no longer declares the pre-existing \`$job\` job"
done

# Extract the exact `run:` block of the gate step so the assertion below is
# about the bytes CI would actually execute, not a paraphrase of them.
gate_block="$(
  awk '
    /^[[:space:]]*docs-key-docs:[[:space:]]*$/ { in_job = 1; next }
    in_job && /^[[:space:]]*-[[:space:]]*name:[[:space:]]*KEY_DOCS drift gate[[:space:]]*$/ {
      in_step = 1; next
    }
    in_step && /^[[:space:]]*run:[[:space:]]*\|[[:space:]]*$/ {
      match($0, /^ */); run_indent = RLENGTH; in_run = 1; next
    }
    in_run {
      if ($0 ~ /^[[:space:]]*$/) next
      match($0, /^ */)
      if (RLENGTH <= run_indent) exit
      print substr($0, run_indent + 1)
    }
  ' "$CI"
)"
[[ -n "$gate_block" ]] \
  || fail "could not extract the \`KEY_DOCS drift gate\` run block from $CI (did the step get renamed/reindented?)"
grep -q 'gen-key-docs\.py --check' <<<"$gate_block" \
  || fail "the docs-key-docs job no longer runs \`scripts/gen-key-docs.py --check\`; block:
$gate_block"

scratch="$(make_scratch)"
gate_sh="$scratch/gate.sh"
printf '%s\n' "$gate_block" >"$gate_sh"

# The extracted block is executed exactly as CI does: cwd = repo root.
run_gate() {
  bash "$gate_sh" >"$1" 2>&1
}

# ── 2. green on the current tree ────────────────────────────────────
run_gate "$scratch/green.log" \
  || { cat "$scratch/green.log"; fail "the CI gate is red on a current tree — run scripts/gen-key-docs.py"; }
grep -q 'is current' "$scratch/green.log" \
  || fail "the CI gate passed without reporting the index is current: $(cat "$scratch/green.log")"

# ── 3. red on a stale index, with the drift diff ────────────────────
cp "$OUT" "$scratch/KEY_DOCS.md.orig"
restore_out() { cp "$scratch/KEY_DOCS.md.orig" "$OUT"; }
# Backstop: the worktree must never be left holding the tampered bytes.
add_cleanup_hook restore_out

printf '\n<!-- tampered: key_docs_ci_gate regression probe -->\n' >>"$OUT"

red_rc=0
run_gate "$scratch/red.log" || red_rc=$?
if [[ $red_rc -eq 0 ]]; then
  restore_out
  fail "the CI gate ACCEPTED a tampered $OUT (expected exit 1) — the staleness check is a no-op"
fi
grep -q 'is STALE' "$scratch/red.log" || {
  restore_out
  fail "the gate failed without reporting STALE:
$(cat "$scratch/red.log")"
}
grep -q '+++ .*(regenerated)' "$scratch/red.log" || {
  restore_out
  fail "the gate failed without printing the drift diff (no '+++ … (regenerated)' hunk):
$(cat "$scratch/red.log")"
}
grep -q 'tampered: key_docs_ci_gate regression probe' "$scratch/red.log" || {
  restore_out
  fail "the drift diff does not name the tampered line:
$(cat "$scratch/red.log")"
}

# ── 4. restoring the committed bytes turns it green again ───────────
restore_out
run_gate "$scratch/green2.log" \
  || { cat "$scratch/green2.log"; fail "the gate is still red after restoring $OUT"; }
git diff --quiet -- "$OUT" || fail "restoring $OUT left the worktree dirty"

# ── 5. the documented remedy (regenerate) is what makes it green ────
# POLICY §9 / docs/DEV.md: stage the doc move, run the generator, commit both.
printf '\n<!-- stale again: remedy probe -->\n' >>"$OUT"
./"$GEN" >"$scratch/regen.log" 2>&1 \
  || { cat "$scratch/regen.log"; restore_out; fail "scripts/gen-key-docs.py failed to regenerate $OUT"; }
run_gate "$scratch/green3.log" \
  || { cat "$scratch/green3.log"; restore_out; fail "the gate is red after regenerating $OUT"; }
git diff --quiet -- "$OUT" \
  || { restore_out; fail "regenerating $OUT did not reproduce the committed bytes — the committed index is not reproducible from $TSV"; }

# Belt and braces: leave the tree exactly as we found it.
restore_out
git diff --quiet -- "$OUT" || fail "scenario left $OUT modified"

echo "PASS: docs-key-docs CI gate is wired, red on drift (with the diff), green when current"
exit 0
