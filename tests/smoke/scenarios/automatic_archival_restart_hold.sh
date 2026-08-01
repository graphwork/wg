#!/usr/bin/env bash
# Candidate-binary attended regression for surprise restart-time archival.
# A legacy daemon state + month-old terminal graph must become an exact,
# visible archival hold; only the persisted confirmation batch may move.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
unset WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER WG_SPAWN_EPOCH
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required for the attended TUI proof"

scratch=$(make_scratch)
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then
  WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
else
  # `wg done` runs this after the project-required candidate install. Avoid a
  # second full Rust link inside the scenario (which can exceed the smoke
  # timeout and tests build latency rather than archival behavior).
  WG_BIN="$(command -v wg 2>/dev/null || true)"
fi
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"
export PATH="$(dirname "$WG_BIN"):$PATH"

export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export WG_GLOBAL_DIR="$HOME/.wg"
mkdir -p "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR"
cd "$scratch"

"$WG_BIN" init --no-agency -x shell >/dev/null
G="$scratch/.wg"
# No worker is dispatchable, but service startup still requires an explicit
# handler-first route selection. Native CLI auth is never touched here.
"$WG_BIN" --dir "$G" config --local --model claude:opus --no-reload >/dev/null
"$WG_BIN" --dir "$G" config set coordinator.archive_retention_days 7 --no-reload >/dev/null
"$WG_BIN" --dir "$G" config set agency.auto_assign false --no-reload >/dev/null
"$WG_BIN" --dir "$G" config set agency.auto_evaluate false --no-reload >/dev/null

for id in historical-a historical-b historical-c; do
  "$WG_BIN" --dir "$G" add "Historical $id" --id "$id" >/dev/null
done
"$WG_BIN" --dir "$G" add "Active dependent" --id active --after historical-a >/dev/null

# Canonical month-old terminal records, including logs and dependency caches.
python3 - "$G/graph.jsonl" <<'PY'
import datetime, json, os, sys
path=sys.argv[1]
old=(datetime.datetime.now(datetime.timezone.utc)-datetime.timedelta(days=38)).isoformat()
out=[]
for line in open(path):
    if not line.strip(): continue
    obj=json.loads(line)
    if obj.get("kind")=="task" and obj.get("id", "").startswith("historical-"):
        obj["status"]="done"
        obj["completed_at"]=old
        obj["created_at"]=old
        obj["log"]=[{"timestamp":old,"actor":"legacy","user":"operator","message":"exact historical log"}]
    if obj.get("kind")=="task" and obj.get("id")=="historical-a":
        obj["before"]=["active"]
    if obj.get("kind")=="task" and obj.get("id")=="active":
        obj["paused"]=True
        obj["after"]=["historical-a"]
    out.append(obj)
tmp=path+".tmp"
with open(tmp,"w") as f:
    for obj in out: f.write(json.dumps(obj,separators=(",",":"))+"\n")
os.replace(tmp,path)
PY

snapshot_tasks() {
  python3 - "$G/graph.jsonl" "$1" <<'PY'
import json,sys
path,out=sys.argv[1:]
rows=[]
for line in open(path):
    obj=json.loads(line)
    if obj.get("kind")=="task" and obj.get("id","").startswith("historical-"):
        rows.append(obj)
open(out,"w").write(json.dumps(sorted(rows,key=lambda x:x["id"]),sort_keys=True,separators=(",",":")))
PY
}
snapshot_tasks "$scratch/historical.before.json"

# Simulate the unverified/legacy service record seen during the upgrade.
mkdir -p "$G/service"
cat >"$G/service/state.json" <<EOF
{"pid":999999,"socket_path":"$G/service/daemon.sock","started_at":"2026-06-24T00:00:00Z"}
EOF

start_wg_daemon "$scratch" --interval 1 --max-agents 1 --no-chat-agent --no-supervise
for _ in $(seq 1 100); do
  [[ -f "$G/archive-auto-state.json" ]] && break
  sleep 0.1
done
[[ -f "$G/archive-auto-state.json" ]] || loud_fail "daemon did not persist archival hold"

# Restart/upgrade safety: all three complete records remain active and archive is empty.
python3 - "$G" <<'PY'
import json,os,sys
G=sys.argv[1]
active=[]
for line in open(G+"/graph.jsonl"):
    obj=json.loads(line)
    if obj.get("kind")=="task" and obj.get("id","").startswith("historical-"):
        active.append(obj["id"])
assert sorted(active)==["historical-a","historical-b","historical-c"],active
p=G+"/archive.jsonl"
assert not os.path.exists(p) or not open(p).read().strip(),"restart archived before confirmation"
PY

status=$("$WG_BIN" --dir "$G" service status)
[[ "$status" == *"PENDING OPERATOR CONFIRMATION (3 task(s)"* ]] || loud_fail "service status omitted hold: $status"
[[ "$status" == *"wg archive auto --dry-run"* && "$status" == *"wg archive auto --confirm"* ]] \
  || loud_fail "service status omitted actionable commands: $status"

dry=$("$WG_BIN" --dir "$G" --json archive auto --dry-run)
python3 - "$dry" <<'PY'
import json,sys
v=json.loads(sys.argv[1])
assert v["pending"] is True,v
assert v["pending_count"]==3,v
assert v["task_ids"]==["historical-a","historical-b","historical-c"],v
PY

# Actual attended surface: the real TUI pulse and service-details modal explain the hold.
session="wg-archive-hold-$$"
tmux new-session -d -s "$session" -x 180 -y 38 \
  "env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' TERM=xterm-256color '$WG_BIN' --dir '$G' tui"
tmux set-option -t "$session" mouse on
tmux resize-window -t "$session" -x 180 -y 38
capture() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
for _ in $(seq 1 200); do capture | grep -Fq "ARCHIVE-HOLD:3" && break; sleep 0.05; done
capture | grep -Fq "ARCHIVE-HOLD:3" || loud_fail "TUI workspace pulse omitted archival hold: $(capture | tr '\n' '|')"
needle=$(basename "$scratch")
xy=$(capture | python3 -c 'import sys
needle=sys.argv[1]
for y,row in enumerate(sys.stdin.read().splitlines(),1):
    x=row.find(needle)
    if x>=0: print(x+1,y); raise SystemExit(0)
raise SystemExit(1)' "$needle") || loud_fail "TUI project identity was not clickable"
read -r click_x click_y <<<"$xy"
tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sM\033[<0;%s;%sm' "$click_x" "$click_y" "$click_x" "$click_y")"
for _ in $(seq 1 100); do capture | grep -Fq "PENDING OPERATOR CONFIRMATION" && break; sleep 0.05; done
screen=$(capture)
[[ "$screen" == *"PENDING OPERATOR CONFIRMATION"* && "$screen" == *"wg archive auto --dry-run"* && "$screen" == *"wg archive auto --confirm"* ]] \
  || loud_fail "TUI service details omitted hold actions: $(echo "$screen" | tr '\n' '|')"
tmux kill-session -t "$session" >/dev/null 2>&1 || true

confirm=$("$WG_BIN" --dir "$G" --json archive auto --confirm)
python3 - "$confirm" <<'PY'
import json,sys
v=json.loads(sys.argv[1])
assert v["archived_count"]==3,v
assert v["task_ids"]==["historical-a","historical-b","historical-c"],v
PY
python3 - "$G" <<'PY'
import json,sys
G=sys.argv[1]
arch=[json.loads(x)["id"] for x in open(G+"/archive.jsonl") if x.strip()]
assert arch==["historical-a","historical-b","historical-c"],arch
state=json.load(open(G+"/archive-auto-state.json"))
assert state["pending"] is None,state
assert state["last_confirmed_cutoff"],state
PY
# Re-confirm cannot duplicate the exact batch.
if "$WG_BIN" --dir "$G" archive auto --confirm >/dev/null 2>&1; then
  loud_fail "second confirmation unexpectedly reapplied the batch"
fi
[[ $(wc -l <"$G/archive.jsonl") -eq 3 ]] || loud_fail "confirmation duplicated archive records"

"$WG_BIN" --dir "$G" archive --undo >/dev/null
snapshot_tasks "$scratch/historical.after.json"
cmp -s "$scratch/historical.before.json" "$scratch/historical.after.json" \
  || loud_fail "undo did not restore exact historical task status/log/dependencies"
[[ ! -s "$G/archive.jsonl" ]] || loud_fail "undo left confirmed records in archive"

# Stop, then move the acknowledged cutoff back only one hour and add exactly
# one task in that newly-eligible interval. Restart may archive that increment,
# but must not reprocess the restored month-old history before the watermark.
"$WG_BIN" --dir "$G" service stop >/dev/null
python3 - "$G/archive-auto-state.json" "$G/graph.jsonl" <<'PY'
import datetime,json,os,sys
state_path,graph_path=sys.argv[1:]
now=datetime.datetime.now(datetime.timezone.utc)
state=json.load(open(state_path))
state["last_confirmed_cutoff"]=(now-datetime.timedelta(days=7,hours=1)).isoformat()
state["pending"]=None
open(state_path,"w").write(json.dumps(state,indent=2))
# Clone a canonical task shell and create a terminal record 30m into the new interval.
rows=[json.loads(x) for x in open(graph_path) if x.strip()]
template=next(x for x in rows if x.get("kind")=="task" and x.get("id")=="historical-a")
inc=json.loads(json.dumps(template))
inc["id"]="incremental"
inc["title"]="Newly eligible increment"
inc["before"]=[]; inc["after"]=[]; inc["log"]=[]
inc["completed_at"]=(now-datetime.timedelta(days=7,minutes=30)).isoformat()
inc["created_at"]=inc["completed_at"]
rows.append(inc)
with open(graph_path,"w") as f:
    for x in rows: f.write(json.dumps(x,separators=(",",":"))+"\n")
PY
start_wg_daemon "$scratch" --interval 1 --max-agents 1 --no-chat-agent --no-supervise
for _ in $(seq 1 100); do
  python3 - "$G/archive.jsonl" <<'PY' >/dev/null 2>&1 && break
import json,sys
assert [json.loads(x)["id"] for x in open(sys.argv[1]) if x.strip()]==["incremental"]
PY
  sleep 0.1
done
python3 - "$G" <<'PY'
import json,sys
G=sys.argv[1]
active={x.get("id") for x in map(json.loads,open(G+"/graph.jsonl")) if x.get("kind")=="task"}
assert {"historical-a","historical-b","historical-c"} <= active,active
arch=[json.loads(x)["id"] for x in open(G+"/archive.jsonl") if x.strip()]
assert arch==["incremental"],arch
PY

# Explicit retention=0 remains a hard disable across another restart.
"$WG_BIN" --dir "$G" service stop >/dev/null
"$WG_BIN" --dir "$G" config set coordinator.archive_retention_days 0 --no-reload >/dev/null
"$WG_BIN" --dir "$G" archive --undo >/dev/null
start_wg_daemon "$scratch" --interval 1 --max-agents 1 --no-chat-agent --no-supervise
sleep 2
[[ ! -s "$G/archive.jsonl" ]] || loud_fail "retention=0 archived a task"
zero_status=$("$WG_BIN" --dir "$G" service status)
[[ "$zero_status" == *"Automatic archival: disabled"* ]] || loud_fail "retention=0 not visible in status: $zero_status"

echo "automatic archival restart hold passed"
