#!/usr/bin/env bash
# Live regression for bound-and-de: a retained worktree lives on a simulated
# hung filesystem (its cleanup marker is a FIFO whose read blocks). The real
# daemon must still dispatch a ready shell task because every retained-path
# probe runs on the single-flight maintenance worker, never coordinator_tick.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
# This scenario intentionally drives the human service-control flow with its
# isolated candidate binary, not the surrounding worker's control context.
unset WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER

command -v cargo >/dev/null 2>&1 || loud_skip "MISSING CARGO" "candidate binary build requires cargo"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "cleanup status assertions require python3"
command -v mkfifo >/dev/null 2>&1 || loud_skip "MISSING MKFIFO" "blocked filesystem probe uses a FIFO"

REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null)" \
  || loud_fail "cannot locate repository root"
(cd "$REPO_ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg) \
  || loud_fail "candidate wg build failed"
WG_BIN="$REPO_ROOT/target/debug/wg"
export PATH="$(dirname "$WG_BIN"):$PATH"

# Keep the Unix-domain socket under sun_path's small platform limit.
export WG_SMOKE_SCENARIO=slow-wt-sweep
scratch=$(make_scratch)
export HOME="$scratch/home"
export WG_GLOBAL_DIR="$scratch/global"
export TMPDIR="$scratch/tmp"
mkdir -p "$HOME" "$WG_GLOBAL_DIR" "$TMPDIR"
project="$scratch/project"
mkdir -p "$project"
cd "$project"
git init -q -b main
git config user.email slow-sweep@test.invalid
git config user.name "Slow Sweep Smoke"
printf 'main\n' >main.txt
git add main.txt
git commit -qm initial

"$WG_BIN" init -m claude:opus --no-agency >init.log 2>&1 \
  || loud_fail "wg init failed: $(tail -30 init.log)"
wg_dir="$project/.wg"

# A real Git worktree makes the pre-fix synchronous sweep reach the source
# status/marker validation gate. Reading this FIFO models an indefinitely slow
# network-filesystem marker read without sleeps or timing luck.
retained="$project/.wg-worktrees/agent-900"
mkdir -p "$project/.wg-worktrees"
git worktree add -q -b wg/agent-900/retained "$retained" HEAD
marker="$retained/.wg-cleanup-pending"
mkfifo "$marker"

release_blocked_sweep() {
  # Rendezvous with the blocked reader. Timeout protects cleanup if the worker
  # never reached the FIFO for an unrelated setup failure.
  timeout 2 sh -c 'printf x > "$1"' sh "$marker" >/dev/null 2>&1 || true
}
add_cleanup_hook release_blocked_sweep

"$WG_BIN" --dir "$wg_dir" add "dispatch while retained sweep is blocked" \
  --id slow-sweep-dispatch --exec 'true' --exec-mode shell >add.log 2>&1 \
  || loud_fail "failed to add shell probe: $(tail -30 add.log)"
"$WG_BIN" --dir "$wg_dir" publish slow-sweep-dispatch >publish.log 2>&1 \
  || loud_fail "failed to publish shell probe: $(tail -30 publish.log)"

start_wg_daemon "$project" --no-coordinator-agent --max-agents 2 --interval 1
wg_dir="$WG_SMOKE_DAEMON_DIR"
daemon_log="$wg_dir/service/daemon.log"
status_file="$wg_dir/service/worktree-cleanup.json"

# Prove the maintenance worker is actually inside the blocked sweep.
for _ in $(seq 1 100); do
  if [[ -f "$status_file" ]] && python3 - "$status_file" <<'PY' >/dev/null 2>&1
import json, sys
with open(sys.argv[1]) as f:
    assert json.load(f)["phase"] == "running"
PY
  then
    break
  fi
  sleep 0.05
done
[[ -f "$status_file" ]] || loud_fail "cleanup lane never published running diagnostics. daemon log: $(tail -80 "$daemon_log" 2>/dev/null); service files: $(find "$wg_dir/service" -maxdepth 2 -type f -print 2>/dev/null)"
phase=$(python3 - "$status_file" <<'PY'
import json, sys
with open(sys.argv[1]) as f: print(json.load(f)["phase"])
PY
)
[[ "$phase" == "running" ]] \
  || loud_fail "cleanup lane was not blocked as injected (phase=$phase): $(cat "$status_file")"
status_json=$("$WG_BIN" --dir "$wg_dir" service status --json)
printf '%s' "$status_json" | python3 -c 'import json,sys; s=json.load(sys.stdin); assert s["retained_worktree_cleanup"]["phase"] == "running", s' \
  || loud_fail "service status did not surface the running cleanup lane: $status_json"

# Dispatch must proceed while the worker remains blocked. On the old code the
# first coordinator tick blocks in the retained sweep and this line never
# appears until the FIFO is released by teardown.
spawned=0
for _ in $(seq 1 100); do
  if grep -q "Spawned shell .*slow-sweep-dispatch\|Spawning shell task inline for: slow-sweep-dispatch" "$daemon_log" 2>/dev/null; then
    spawned=1
    break
  fi
  sleep 0.05
done
[[ "$spawned" == 1 ]] \
  || loud_fail "ready task was starved by blocked retained cleanup. log tail: $(tail -80 "$daemon_log" 2>/dev/null)"
[[ -p "$marker" && -d "$retained" ]] \
  || loud_fail "hung/unknown retained source was not preserved fail-closed"

# Timer and watcher wakes accumulated while the one worker was blocked. Release
# it once, then observe a single bounded completion carrying the coalesced count.
release_blocked_sweep
for _ in $(seq 1 100); do
  if python3 - "$status_file" <<'PY' >/dev/null 2>&1
import json, sys
with open(sys.argv[1]) as f: s=json.load(f)
assert s["batches_completed"] >= 1 and s["coalesced"] >= 1
PY
  then
    break
  fi
  sleep 0.05
done
python3 - "$status_file" <<'PY' || loud_fail "cleanup diagnostics did not surface bounded/coalesced completion: $(cat "$status_file")"
import json, sys
with open(sys.argv[1]) as f: s=json.load(f)
assert s["batches_completed"] >= 1, s
assert s["coalesced"] >= 1, s
assert s["last_duration_ms"] >= 0, s
PY

summary_lines=$(grep -c "Retained-worktree sweep .*duration=" "$daemon_log" 2>/dev/null || true)
skip_lines=$(grep -c "\[worktree-sweep\] Skipping" "$daemon_log" 2>/dev/null || true)
(( summary_lines <= 2 )) || loud_fail "cleanup emitted unbounded summary spam ($summary_lines lines)"
(( skip_lines == 0 )) || loud_fail "legacy per-worktree skip spam returned ($skip_lines lines)"

echo "PASS: blocked retained-filesystem sweep stayed single-flight/off tick; ready shell task dispatched; wakes coalesced; source preserved"
