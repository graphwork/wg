#!/usr/bin/env bash
# Scenario: shared_sccache_warm_rebuild
#
# Pins the A+E thinnest slice from .wg/survey/survey-cow-worktrees.md §5:
#   * a build-capable spawn is wired with a shared `sccache` compiler cache
#     (`RUSTC_WRAPPER` -> a WG-generated wrapper, SCCACHE_DIR, SCCACHE_CACHE_SIZE,
#     SCCACHE_BASEDIRS=<project root>, and CC/CXX wrapped so C/C++ build scripts
#     cache too) while keeping a PRIVATE per-attempt CARGO_TARGET_DIR; and
#   * a second identical build in a fresh layer is a warm sccache cache hit.
#
# The wrapper is load-bearing: sccache folds `CARGO_TARGET_DIR` into its cache
# key, and every attempt has a distinct private target path, so leaving it set
# would make even byte-identical builds miss. The generated wrapper unsets it
# before exec'ing sccache. This scenario proves a fresh-layer rebuild hits.
#
# Two concurrent build-capable spawns must receive distinct private target dirs
# (no shared mutable target, no cargo target-lock serialization).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# Unix-domain sockets have a short path cap; keep this disposable fixture short
# even when the harness lives in a deep worktree.
export WG_SMOKE_ROOT="${WG_SCCACHE_SMOKE_ROOT:-/tmp/wgsc-${BASHPID}}"
. "$HERE/_helpers.sh"

# Run the scenario against the candidate binary under test, not the operator's
# installed `wg` (the whole point is to exercise the current worktree's spawn
# wiring). Prefer an already-built candidate; otherwise build it.
REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-}"
if [[ -z "$WG_BIN" ]]; then
  if [[ -x "${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wg" ]]; then
    WG_BIN="${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wg"
  else
    CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo build --quiet --locked \
      --manifest-path "$REPO_ROOT/Cargo.toml" --bin wg
    WG_BIN="${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wg"
  fi
fi
[[ -x "$WG_BIN" ]] || loud_fail "candidate wg binary missing: $WG_BIN"
export PATH="$(dirname "$WG_BIN"):$PATH"
require_wg
command -v cargo >/dev/null 2>&1 || loud_skip "MISSING CARGO" "shared sccache fixture requires cargo"

SCCACHE_BIN="${WG_SCCACHE_BIN:-$(command -v sccache 2>/dev/null || true)}"
[[ -n "$SCCACHE_BIN" && -x "$SCCACHE_BIN" ]] || \
  loud_skip "MISSING SCCACHE" "shared compiler-cache scenario requires sccache (install it or set WG_SCCACHE_BIN)"

scratch=$(make_scratch)
project="$scratch/project"
sccache_dir="$scratch/sccache"
fixture="$scratch/fixture"
mkdir -p "$project" "$fixture/dep/src" "$fixture/app/src"

# Fixed-path local dependency so the compile source path is identical across
# layers (no registry / network needed).
cat >"$fixture/dep/Cargo.toml" <<'EOF'
[package]
name = "scc_dep"
version = "0.1.0"
edition = "2021"
EOF
printf 'pub fn dep_value() -> u32 { 41 }\n' >"$fixture/dep/src/lib.rs"
cat >"$fixture/app/Cargo.toml" <<EOF
[package]
name = "scc_app"
version = "0.1.0"
edition = "2021"

[dependencies]
scc_dep = { path = "$fixture/dep" }
EOF
printf 'pub fn answer() -> u32 { scc_dep::dep_value() + 1 }\n' >"$fixture/app/src/lib.rs"

probe="$scratch/probe.sh"
cat >"$probe" <<'EOF'
#!/usr/bin/env bash
set -u
mode="$1"; outdir="$2"; fixture="$3"
mkdir -p "$outdir"
{
  echo "RUSTC_WRAPPER=${RUSTC_WRAPPER:-}"
  echo "SCCACHE_DIR=${SCCACHE_DIR:-}"
  echo "SCCACHE_BASEDIRS=${SCCACHE_BASEDIRS:-}"
  echo "SCCACHE_CACHE_SIZE=${SCCACHE_CACHE_SIZE:-}"
  echo "CC=${CC:-}"
  echo "CXX=${CXX:-}"
  echo "CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-}"
} >"$outdir/env.txt"
if [ "$mode" = "build" ]; then
  cd "$fixture/app"
  rm -rf "$fixture/layer1" "$fixture/layer2"
  CARGO_TARGET_DIR="$fixture/layer1" cargo build -q >"$outdir/build1.log" 2>&1
  CARGO_TARGET_DIR="$fixture/layer2" cargo build -q >"$outdir/build2.log" 2>&1
  # The generated wrapper doubles as the sccache entry point for stats.
  "${RUSTC_WRAPPER:-sccache}" --show-stats >"$outdir/stats.txt" 2>&1 || true
fi
echo ok >"$outdir/done"
EOF
chmod +x "$probe"

# The fixture models a human launching the selected project.
unset WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_REASONING WG_TIER WG_SPAWN_EPOCH \
  WG_WORKER_CAPABILITY WG_WORKER_CONTROL_PROTOCOL WG_WORKER_IPC \
  WG_WORKER_CONTROL_MODE WG_WORKER_GENERATION WG_WORKER_ATTEMPT_ID \
  WG_WORKER_ATTEMPT_FENCE WG_GRAPH_ID WG_SPAWN_RUN_ID || true

(
  cd "$project"
  wg init --no-agency >/dev/null
)
cat >"$project/.wg/config.toml" <<'EOF'
[agency]
auto_assign = false
auto_evaluate = false

[dispatcher]
worktree_isolation = false
max_agents = 2

[dispatcher.resource_management]
disk_sentinel_enabled = true
sccache_enabled = true
sccache_cache_size = "2G"
disk_warning_bytes = 0
disk_pause_build_bytes = 0
disk_hard_refuse_bytes = 0
disk_warning_percent = 0.0
disk_pause_build_percent = 0.0
disk_hard_refuse_percent = 0.0
disk_resume_hysteresis_bytes = 0
disk_resume_hysteresis_percent = 0.0
estimated_build_bytes = 0
estimated_build_heavy_bytes = 0
estimated_cargo_baseline_bytes = 0
build_link_test_safety_bytes = 0
EOF

# A shell task still needs a route selected before admission (the route selects
# the executor plan; the shell task itself never invokes it). codex self-auths
# and is never launched, so this stays credential-free.
wg --dir "$project" config --local --model codex:gpt-5.5 --no-reload >/dev/null

out_a="$scratch/out-a"
out_b="$scratch/out-b"
wg --dir "$project" add "cargo build shared sccache warm rebuild" \
  --id sccache-probe-build \
  --exec-mode shell \
  --exec "bash '$probe' build '$out_a' '$fixture'" >/dev/null
wg --dir "$project" add "cargo build shared sccache concurrent peer" \
  --id sccache-probe-peer \
  --exec-mode shell \
  --exec "bash '$probe' env '$out_b' '$fixture'" >/dev/null
wg --dir "$project" publish sccache-probe-build --only >/dev/null
wg --dir "$project" publish sccache-probe-peer --only >/dev/null

export WG_SCCACHE_BIN="$SCCACHE_BIN" WG_SCCACHE_DIR="$sccache_dir"
start_wg_daemon "$project" --max-agents 2 --no-chat-agent

wait_for() {
  local file="$1" label="$2"
  for _ in $(seq 1 160); do
    [ -s "$file" ] && return 0
    sleep 0.25
  done
  loud_fail "$label never completed; task=$(wg --dir "$project" show sccache-probe-build 2>&1 | tail -60); daemon=$(tail -120 "$project/.wg/service/daemon.log" 2>/dev/null || true)"
}

wait_for "$out_a/done" "build probe"
wait_for "$out_b/done" "peer probe"

# ── Wiring assertions ────────────────────────────────────────────────
grep -q "^RUSTC_WRAPPER=.*wg-rustc-wrapper.sh$" "$out_a/env.txt" || \
  loud_fail "spawn did not set the WG sccache rustc wrapper: $(cat "$out_a/env.txt")"
wrapper=$(sed -n 's/^RUSTC_WRAPPER=//p' "$out_a/env.txt")
[ -x "$wrapper" ] || loud_fail "generated rustc wrapper is not executable: $wrapper"
grep -q "unset CARGO_TARGET_DIR" "$wrapper" || \
  loud_fail "wrapper does not hide the per-attempt target dir from sccache: $(cat "$wrapper")"

grep -q "^SCCACHE_DIR=$sccache_dir$" "$out_a/env.txt" || \
  loud_fail "SCCACHE_DIR was not the configured shared dir: $(cat "$out_a/env.txt")"
grep -q "^SCCACHE_BASEDIRS=$project$" "$out_a/env.txt" || \
  loud_fail "SCCACHE_BASEDIRS was not the project root: $(cat "$out_a/env.txt")"
grep -q "^SCCACHE_CACHE_SIZE=2G$" "$out_a/env.txt" || \
  loud_fail "SCCACHE_CACHE_SIZE was not applied: $(cat "$out_a/env.txt")"
grep -q "^CC=.*$(basename "$SCCACHE_BIN")" "$out_a/env.txt" || \
  loud_fail "CC was not wrapped by sccache: $(cat "$out_a/env.txt")"
grep -q "^CXX=.*$(basename "$SCCACHE_BIN")" "$out_a/env.txt" || \
  loud_fail "CXX was not wrapped by sccache: $(cat "$out_a/env.txt")"

[ -d "$sccache_dir" ] || loud_fail "shared sccache dir was not created: $sccache_dir"
[ "$(find "$sccache_dir" -type f | head -1)" ] || \
  loud_fail "shared sccache dir is empty after a build: $sccache_dir"

# ── Warm rebuild in a fresh layer ────────────────────────────────────
[ -d "$fixture/layer2" ] || loud_fail "second fresh-layer build did not run"
hits=$(sed -n 's/^Cache hits *//p' "$out_a/stats.txt" | head -1 | tr -d '[:space:]')
[ -n "$hits" ] || loud_fail "could not read sccache stats: $(cat "$out_a/stats.txt")"
[ "$hits" -ge 1 ] || \
  loud_fail "second identical build in a fresh layer was NOT a cache hit (hits=$hits): $(cat "$out_a/stats.txt")"

# ── Concurrency: distinct private target dirs ────────────────────────
target_a=$(sed -n 's/^CARGO_TARGET_DIR=//p' "$out_a/env.txt")
target_b=$(sed -n 's/^CARGO_TARGET_DIR=//p' "$out_b/env.txt")
[ -n "$target_a" ] && [ -n "$target_b" ] || \
  loud_fail "build-capable spawns lacked a private CARGO_TARGET_DIR (a='$target_a' b='$target_b')"
[ "$target_a" != "$target_b" ] || \
  loud_fail "two concurrent build-capable spawns shared one target dir: $target_a"

echo "PASS: shared sccache wired with a private per-attempt target; fresh-layer rebuild was warm ($hits hit(s)); concurrent spawns kept distinct targets"
