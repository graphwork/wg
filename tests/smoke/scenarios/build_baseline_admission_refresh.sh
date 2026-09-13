#!/usr/bin/env bash
# Candidate-binary daemon regression for exact Cargo baseline ownership: generic
# work may run beside the one real cold builder, while a second exact-key Cargo
# task waits with an actionable builder identity.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v cargo >/dev/null 2>&1 || loud_skip "MISSING CARGO" "exact baseline fixture requires cargo"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "registry assertions require python3"

REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"
scratch="$(make_scratch)"
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then
  WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
else
  CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo build --quiet --locked --manifest-path "$REPO_ROOT/Cargo.toml" --bin wg
  WG_BIN="${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wg"
fi
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"
export PATH="$(dirname "$WG_BIN"):$PATH"

real_home="$HOME"
export HOME="$scratch/home" WG_GLOBAL_DIR="$scratch/home/.wg" XDG_CONFIG_HOME="$scratch/home/.config"
export CARGO_HOME="${CARGO_HOME:-$real_home/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$real_home/.rustup}"
project="$scratch/project"; cache="$scratch/target-cache"
mkdir -p "$project/src" "$cache" "$HOME" "$XDG_CONFIG_HOME"
cd "$project"
git init -q -b main
git config user.name 'WG Baseline Smoke'
git config user.email wg@example.invalid
cat >Cargo.toml <<'EOF'
[package]
name = "wg_baseline_smoke"
version = "0.1.0"
edition = "2024"
EOF
printf 'fn main() {}\n' >src/main.rs
cargo generate-lockfile --quiet
git add Cargo.toml Cargo.lock src/main.rs
git commit -qm cargo-fixture
"$WG_BIN" --dir "$project/.wg" init --no-agency >/dev/null
cat >.wg/config.toml <<EOF
[agency]
auto_assign = false
auto_evaluate = false

[dispatcher]
max_agents = 3
poll_interval = 1
settling_delay_ms = 0
worktree_isolation = false

[dispatcher.resource_management]
disk_sentinel_enabled = true
cargo_target_root = "$cache"
disk_warning_bytes = 0
disk_pause_build_bytes = 0
disk_hard_refuse_bytes = 0
disk_warning_percent = 0.0
disk_pause_build_percent = 0.0
disk_hard_refuse_percent = 0.0
estimated_build_bytes = 1048576
estimated_build_heavy_bytes = 1048576
estimated_cargo_baseline_bytes = 67108864
build_link_test_safety_bytes = 0
max_build_agents = 3
disk_agent_heartbeat_seconds = 60
EOF
"$WG_BIN" --dir "$project/.wg" config --local --model codex:gpt-5.5 --no-reload >/dev/null
git add .gitignore AGENTS.md CLAUDE.md worksgood.toml
git commit -qm wg-fixture
[[ -z "$(git status --porcelain)" ]] || loud_fail "baseline fixture source is dirty"

exact='cargo check && sleep 20'
"$WG_BIN" --dir "$project/.wg" add 'long non-Rust operation' --id a-non-rust --priority 100 \
  --exec 'sleep 20' --exec-mode shell >/dev/null
"$WG_BIN" --dir "$project/.wg" add 'exact Cargo cold builder' --id b-cold-builder --priority 90 \
  --exec "$exact" --exec-mode shell >/dev/null
"$WG_BIN" --dir "$project/.wg" add 'exact Cargo waiting follower' --id c-cold-follower --priority 80 \
  --exec "$exact" --exec-mode shell >/dev/null
for task in a-non-rust b-cold-builder c-cold-follower; do
  "$WG_BIN" --dir "$project/.wg" publish "$task" --only >/dev/null
done

start_wg_daemon "$project" --max-agents 3 --no-chat-agent --interval 1
pid="$(python3 -c 'import json; print(json.load(open(".wg/service/state.json"))["pid"])')"
status=''
for _ in $(seq 1 150); do
  status="$($WG_BIN --dir "$project/.wg" service status 2>/dev/null || true)"
  agents="$(python3 - <<'PY'
import json
try:
 d=json.load(open('.wg/service/registry.json'))
 print(' '.join(sorted(a['task_id'] for a in d.get('agents',{}).values())))
except Exception:
 print('')
PY
)"
  if grep -q 'a-non-rust' <<<"$agents" \
    && grep -q 'b-cold-builder' <<<"$agents" \
    && ! grep -q 'c-cold-follower' <<<"$agents" \
    && grep -q "active baseline builder task 'b-cold-builder'" <<<"$status"; then
    break
  fi
  sleep 0.1
done

grep -q 'a-non-rust' <<<"${agents:-}" \
  || loud_fail "non-Rust worker did not start: ${agents:-}; $(tail -80 .wg/service/daemon.log)"
grep -q 'b-cold-builder' <<<"${agents:-}" \
  || loud_fail "non-Rust work was falsely treated as the Cargo baseline builder: ${agents:-}; $status"
! grep -q 'c-cold-follower' <<<"${agents:-}" \
  || loud_fail "two cold exact Cargo builders were admitted: ${agents:-}"
grep -q "active baseline builder task 'b-cold-builder'" <<<"$status" \
  || loud_fail "status omitted the real active baseline builder: $status"
grep -q 'next action' <<<"$status" \
  || loud_fail "status omitted baseline recovery action: $status"
! grep -q 'WG-EXEC-ROUTE-MISSING' <<<"$status" \
  || loud_fail "cold baseline wait was mislabeled as missing route: $status"
[[ "$pid" == "$(python3 -c 'import json; print(json.load(open(".wg/service/state.json"))["pid"])')" ]] \
  || loud_fail "daemon restarted during baseline admission"

python3 - "$cache" <<'PY'
import json,sys
from pathlib import Path
root=Path(sys.argv[1])/'layers'
rows=[]
for p in root.glob('*/*/target/.wg-target-layer.json'):
    rows.append((p,json.loads(p.read_text())))
nonrust=[v for p,v in rows if '/agent-1/' in str(p)]
assert nonrust and nonrust[0]['key']['baseline_reusable'] is False, rows
cargo=[v for p,v in rows if v['key']['command_identity']=='cargo check && sleep 20']
assert len(cargo)==1, rows
assert cargo[0]['key']['baseline_reusable'] is True, cargo
PY

printf '%s\n' "PASS: generic non-Rust work runs beside exactly one cold exact Cargo builder, and status names its live owner and bounded next action without restarting the daemon"
