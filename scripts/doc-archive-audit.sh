#!/usr/bin/env bash
# doc-archive-audit.sh — the "grep-before-archive" gate for WG documentation.
#
# The policy this enforces lives in `docs/archive/POLICY.md`. A documentation
# file may only be moved into `docs/archive/` after this script reports CLEAR
# for it. The script exists so the rule is *mechanical* instead of a habit:
# it answers one question — "does anything still point at this file, and is
# that something code or prose?"
#
# Verdicts (per candidate):
#   BLOCK  something load-bearing still names the file: a Rust/TS/JS/Python
#          source file, a shell script (scripts/, tests/smoke/, .github/),
#          CI, the Makefile, or Cargo/JSON/TOML config. Do NOT archive —
#          repoint the reference first (a code comment that cites a design doc
#          is a real citation, not noise).
#   WARN   only prose names it (another markdown file). Archiving is allowed
#          if you repoint those markdown links in the SAME commit.
#   CLEAR  nothing names it. Archive freely.
#
# Usage:
#   scripts/doc-archive-audit.sh PATH...            # audit specific files/dirs
#   scripts/doc-archive-audit.sh --root-reports     # every root-level *.md except the keep set
#   scripts/doc-archive-audit.sh --orphans DIR...   # every tracked *.md under DIR with no inbound refs
#   scripts/doc-archive-audit.sh --batch PATH...    # treat PATHs as one move: refs *between*
#                                                   # candidates are ignored (they travel together).
#                                                   # Composes with --orphans / --root-reports.
#   scripts/doc-archive-audit.sh --quiet PATH...    # only print non-CLEAR candidates
#
# Exit status: 0 when no candidate is BLOCK (and, with --orphans, when nothing
# at all was found), 1 when at least one candidate is BLOCK.
#
# Reference matching is boundary-aware so `context.md` does not match
# `workspace-context.md` and `design-claim-lifecycle.md` does not match
# `claim-lifecycle.md`.

set -uo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || {
  echo "doc-archive-audit: not inside a git work tree" >&2
  exit 2
}
cd "$REPO_ROOT" || exit 2

# The root-level keep set: files that are a documented surface and are therefore
# never reported as archive candidates (see docs/archive/POLICY.md §3).
ROOT_KEEP="README.md AGENTS.md CLAUDE.md"

MODE=paths
BATCH=0
QUIET=0
export ROOT_KEEP

usage() {
  sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'
  exit 2
}

CANDIDATES=()
for arg in "$@"; do
  case "$arg" in
  --root-reports) MODE=root-reports ;;
  --orphans) MODE=orphans ;;
  --batch) BATCH=1 ;;
  --quiet) QUIET=1 ;;
  -h | --help) usage ;;
  -*) echo "doc-archive-audit: unknown option $arg" >&2 && exit 2 ;;
  *) CANDIDATES+=("$arg") ;;
  esac
done

# --- candidate selection -----------------------------------------------------

collect_under() {
  local path="$1"
  if [[ -d "$path" ]]; then
    git ls-files -- "$path" | grep -E '\.md$' || true
  elif [[ -f "$path" ]]; then
    echo "$path"
  else
    echo "doc-archive-audit: no such file or directory: $path" >&2
    return 1
  fi
}

case "$MODE" in
root-reports)
  while IFS= read -r f; do
    skip=0
    for k in $ROOT_KEEP; do [[ "$f" == "$k" ]] && skip=1; done
    [[ "$f" == */* ]] && skip=1
    case "$f" in *.md) ;; *) skip=1 ;; esac
    [[ $skip -eq 0 ]] && CANDIDATES+=("$f")
  done < <(git ls-files | grep -E '^[^/]+\.md$')
  ;;
orphans)
  [[ ${#CANDIDATES[@]} -eq 0 ]] && CANDIDATES=("docs")
  ;;
esac

[[ ${#CANDIDATES[@]} -eq 0 ]] && {
  echo "doc-archive-audit: no candidates" >&2
  exit 0
}

# --- reference search --------------------------------------------------------

# escape a literal for ERE
ere_escape() { printf '%s' "$1" | sed -e 's/[][\.^$*+?(){}|\/]/\\&/g'; }

# classify a referencing path: code/ci (BLOCK) or doc (WARN)
ref_kind() {
  local p="$1"
  case "$p" in
  *.rs | *.ts | *.tsx | *.js | *.mjs | *.py | *.toml | *.json | *.lock | *.yml | *.yaml | *.cmake | Makefile)
    echo code
    ;;
  scripts/* | tests/* | .github/* | .claude/*)
    echo code
    ;;
  *) echo doc ;;
  esac
}

# Build the whole reference index in ONE `git grep` pass. Needed because a
# directory wave audits hundreds of candidates and per-candidate grep is
# quadratic in repo size (measured: 2m23s for 96 files vs <1s for the index).
#
# Emits `referrer<TAB>basename` pairs. Boundary-aware: `context.md` does not
# match `workspace-context.md`. If two candidates share a basename the pair is
# attributed to whichever candidate asks for it, which can only *over*-report
# (a candidate looks more referenced than it is) — the safe direction for a
# gate that decides whether to move a file.
INDEX_FILE=""
build_index() {
  local union="" base
  local -A seen=()
  for base in "${BASENAMES[@]}"; do
    [[ -n "${seen[$base]:-}" ]] && continue
    seen[$base]=1
    union+="${union:+|}${base}"
  done

  INDEX_FILE="$(mktemp)"
  BASENAME_SET="$(mktemp)"
  printf '%s\n' "${!seen[@]}" >"$BASENAME_SET"

  git grep -I -o -E -- "(^|[^[:alnum:]_.-])($union)($|[^[:alnum:]_.-])" -- . 2>/dev/null |
    awk -v setfile="$BASENAME_SET" '
      BEGIN { while ((getline l < setfile) > 0) if (l != "") names[l] = 1 }
      {
        i = index($0, ":")
        if (i == 0) next
        p = substr($0, 1, i - 1)
        m = substr($0, i + 1)
        gsub(/^[^A-Za-z0-9_.-]+/, "", m)   # drop the leading boundary char
        gsub(/[^A-Za-z0-9_.-]+$/, "", m)   # drop a trailing boundary char
        if (m in names) { print p "\t" m; next }
        sub(/\.+$/, "", m)                 # ".../x.md." at end of a sentence
        if (m in names) print p "\t" m
      }' >"$INDEX_FILE"
}

# All refs to $1 across tracked files, excluding $1 itself and (in --batch mode)
# the other candidates.
refs_for() {
  local cand="$1" base other
  base="$(basename "$cand")"

  if [[ -n "$INDEX_FILE" ]]; then
    local hits
    hits="$(awk -F'\t' -v b="$base" -v self="$cand" \
      '$2 == b && $1 != self { print $1 }' "$INDEX_FILE" | LC_ALL=C sort -u)"
    [[ -z "$hits" ]] && return
    if [[ $BATCH -eq 1 ]]; then
      # candidates travel together, so refs between them are not dangling
      printf '%s\n' "${CANDIDATES[@]}" >"$BATCH_FILE"
      printf '%s\n' "$hits" | grep -v -F -x -f "$BATCH_FILE" || true
      return
    fi
    printf '%s\n' "$hits"
    return
  fi

  local pattern
  pattern="(^|[^[:alnum:]_.-])$(ere_escape "$base")(\$|[^[:alnum:]_.-])"
  local excludes=(":(exclude)${cand}")
  if [[ $BATCH -eq 1 ]]; then
    for other in "${CANDIDATES[@]}"; do
      [[ "$other" == "$cand" ]] || excludes+=(":(exclude)${other}")
    done
  fi
  git grep -l -I -E -- "$pattern" -- . "${excludes[@]}" 2>/dev/null || true
}

# --- audit -------------------------------------------------------------------

blocked=0
warned=()
blocked_list=()
clear_list=()

# Expand candidates against the working tree first so the single-pass index can
# be built from their basenames.
EXPANDED=()
for cand in "${CANDIDATES[@]}"; do
  if [[ "$MODE" == orphans ]]; then
    while IFS= read -r f; do [[ -n "$f" ]] && EXPANDED+=("$f"); done < <(collect_under "$cand")
  else
    EXPANDED+=("$cand")
  fi
done
[[ ${#EXPANDED[@]} -eq 0 ]] && {
  echo "doc-archive-audit: no candidates" >&2
  exit 0
}
CANDIDATES=("${EXPANDED[@]}")

BASENAMES=()
for cand in "${CANDIDATES[@]}"; do BASENAMES+=("$(basename "$cand")"); done
if [[ ${#CANDIDATES[@]} -gt 8 ]]; then
  BATCH_FILE="$(mktemp)"
  build_index
fi
trap 'rm -f "${INDEX_FILE:-}" "${BASENAME_SET:-}" "${BATCH_FILE:-}"' EXIT

for cand in "${CANDIDATES[@]}"; do
  for f in "$cand"; do
    refs="$(refs_for "$f")"

    code_refs=""
    doc_refs=""
    while IFS= read -r r; do
      [[ -n "$r" ]] || continue
      if [[ "$(ref_kind "$r")" == code ]]; then
        code_refs+="${r} "
      else
        doc_refs+="${r} "
      fi
    done <<<"$refs"

    if [[ -n "$code_refs" ]]; then
      verdict=BLOCK
      blocked=$((blocked + 1))
      blocked_list+=("$f|${code_refs% }")
    elif [[ -n "$doc_refs" ]]; then
      verdict=WARN
      warned+=("$f|${doc_refs% }")
    else
      verdict=CLEAR
      clear_list+=("$f")
    fi

    if [[ $QUIET -eq 0 || "$verdict" != CLEAR ]]; then
      case "$verdict" in
      BLOCK) printf 'BLOCK  %-70s %s\n' "$f" "${code_refs% }" ;;
      WARN) printf 'WARN   %-70s %s\n' "$f" "${doc_refs% }" ;;
      CLEAR) printf 'CLEAR  %s\n' "$f" ;;
      esac
    fi
  done
done

echo
printf 'doc-archive-audit: %d candidate(s) — %d BLOCK, %d WARN, %d CLEAR\n' \
  "$((${#blocked_list[@]} + ${#warned[@]} + ${#clear_list[@]}))" \
  "${#blocked_list[@]}" "${#warned[@]}" "${#clear_list[@]}"

if [[ $QUIET -eq 1 && ${#clear_list[@]} -gt 0 ]]; then
  echo "CLEAR (archivable as-is):"
  printf '  %s\n' "${clear_list[@]}"
fi

[[ $blocked -eq 0 ]] || exit 1
exit 0
