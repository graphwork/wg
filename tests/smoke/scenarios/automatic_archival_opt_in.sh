#!/usr/bin/env bash
# Candidate-binary clean-room proof that task-graph automatic archival is
# disabled by default, held plans are cancelled by retention=0, and an
# explicit opt-in archives only the exact operator-reviewed batch.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
unset WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER WG_SPAWN_EPOCH
command -v cargo >/dev/null 2>&1 || loud_skip "MISSING CARGO" "cargo is required"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"

scratch=$(make_scratch)
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then
  WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
else
  export CARGO_TARGET_DIR="$scratch/candidate-target"
  (cd "$REPO_ROOT" && CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo build --quiet --bin wg)
  WG_BIN="$CARGO_TARGET_DIR/debug/wg"
fi
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"
export PATH="$(dirname "$WG_BIN"):$PATH"

# No host profile, graph, or config may leak into this proof.
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export WG_GLOBAL_DIR="$HOME/.wg"
mkdir -p "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR"
cd "$scratch"

"$WG_BIN" init --no-agency -x shell >/dev/null
G="$scratch/.wg"
[[ -d "$G" ]] || loud_fail "new graph missing at explicit dir $G"
"$WG_BIN" --dir "$G" config --local --model claude:opus --no-reload >/dev/null
"$WG_BIN" --dir "$G" config set agency.auto_assign false --no-reload >/dev/null
"$WG_BIN" --dir "$G" config set agency.auto_evaluate false --no-reload >/dev/null

# Upgrade simulation: an older/minimal config may not mention the key at all.
# Its missing value must resolve to the new safe default, never to an age
# trigger introduced by the candidate build.
python3 - "$G/config.toml" <<'PY'
import pathlib,sys
p=pathlib.Path(sys.argv[1])
lines=[line for line in p.read_text().splitlines() if not line.strip().startswith("archive_retention_days =")]
p.write_text("\n".join(lines)+"\n")
PY
resolved=$("$WG_BIN" --dir "$G" --json config get dispatcher.archive_retention_days)
python3 - "$resolved" <<'PY'
import json,sys
v=json.loads(sys.argv[1])
assert v["value"] == 0, v
assert v["source"] == "default", v
PY

for id in reviewed-a reviewed-b; do
  "$WG_BIN" --dir "$G" add "Historical evidence $id" --id "$id" >/dev/null
done
"$WG_BIN" --dir "$G" add "Still visible open task" --id visible-open >/dev/null
python3 - "$G/graph.jsonl" <<'PY'
import datetime,json,os,sys
p=sys.argv[1]
old=(datetime.datetime.now(datetime.timezone.utc)-datetime.timedelta(days=40)).isoformat()
rows=[]
for line in open(p):
    if not line.strip(): continue
    obj=json.loads(line)
    if obj.get("kind")=="task" and obj.get("id","").startswith("reviewed-"):
        obj["status"]="done"
        obj["created_at"]=old
        obj["completed_at"]=old
        obj["log"]=[{"timestamp":old,"actor":"operator","user":"smoke","message":"preserved evidence"}]
    rows.append(obj)
tmp=p+".tmp"
with open(tmp,"w") as f:
    for obj in rows: f.write(json.dumps(obj,separators=(",",":"))+"\n")
os.replace(tmp,p)
PY
cp "$G/graph.jsonl" "$scratch/default-before.jsonl"

# Fresh start and restart (candidate upgrade simulation) are both inert at 0.
start_wg_daemon "$scratch" --interval 1 --max-agents 1 --no-chat-agent --no-supervise
sleep 2
status=$("$WG_BIN" --dir "$G" service status)
[[ "$status" == *"Automatic archival: disabled (retention=0; visible history preserved)"* ]] \
  || loud_fail "service status did not clearly report disabled archival: $status"
dry_disabled=$("$WG_BIN" --dir "$G" archive auto --dry-run)
[[ "$dry_disabled" == *"Automatic archival is disabled (coordinator.archive_retention_days=0)"* ]] \
  || loud_fail "archive auto dry-run did not clearly report disabled archival: $dry_disabled"
"$WG_BIN" --dir "$G" service stop >/dev/null
start_wg_daemon "$scratch" --interval 1 --max-agents 1 --no-chat-agent --no-supervise
sleep 2
cmp -s "$scratch/default-before.jsonl" "$G/graph.jsonl" \
  || loud_fail "default-disabled restart rewrote visible task history"
[[ ! -s "$G/archive.jsonl" ]] || loud_fail "default-disabled restart archived historical tasks"

# Enter a real hold, then disable via live config reload. The pending batch
# must be cleared/neutralized before the candidate daemon is restarted.
"$WG_BIN" --dir "$G" config set coordinator.archive_retention_days 1 >/dev/null
for _ in $(seq 1 100); do
  if [[ -f "$G/archive-auto-state.json" ]] && python3 - "$G/archive-auto-state.json" <<'PY' >/dev/null 2>&1
import json,sys
v=json.load(open(sys.argv[1]))
assert len(v.get("pending",{}).get("tasks",[]))==2
PY
  then break; fi
  sleep 0.1
done
held=$("$WG_BIN" --dir "$G" --json archive auto --dry-run)
python3 - "$held" <<'PY'
import json,sys
v=json.loads(sys.argv[1])
assert v["pending"] is True,v
assert v["task_ids"]==["reviewed-a","reviewed-b"],v
PY
cp "$G/graph.jsonl" "$scratch/held-before-disable.jsonl"
"$WG_BIN" --dir "$G" config set coordinator.archive_retention_days 0 >/dev/null
for _ in $(seq 1 100); do
  python3 - "$G/archive-auto-state.json" <<'PY' >/dev/null 2>&1 && break
import json,sys
v=json.load(open(sys.argv[1]))
assert v["retention_days"]==0,v
assert v["pending"] is None,v
PY
  sleep 0.1
done
# The attended dry-run is an additional immediate cancellation boundary and
# must stay non-destructive even if the reload race has not settled.
"$WG_BIN" --dir "$G" archive auto --dry-run >/dev/null
"$WG_BIN" --dir "$G" service stop >/dev/null
start_wg_daemon "$scratch" --interval 1 --max-agents 1 --no-chat-agent --no-supervise
sleep 2
cmp -s "$scratch/held-before-disable.jsonl" "$G/graph.jsonl" \
  || loud_fail "disabling a held batch changed visible task records/states"
[[ ! -s "$G/archive.jsonl" ]] || loud_fail "disabling a held batch archived tasks"
python3 - "$G" <<'PY'
import json,os,sys
G=sys.argv[1]
rows={o["id"]:o for o in map(json.loads,open(G+"/graph.jsonl")) if o.get("kind")=="task"}
assert rows["reviewed-a"]["status"]=="done",rows
assert rows["reviewed-b"]["status"]=="done",rows
assert rows["visible-open"]["status"]=="open",rows
state=json.load(open(G+"/archive-auto-state.json"))
assert state["retention_days"]==0 and state["pending"] is None,state
PY

# Deliberately opt back in. Review the exact two-task batch, stop the daemon,
# add another eligible historical record after review, then confirm. The new
# record is not in the digest-pinned plan and must remain visible.
"$WG_BIN" --dir "$G" service stop >/dev/null
"$WG_BIN" --dir "$G" config set coordinator.archive_retention_days 1 --no-reload >/dev/null
start_wg_daemon "$scratch" --interval 1 --max-agents 1 --no-chat-agent --no-supervise
for _ in $(seq 1 100); do
  reviewed=$("$WG_BIN" --dir "$G" --json archive auto --dry-run 2>/dev/null || true)
  if python3 - "$reviewed" <<'PY' >/dev/null 2>&1
import json,sys
v=json.loads(sys.argv[1])
assert v["pending"] is True and v["task_ids"]==["reviewed-a","reviewed-b"]
PY
  then break; fi
  sleep 0.1
done
python3 - "$reviewed" <<'PY'
import json,sys
v=json.loads(sys.argv[1])
assert v["task_ids"]==["reviewed-a","reviewed-b"],v
PY
"$WG_BIN" --dir "$G" service stop >/dev/null
"$WG_BIN" --dir "$G" add "Late unreviewed evidence" --id late-unreviewed >/dev/null
python3 - "$G/graph.jsonl" <<'PY'
import datetime,json,os,sys
p=sys.argv[1]
old=(datetime.datetime.now(datetime.timezone.utc)-datetime.timedelta(days=40)).isoformat()
rows=[]
for line in open(p):
    if not line.strip(): continue
    obj=json.loads(line)
    if obj.get("kind")=="task" and obj.get("id")=="late-unreviewed":
        obj["status"]="abandoned"
        obj["created_at"]=old
        obj["completed_at"]=None
    rows.append(obj)
tmp=p+".tmp"
with open(tmp,"w") as f:
    for obj in rows: f.write(json.dumps(obj,separators=(",",":"))+"\n")
os.replace(tmp,p)
PY
confirmed=$("$WG_BIN" --dir "$G" --json archive auto --confirm)
python3 - "$confirmed" "$G" <<'PY'
import json,sys
v=json.loads(sys.argv[1]); G=sys.argv[2]
assert v["archived_count"]==2,v
assert v["task_ids"]==["reviewed-a","reviewed-b"],v
arch=[json.loads(x)["id"] for x in open(G+"/archive.jsonl") if x.strip()]
assert arch==["reviewed-a","reviewed-b"],arch
active={o.get("id"):o for o in map(json.loads,open(G+"/graph.jsonl")) if o.get("kind")=="task"}
assert active["late-unreviewed"]["status"]=="abandoned",active
assert active["visible-open"]["status"]=="open",active
PY
manual=$("$WG_BIN" --dir "$G" archive late-unreviewed --dry-run)
[[ "$manual" == *"late-unreviewed"* ]] || loud_fail "manual archival dry-run is no longer available: $manual"
[[ $(wc -l <"$G/archive.jsonl") -eq 2 ]] || loud_fail "manual dry-run mutated the reviewed archive batch"

echo "automatic archival opt-in passed"
