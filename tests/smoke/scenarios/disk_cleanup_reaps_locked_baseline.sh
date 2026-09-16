#!/usr/bin/env bash
# Regression for fix-wg-disk: a stale explicitly-owned cache that contains a
# WG-published `.wg-owned-baseline` tree locked read-only (555 directories)
# must be reaped by `wg disk cleanup --execute`, not silently preserved with
# "remove failed: Permission denied (os error 13)".
#
# Pre-fix behavior (reproduced on the base revision): the dry run lists the
# cache as eligible ("every owner passes all removal guards") and the execute
# run reports reaped=0 / freed=0 while preserving the cache forever, so the
# stale count in `wg disk doctor` never drops.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON" "python3 required for JSON assertions"

scratch=$(make_scratch)
project="$scratch/project"
mkdir -p "$project"
cd "$project"
wg init --no-agency >/dev/null
cat > .wg/config.toml <<'EOF'
[agency]
auto_assign = false
auto_evaluate = false
EOF

# Terminal task + dead agent in the registry, exactly the stale-owner state
# that must authorize reaping. The graph/registry rows are written directly
# (the scenario only needs a terminal owner, not a full completion flow).
python3 - <<'PY'
import json, os
from pathlib import Path
wg = Path('.wg').resolve()
task = {
    'kind': 'task', 'id': 'stale-locked-baseline-owner',
    'title': 'stale locked-baseline cache owner', 'status': 'done',
    'created_at': '2020-01-01T00:00:00Z',
    'completed_at': '2020-01-01T00:00:01Z',
}
(wg / 'graph.jsonl').write_text(json.dumps(task) + '\n')
os.makedirs(wg / 'service', exist_ok=True)
registry = {
    'agents': {
        'agent-stale-locked-baseline': {
            'id': 'agent-stale-locked-baseline', 'pid': 999999999,
            'task_id': 'stale-locked-baseline-owner', 'executor': 'shell',
            'started_at': '2020-01-01T00:00:00Z',
            'last_heartbeat': '2020-01-01T00:00:00Z',
            'status': 'done', 'output_file': str(wg / 'agents/stale/output.log'),
            'completed_at': '2020-01-01T00:00:01Z',
            'worktree_path': None,
        }
    },
    'next_agent_id': 100,
}
(wg / 'service/registry.json').write_text(json.dumps(registry))
PY

# Build the owned cache: a stale target dir containing a WG-published
# baseline locked read-only (555 dirs / 444 files) with the ownership marker
# WG itself writes on promotion.
owned_cache="$scratch/stale-locked-baseline-cache"
python3 - "$owned_cache" <<'PY'
import json, os, sys
from pathlib import Path
cache = Path(sys.argv[1]).resolve()
task_id = 'stale-locked-baseline-owner'
baseline = cache / 'baselines' / ('d' * 64)
nested = baseline / 'target' / 'debug'
nested.mkdir(parents=True)
(nested / 'lib.rmeta').write_bytes(b'x' * 8192)
(baseline / '.wg-owned-baseline').write_bytes(b'wg-owned Cargo baseline\n')
for root, dirs, files in os.walk(baseline, topdown=False):
    for f in files:
        os.chmod(os.path.join(root, f), 0o444)
    for d in dirs:
        os.chmod(os.path.join(root, d), 0o555)
os.chmod(baseline, 0o555)

wg = Path('.wg').resolve()
registry_path = wg / 'service/registry.json'
registry = json.loads(registry_path.read_text()) if registry_path.exists() else {'agents': {}, 'next_agent_id': 100}
os.makedirs(wg / 'service/disk', exist_ok=True)
registry['agents']['agent-stale-locked-baseline'] = {
    'id': 'agent-stale-locked-baseline', 'pid': 999999999,
    'task_id': task_id, 'executor': 'shell',
    'started_at': '2020-01-01T00:00:00Z',
    'last_heartbeat': '2020-01-01T00:00:00Z',
    'status': 'done', 'output_file': str(wg / 'agents/stale/output.log'),
    'completed_at': '2020-01-01T00:00:01Z',
    'worktree_path': None,
}
(wg / 'service/registry.json').write_text(json.dumps(registry))
ownership = {
    'schema': 1,
    'caches': [{
        'path': str(cache), 'kind': 'cargo-target',
        'task_id': task_id,
        'agent_id': 'agent-stale-locked-baseline', 'pid': 999999999,
        'pid_start_epoch': None, 'mount_id': f'dev:{cache.stat().st_dev}',
        'created_at': '2020-01-01T00:00:00Z',
        'lease_expires_at': '2020-01-01T00:00:01Z',
    }],
}
(wg / 'service/disk/owned-caches.json').write_text(json.dumps(ownership))
PY

# Dry run: the cache must surface as stale-eligible (owner is terminal/dead).
dry=$(wg disk cleanup --json)
echo "$dry" | python3 -c '
import json, sys
report = json.load(sys.stdin)
assert report["considered"] == 1, report
assert len(report["eligible"]) == 1, report
' || loud_fail "dry run did not report the stale locked-baseline cache as eligible: $dry"

# Execute: the WG-owned locked baseline must be unlocked and the cache reaped.
applied=$(wg disk cleanup --execute --json)
echo "$applied" | python3 -c '
import json, sys
report = json.load(sys.stdin)
assert report["reaped"] == 1, report
assert report["bytes_freed"] > 0, report
assert report["preserved"] == [], report
' || loud_fail "execute did not reap the locked-baseline cache: $applied"
[ ! -e "$owned_cache" ] \
  || loud_fail "stale owned cache containing a WG-owned 555 baseline was preserved after execute"

# Idempotence: the ownership row is retired; a second pass finds nothing.
second=$(wg disk cleanup --execute --json)
echo "$second" | python3 -c '
import json, sys
report = json.load(sys.stdin)
assert report["considered"] == 0, report
assert report["reaped"] == 0, report
' || loud_fail "second cleanup pass was not idempotent: $second"

echo "PASS: stale owned cache containing a WG-owned 555 .wg-owned-baseline tree was unlocked and reaped; dry-run eligibility matched the execute outcome and the pass was idempotent"
