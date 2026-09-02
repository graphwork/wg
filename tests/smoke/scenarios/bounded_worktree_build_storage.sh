#!/usr/bin/env bash
# Exact-HEAD candidate regression for immutable Cargo baselines and private CoW layers.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
# Unix-domain sockets have a short fixed path budget; worker TMPDIR can be a
# deeply nested owned scratch root, so pin this daemon fixture under /tmp.
export WG_SMOKE_ROOT="${WG_SMOKE_ROOT:-/tmp/wgs-bounded-$$}"
. "$HERE/_helpers.sh"
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
command -v cargo >/dev/null 2>&1 || loud_skip "NO CARGO" "cargo not on PATH"
command -v python3 >/dev/null 2>&1 || loud_skip "NO PYTHON" "python3 required"
command -v git >/dev/null 2>&1 || loud_skip "NO GIT" "git required"
command -v sha256sum >/dev/null 2>&1 || loud_skip "NO SHA256" "sha256sum required"

scratch=$(make_scratch)

# Build the executable used by this scenario from the exact submitted HEAD.
# An inherited/prebuilt target/debug/wg is never provenance authority.
cd "$REPO_ROOT"
source_commit=$(git rev-parse --verify HEAD)
source_tree=$(git rev-parse --verify 'HEAD^{tree}')
if [ -n "$(git status --porcelain --untracked-files=normal -- . ':!.wg-cleanup-pending')" ]; then
  loud_fail "candidate source is not the exact clean submitted HEAD $source_commit"
fi
build_target="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
cargo build --locked --bin wg --target-dir "$build_target" >/dev/null
mkdir -p "$scratch/candidate-bin"
WG_BIN="$scratch/candidate-bin/wg"
cp "$build_target/debug/wg" "$WG_BIN"
[ -x "$WG_BIN" ] || loud_fail "exact-HEAD cargo build did not produce $WG_BIN"
candidate_sha=$(sha256sum "$WG_BIN" | cut -d' ' -f1)
receipt="$scratch/candidate-build-receipt.json"
python3 - "$receipt" "$REPO_ROOT" "$source_commit" "$source_tree" "$WG_BIN" "$candidate_sha" "$build_target" <<'PY'
import json, sys
from pathlib import Path
out, root, commit, tree, binary, digest, target = sys.argv[1:]
Path(out).write_text(json.dumps({
    'schema': 1,
    'source_root': str(Path(root).resolve()),
    'source_commit': commit,
    'source_tree': tree,
    'binary_path': str(Path(binary).resolve()),
    'binary_sha256': digest,
    'build': ['cargo','build','--locked','--bin','wg','--target-dir',str(Path(target).resolve())],
}, sort_keys=True, separators=(',',':')) + '\n')
PY
verify_candidate_receipt() {
  python3 - "$receipt" "$REPO_ROOT" "$WG_BIN" <<'PY'
import hashlib, json, subprocess, sys
from pathlib import Path
receipt, root, expected = map(Path, sys.argv[1:])
r=json.loads(receipt.read_text())
commit=subprocess.check_output(['git','rev-parse','--verify','HEAD'],cwd=root,text=True).strip()
tree=subprocess.check_output(['git','rev-parse','--verify','HEAD^{tree}'],cwd=root,text=True).strip()
binary=expected.resolve()
actual=hashlib.sha256(binary.read_bytes()).hexdigest()
assert r['source_root'] == str(root.resolve()), (r,root)
assert r['source_commit'] == commit, (r['source_commit'],commit)
assert r['source_tree'] == tree, (r['source_tree'],tree)
assert r['binary_path'] == str(binary), (r['binary_path'],binary)
assert r['binary_sha256'] == actual, (r['binary_sha256'],actual)
PY
}
verify_candidate_receipt || loud_fail "fresh candidate receipt did not verify"

# A stale executable copied into the expected path must fail the receipt check.
cp "$WG_BIN" "$scratch/exact-candidate.saved"
stale_bin=$(command -v wg 2>/dev/null || true)
if [ -n "$stale_bin" ] && [ -f "$stale_bin" ] && [ "$(sha256sum "$stale_bin" | cut -d' ' -f1)" != "$candidate_sha" ]; then
  cp "$stale_bin" "$WG_BIN"
else
  cp /bin/true "$WG_BIN"
fi
if verify_candidate_receipt >/dev/null 2>&1; then
  loud_fail "candidate receipt accepted a stale substituted executable"
fi
mv "$scratch/exact-candidate.saved" "$WG_BIN"
verify_candidate_receipt || loud_fail "restored exact candidate no longer verifies"
"$WG_BIN" --version >/dev/null || loud_fail "receipt-bound exact candidate did not execute"
receipt_sha=$(sha256sum "$receipt" | cut -d' ' -f1)

fakebin="$scratch/fakebin"
mkdir -p "$fakebin"
ln -s "$WG_BIN" "$fakebin/wg"
export PATH="$fakebin:$PATH"
unset WG_AGENT_ID WG_TASK_ID WG_WORKER_CAPABILITY WG_WORKER_CONTROL_PROTOCOL \
  WG_WORKER_IPC WG_WORKER_CONTROL_MODE WG_WORKER_GENERATION WG_WORKER_ATTEMPT_ID \
  WG_WORKER_ATTEMPT_FENCE WG_GRAPH_ID WG_SPAWN_RUN_ID WG_SPAWN_EPOCH || true
[ "$(readlink -f "$(command -v wg)")" = "$(readlink -f "$WG_BIN")" ] \
  || loud_fail "smoke PATH did not bind to receipt candidate $WG_BIN"

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
pub static PAYLOAD: [u8; 16 * 1024 * 1024] = [7; 16 * 1024 * 1024];
pub fn answer() -> usize { PAYLOAD[0] as usize + 35 }
EOF
cat > "$project/src/main.rs" <<'EOF'
fn main() { println!("{}", base::answer()); }
EOF
cd "$project"
cargo generate-lockfile --offline >/dev/null
git init -q -b main
git add Cargo.toml Cargo.lock src
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
mkdir -p "$cache"
# Probe the same FICLONE primitive used by the candidate. This selects the
# workload-level physical bound below; unsupported filesystems retain the safe
# full-private-copy fallback assertion.
reflink_supported=$(python3 - "$cache" <<'PY'
import fcntl, os, sys
from pathlib import Path
root=Path(sys.argv[1]); src=root/'.reflink-probe-source'; dst=root/'.reflink-probe-dest'
try:
    src.write_bytes(b'R' * (8 * 1024 * 1024))
    with src.open('rb') as source, dst.open('xb') as output:
        fcntl.ioctl(output.fileno(), 0x40049409, source.fileno())
        output.flush(); os.fsync(output.fileno())
    print(1)
except OSError:
    print(0)
finally:
    for path in (dst,src):
        try: path.unlink()
        except FileNotFoundError: pass
PY
)
sync
physical_free_before=$(python3 - "$cache" <<'PY'
import os,sys
v=os.statvfs(sys.argv[1]); print(v.f_bavail*v.f_frsize)
PY
)
start_wg_daemon "$project" --max-agents 3 --no-chat-agent --interval 1

# The tiny accepted grammar is exactly one Cargo command with an optional inert
# bounded sleep. Stateful setup, redirection and arbitrary compound commands do
# not enter this reusable namespace (unit tests pin those fail-closed cases).
exact_command='cargo build --quiet && sleep 30 && wg wait "$WG_TASK_ID" --until message --checkpoint '\''storage fixture complete'\'''
wg add "warm exact cargo build" --id warm-cargo-build --exec "$exact_command" >/dev/null
wg publish warm-cargo-build --only >/dev/null
for _ in $(seq 1 160); do
  status=$(wg show warm-cargo-build --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))' 2>/dev/null || true)
  { [ "$status" = waiting ] || [ "$status" = done ]; } && break
  sleep 0.25
done
{ [ "${status:-}" = waiting ] || [ "${status:-}" = done ]; } || loud_fail "warm exact command did not complete: $(wg show warm-cargo-build --json 2>&1); wrapper=$(tail -60 "$project/daemon.log" 2>&1); daemon=$(tail -120 "$project/.wg/service/daemon.log" 2>&1)"
for _ in $(seq 1 80); do
  warm_live=$(python3 - "$project" <<'PY'
import json, sys
from pathlib import Path
r=json.loads((Path(sys.argv[1])/'.wg/service/registry.json').read_text())
a=next(a for a in r['agents'].values() if a.get('task_id')=='warm-cargo-build')
print(int((Path('/proc')/str(a['pid'])).exists()))
PY
)
  [ "$warm_live" = 0 ] && break
  sleep 0.1
done
[ "${warm_live:-1}" = 0 ] || loud_fail "warm wrapper remained live after lifecycle wait"
wg disk cleanup --execute --json > "$scratch/warm-cleanup.json"
baseline=$(find "$cache/baselines" -mindepth 1 -maxdepth 1 -type d -print -quit 2>/dev/null || true)
[ -n "$baseline" ] && [ -f "$baseline/READY" ] \
  || loud_fail "exact command did not publish immutable baseline: $(cat "$scratch/warm-cleanup.json")"
find "$baseline" -type f -print0 | sort -z | xargs -0 sha256sum > "$scratch/baseline.before"
sync
physical_free_after_baseline=$(python3 - "$cache" <<'PY'
import os,sys
v=os.statvfs(sys.argv[1]); print(v.f_bavail*v.f_frsize)
PY
)

# Three same-command workers overlap while sleeping after Cargo. Their regular
# artifacts must be private inodes even when the filesystem shares CoW extents.
for n in 1 2 3; do
  wg add "isolated exact cargo worker $n" --id "cow-build-$n" --exec "$exact_command" >/dev/null
  wg publish "cow-build-$n" --only >/dev/null
done
for _ in $(seq 1 240); do
  ready=$(python3 - "$project" <<'PY'
import json, sys
from pathlib import Path
p=Path(sys.argv[1]); f=p/'.wg/service/disk/owned-caches.json'
if not f.exists(): print(0)
else:
 d=json.loads(f.read_text()); rows=[Path(c['path']) for c in d.get('caches',[]) if c.get('task_id','').startswith('cow-build-') and c.get('kind')=='cargo-target']
 print(sum((x/'debug/cow-smoke').is_file() for x in rows))
PY
)
  [ "$ready" = 3 ] && break
  sleep 0.25
done
[ "${ready:-0}" = 3 ] || loud_fail "three receipt-candidate workers did not overlap: $(tail -100 .wg/service/daemon.log 2>&1)"

sync
physical_free_with_layers=$(python3 - "$cache" <<'PY'
import os,sys
v=os.statvfs(sys.argv[1]); print(v.f_bavail*v.f_frsize)
PY
)
python3 - "$project" "$baseline/target" "$scratch/metrics.json" "$scratch/owned-paths.json" \
  "$physical_free_before" "$physical_free_after_baseline" "$physical_free_with_layers" \
  "$reflink_supported" <<'PY'
import hashlib, json, os, sys
from pathlib import Path
project, baseline, output, owned_output = map(Path, sys.argv[1:5])
free_before, free_baseline, free_layers = map(int, sys.argv[5:8])
reflink_supported = bool(int(sys.argv[8]))
ownership=json.loads((project/'.wg/service/disk/owned-caches.json').read_text())
owned_rows=[c for c in ownership['caches'] if c.get('task_id','').startswith('cow-build-')]
layers=[Path(c['path']) for c in owned_rows if c.get('kind')=='cargo-target']
assert len(layers)==3 and all(p.is_dir() for p in layers), layers
artifact=Path('debug/cow-smoke')
paths=[baseline/artifact,*[p/artifact for p in layers]]
stats=[p.stat() for p in paths]
inodes={(s.st_dev,s.st_ino) for s in stats}
assert len(inodes)==4, [(str(p),s.st_dev,s.st_ino) for p,s in zip(paths,stats)]
before=[hashlib.sha256(p.read_bytes()).hexdigest() for p in paths]

# Adversarial in-place truncate/overwrite followed by rename in one worker.
victim=paths[1]
with victim.open('r+b') as f:
    f.truncate(97); f.seek(0); f.write(b'worker-private-overwrite')
renamed=victim.with_name('cow-smoke-renamed')
victim.rename(renamed)
after=[hashlib.sha256(p.read_bytes()).hexdigest() for p in [paths[0],paths[2],paths[3]]]
assert after == [before[0],before[2],before[3]], (before,after)
# Directory and symlink publication are private directory entries as well.
private_dir=layers[0]/'debug/private-dir'; private_dir.mkdir()
(private_dir/'bytes').write_bytes(b'private')
link=layers[0]/'debug/private-link'; link.symlink_to('private-dir/bytes')
link.rename(layers[0]/'debug/private-link-renamed')
private_dir.rename(layers[0]/'debug/private-dir-renamed')
assert (paths[0]).exists() and (paths[2]).exists() and (paths[3]).exists()

def logical(root):
    return sum(p.stat().st_size for p in root.rglob('*') if p.is_file() and not p.is_symlink())
def allocated(root):
    seen=set(); total=0
    for p in root.rglob('*'):
        if not p.is_file() or p.is_symlink(): continue
        s=p.stat(); identity=(s.st_dev,s.st_ino)
        if identity in seen: continue
        seen.add(identity); total += getattr(s,'st_blocks',0)*512
    return total
baseline_allocated=allocated(baseline)
layer_allocated=[allocated(p) for p in layers]
baseline_delta=max(0,free_before-free_baseline)
private_delta=max(0,free_baseline-free_layers)
# FICLONE-capable filesystems must hold the three workload layers as one
# baseline plus small private metadata/output deltas, not three full copies.
# The allowance absorbs daemon/log allocation elsewhere on the same mount.
reflink_bound=max(16*1024*1024, baseline_allocated//4)
fallback_bound=baseline_allocated*len(layers)+32*1024*1024
if reflink_supported:
    assert private_delta <= reflink_bound, (private_delta,reflink_bound,baseline_allocated,layer_allocated)
else:
    # Portable safe fallback: private byte copies may cost one complete layer
    # each, but physical growth remains bounded by those charged bytes.
    assert private_delta <= fallback_bound, (private_delta,fallback_bound,layer_allocated)
assert max(0,free_before-free_layers) <= baseline_delta + (reflink_bound if reflink_supported else fallback_bound)
owned_paths=[str(Path(c['path'])) for c in owned_rows]
assert len([p for p in owned_paths if Path(p).is_dir()]) == len(owned_paths), owned_paths
owned_output.write_text(json.dumps(owned_paths,sort_keys=True,indent=2))
data={'baseline_logical':logical(baseline),'layer_logical':[logical(p) for p in layers],
      'baseline_allocated':baseline_allocated,'layer_allocated_charged':layer_allocated,
      'physical_free_before':free_before,'physical_free_after_baseline':free_baseline,
      'physical_free_with_layers':free_layers,'baseline_physical_delta':baseline_delta,
      'private_layer_physical_delta':private_delta,'reflink_supported':reflink_supported,
      'applied_private_delta_bound':reflink_bound if reflink_supported else fallback_bound,
      'baseline_inode':stats[0].st_ino,'layer_inodes':[s.st_ino for s in stats[1:]],
      'layers':[str(p) for p in layers], 'all_owned_paths':owned_paths,
      'mutated_layer':str(layers[0])}
output.write_text(json.dumps(data,sort_keys=True,indent=2))
PY
find "$baseline" -type f -print0 | sort -z | xargs -0 sha256sum > "$scratch/baseline.after"
cmp -s "$scratch/baseline.before" "$scratch/baseline.after" \
  || loud_fail "in-place/rename adversary mutated immutable baseline"

# Let workers finish and prove restart cleanup converges without touching base.
for _ in $(seq 1 160); do
  done_count=0
  for n in 1 2 3; do
    state=$(wg show "cow-build-$n" --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))' 2>/dev/null || true)
    { [ "$state" = waiting ] || [ "$state" = done ]; } && done_count=$((done_count+1))
  done
  [ "$done_count" = 3 ] && break
  sleep 0.25
done
[ "${done_count:-0}" = 3 ] || loud_fail "overlap workers did not finish"
wg service stop >/dev/null
start_wg_daemon "$project" --max-agents 3 --no-chat-agent --interval 1
for _ in $(seq 1 160); do
  remaining=$(python3 - "$project" <<'PY'
import json, sys
from pathlib import Path
f=Path(sys.argv[1])/'.wg/service/disk/owned-caches.json'
if not f.exists(): print(0)
else: print(sum(c.get('task_id','').startswith('cow-build-') for c in json.loads(f.read_text()).get('caches',[])))
PY
)
  [ "$remaining" = 0 ] && break
  sleep 0.25
done
[ "${remaining:-1}" = 0 ] || loud_fail "restart did not reap terminal private layers"
python3 - "$scratch/owned-paths.json" <<'PY' || loud_fail "restart removed ownership rows but left physical worker layers"
import json,sys
from pathlib import Path
paths=[Path(p) for p in json.loads(Path(sys.argv[1]).read_text())]
left=[str(p) for p in paths if p.exists()]
assert not left, left
PY
find "$baseline" -type f -print0 | sort -z | xargs -0 sha256sum > "$scratch/baseline.restart"
cmp -s "$scratch/baseline.before" "$scratch/baseline.restart" \
  || loud_fail "restart cleanup changed immutable baseline"

receipt_json=$(cat "$receipt")
metrics=$(cat "$scratch/metrics.json")
printf '%s\n' "PASS: candidate-build-receipt-sha256=$receipt_sha receipt=$receipt_json exact_path=$WG_BIN metrics=$metrics; stale substitution rejected, private inodes isolated mutations, and baseline/restart bytes remained immutable"
