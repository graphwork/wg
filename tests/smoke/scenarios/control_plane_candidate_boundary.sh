#!/usr/bin/env bash
# Candidate-binary regression for the live .wg/source trust boundary.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v cargo >/dev/null 2>&1 || loud_skip "MISSING CARGO" "candidate binary build requires cargo"
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
scratch="$(make_scratch)"
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then
  WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
else
  export CARGO_TARGET_DIR="$scratch/candidate-target"
  (cd "$REPO_ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg) \
    || loud_fail "candidate wg build failed"
  WG_BIN="$CARGO_TARGET_DIR/debug/wg"
fi
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"

new_project() {
  local name=$1 project="$scratch/$1" home="$scratch/$1-home"
  mkdir -p "$project" "$home/.config"
  (
    cd "$project"
    git init -q -b main
    git config user.email boundary@test.invalid
    git config user.name Boundary
    printf 'base-%s\n' "$name" > source.txt
    git add source.txt
    git commit -qm base
    env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH \
      HOME="$home" XDG_CONFIG_HOME="$home/.config" "$WG_BIN" init --no-agency >/dev/null
  )
  mkdir -p "$project/.wg/chat/session-a"
  printf '# config-%s\n' "$name" >"$project/.wg/config.toml"
  printf '{"session-a":{"title":"%s"}}\n' "$name" >"$project/.wg/chat/sessions.json"
  printf 'chat-%s\n' "$name" >"$project/.wg/chat/session-a/conversation.jsonl"
  printf '%s|%s' "$project" "$home"
}

wgrun() {
  local project=$1 home=$2; shift 2
  (cd "$project" && env -u WG_AGENT_ID -u WG_TASK_ID \
    WG_DIR="$project/.wg" HOME="$home" XDG_CONFIG_HOME="$home/.config" "$WG_BIN" "$@")
}

digest_control() {
  local project=$1
  sha256sum \
    "$project/.wg/graph.jsonl" \
    "$project/.wg/config.toml" \
    "$project/.wg/chat/sessions.json" \
    "$project/.wg/chat/session-a/conversation.jsonl" \
    | sha256sum | awk '{print $1}'
}

new_attempt() {
  local project=$1 home=$2 id=$3
  local wt="$scratch/$id-wt" branch="wg/boundary/$id"
  wgrun "$project" "$home" add "$id" --id "$id" -d $'Control-plane boundary fixture.\n\n## Validation\n- exact candidate source' >/dev/null
  wgrun "$project" "$home" claim "$id" >/dev/null
  git -C "$project" worktree add -q -b "$branch" "$wt"
  wgrun "$project" "$home" pi-watchdog fixture-init "$id" --worktree "$wt" --now 0 >/dev/null
  printf '%s|%s' "$wt" "$branch"
}

# Graph A: reproduce the incident exactly. A worker force-adds the runtime .wg
# link and commits it. Sealing refuses before any candidate/ref/root projection,
# and exact graph/config/chat bytes remain unchanged.
IFS='|' read -r project_a home_a <<<"$(new_project graph-a)"
IFS='|' read -r wt_a branch_a <<<"$(new_attempt "$project_a" "$home_a" malicious-link)"
control_a_before=$(digest_control "$project_a")
main_a_before=$(git -C "$project_a" rev-parse main)
ln -s "$project_a/.wg" "$wt_a/.wg"
git -C "$wt_a" add -f .wg
git -C "$wt_a" commit -qm 'accidentally commit runtime control link'
if wgrun "$project_a" "$home_a" finalize checkpoint malicious-link \
    --worktree "$wt_a" --quiescence-receipt receipt:malicious >"$scratch/malicious.out" 2>&1; then
  loud_fail "candidate sealing accepted committed .wg symlink"
fi
grep -Eq 'control-plane\.(tracked_tree_refused|candidate_change_refused)' "$scratch/malicious.out" \
  || loud_fail "refusal lacked control-plane diagnostic: $(cat "$scratch/malicious.out")"
[[ $(git -C "$project_a" rev-parse main) == "$main_a_before" ]] \
  || loud_fail "malicious candidate moved main"
[[ $(digest_control "$project_a") == "$control_a_before" ]] \
  || loud_fail "malicious candidate changed graph/config/chat bytes"
[[ -d "$project_a/.wg" && ! -L "$project_a/.wg" ]] \
  || loud_fail "live control identity/type changed"

# Put the bad commit on main by ref-only CAS to model an already-affected base
# history, then exercise the operator recovery. It creates a clean descendant
# from the index and never reads/removes/renames live .wg bytes.
bad_commit=$(git -C "$wt_a" rev-parse HEAD)
git -C "$project_a" update-ref refs/heads/main "$bad_commit" "$main_a_before"
wgrun "$project_a" "$home_a" candidate recover-control-plane --yes >"$scratch/recovery.out"
grep -q 'Recovered tracked control plane without touching live bytes' "$scratch/recovery.out" \
  || loud_fail "recovery command lacked receipt: $(cat "$scratch/recovery.out")"
if git -C "$project_a" ls-tree -r --name-only HEAD | grep -Eiq '(^|/)\.wg(/|$)'; then
  loud_fail "recovery left protected entry tracked"
fi
[[ $(digest_control "$project_a") == "$control_a_before" ]] \
  || loud_fail "recovery command touched live graph/config/chat bytes"

# Graph B: ordinary source still lands through the exact-path projector. The
# worker has no trackable .wg helper, yet WG commands work from it through the
# inherited absolute WG_DIR (the real out-of-band launch contract).
IFS='|' read -r project_b home_b <<<"$(new_project graph-b)"
IFS='|' read -r wt_b branch_b <<<"$(new_attempt "$project_b" "$home_b" ordinary-source)"
[[ ! -e "$wt_b/.wg" && ! -L "$wt_b/.wg" ]] || loud_fail "new worktree exposed trackable .wg helper"
(cd "$wt_b" && env -u WG_AGENT_ID -u WG_TASK_ID WG_DIR="$project_b/.wg" \
  HOME="$home_b" XDG_CONFIG_HOME="$home_b/.config" "$WG_BIN" show ordinary-source >/dev/null) \
  || loud_fail "worker WG access failed through out-of-band WG_DIR"
control_b_before=$(digest_control "$project_b")
printf 'landed ordinary source\n' >"$wt_b/source.txt"
wgrun "$project_b" "$home_b" finalize checkpoint ordinary-source \
  --worktree "$wt_b" --quiescence-receipt receipt:ordinary >/dev/null
wgrun "$project_b" "$home_b" finalize reconcile ordinary-source >/dev/null
[[ $(cat "$project_b/source.txt") == 'landed ordinary source' ]] \
  || loud_fail "ordinary source did not land"
[[ $(digest_control "$project_b") == "$control_b_before" ]] \
  || loud_fail "ordinary promotion changed graph/config/chat bytes"
[[ -d "$project_b/.git/wg-control-plane/snapshots" ]] \
  || loud_fail "durable external control snapshot missing"
find "$project_b/.git/wg-control-plane/snapshots" -name receipt.json -type f | grep -q . \
  || loud_fail "external snapshot receipt missing"

# Diagnostic path: a vanished graph parent stays vanished and retains ENOENT +
# exact failing path instead of being recreated as an empty control plane.
mv "$project_b/.wg" "$project_b/.wg-preserved"
if env -u WG_AGENT_ID -u WG_TASK_ID WG_DIR="$project_b/.wg" HOME="$home_b" \
    XDG_CONFIG_HOME="$home_b/.config" "$WG_BIN" log ordinary-source probe \
    >"$scratch/enoent.out" 2>&1; then
  loud_fail "graph mutation unexpectedly succeeded with vanished control path"
fi
grep -q 'ENOENT' "$scratch/enoent.out" || loud_fail "ENOENT cause stripped: $(cat "$scratch/enoent.out")"
grep -Eq 'path=.*\.wg/graph\.(lock|jsonl)' "$scratch/enoent.out" || loud_fail "failing path stripped: $(cat "$scratch/enoent.out")"
[[ ! -e "$project_b/.wg" ]] || loud_fail "failed graph mutation recreated hollow .wg"

echo "PASS: committed .wg link refused with exact control digests stable; ordinary exact-path land and out-of-band worker access succeeded; durable receipt + ENOENT path retained"
