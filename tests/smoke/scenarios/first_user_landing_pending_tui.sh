#!/usr/bin/env bash
# Real tmux/PTY regression for the first accepted landing's recovery surface.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
export WG_SMOKE_ROOT="${WG_SMOKE_ROOT:-/tmp/wgs-first-landing-$$}"
. "$HERE/_helpers.sh"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required for the real TUI flow"

scratch=$(make_scratch)
project="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"; state="$scratch/state"
mkdir -p "$project" "$home/.config" "$fakebin" "$state"
ROOT="$(cd "$HERE/../../.." && pwd)"
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-$ROOT/target/debug/wg}"
[[ -x "$WG_BIN" ]] || (cd "$ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"
ln -s "$WG_BIN" "$fakebin/wg"

cat >"$fakebin/pi" <<'FAKE_PI'
#!/usr/bin/env bash
set -euo pipefail
args="$*"
if [[ " $args " == *" --list-models "* ]]; then
  printf 'provider model context max-out thinking images\n'
  printf 'openrouter fake-worker 128K 16K yes no\n'
  printf 'openrouter fake-review 128K 16K yes no\n'
  exit 0
fi
model=""
while (($#)); do
  case "$1" in --model) model="$2"; shift 2;; *) shift;; esac
done
prompt=$(cat || true)
case "$model" in
  fake-review|openrouter:fake-review)
    FAKE_PROMPT="$prompt" python3 - <<'PY'
import json,os
if 'worksgood-flip-blind-inference-v1' in os.environ.get('FAKE_PROMPT',''):
    text=json.dumps({'goal':'reconstructed fixture goal','constraints':[],'invariants':[],'failure_modes':[]},separators=(',',':'))
else:
    text=json.dumps({'verdict':'pass','findings':[]},separators=(',',':'))
print(json.dumps({'type':'turn_end','message':{'role':'assistant','content':[{'type':'text','text':text}],
'provider':'test','model':'fake-review','stopReason':'stop','rawStopReason':'completed',
'usage':{'input':1,'output':1,'cacheRead':0,'cacheWrite':0,'totalTokens':2,'cost':{'total':0}}}},separators=(',',':')))
PY
    ;;
  fake-worker|openrouter:fake-worker)
    printf 'candidate bytes\n' >candidate.txt
    git add candidate.txt && git commit -qm 'immutable candidate'
    # User bytes in the attached checkout force the safe LandingPending path.
    printf 'operator in-flight bytes\n' >"${WG_PROJECT_ROOT:?}/base.txt"
    wg done "$WG_TASK_ID" >/dev/null 2>"$HOME/worker-done.err"
    : >"$HOME/worker-finished"
    printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"finished"}],"provider":"test","model":"fake-worker","stopReason":"stop","rawStopReason":"completed","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"total":0}}}}'
    ;;
  *) echo "unexpected fake model $model" >&2; exit 91;;
esac
FAKE_PI
chmod +x "$fakebin/pi"

export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg"
export PATH="$fakebin:$PATH"
unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_PROJECT_ROOT WG_WORKTREE_PATH WG_WORKTREE_ACTIVE \
  WG_BRANCH WG_GRAPH_ID WG_SPAWN_RUN_ID WG_SPAWN_EPOCH || true

cd "$project"
git init -q -b main
git config user.email first-landing@test.invalid
git config user.name FirstLanding
printf 'base bytes\n' >base.txt
git add base.txt && git commit -qm base
"$WG_BIN" init --no-agency >/dev/null
G="$project/.wg"
wgrun(){ env -u WG_TASK_ID -u WG_AGENT_ID "$WG_BIN" --dir "$G" "$@"; }
wgrun setup --route pi --model pi:openrouter:fake-worker --yes >/dev/null
for role in reviewer flip_inference flip_comparison evaluator; do
  wgrun config set "models.$role.model" pi:openrouter:fake-review >/dev/null
  wgrun config set "models.$role.reasoning" low >/dev/null
done
wgrun config set agency.completion_review_strict true >/dev/null
wgrun config set agency.auto_assign false >/dev/null
wgrun config set agency.auto_evaluate false >/dev/null
wgrun config set dispatcher.max_agents 1 >/dev/null
wgrun config set dispatcher.poll_interval 1 >/dev/null
wgrun config set dispatcher.settling_delay_ms 0 >/dev/null
wgrun config set dispatcher.worktree_isolation true >/dev/null
wgrun config set dispatcher.resource_management.disk_sentinel_enabled false >/dev/null
git add AGENTS.md CLAUDE.md worksgood.toml .gitignore
git commit -qm init-wg

wgrun add 'First landing recovery' --id first-landing \
  -d $'Create candidate.txt.\n\n## Validation\n- [ ] candidate reaches reviewed LandingPending' >/dev/null
wgrun publish first-landing --only >/dev/null
start_wg_daemon "$project" --no-chat-agent --interval 1
for _ in $(seq 1 500); do [[ -e "$home/worker-finished" ]] && break; sleep .05; done
[[ -e "$home/worker-finished" ]] \
  || loud_fail "worker did not reach completion: $(cat "$home/worker-done.err" 2>/dev/null || true)"
for _ in $(seq 1 200); do
  status=$(wgrun show first-landing --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])')
  [[ "$status" == waiting ]] && break
  sleep .05
done
[[ "$status" == waiting ]] || loud_fail "candidate did not park in LandingPending"
wgrun show first-landing --json >"$scratch/pending.json"
python3 - "$scratch/pending.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); blocker=x.get('completion_blocker') or {}
assert x.get('assigned') is None,x
assert blocker.get('kind')=='landing-pending',blocker
assert 'wg resume first-landing --only' in blocker.get('safe_next',''),blocker
PY

session="wg-first-landing-tui-$$"
printf '%s\n' first-landing >"$G/.new_task_focus"
cleanup_tui(){ tmux kill-session -t "$session" >/dev/null 2>&1 || true; }
add_cleanup_hook cleanup_tui
tmux new-session -d -s "$session" -x 180 -y 44 \
  "cd '$project' && HOME='$home' XDG_CONFIG_HOME='$home/.config' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$G' tui; sleep 30" \
  || loud_fail "could not start tmux TUI"
capture(){ tmux capture-pane -p -t "$session" 2>/dev/null || true; }
for _ in $(seq 1 400); do capture | grep -Fq first-landing && break; sleep .025; done
capture | grep -Fq first-landing || loud_fail "TUI did not render first-landing"
tmux send-keys -t "$session" Home
tmux send-keys -t "$session" Enter
seen=''
for _ in $(seq 1 12); do
  frame=$(capture); seen+=$'\n'"$frame"
  grep -Fq 'SAFE RESUME: wg resume first-landing --only' <<<"$seen" && break
  tmux send-keys -t "$session" PageDown
  sleep .05
done
grep -Fq 'LANDING PENDING:' <<<"$seen" || loud_fail "TUI omitted the LandingPending explanation"
grep -Fq 'SAFE RESUME: wg resume first-landing --only' <<<"$seen" \
  || loud_fail "TUI omitted the exact safe resume action: $(capture | tr '\n' '|')"

echo "PASS: a reviewed candidate parked on user dirtiness and the tmux-driven TUI showed the exact safe wg resume first-landing --only action"
