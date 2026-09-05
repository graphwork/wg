#!/usr/bin/env bash
# Composed clean-graph golden path: trusted fan-out, daemon outage, restart,
# ordered exact landing, and dependent dispatch without source-failure charge.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"

scratch=$(make_scratch)
project="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"
mkdir -p "$project" "$home" "$fakebin"
ROOT="$(cd "$HERE/../../.." && pwd)"
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-$ROOT/target/debug/wg}"
[[ -x "$WG_BIN" ]] || (cd "$ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)
ln -s "$WG_BIN" "$fakebin/wg"
cat >"$fakebin/pi" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
model=""
while (($#)); do case "$1" in --model) model="$2"; shift 2;; *) shift;; esac; done
prompt=$(cat || true)
if [[ "$model" == "openrouter:fake-review" || "$model" == "fake-review" ]]; then
  if grep -q 'worksgood-flip-blind-inference-v1' <<<"$prompt"; then
    text='{\"goal\":\"reconstructed fanout goal\",\"constraints\":[],\"invariants\":[],\"failure_modes\":[]}'
  else
    text='{\"verdict\":\"pass\",\"findings\":[]}'
  fi
  printf '%s\n' "{\"type\":\"turn_end\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"$text\"}],\"provider\":\"test\",\"model\":\"fake-review\",\"stopReason\":\"stop\",\"usage\":{\"input\":1,\"output\":1,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":2,\"cost\":{\"total\":0}}}}"
  exit 0
fi
[[ "$model" == "openrouter:fake-worker" || "$model" == "fake-worker" ]] || { echo "unexpected model $model" >&2; exit 88; }
case "$WG_TASK_ID" in
  fan-a)
    wg edit fan-b --description $'Worker A refined B while collaborating locally.\n\n## Validation\n- [ ] B records A metadata improvement' >/dev/null
    wg msg send fan-b "A coordinated B metadata" >/dev/null
    echo A > fan-a.txt; git add fan-a.txt; git commit -qm fan-a
    : > "$HOME/a-coordinated"
    while [[ ! -e "$HOME/release-a" ]]; do sleep .05; done
    git merge refs/heads/main --no-edit >/dev/null
    wg done fan-a >"$HOME/a-done.out" 2>"$HOME/a-done.err"
    : > "$HOME/a-done"
    ;;
  fan-b)
    wg add "B child" --id fan-b-child --after fan-b --description $'Legitimate linked child created by B.\n\n## Validation\n- [ ] remains an explicit draft' >/dev/null
    echo B > fan-b.txt; git add fan-b.txt; git commit -qm fan-b
    : > "$HOME/b-coordinated"
    while [[ ! -e "$HOME/release-b" ]]; do sleep .05; done
    git merge refs/heads/main --no-edit >/dev/null
    wg done fan-b >"$HOME/b-done.out" 2>"$HOME/b-done.err"
    : > "$HOME/b-done"
    ;;
  fan-c)
    : > "$HOME/c-started"
    echo C > fan-c.txt; git add fan-c.txt; git commit -qm fan-c
    git merge refs/heads/main --no-edit >/dev/null
    wg done fan-c >"$HOME/c-done.out" 2>"$HOME/c-done.err"
    : > "$HOME/c-done"
    ;;
  *) exit 89;;
esac
printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"completed"}],"provider":"test","model":"fake-worker","stopReason":"stop","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"total":0}}}}'
SH
chmod +x "$fakebin/pi"

export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg" PATH="$fakebin:$PATH"
unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_PROJECT_ROOT WG_WORKTREE_PATH
cd "$project"
git init -q -b main
git config user.email fanout@test.invalid
git config user.name Fanout
echo base > base.txt; git add base.txt; git commit -qm base
"$WG_BIN" init --no-agency >/dev/null
wgrun(){ env -u WG_TASK_ID -u WG_AGENT_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC WG_DIR="$project/.wg" "$WG_BIN" "$@"; }
wgrun setup --route pi --model pi:openrouter:fake-worker --yes >/dev/null
wgrun config set models.reviewer.model pi:openrouter:fake-review >/dev/null
wgrun config set models.reviewer.reasoning low >/dev/null
wgrun config set models.flip_inference.model pi:openrouter:fake-review >/dev/null
wgrun config set models.flip_inference.reasoning low >/dev/null
wgrun config set models.flip_comparison.model pi:openrouter:fake-review >/dev/null
wgrun config set models.flip_comparison.reasoning low >/dev/null
wgrun config set models.evaluator.model pi:openrouter:fake-review >/dev/null
wgrun config set models.evaluator.reasoning low >/dev/null
wgrun config set dispatcher.max_agents 2 >/dev/null
wgrun config set dispatcher.poll_interval 1 >/dev/null
wgrun config set dispatcher.settling_delay_ms 0 >/dev/null
wgrun config set dispatcher.worktree_isolation true >/dev/null
wgrun config set dispatcher.resource_management.disk_sentinel_enabled false >/dev/null
git add .gitignore AGENTS.md CLAUDE.md worksgood.toml; git commit -qm init-wg
printf '/.wg-worktrees/\n/daemon.log\n' >>.git/info/exclude
wgrun add "Fan A" --id fan-a --priority 100 >/dev/null
wgrun add "Fan B" --id fan-b --priority 90 >/dev/null
wgrun add "Fan C synthesis" --id fan-c --after fan-a,fan-b >/dev/null
wgrun publish fan-a --wcc >/dev/null
start_wg_daemon "$project" --no-chat-agent --interval 1

for _ in $(seq 1 240); do
  [[ -e "$home/a-coordinated" && -e "$home/b-coordinated" ]] && break
  sleep .1
done
if [[ ! -e "$home/a-coordinated" || ! -e "$home/b-coordinated" ]]; then
  diagnostics=$(find .wg/log/agents .wg/agents -type f \( -name output.txt -o -name output.log \) -exec sh -c 'echo ===$1; tail -40 "$1"' _ {} \; 2>/dev/null || true)
  daemon_tail=$(tail -80 .wg/service/daemon.log 2>/dev/null || true)
  loud_fail "initial fan-out did not coordinate: $(wgrun list --all 2>&1) $diagnostics daemon=$daemon_tail"
fi
[[ ! -e "$home/c-started" ]] || loud_fail "dependent C started before A/B completion"
wgrun show fan-b --json | grep -q 'Worker A refined B' || loud_fail "A did not improve B metadata"
grep -q 'A coordinated B metadata' .wg/messages/fan-b.jsonl || loud_fail "A-to-B message missing"
grep -q '"id":"fan-b-child"' .wg/graph.jsonl || loud_fail "B did not create linked child"

# Stop only the dispatcher. Detached trusted workers continue; A completes its
# exact candidate while the daemon is absent.
wgrun service stop
touch "$home/release-a"
for _ in $(seq 1 240); do [[ -e "$home/a-done" ]] && break; sleep .1; done
[[ -e "$home/a-done" ]] || loud_fail "trusted A could not complete during daemon outage"
[[ $(wgrun show fan-a --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])') == done ]] || loud_fail "A not Done after offline completion: $(cat "$HOME/a-done.err" 2>/dev/null || true) $(cat "$HOME/a-done.out" 2>/dev/null || true)"

# Restart dispatch and finish B. C must then start automatically from the same
# graph with no failed generation/retry charge.
start_wg_daemon "$project" --no-chat-agent --interval 1
touch "$home/release-b"
for _ in $(seq 1 300); do
  b_status=$(wgrun show fan-b --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])' 2>/dev/null || true)
  c_status=$(wgrun show fan-c --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])' 2>/dev/null || true)
  [[ "$b_status" == done && "$c_status" == done ]] && break
  sleep .1
done
[[ -e "$home/c-started" && "${b_status:-}" == done && "${c_status:-}" == done ]] || loud_fail "restart did not converge B then dispatch/complete C: markers=$(find "$HOME" -maxdepth 1 -type f -printf '%f ' 2>/dev/null || true) tasks=$(wgrun list --all 2>&1) B=$(cat "$HOME/b-done.err" 2>/dev/null || true) C=$(cat "$HOME/c-done.err" 2>/dev/null || true) daemon=$(tail -40 .wg/service/daemon.log 2>/dev/null || true)"
for id in fan-a fan-b fan-c; do
  json=$(wgrun show "$id" --json)
  [[ $(python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])' <<<"$json") == done ]] || loud_fail "$id is not Done"
  [[ $(python3 -c 'import json,sys; print(json.load(sys.stdin).get("retry_count",0))' <<<"$json") == 0 ]] || loud_fail "$id acquired a source retry charge"
done
[[ -f fan-a.txt && -f fan-b.txt && -f fan-c.txt ]] || loud_fail "main lacks one or more exact landed outputs"

echo "PASS: request → fan-out → trusted coordination → daemon outage/restart → reviewed completion"
