#!/usr/bin/env bash
# Live regression: required worktree isolation must reallocate around an unknown
# stale agent path and both the WG wrapper and its real shell handler must run
# from the verified isolated checkout, never the shared repository.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/_helpers.sh"

[[ "$(uname -s)" == "Linux" ]] || loud_skip "LINUX /proc REQUIRED" "cwd identity assertions use /proc/<pid>/cwd"
command -v cargo >/dev/null 2>&1 || loud_skip "MISSING CARGO" "candidate binary build requires cargo"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "metadata assertion requires python3"

REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null)" \
  || loud_fail "cannot locate repository root from $HERE"
(cd "$REPO_ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg) \
  || loud_fail "candidate wg build failed"
WG_BIN="$REPO_ROOT/target/debug/wg"
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"
# The spawned wrapper shells out to `wg`; force that to the same unmerged
# candidate without globally installing it.
export PATH="$(dirname "$WG_BIN"):$PATH"

scratch=$(make_scratch)
export HOME="$scratch/home"
export WG_GLOBAL_DIR="$scratch/global"
export TMPDIR="$scratch/tmp"
mkdir -p "$HOME" "$WG_GLOBAL_DIR" "$TMPDIR"
project="$scratch/project"
mkdir -p "$project"
cd "$project"
git init -q
git config user.email isolation-smoke@test.invalid
git config user.name "Isolation Smoke"
echo "shared checkout sentinel" > source.txt
git add source.txt
git commit -qm initial

"$WG_BIN" init -m claude:opus >init.log 2>&1 \
  || loud_fail "wg init failed: $(tail -30 init.log)"
wg_dir="$project/.wg"

# Reproduce the production collision: next_agent_id is 1, but its target path
# already contains unknown dirty source with no Git worktree metadata.
stale="$project/.wg-worktrees/agent-1"
mkdir -p "$stale"
echo "unknown dirty source — preserve byte-for-byte" > "$stale/valuable.txt"

handler_cmd="bash -c 'echo \"\$\$\" > .wg/isolation-handler.pid; pwd -P > .wg/isolation-handler.cwd; sleep 60'"
"$WG_BIN" --dir "$wg_dir" add "Isolation collision probe" --id isolation-probe \
  --exec "$handler_cmd" >add.log 2>&1 \
  || loud_fail "wg add failed: $(tail -30 add.log)"

wrapper_pid=""
cleanup_spawn() {
  if [[ -n "${wrapper_pid:-}" ]]; then
    kill -KILL -- "-$wrapper_pid" 2>/dev/null || true
    kill -KILL "$wrapper_pid" 2>/dev/null || true
    wait "$wrapper_pid" 2>/dev/null || true
  fi
}
add_cleanup_hook cleanup_spawn

spawn_out=$("$WG_BIN" --dir "$wg_dir" spawn isolation-probe --executor shell --timeout 2m 2>&1) \
  || loud_fail "isolated spawn failed instead of reallocating: $spawn_out"
printf '%s\n' "$spawn_out" >spawn.log
agent_id=$(printf '%s\n' "$spawn_out" | grep -oE 'Spawned agent-[0-9]+' | head -1 | awk '{print $2}')
wrapper_pid=$(printf '%s\n' "$spawn_out" | sed -n 's/^[[:space:]]*PID: \([0-9][0-9]*\)$/\1/p' | head -1)
[[ "$agent_id" == "agent-2" ]] \
  || loud_fail "stale agent-1 path did not trigger collision-free reallocation: $spawn_out"
[[ "$wrapper_pid" =~ ^[0-9]+$ ]] \
  || loud_fail "could not parse wrapper PID from spawn output: $spawn_out"

[[ "$(cat "$stale/valuable.txt")" == "unknown dirty source — preserve byte-for-byte" ]] \
  || loud_fail "unknown stale directory was modified or deleted"
[[ ! -e "$stale/.git" ]] \
  || loud_fail "WG wrote Git metadata into the unknown stale directory"

for _ in $(seq 1 200); do
  [[ -s "$wg_dir/isolation-handler.pid" && -s "$wg_dir/isolation-handler.cwd" ]] && break
  sleep 0.025
done
[[ -s "$wg_dir/isolation-handler.pid" ]] \
  || loud_fail "real handler never started: $(tail -80 "$wg_dir/agents/$agent_id/output.log" 2>/dev/null || true)"
handler_pid=$(cat "$wg_dir/isolation-handler.pid")
[[ "$handler_pid" =~ ^[0-9]+$ && -e "/proc/$handler_pid/cwd" ]] \
  || loud_fail "handler PID is not live: $handler_pid"

isolated_real=$(realpath "$project/.wg-worktrees/$agent_id")
shared_real=$(realpath "$project")
wrapper_cwd=$(readlink -f "/proc/$wrapper_pid/cwd")
handler_cwd=$(readlink -f "/proc/$handler_pid/cwd")
[[ "$wrapper_cwd" == "$isolated_real" ]] \
  || loud_fail "wrapper escaped isolation: cwd=$wrapper_cwd expected=$isolated_real shared=$shared_real"
[[ "$handler_cwd" == "$isolated_real" ]] \
  || loud_fail "handler escaped isolation: cwd=$handler_cwd expected=$isolated_real shared=$shared_real"
[[ "$wrapper_cwd" != "$shared_real" && "$handler_cwd" != "$shared_real" ]] \
  || loud_fail "wrapper or handler silently fell back to shared checkout"
[[ "$(cat "$wg_dir/isolation-handler.cwd")" == "$isolated_real" ]] \
  || loud_fail "handler's own pwd disagrees with /proc cwd"

if ! python3 - "$wg_dir/agents/$agent_id/metadata.json" "$isolated_real" <<'PY'
import json, os, sys
with open(sys.argv[1]) as f:
    data = json.load(f)
assert data["worktree_isolation_enabled"] is True, data
assert data["isolation_mode"] == "required-worktree", data
assert os.path.realpath(data["worktree_path"]) == sys.argv[2], data
assert os.path.realpath(data["effective_cwd"]) == sys.argv[2], data
PY
then
  loud_fail "spawn metadata did not record required isolation"
fi

# Git metadata and filesystem identity must agree before this is considered a
# passing isolated attempt.
branch=$(git -C "$isolated_real" symbolic-ref --quiet --short HEAD)
[[ "$branch" == "wg/$agent_id/isolation-probe" ]] \
  || loud_fail "branch ownership mismatch: $branch"
git -C "$project" worktree list --porcelain | grep -Fq "worktree $isolated_real" \
  || loud_fail "Git porcelain does not register isolated path $isolated_real"
git_admin=$(git -C "$isolated_real" rev-parse --absolute-git-dir)
owner_record="$git_admin/wg-spawn-owner.json"
if ! python3 - "$owner_record" "$agent_id" "$isolated_real" "$branch" <<'PY'
import json, os, sys
with open(sys.argv[1]) as f:
    owner = json.load(f)
assert owner["schema"] == 1, owner
assert owner["agent_id"] == sys.argv[2], owner
assert owner["task_id"] == "isolation-probe", owner
assert os.path.realpath(owner["path"]) == sys.argv[3], owner
assert owner["branch"] == sys.argv[4], owner
assert owner["token"], owner
assert owner["base_oid"], owner
PY
then
  loud_fail "private Git owner record does not match the live isolated attempt"
fi

cleanup_spawn
wrapper_pid=""
echo "PASS: required isolation reallocates stale ID and pins wrapper+handler cwd"
