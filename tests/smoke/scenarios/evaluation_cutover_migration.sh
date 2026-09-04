#!/usr/bin/env bash
# Versioned retirement of stale synthetic evaluation rows.
set -euo pipefail
source "$(dirname "$0")/_helpers.sh"
: "${WG_BIN:?smoke harness must provide candidate WG_BIN}"
[[ -x $WG_BIN ]] || loud_fail "candidate WG_BIN is not executable: $WG_BIN"

# Unix-domain sockets cap path length; project-local Cargo scratch paths can
# be very deep, so keep this daemon fixture under a short helper-owned root.
export WG_SMOKE_ROOT="${WG_EVAL_CUTOVER_SMOKE_ROOT:-/tmp/wgsmoke-evaluation-cutover-$$}"
scratch=$(make_scratch "evaluation-cutover")
project="$scratch/project"
home="$scratch/home"
mkdir -p "$project" "$home"
export HOME="$home" XDG_CONFIG_HOME="$home/.config"
unset WG_AGENT_ID WG_TASK_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_PROJECT_ROOT WG_WORKTREE_PATH WG_DIR
git -C "$project" init -q -b main
git -C "$project" config user.email cutover@test.invalid
git -C "$project" config user.name Cutover
printf 'base\n' >"$project/README.md"
git -C "$project" add README.md
git -C "$project" commit -qm base
cd "$project"
wgrun() { env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC WG_DIR="$project/.wg" "$WG_BIN" "$@"; }
restore_legacy_evaluator() {
  python3 - "$project/.wg/graph.jsonl" "$project/.wg/lifecycle/events.jsonl" <<'PY'
import json,os,sys

graph_path,ledger_path=sys.argv[1:]
rows=[json.loads(line) for line in open(graph_path)]
legacy=next(row for row in rows if row.get('id')=='.evaluate-reviewed-source')
legacy['status']='open'
legacy.pop('completed_at',None)
legacy.pop('lifecycle',None)
with open(graph_path+'.tmp','w') as out:
    for row in rows:
        out.write(json.dumps(row,separators=(',',':'))+'\n')
os.replace(graph_path+'.tmp',graph_path)
if os.path.exists(ledger_path):
    events=[json.loads(line) for line in open(ledger_path)]
    with open(ledger_path+'.tmp','w') as out:
        for event in events:
            if event.get('event',{}).get('task_id')!='.evaluate-reviewed-source':
                out.write(json.dumps(event,separators=(',',':'))+'\n')
    os.replace(ledger_path+'.tmp',ledger_path)
PY
}

wgrun init --no-agency --route pi --model pi:test:unused >/dev/null
wgrun config set dispatcher.settling_delay_ms 0 >/dev/null
wgrun config set dispatcher.worktree_isolation false >/dev/null
wgrun add 'Reviewed source' --id reviewed-source >/dev/null
wgrun add 'Stale evaluator' --id .evaluate-reviewed-source --after reviewed-source --priority 100 >/dev/null
wgrun add 'Downstream work' --id downstream --after reviewed-source >/dev/null
wgrun publish reviewed-source --only >/dev/null
wgrun publish .evaluate-reviewed-source --only >/dev/null
wgrun publish downstream --only >/dev/null

# Bug 2: repeated ticks must diagnose once and must not priority-promote/list
# the retired row on every tick. Current publish eagerly abandons synthetic
# rows, so restore the legacy Open projection after pausing the source. This
# leaves no legitimate work for the one-shot tick to dispatch.
wgrun pause reviewed-source >/dev/null
restore_legacy_evaluator
tick1=$(wgrun service tick --max-agents 1 2>&1)
tick2=$(wgrun service tick --max-agents 1 2>&1)
grep -q 'excluded before priority ordering' <<<"$tick1" \
  || loud_fail "first tick omitted bounded retired-row notice: $tick1"
if grep -qE 'Priority (bump|inheritance|dispatch order).*\.evaluate-reviewed-source|excluded before priority ordering' <<<"$tick2"; then
  loud_fail "second tick repeated retired-row promotion/diagnostic: $tick2"
fi
wgrun resume reviewed-source --only >/dev/null

# Produce a real typed, immutable operator receipt rather than blessing edited
# graph bytes. Modern completion retires the synthetic row, so restore only the
# legacy Open projection and remove its lifecycle event to model an upgraded
# pre-cutover graph faithfully.
wgrun done reviewed-source --operator-accept --reason 'fixture receipt for upgraded graph' >/dev/null
restore_legacy_evaluator

# Bug 1: the stale evaluator has no authority even before migration.
wgrun ready --json | grep -q 'downstream' \
  || loud_fail 'stale evaluator blocked receipt-backed Done/Landed source before migration'
# Bug 3: arbitrary high manual score is a different store and cannot forge
# candidate acceptance or clear the obsolete graph-row gate.
wgrun evaluate record --task reviewed-source --score 1.0 --source manual --notes unrelated >/dev/null
wgrun ready --json | grep -q 'downstream' \
  || loud_fail 'unrelated score restored obsolete evaluator authority'
show_before=$(wgrun show reviewed-source)
grep -q 'already non-authoritative' <<<"$show_before" \
  || loud_fail "show did not explain pre-migration semantics: $show_before"

# The removed adjudication escape hatch cannot be re-authorized by toggling a
# caller-controlled worker environment variable.
if env WG_AGENT_ID=forged-worker WG_DIR="$project/.wg" "$WG_BIN" migrate evaluation-cutover --accept reviewed-source 2>/dev/null; then
  loud_fail 'worker-controlled environment authorized removed cutover adjudication'
fi
if env -u WG_AGENT_ID WG_DIR="$project/.wg" "$WG_BIN" migrate evaluation-cutover --accept reviewed-source 2>/dev/null; then
  loud_fail 'environment absence restored removed cutover adjudication'
fi

cp "$project/.wg/graph.jsonl" "$scratch/pre-migrate.graph.jsonl"
dry=$(wgrun migrate evaluation-cutover --dry-run --json)
grep -q '"dry_run": true' <<<"$dry" || loud_fail "dry-run output malformed: $dry"
grep -q '".evaluate-reviewed-source"' <<<"$dry" || loud_fail "dry-run omitted row: $dry"
cmp -s "$project/.wg/graph.jsonl" "$scratch/pre-migrate.graph.jsonl" \
  || loud_fail 'dry-run changed graph bytes'
[[ ! -d $project/.wg/migrations/evaluation-cutover-v1/backups ]] \
  || loud_fail 'dry-run created a backup/write'

apply=$(wgrun migrate evaluation-cutover --json)
backup=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["backup_path"])' <<<"$apply")
[[ -f $backup ]] || loud_fail "migration backup missing: $apply"
cmp -s "$backup" "$scratch/pre-migrate.graph.jsonl" \
  || loud_fail 'backup did not preserve exact pre-migration graph bytes'
show_after=$(wgrun show .evaluate-reviewed-source)
grep -q 'inert historical evidence' <<<"$show_after" \
  || loud_fail "show omitted inert historical condition: $show_after"
grep -q 'fixture receipt for upgraded graph' "$backup" \
  || loud_fail 'receipt-backed source history missing from backup'
find "$project/.wg/agency/evaluations" -type f -print0 | xargs -0 grep -Eq '"score"[[:space:]]*:[[:space:]]*1(\.0)?' \
  || loud_fail 'manual evaluation bytes were not preserved'
wgrun ready --json | grep -q 'downstream' \
  || loud_fail 'supported migration did not release downstream'

# Idempotence: replay is a byte no-op and creates no second backup.
cp "$project/.wg/graph.jsonl" "$scratch/once.graph.jsonl"
second=$(wgrun migrate evaluation-cutover --json)
cmp -s "$project/.wg/graph.jsonl" "$scratch/once.graph.jsonl" \
  || loud_fail 'second migration changed graph bytes'
[[ $(find "$project/.wg/migrations/evaluation-cutover-v1/backups" -type f | wc -l) -eq 1 ]] \
  || loud_fail 'idempotent replay created another backup'

# The legacy row remains visible and its original status survives in active graph.
python3 - "$project/.wg/graph.jsonl" <<'PY'
import json,sys
rows=[json.loads(x) for x in open(sys.argv[1])]
e=next(x for x in rows if x.get('id')=='.evaluate-reviewed-source')
assert 'evaluation-cutover:v1:historical-inert' in e.get('tags',[]),e
assert e['status']=='open',e
PY

echo 'PASS: evaluation-cutover v1 preserved exact history, removed environment-based adjudication, bounded retired-row diagnostics, and kept Done/Landed authoritative before migration'
