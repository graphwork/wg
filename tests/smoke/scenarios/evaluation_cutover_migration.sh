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

wgrun init --no-agency --route pi --model pi:test:unused >/dev/null
wgrun config set dispatcher.settling_delay_ms 0 >/dev/null
wgrun config set dispatcher.worktree_isolation false >/dev/null
wgrun add 'Reviewed source' --id reviewed-source >/dev/null
wgrun add 'Stale evaluator' --id .evaluate-reviewed-source --after reviewed-source --priority 100 >/dev/null
marker="$project/downstream-dispatched"
wgrun add 'Downstream work' --id downstream --after reviewed-source \
  --exec "printf dispatched > '$marker'" --exec-mode shell >/dev/null
wgrun publish reviewed-source --only >/dev/null
wgrun publish .evaluate-reviewed-source --only >/dev/null
wgrun publish downstream --only >/dev/null

# Fixture boundary: model a supported old graph whose source was reviewed/Done
# while its pre-receipt evaluator row remained Open. No migration step edits it.
python3 - "$project/.wg/graph.jsonl" <<'PY'
import json,sys
p=sys.argv[1]
rows=[]
for line in open(p):
    row=json.loads(line)
    if row.get('id') == 'reviewed-source':
        row['status']='done'
        row['completion_disposition']='landed'
        row['completed_at']='2026-01-01T00:00:00Z'
        row.setdefault('log',[]).append({'timestamp':'2026-01-01T00:00:00Z','actor':'legacy-review','message':'exact historical review passed'})
    if row.get('id') == '.evaluate-reviewed-source':
        row['status']='open'
        row['paused']=False
        row['completed_at']=None
    rows.append(row)
with open(p,'w') as f:
    for row in rows: f.write(json.dumps(row,separators=(',',':'))+'\n')
PY

# Bug 1: stale evaluator permanently blocks an ordinary downstream edge.
if wgrun ready --json | grep -q 'downstream'; then
  loud_fail 'stale evaluator did not reproduce the obsolete dependency gate'
fi
# Bug 3: arbitrary high manual score is a different store and cannot forge
# candidate acceptance or clear the obsolete graph-row gate.
wgrun evaluate record --task reviewed-source --score 1.0 --source manual --notes unrelated >/dev/null
if wgrun ready --json | grep -q 'downstream'; then
  loud_fail 'unrelated scored evaluation forged legacy candidate acceptance'
fi
show_before=$(wgrun show reviewed-source)
grep -q 'wg migrate evaluation-cutover' <<<"$show_before" \
  || loud_fail "show omitted supported migration: $show_before"
grep -q 'ordinary `wg evaluate record` scores' <<<"$show_before" \
  || loud_fail "show did not distinguish advisory scores: $show_before"

# Bug 2: repeated ticks must diagnose once and must not priority-promote/list
# the retired row on every tick.
tick1=$(wgrun service tick --max-agents 1 2>&1)
tick2=$(wgrun service tick --max-agents 1 2>&1)
grep -q 'excluded before priority ordering' <<<"$tick1" \
  || loud_fail "first tick omitted bounded retired-row notice: $tick1"
if grep -qE 'Priority (bump|inheritance|dispatch order).*\.evaluate-reviewed-source|excluded before priority ordering' <<<"$tick2"; then
  loud_fail "second tick repeated retired-row promotion/diagnostic: $tick2"
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
grep -q 'exact historical review passed' "$backup" \
  || loud_fail 'legacy row/log bytes missing from backup'
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

# Real dispatcher flow after migration; no rm-dep and no graph editing.
start_wg_daemon "$project" --max-agents 1 --no-coordinator-agent --no-supervise --interval 1
for _ in $(seq 1 200); do
  [[ -f $marker ]] && break
  sleep 0.05
done
[[ -f $marker ]] || loud_fail "downstream was not dispatched: $(tail -100 "$project/.wg/service/daemon.log" 2>/dev/null || true)"

# The legacy row remains visible and its original log survives in active graph.
python3 - "$project/.wg/graph.jsonl" <<'PY'
import json,sys
rows=[json.loads(x) for x in open(sys.argv[1])]
e=next(x for x in rows if x.get('id')=='.evaluate-reviewed-source')
assert 'evaluation-cutover:v1:historical-inert' in e.get('tags',[]),e
assert e['status']=='open',e
PY

echo 'PASS: evaluation-cutover v1 preserved exact history, rejected unrelated scores, bounded retired-row diagnostics, and dispatched downstream without dependency surgery'
