#!/usr/bin/env bash
# Real terminal flow for immutable-candidate landing after a descendant target
# advance. No reset/requeue/retry/unclaim is used: the released worker's exact
# candidate is reconciled by the finalizer under renewed validation evidence.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
export WG_SMOKE_ROOT="${WG_SMOKE_ROOT:-/tmp/wgs-land-reconcile-$$}"
. "$HERE/_helpers.sh"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"

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
model=""
while (($#)); do
  case "$1" in --model) model="$2"; shift 2;; *) shift;; esac
done
cat >/dev/null || true
case "$model" in
  fake-review|openrouter:fake-review)
    printf 'review\n' >>"${FAKE_STATE:?}/calls"
    python3 - <<'PY'
import json
text=json.dumps({'verdict':'pass','findings':[]},separators=(',',':'))
print(json.dumps({'type':'turn_end','message':{'role':'assistant','content':[{'type':'text','text':text}],
'provider':'test','model':'fake-review','stopReason':'stop','rawStopReason':'completed',
'usage':{'input':1,'output':1,'cacheRead':0,'cacheWrite':0,'totalTokens':2,'cost':{'total':0}}}},separators=(',',':')))
PY
    ;;
  fake-worker|openrouter:fake-worker)
    printf 'worker\n' >>"${FAKE_STATE:?}/calls"
    printf 'candidate bytes\n' >candidate.txt
    git add candidate.txt && git commit -qm 'immutable candidate'
    git rev-parse HEAD >"$HOME/candidate-oid"
    # Deliberate user dirtiness parks the already-reviewed candidate and releases
    # this source worker. The WG-created .wg-worktrees directory itself must not
    # appear as dirtiness and needs no committed .gitignore change.
    printf 'operator in-flight bytes\n' >"${WG_PROJECT_ROOT:?}/base.txt"
    wg done "$WG_TASK_ID" >"$HOME/worker-done.out" 2>"$HOME/worker-done.err"
    : >"$HOME/worker-finished"
    printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"finished"}],"provider":"test","model":"fake-worker","stopReason":"stop","rawStopReason":"completed","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"total":0}}}}'
    ;;
  *) echo "unexpected fake model $model" >&2; exit 91;;
esac
FAKE_PI
chmod +x "$fakebin/pi"

export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg"
export PATH="$fakebin:$PATH" FAKE_STATE="$state"
unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_PROJECT_ROOT \
  WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH WG_GRAPH_ID WG_SPAWN_RUN_ID WG_SPAWN_EPOCH || true

cd "$project"
git init -q -b main
git config user.email land-reconcile@test.invalid
git config user.name LandReconcile
printf 'base bytes\n' >base.txt
git add base.txt && git commit -qm base
"$WG_BIN" init --no-agency --route pi --model pi:openrouter:fake-worker >/dev/null
# Prove the project does not have to commit WG's worktree runtime exclusion.
if [[ -f .gitignore ]]; then
  grep -v '\.wg-worktrees' .gitignore >.gitignore.tmp || true
  mv .gitignore.tmp .gitignore
fi
git add AGENTS.md CLAUDE.md
[[ ! -f .gitignore ]] || git add .gitignore
git commit -qm init-wg
! git show HEAD:.gitignore 2>/dev/null | grep -q '\.wg-worktrees' \
  || loud_fail "test fixture unexpectedly committed a .wg-worktrees ignore"
G="$project/.wg"
wgrun(){ env -u WG_TASK_ID -u WG_AGENT_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC "$WG_BIN" --dir "$G" "$@"; }
wgrun config set models.reviewer.model pi:openrouter:fake-review >/dev/null
wgrun config set models.reviewer.reasoning low >/dev/null
wgrun config set models.evaluator.model pi:openrouter:fake-review >/dev/null
wgrun config set models.evaluator.reasoning low >/dev/null
wgrun config set agency.completion_review_strict true >/dev/null
wgrun config set agency.auto_assign false >/dev/null
wgrun config set agency.auto_evaluate false >/dev/null
wgrun config set dispatcher.max_agents 1 >/dev/null
wgrun config set dispatcher.poll_interval 1 >/dev/null
wgrun config set dispatcher.settling_delay_ms 0 >/dev/null
wgrun config set dispatcher.worktree_isolation true >/dev/null
wgrun add 'Landing reconciliation human flow' --id landing-reconcile --priority 100 \
  --validation-command 'test -f candidate.txt' \
  -d $'Create candidate.txt.\n\n## Validation\n- [ ] candidate is retained and target-bound evidence is renewed' >/dev/null
wgrun publish landing-reconcile --only >/dev/null
start_wg_daemon "$project" --no-chat-agent --interval 1
for _ in $(seq 1 400); do [[ -e "$home/worker-finished" ]] && break; sleep .05; done
[[ -e "$home/worker-finished" ]] || loud_fail "worker did not reach LandingPending"
for _ in $(seq 1 200); do
  status=$(wgrun show landing-reconcile --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])')
  [[ "$status" == waiting ]] && break
  sleep .05
done
[[ "$status" == waiting ]] || loud_fail "candidate did not park in Waiting/LandingPending"
wgrun service stop >/dev/null || true

# Only the deliberate tracked edit is dirty. The self-created runtime tree is
# excluded through Git administrative state, not a committed project policy.
status_before=$(git status --porcelain --untracked-files=all)
[[ "$status_before" == *"base.txt"* ]] || loud_fail "deliberate user dirtiness missing: $status_before"
[[ "$status_before" != *".wg-worktrees"* ]] || loud_fail "WG runtime dirtied root checkout: $status_before"
grep -Eq '^/.wg-worktrees/[^/]+/$' .git/info/exclude || loud_fail "exact repository-local WG runtime exclusion missing"
! grep -q '^/.wg-worktrees/$' .git/info/exclude || loud_fail "broad runtime exclusion hides user-owned siblings"
printf '/daemon.log\n' >>.git/info/exclude

candidate=$(cat "$home/candidate-oid")
git restore -- base.txt
printf 'independent target advance\n' >target.txt
git add target.txt && git commit -qm 'advance landing target'
target=$(git rev-parse HEAD)
[[ -z $(git status --porcelain --untracked-files=all) ]] || loud_fail "root not clean before recovery"

# This is the supported operator action. The worker is released and the daemon
# is stopped, proving no live source process participates.
wgrun resume landing-reconcile --only >"$scratch/resume.out" 2>"$scratch/resume.err"
wgrun show landing-reconcile --json >"$scratch/done.json"
wgrun merge-resolution status landing-reconcile >"$scratch/reconcile.status"
landed=$(git rev-parse HEAD)
python3 - "$scratch/done.json" "$candidate" "$target" "$landed" <<'PY'
import json,subprocess,sys
x=json.load(open(sys.argv[1])); candidate,target,landed=sys.argv[2:]
assert x['status']=='done' and x['completion_disposition']=='landed',x
assert x.get('completion_blocker') is None and x.get('assigned') is None,x
parents=subprocess.check_output(['git','rev-list','--parents','-n','1',landed],text=True).split()
assert target in parents[1:] and candidate in parents[1:],parents
assert subprocess.run(['git','merge-base','--is-ancestor',candidate,landed]).returncode==0
assert subprocess.run(['git','merge-base','--is-ancestor',target,landed]).returncode==0
PY
grep -q 'Landing reconciliation Landed' "$scratch/reconcile.status" \
  || loud_fail "operator status omitted landed reconciliation: $(cat "$scratch/reconcile.status")"
grep -q 'renewed validation: 2' "$scratch/reconcile.status" \
  || loud_fail "configured+baseline refreshed evidence missing: $(cat "$scratch/reconcile.status")"
[[ $(grep -c '^worker$' "$state/calls") == 1 ]] || loud_fail "source worker was rerun"
[[ $(grep -c '^review$' "$state/calls") == 2 ]] || loud_fail "semantic review was rerun or skipped"
[[ $(cat candidate.txt) == 'candidate bytes' && $(cat target.txt) == 'independent target advance' ]] \
  || loud_fail "reconciled checkout lost candidate or target bytes"
[[ -z $(git status --porcelain --untracked-files=all) ]] || loud_fail "landed root checkout is dirty"

echo "PASS: WG runtime stayed administratively excluded; Waiting/LandingPending survived worker release; descendant target advance received renewed configured+baseline validation and an immutable target-binding receipt; supported resume landed the retained candidate without reset/retry/requeue/unclaim"
