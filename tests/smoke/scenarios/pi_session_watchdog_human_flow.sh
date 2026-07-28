#!/usr/bin/env bash
# Installed-binary PTY flow for implement-pi-stalled.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v tmux >/dev/null 2>&1 || skip "tmux unavailable"

scratch="$(make_scratch)"
project="$scratch/project"
home="$scratch/home"
mkdir -p "$project" "$home/.config/workgraph"
: >"$home/.config/workgraph/config.toml"
(
  cd "$project"
  git init -q
  git config user.email pi-watchdog@test.invalid
  git config user.name 'Pi Watchdog Smoke'
  printf 'baseline\n' >README
  git add README && git commit -qm baseline
  HOME="$home" XDG_CONFIG_HOME="$home/.config" wg init --no-agency >/dev/null
)

wgrun() { (cd "$project" && env -u WG_AGENT_ID -u WG_TASK_ID -u WG_TIER HOME="$home" XDG_CONFIG_HOME="$home/.config" wg "$@"); }
new_task() {
  local id=$1
  wgrun add "$id" --id "$id" -d $'Fake Pi watchdog flow.\n\n## Validation\n- explicit receipt required' >/dev/null
  wgrun claim "$id" >/dev/null
  mkdir -p "$scratch/$id-worktree"
}
start_fake() {
  local id=$1 session="pi-wd-$1-$$"
  tmux new-session -d -x 240 -y 60 -s "$session" "env HOME='$home' XDG_CONFIG_HOME='$home/.config' '$HERE/../../fixtures/fake-pi-watchdog' '$project/.wg' '$id' '$scratch/$id-worktree'"
  printf '%s' "$session"
}
send() { tmux send-keys -t "$1" "$2" Enter; sleep 0.15; }
capture() { tmux capture-pane -p -S - -t "$1"; }
stop_fake() { send "$1" quit || true; tmux kill-session -t "$1" 2>/dev/null || true; }

# Split clocks: production values are displayed first, virtual fixture time then
# crosses each edge without weakening the static policy.
new_task clock
s=$(start_fake clock)
send "$s" 'init 0'
send "$s" 'observe provider-start 0'
send "$s" 'tick 299'
send "$s" 'tick 300'
send "$s" 'tick 480'
send "$s" 'tick 899'
send "$s" 'tick 900'
send "$s" 'tick 959'
send "$s" 'observe probe 960'
send "$s" 'tick 960'
send "$s" 'observe launched 961'
send "$s" 'observe permit 962'
send "$s" status
out=$(capture "$s")
grep -q 'production soft=300s free/low hard>=900s grace=60s' <<<"$out" || loud_fail "production policy not visible: $out"
grep -q 'tick=299 classification=Active' <<<"$out" || loud_fail "299s not active: $out"
grep -q 'tick=300 classification=Suspect actions=\[ReadOnlyProbe\]' <<<"$out" || loud_fail "soft probe missing: $out"
grep -q 'tick=480 classification=Suspect actions=\[\]' <<<"$out" || loud_fail "obsolete grace fenced: $out"
grep -q 'tick=899 classification=Suspect' <<<"$out" || loud_fail "pre-hard process not preserved: $out"
grep -q 'tick=900 classification=HardResumeEligible actions=\[StartHardGrace\]' <<<"$out" || loud_fail "hard eligibility missing: $out"
grep -q 'tick=959 classification=HardResumeEligible actions=\[\]' <<<"$out" || loud_fail "hard grace not honored: $out"
grep -q 'tick=960 classification=Fencing actions=\[ReserveContinuation, FenceExactProcess\]' <<<"$out" || loud_fail "guarded fence missing: $out"
grep -q 'event=launched classification=Resuming' <<<"$out" || loud_fail "resuming transition missing: $out"
grep -q 'event=permit classification=Active' <<<"$out" || loud_fail "permit did not restore active: $out"
show_clock=$(wgrun show clock)
grep -q 'Pi watchdog: Active' <<<"$show_clock" || loud_fail "wg show/TUI read model omitted watchdog diagnostics: $show_clock"
grep -q 'session=fake-session.*route=pi:fake-free:fake-slow' <<<"$show_clock" || loud_fail "wg show omitted frozen proof tuple: $show_clock"
stop_fake "$s"

# Advancing progress, provider retry/in-flight, accepted wait, long tool, and
# unknown alive silence all remain untouched by total runtime.
new_task progress; s=$(start_fake progress); send "$s" 'init 0'; send "$s" 'observe provider-start 0'; send "$s" 'observe token 1200'; send "$s" 'tick 1201'; out=$(capture "$s"); grep -q 'tick=1201 classification=Active actions=\[\]' <<<"$out" || loud_fail "progressing 20m run touched: $out"; stop_fake "$s"
new_task retry; s=$(start_fake retry); send "$s" 'init 0'; send "$s" 'observe provider-start 0'; send "$s" 'observe provider-retry 899'; send "$s" 'tick 900'; out=$(capture "$s"); grep -q 'classification=Active actions=\[\]' <<<"$out" || loud_fail "provider retry not progress: $out"; stop_fake "$s"
new_task waiting; s=$(start_fake waiting); send "$s" 'init 0'; send "$s" 'observe wait 1'; send "$s" 'tick 10000'; out=$(capture "$s"); grep -q 'classification=WaitingUser actions=\[\]' <<<"$out" || loud_fail "accepted wait interrupted: $out"; stop_fake "$s"
new_task longtool; s=$(start_fake longtool); send "$s" 'init 0'; send "$s" 'observe long-tool 1'; send "$s" 'tick 5000'; out=$(capture "$s"); grep -q 'classification=LongTool actions=\[\]' <<<"$out" || loud_fail "long tool interrupted: $out"; stop_fake "$s"
new_task unknown; s=$(start_fake unknown); send "$s" 'init 0'; send "$s" 'observe unknown 0'; send "$s" 'tick 300'; send "$s" 'tick 10000'; out=$(capture "$s"); grep -q 'tick=300 classification=Suspect actions=\[ReadOnlyProbe\]' <<<"$out" || loud_fail "unknown soft probe missing: $out"; grep -q 'tick=10000 classification=StalledOperatorRequired actions=\[\]' <<<"$out" || loud_fail "unknown silence was killed/resumed: $out"; stop_fake "$s"

# Settled and safe exit promptly get one append-once SAME-session action and
# remain nonterminal. Restarting the PTY around the prompt must not duplicate it.
for id in settled safeexit; do
  new_task "$id"; s=$(start_fake "$id"); send "$s" 'init 0'
  if [[ $id == settled ]]; then send "$s" 'observe settled 1'; else send "$s" 'observe exit-zero 1'; fi
  out=$(capture "$s"); flat=$(tr -d '\n\r ' <<<"$out")
  grep -q 'classification=NeedsFinalization' <<<"$flat" || loud_fail "$id did not promptly need finalization: $out"
  grep -q 'prompts=1' <<<"$flat" || loud_fail "$id prompt projection wrong: $out"
  grep -q 'terminal=false' <<<"$flat" || loud_fail "$id inferred a terminal outcome: $out"
  stop_fake "$s"
  s=$(start_fake "$id"); if [[ $id == settled ]]; then send "$s" 'observe settled 2'; else send "$s" 'observe exit-zero 2'; fi; send "$s" status
  out=$(capture "$s"); flat=$(tr -d '\n\r ' <<<"$out")
  grep -q 'prompts=1' <<<"$flat" || loud_fail "$id replay duplicated prompt: $out"
  grep -q 'terminal=false' <<<"$flat" || loud_fail "$id replay inferred terminal: $out"
  attempt=$(wgrun show "$id" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["lifecycle"]["current_attempt"]["id"])')
  runtime=$(attempt_runtime_dir "$project/.wg" "$id" "$attempt")
  marker="$runtime/pi/session/fake-session.jsonl"
  [[ $(grep -c 'wg-pi-continuation' "$marker") -eq 1 ]] || loud_fail "$id session marker not exactly once"
  stop_fake "$s"
done

# Explicit current-epoch tools are all reservations while a writer may live.
# Candidate finalization consumes them only after exact reap + durable rescue.
new_task doneintent; s=$(start_fake doneintent); send "$s" 'init 0'; send "$s" 'observe done 1'; out=$(capture "$s"); grep -q 'terminal=true' <<<"$out" || loud_fail "done receipt not accepted"; [[ $(wgrun show doneintent --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])') == in-progress ]] || loud_fail "success intent became Done early"; stop_fake "$s"
new_task failintent; s=$(start_fake failintent); send "$s" 'init 0'; send "$s" 'observe fail 1'; [[ $(wgrun show failintent --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])') == in-progress ]] || loud_fail "failure intent terminalized before rescue"; stop_fake "$s"
new_task parkintent; s=$(start_fake parkintent); send "$s" 'init 0'; send "$s" 'observe park 1'; [[ $(wgrun show parkintent --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])') == in-progress ]] || loud_fail "park intent terminalized before rescue"; stop_fake "$s"

# Manual finite grant is charged once by stable action ID, and explicit abort
# uses the lifecycle CAS. Status exposes all diagnostics through installed CLI.
new_task manual; s=$(start_fake manual); send "$s" 'init 0'; send "$s" 'observe unknown 0'; send "$s" 'tick 300'; send "$s" 'tick 10000'; send "$s" 'resume inspected'; send "$s" 'resume inspected'; send "$s" status; out=$(capture "$s"); grep -q 'budget: epochs=0/3+1 elapsed-reserved=0/1800+600s' <<<"$out" || loud_fail "manual grant duplicated/replenished: $out"; stop_fake "$s"
new_task abortme; s=$(start_fake abortme); send "$s" 'init 0'; send "$s" 'abort operator-stop'; out=$(capture "$s"); grep -q 'Operator abort accepted' <<<"$out" || loud_fail "operator abort missing: $out"; [[ $(wgrun show abortme --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])') == in-progress ]] || loud_fail "abort terminalized before rescue"; stop_fake "$s"

# No source retry/admission/breaker/evaluation/owner duplication is possible in
# this credential-free fixture: each task has exactly one lifecycle attempt.
python3 - "$project/.wg/graph.jsonl" <<'PY'
import json,sys
for line in open(sys.argv[1]):
    t=json.loads(line)
    if t.get('id') in {'clock','settled','safeexit','manual'}:
        l=t['lifecycle']
        assert l['attempt_sequence']==1, (t['id'],l)
        assert l['generation']==0, (t['id'],l)
PY

echo "pi session watchdog human flow passed"
