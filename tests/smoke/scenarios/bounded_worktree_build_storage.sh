#!/usr/bin/env bash
# Candidate-binary regression for immutable Cargo baseline + private attempt deltas.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wg}"
[ -x "$WG_BIN" ] || loud_fail "fresh candidate wg binary is missing or not executable: $WG_BIN"
candidate_sha=$(sha256sum "$WG_BIN" | cut -d' ' -f1)
fakebin=$(mktemp -d "${TMPDIR:-/tmp}/wg-bounded-candidate.XXXXXX")
register_scratch "$fakebin"
ln -s "$WG_BIN" "$fakebin/wg"
export PATH="$fakebin:$PATH"
# The scenario creates its own graph/daemon and must not inherit the invoking
# worker's fenced control-plane identity.
unset WG_AGENT_ID WG_TASK_ID WG_WORKER_CAPABILITY WG_WORKER_CONTROL_PROTOCOL \
  WG_WORKER_IPC WG_WORKER_CONTROL_MODE WG_WORKER_GENERATION WG_WORKER_ATTEMPT_ID \
  WG_WORKER_ATTEMPT_FENCE WG_GRAPH_ID WG_SPAWN_RUN_ID WG_SPAWN_EPOCH || true
[ "$(readlink -f "$(command -v wg)")" = "$(readlink -f "$WG_BIN")" ] \
  || loud_fail "smoke PATH did not bind to candidate binary $WG_BIN"
command -v cargo >/dev/null 2>&1 || loud_skip "NO CARGO" "cargo not on PATH"
command -v python3 >/dev/null 2>&1 || loud_skip "NO PYTHON" "python3 required"
command -v git >/dev/null 2>&1 || loud_skip "NO GIT" "git required"

scratch=$(make_scratch)
project="$scratch/tiny"
shared="$scratch/shared-base"
cache="$scratch/build-cache"
mkdir -p "$project/src" "$shared/src"
cat > "$project/Cargo.toml" <<EOF
[package]
name = "cow-smoke"
version = "0.1.0"
edition = "2024"

[dependencies]
base = { path = "$shared" }
EOF
cat > "$shared/Cargo.toml" <<'EOF'
[package]
name = "base"
version = "0.1.0"
edition = "2024"
EOF
cat > "$shared/src/lib.rs" <<'EOF'
pub static PAYLOAD: [u8; 8 * 1024 * 1024] = [7; 8 * 1024 * 1024];
pub fn answer() -> usize { 42 }
EOF
cat > "$project/src/main.rs" <<'EOF'
fn main() {
    let worker = if cfg!(worker_two) { 2 } else if cfg!(worker_three) { 3 } else { 1 };
    println!("{}:{}", worker, base::answer());
}
EOF
cat > "$project/build.rs" <<'EOF'
fn main() {
    println!("cargo:rustc-check-cfg=cfg(worker_two)");
    println!("cargo:rustc-check-cfg=cfg(worker_three)");
    println!("cargo:rerun-if-env-changed=WORKER_VARIANT");
    match std::env::var("WORKER_VARIANT").as_deref() {
        Ok("two") => println!("cargo:rustc-cfg=worker_two"),
        Ok("three") => println!("cargo:rustc-cfg=worker_three"),
        _ => {}
    }
}
EOF
cd "$project"
cargo generate-lockfile --offline >/dev/null
git init -q -b main
git add Cargo.toml Cargo.lock build.rs src
git -c user.name='WG Smoke' -c user.email='wg@example.invalid' commit -qm base
wg init --no-agency >/dev/null
git add .gitignore AGENTS.md CLAUDE.md
git -c user.name='WG Smoke' -c user.email='wg@example.invalid' commit -qm wg-init
cat > .wg/config.toml <<EOF
[agency]
auto_assign = false
auto_evaluate = false

[dispatcher]
max_agents = 3
poll_interval = 1
settling_delay_ms = 0

[dispatcher.resource_management]
disk_sentinel_enabled = true
cargo_target_root = "$cache"
disk_warning_bytes = 0
disk_pause_build_bytes = 0
disk_hard_refuse_bytes = 0
disk_warning_percent = 0.0
disk_pause_build_percent = 0.0
disk_hard_refuse_percent = 0.0
estimated_build_bytes = 0
estimated_build_heavy_bytes = 0
estimated_cargo_baseline_bytes = 0
build_link_test_safety_bytes = 0
disk_scan_interval_seconds = 1
owned_cache_lease_seconds = 3600
max_build_agents = 3
EOF

start_wg_daemon "$project" --max-agents 3 --no-chat-agent --interval 1

# Seed the exact-key baseline once through the real candidate spawn/lease path.
warm_artifact="$scratch/warm.sha"
wg add "warm cargo build" --id warm-cargo-build \
  --exec "cargo build --quiet && sha256sum \"\$CARGO_TARGET_DIR/debug/cow-smoke\" > '$warm_artifact' && wg artifact \"\$WG_TASK_ID\" '$warm_artifact' && wg wait \"\$WG_TASK_ID\" --until message --checkpoint 'warm baseline built'" >/dev/null
wg publish warm-cargo-build --only >/dev/null
for _ in $(seq 1 240); do
  [ -s "$warm_artifact" ] && break
  sleep 0.25
done
[ -s "$warm_artifact" ] || loud_fail "warm candidate worker did not build: $(tail -100 .wg/service/daemon.log 2>&1)"
for _ in $(seq 1 120); do
  status=$(wg show warm-cargo-build --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))' 2>/dev/null || true)
  { [ "$status" = done ] || [ "$status" = waiting ]; } && break
  sleep 0.25
done
{ [ "${status:-}" = done ] || [ "${status:-}" = waiting ]; } || loud_fail "warm worker did not become terminal: status=${status:-}; show=$(wg show warm-cargo-build --json 2>&1); output=$(find .wg/agents -name output.log -exec tail -60 {} \; 2>&1)"
for _ in $(seq 1 120); do
  warm_live=$(python3 - "$project" <<'PY'
import json, sys
from pathlib import Path
p=Path(sys.argv[1]); r=json.loads((p/'.wg/service/registry.json').read_text())
a=next(a for a in r['agents'].values() if a.get('task_id')=='warm-cargo-build')
print(int((Path('/proc')/str(a['pid'])).exists()))
PY
)
  [ "$warm_live" = 0 ] && break
  sleep 0.1
done
[ "${warm_live:-1}" = 0 ] || loud_fail "warm wrapper PID remained live"
wg disk cleanup --execute --json > "$scratch/warm-cleanup.json"
baseline=$(find "$cache/baselines" -mindepth 1 -maxdepth 1 -type d -print -quit 2>/dev/null || true)
[ -n "$baseline" ] && [ -f "$baseline/READY" ] \
  || loud_fail "terminal clean worker did not publish an immutable baseline: $(cat "$scratch/warm-cleanup.json")"
find "$baseline" -type f -print0 | sort -z | xargs -0 sha256sum > "$scratch/baseline.before"
printf 'valuable uncommitted source must survive cache cleanup\n' > "$project/valuable-dirty-source.rs"
dirty_before=$(sha256sum "$project/valuable-dirty-source.rs" | cut -d' ' -f1)

# Three isolated workers launch together. One is a repeated no-change build;
# two diverge in distinct dirty worktrees. They remain live for measurement.
for n in 1 2 3; do
  artifact="$scratch/result-$n.sha"
  ready="$scratch/ready-$n"
  case "$n" in
    1) build_cmd="cargo build" ;;
    2) build_cmd="printf 'fn main() { println!(\"2:{}\", base::answer()); }\\n' > src/main.rs && cargo clean -p cow-smoke && cargo build" ;;
    3) build_cmd="printf 'fn main() { println!(\"3:{}\", base::answer()); }\\n' > src/main.rs && cargo clean -p cow-smoke && cargo build" ;;
  esac
  wg add "isolated cargo build worker $n" --id "cow-build-$n" \
    --exec "$build_cmd --quiet; sha256sum \"\$CARGO_TARGET_DIR/debug/cow-smoke\" > '$artifact'; wg artifact \"\$WG_TASK_ID\" '$artifact'; touch '$ready'; while [ ! -e '$scratch/release' ]; do sleep 0.1; done; wg wait \"\$WG_TASK_ID\" --until message --checkpoint 'storage fixture complete'" >/dev/null
  wg publish "cow-build-$n" --only >/dev/null
done
for _ in $(seq 1 480); do
  [ -e "$scratch/ready-1" ] && [ -e "$scratch/ready-2" ] && [ -e "$scratch/ready-3" ] && break
  sleep 0.25
done
[ -e "$scratch/ready-1" ] && [ -e "$scratch/ready-2" ] && [ -e "$scratch/ready-3" ] \
  || loud_fail "three candidate build workers did not overlap: task=$(wg show cow-build-2 --json 2>&1); daemon=$(tail -120 .wg/service/daemon.log 2>&1)"
sha256sum "$scratch"/result-*.sha > "$scratch/artifacts.before"

python3 - "$project" "$baseline/target" "$scratch/metrics.json" <<'PY'
import json, os, sys
from pathlib import Path
project, baseline, output = map(Path, sys.argv[1:])
registry = json.loads((project/'.wg/service/registry.json').read_text())
ownership = json.loads((project/'.wg/service/disk/owned-caches.json').read_text())
tasks = {f'cow-build-{i}' for i in range(1,4)}
agents = [a for a in registry['agents'].values() if a.get('task_id') in tasks]
assert len(agents) == 3, agents
worktrees = {a.get('worktree_path') for a in agents}
assert None not in worktrees and len(worktrees) == 3, worktrees
layers = [Path(c['path']) for c in ownership['caches'] if c.get('kind') == 'cargo-target' and c.get('task_id') in tasks]
assert len(layers) == 3 and all(p.is_dir() for p in layers), layers

def files(root):
    for parent, _, names in os.walk(root):
        for name in names:
            p = Path(parent)/name
            try: st = p.stat()
            except FileNotFoundError: continue
            if p.is_file(): yield p, st

def logical(root): return sum(st.st_size for _, st in files(root))
def private(root, baseline_inodes):
    seen=set(); total=0
    for _,st in files(root):
        inode=(st.st_dev,st.st_ino)
        if inode not in baseline_inodes and inode not in seen:
            seen.add(inode); total += st.st_blocks*512
    return total
def distinct(roots):
    seen=set(); total=0
    for root in roots:
        for _, st in files(root):
            key=(st.st_dev, st.st_ino)
            if key not in seen:
                seen.add(key); total += st.st_blocks*512
    return total, len(seen)

base_logical=logical(baseline)
base_physical,_=distinct([baseline])
base_inodes={(st.st_dev,st.st_ino) for _,st in files(baseline)}
layer_logical=[logical(p) for p in layers]
layer_private=[private(p,base_inodes) for p in layers]
physical,inodes=distinct([baseline,*layers])
logical_total=base_logical+sum(layer_logical)
shared=0
for layer in layers:
    shared += sum((st.st_dev,st.st_ino) in base_inodes for _,st in files(layer))
assert base_logical > 0 and shared > 0, (base_logical,shared)
assert all(value >= base_logical//2 for value in layer_logical), layer_logical
assert physical < logical_total, (physical,logical_total)
assert physical <= base_physical + sum(layer_private) + 4096, (physical,base_physical,layer_private)
# The repeated no-change worker is whichever layer has the smallest delta; it
# must not materialize another complete baseline.
assert min(layer_private) < max(base_physical//2, 65536), (base_physical,layer_private)
data={'baseline_logical':base_logical,'baseline_physical':base_physical,
      'layer_logical':layer_logical,'layer_private':layer_private,
      'total_logical':logical_total,'total_physical':physical,
      'shared_baseline_links':shared,'worktrees':sorted(worktrees),
      'layers':[str(p) for p in layers]}
output.write_text(json.dumps(data,indent=2))
print(json.dumps(data,sort_keys=True))
PY

# Crash/restart between process exit and cache cleanup. Workers are detached;
# source attempts finish while the daemon is absent. Restart must converge.
wg service stop >/dev/null
touch "$scratch/release"
for _ in $(seq 1 240); do
  live=$(python3 - "$project" <<'PY'
import json, os, sys
from pathlib import Path
p=Path(sys.argv[1]); r=json.loads((p/'.wg/service/registry.json').read_text())
ids={f'cow-build-{i}' for i in range(1,4)}
print(sum(1 for a in r['agents'].values() if a.get('task_id') in ids and Path('/proc') .joinpath(str(a['pid'])).exists()))
PY
)
  [ "$live" = 0 ] && break
  sleep 0.25
done
[ "${live:-1}" = 0 ] || loud_fail "detached workers did not exit after daemon stop"
rm -f .wg/service/disk/disk-sentinel.json
start_wg_daemon "$project" --max-agents 3 --no-chat-agent --interval 1
for _ in $(seq 1 240); do
  remaining=$(python3 - "$project" <<'PY'
import json, sys
from pathlib import Path
p=Path(sys.argv[1]); f=p/'.wg/service/disk/owned-caches.json'
if not f.exists(): print(0)
else:
 d=json.loads(f.read_text()); print(sum(c.get('task_id','').startswith('cow-build-') for c in d.get('caches',[])))
PY
)
  [ "$remaining" = 0 ] && break
  sleep 0.25
done
[ "${remaining:-1}" = 0 ] || loud_fail "restart did not automatically compact terminal private-layer leases"
python3 - "$scratch/metrics.json" <<'PY'
import json, sys
from pathlib import Path
m=json.loads(Path(sys.argv[1]).read_text())
assert all(not Path(p).exists() for p in m['layers']), m['layers']
PY
find "$baseline" -type f -print0 | sort -z | xargs -0 sha256sum > "$scratch/baseline.after"
cmp -s "$scratch/baseline.before" "$scratch/baseline.after" \
  || loud_fail "concurrent divergent builds mutated the immutable baseline"
# Dirty source and registered artifacts survive byte-identically.
[ "$(sha256sum "$project/valuable-dirty-source.rs" | cut -d' ' -f1)" = "$dirty_before" ] \
  || loud_fail "cache cleanup altered dirty source"
for n in 1 2 3; do
  [ -s "$scratch/result-$n.sha" ] || loud_fail "registered artifact $n was removed"
done
sha256sum "$scratch"/result-*.sha > "$scratch/artifacts.after"
cmp -s "$scratch/artifacts.before" "$scratch/artifacts.after" \
  || loud_fail "registered artifact bytes changed during cleanup"
[ "$(cut -d' ' -f1 "$scratch/result-2.sha")" != "$(cut -d' ' -f1 "$scratch/result-3.sha")" ] \
  || loud_fail "divergent workers did not produce independent outputs"

metrics=$(cat "$scratch/metrics.json")
printf '%s\n' "PASS: candidate=$WG_BIN sha256=$candidate_sha launched three isolated Cargo workers; logical/physical metrics=$metrics; one immutable baseline was reused, divergent outputs stayed private, and restart cleanup converged"
