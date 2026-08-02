#!/usr/bin/env bash
# Real generated-wrapper regression for a settled Pi child whose wrapper exits
# before any finish transaction exists. The daemon must resume the exact
# session/worktree and land once without an operator retry.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

scratch="$(make_scratch)"
project="$scratch/project"
home="$scratch/home"
fakebin="$scratch/fakebin"
sync="$scratch/sync"
mkdir -p "$project" "$home/.config/workgraph" "$fakebin" "$sync"
: >"$home/.config/workgraph/config.toml"

cat >"$fakebin/pi" <<'FAKE_PI'
#!/usr/bin/env bash
set -euo pipefail
cat >/dev/null || true
count_file="${FINISH_SYNC:?}/launch-count"
count=0
[[ -f "$count_file" ]] && count=$(cat "$count_file")
count=$((count + 1))
printf '%s\n' "$count" >"$count_file"
session=""
args=("$@")
for ((i=0; i<${#args[@]}; i++)); do
  if [[ ${args[$i]} == --session-id && $((i+1)) -lt ${#args[@]} ]]; then
    session=${args[$((i+1))]}
  fi
done
printf '%s|%s|%s\n' "$count" "$session" "$PWD" >>"$FINISH_SYNC/launches"
mkdir -p convergence
if [[ $count -eq 1 ]]; then
  python3 - <<'PY'
from pathlib import Path
Path('convergence/retained-wip.bin').write_bytes(b'W' * 28672)
PY
  # Durable settlement is semantic-neutral. No wg done/fail transaction exists.
  printf '{"type":"agent_settled"}\n'
  exit 0
fi
[[ -f convergence/retained-wip.bin ]] || exit 91
[[ $(wc -c <convergence/retained-wip.bin) -eq 28672 ]] || exit 92
printf 'same-session continuation observed\n' >convergence/continuation.txt
wg log "$WG_TASK_ID" "same-session continuation retained exact WIP" >/dev/null
wg artifact "$WG_TASK_ID" convergence/continuation.txt >/dev/null
wg done "$WG_TASK_ID" >/dev/null
printf '{"type":"agent_settled"}\n'
FAKE_PI
chmod +x "$fakebin/pi"

(
  cd "$project"
  git init -q -b main
  git config user.email exited-finish@test.invalid
  git config user.name 'Exited Finish Smoke'
  printf 'baseline\n' >README
  git add README && git commit -qm baseline
  HOME="$home" XDG_CONFIG_HOME="$home/.config" wg init --no-agency >/dev/null
)
export HOME="$home" XDG_CONFIG_HOME="$home/.config" PATH="$fakebin:$PATH" FINISH_SYNC="$sync"
wgrun() { (cd "$project" && env -u WG_AGENT_ID -u WG_TASK_ID -u WG_TIER -u WG_EXECUTOR_TYPE wg "$@"); }
wgrun config --local --model pi:openrouter:test/model --poll-interval 1 --auto-assign false --auto-evaluate false --flip-enabled false --no-reload >/dev/null
# Keep the incident bounded instead of waiting the production 30s registry grace.
python3 - "$project/.wg/config.toml" <<'PY'
from pathlib import Path
p=Path(__import__('sys').argv[1]); s=p.read_text()
if 'reaper_grace_seconds = 30' in s:
    s=s.replace('reaper_grace_seconds = 30', 'reaper_grace_seconds = 0', 1)
elif '[agent]' in s:
    s=s.replace('[agent]', '[agent]\nreaper_grace_seconds = 0', 1)
else:
    s += '\n[agent]\nreaper_grace_seconds = 0\n'
p.write_text(s)
PY
# Terminal UX must not call the exact condition "unblocked" while its bounded
# convergence action is pending.
wgrun add why-pending --id why-pending -d $'Pending finish UX.\n\n## Validation\n- concrete convergence action' >/dev/null
wgrun claim why-pending >/dev/null
mkdir -p "$scratch/why-worktree"
wgrun pi-watchdog fixture-init why-pending --worktree "$scratch/why-worktree" --now 0 >/dev/null
# An unrelated process that merely copies the worker environment is not the
# bound wrapper/native child and must remain inert under the attempt fence.
if stale=$(cd "$project" && WG_EXECUTOR_TYPE=pi WG_TASK_ID=why-pending WG_WORKTREE_PATH="$scratch/why-worktree" wg fail why-pending --reason stale-writer 2>&1); then
  loud_fail "unrelated process terminalized the exact attempt: $stale"
fi
grep -q 'stale_process_identity' <<<"$stale" || loud_fail "stale writer rejection was not concrete: $stale"
[[ $(wgrun show why-pending --json | python3 -c 'import json,sys; j=json.load(sys.stdin); print(j["status"], bool(j["lifecycle"].get("pi_terminal_reservation")))') == 'in-progress False' ]] || loud_fail 'stale writer changed task/terminal receipt'
wgrun pi-watchdog fixture-observe why-pending --event settled --now 1 >/dev/null
wgrun pi-watchdog process-exit why-pending --exit-code 0 >/dev/null
why=$(wgrun why-blocked why-pending)
grep -q 'waiting on lifecycle convergence' <<<"$why" || loud_fail "why-blocked called pending convergence unblocked: $why"
grep -q 'Pending action: finish exact durable receipt, or fence dead owner and resume the same session/worktree' <<<"$why" || loud_fail "why-blocked hid concrete action: $why"
grep -q 'Deadline:' <<<"$why" || loud_fail "why-blocked hid convergence deadline: $why"
wgrun abandon why-pending --reason 'UX fixture complete' >/dev/null

wgrun add exited-finish --id exited-finish --model pi:openrouter:test/model -d $'Exited wrapper convergence.\n\n## Validation\n- same session and worktree resume automatically' >/dev/null
wgrun publish exited-finish --only >/dev/null

start_wg_daemon "$project" --max-agents 1 --model pi:openrouter:test/model --no-coordinator-agent --no-supervise
for _ in $(seq 1 400); do
  status=$(wgrun show exited-finish --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])' || true)
  [[ $status == done ]] && break
  sleep .1
done
[[ ${status:-} == done ]] || loud_fail "exited owner did not converge automatically: $(wgrun show exited-finish)
--- daemon tail ---
$(tail -80 "$project/.wg/service/daemon.log" 2>/dev/null || true)
--- agent tails ---
$(find "$project/.wg/agents" -name output.log -type f -exec tail -40 {} \; 2>/dev/null || true)"

[[ $(cat "$sync/launch-count") == 2 ]] || loud_fail "expected one original + one continuation, got $(cat "$sync/launch-count")"
python3 - "$sync/launches" <<'PY' || loud_fail 'session/worktree identity drifted across continuation'
import sys
rows=[line.rstrip('\n').split('|',2) for line in open(sys.argv[1])]
assert len(rows)==2, rows
assert rows[0][1] and rows[0][1]==rows[1][1], rows
assert rows[0][2]==rows[1][2], rows
PY
[[ $(wc -c <"$project/convergence/retained-wip.bin") -eq 28672 ]] || loud_fail 'retained WIP was not landed byte-for-byte'
[[ -f "$project/convergence/continuation.txt" ]] || loud_fail 'same-session continuation result missing'

wgrun show exited-finish --json >"$sync/show.json"
python3 - "$sync/show.json" <<'PY' || loud_fail 'lifecycle fences/generations or breaker accounting invalid'
import json,sys
j=json.load(open(sys.argv[1])); l=j['lifecycle']
assert l['generation']==1, l
assert l['attempt_sequence']==2, l
assert l['fence']>=3, l
assert j.get('spawn_failures',0)==0, j
assert j.get('completion_disposition')=='landed', j
PY
wgrun finalize status exited-finish --json >"$sync/finish.json"
python3 - "$sync/finish.json" <<'PY' || loud_fail 'finish transaction duplicated or lacked cleanup'
import json,sys
j=json.load(open(sys.argv[1]))
assert j['phase']=='cleaned', j
assert j['merge_receipt'] and j['cleanup_receipt'], j
assert j['candidate']['candidate_version']==1, j
PY
# A second daemon pass/restart is an exact-once no-op, never another generation
# or promotion.
wgrun service stop >/dev/null 2>&1 || true
start_wg_daemon "$project" --max-agents 1 --model pi:openrouter:test/model --no-coordinator-agent --no-supervise
sleep .4
[[ $(cat "$sync/launch-count") == 2 ]] || loud_fail 'restart launched a competitor/duplicate continuation'
[[ $(wgrun finalize status exited-finish --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["phase"])') == cleaned ]] || loud_fail 'restart drifted cleanup receipt'

echo 'exited-worker finish convergence passed'
