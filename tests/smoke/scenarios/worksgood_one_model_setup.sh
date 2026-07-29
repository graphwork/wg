#!/usr/bin/env bash
# Real one-paste/one-confirmation concierge flow for --model.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v cargo >/dev/null 2>&1 || loud_skip "MISSING CARGO" "cargo is required"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"

scratch=$(make_scratch)
repo_root="$(cd "$HERE/../../.." && pwd)"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
if [[ -n "${WG_SMOKE_CANDIDATE_DIR:-}" ]]; then
    CARGO_TARGET_DIR="$WG_SMOKE_CANDIDATE_DIR"
else
    CARGO_TARGET_DIR="$scratch/candidate-target"
    (cd "$repo_root" && CARGO_TARGET_DIR="$CARGO_TARGET_DIR" CARGO_BUILD_JOBS=1 \
        cargo build --quiet --bin wg --bin worksgood)
fi
WORKSGOOD="$CARGO_TARGET_DIR/debug/worksgood"
W="$CARGO_TARGET_DIR/debug/wg"
[[ -x "$WORKSGOOD" && -x "$W" ]] || loud_fail "candidate worksgood/wg bundle missing"

export HOME="$scratch/home"
export WG_GLOBAL_DIR="$HOME/.wg"
export XDG_CACHE_HOME="$HOME/.cache"
export XDG_CONFIG_HOME="$HOME/.config"
unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER TMUX TMUX_TMPDIR
mkdir -p "$HOME" "$scratch/fake-bin"
cat >"$scratch/fake-bin/pi" <<'SH'
#!/usr/bin/env bash
# Discovery/readiness fixture. The setup daemon does not make a model call.
exit 0
SH
chmod +x "$scratch/fake-bin/pi"
export PATH="$scratch/fake-bin:$PATH"
M='pi:openrouter:deepseek/deepseek-v4-flash'

mk_repo() {
    mkdir -p "$1"
    git -C "$1" init -q
}
snapshot() {
    local path=$1
    find "$path" -type f -printf '%P %s ' -exec sha256sum {} \; 2>/dev/null | sort
}
wait_for_text() {
    local session=$1 text=$2
    for _ in $(seq 1 1200); do
        tmux capture-pane -p -S - -t "$session" 2>/dev/null | grep -q "$text" && return 0
        sleep 0.05
    done
    loud_fail "session $session did not show '$text': $(tmux capture-pane -p -S - -t "$session" 2>/dev/null | tr '\n' '|')"
}
wait_exit() {
    local session=$1
    for _ in $(seq 1 2400); do
        [[ "$(tmux list-panes -t "$session" -F '#{pane_dead}' 2>/dev/null || echo 1)" = 1 ]] && return 0
        sleep 0.05
    done
    loud_fail "session did not exit: $session"
}
cleanup_daemon() {
    local state="$scratch/repo/.wg/service/state.json" pid=""
    if [[ -f "$state" ]]; then
        pid=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("pid", ""))' "$state" 2>/dev/null || true)
        [[ "$pid" =~ ^[0-9]+$ ]] && kill "$pid" 2>/dev/null || true
    fi
}
add_cleanup_hook cleanup_daemon

# RED-regression surface: help advertises the simple --model path, while --profile
# remains explicitly the existing-base advanced path.
help=$($WORKSGOOD --help)
grep -q -- '--model <pi:<provider>:<model>>' <<<"$help" || loud_fail "help has no one-model shorthand"
grep -q 'existing reusable base profile' <<<"$help" || loud_fail "help does not explain --profile"

# Clean HOME, no profile files: strict dry-run is deterministic and leaves every
# repository/global/cache/plugin/service/TUI surface untouched.
mk_repo "$scratch/dry"
before_repo=$(snapshot "$scratch/dry")
before_home=$(snapshot "$HOME")
dry1=$($WORKSGOOD --project "$scratch/dry" setup --model "$M" --dry-run)
dry_again=$($WORKSGOOD --project "$scratch/dry" setup --model "$M" --dry-run)
bare_dry=$($WORKSGOOD --project "$scratch/dry" --model "$M" --dry-run)
[[ "$dry1" = "$dry_again" ]] || loud_fail "repeated --model dry-run plan is not byte-stable"
grep -qF "$M" <<<"$bare_dry" || loud_fail "bare worksgood --model did not enter setup/reconcile planning"
grep -q 'Open setup-neutral TUI' <<<"$bare_dry" || loud_fail "bare --model plan omitted post-setup TUI"
[[ "$before_repo" = "$(snapshot "$scratch/dry")" ]] || loud_fail "dry-run mutated repository"
[[ "$before_home" = "$(snapshot "$HOME")" ]] || loud_fail "dry-run mutated HOME/cache/plugin/profile/history"
[[ ! -e "$scratch/dry/.wg" && ! -e "$WG_GLOBAL_DIR" ]] || loud_fail "dry-run created graph/global state"
printf '%s' "$dry1" >"$scratch/dry-plan.out"
python3 - "$M" "$scratch/dry-plan.out" <<'PY'
import json, sys
route=sys.argv[1]
s=open(sys.argv[2]).read()
start=s.index('{', s.index('Immutable redacted plan:'))
plan,_=json.JSONDecoder().raw_decode(s[start:])
sel=plan['selection']
assert sel['scope']=='project' and not sel['writes_global_config'] and not sel['writes_global_active_profile'], sel
assert sel['profile'].startswith('concierge-pi-one-model-'), sel['profile']
routes={r['role']:r for r in sel['readiness']['routes']}
expected={'agent','dispatcher','default','task_agent','evaluator','flip_inference','flip_comparison','assigner','evolver','verification','triage','creator','compactor','placer','chat_compactor','coordinator_eval','reviewer','merger'}
assert set(routes)==expected, (set(routes), expected-set(routes))
weak={'evaluator','flip_inference','flip_comparison','assigner','triage','compactor','placer','chat_compactor','coordinator_eval','reviewer'}
for role,item in routes.items():
    assert item['route']==route, (role,item)
    assert item['reasoning']==('low' if role in weak else 'high'), (role,item)
blob=json.dumps(sel,sort_keys=True)
assert 'z-ai/glm' not in blob and 'deepseek-chat' not in blob and 'claude:' not in blob and 'codex:' not in blob, blob
assert plan['actions'].count(next(a for a in plan['actions'] if a.startswith('Select profile'))) == 1
PY

# Parse/conflict failures are structural, actionable, and pre-mutation.
for bad in 'openrouter:deepseek/deepseek-v4-flash' 'nex:openrouter:deepseek/deepseek-v4-flash' 'pi:openrouter:'; do
    if out=$($WORKSGOOD --project "$scratch/dry" setup --model "$bad" --dry-run 2>&1); then
        loud_fail "invalid/unsupported route succeeded: $bad"
    fi
    grep -q 'exact handler-first Pi route\|exact whitespace-free' <<<"$out" || loud_fail "bad route diagnostic not actionable: $out"
done
if out=$($WORKSGOOD --project "$scratch/dry" setup --model 2>&1); then
    loud_fail "missing --model value succeeded"
fi
grep -q 'value.*required' <<<"$out" || loud_fail "missing model diagnostic unclear: $out"
for conflict in '--without-ai' '--profile pi' '--strong-model pi:openrouter:other/model' '--weak-model pi:openrouter:other/model'; do
    if out=$(eval "'$WORKSGOOD' --project '$scratch/dry' setup --model '$M' --dry-run $conflict" 2>&1); then
        loud_fail "--model conflict succeeded: $conflict"
    fi
    grep -qi 'cannot be used with\|conflict' <<<"$out" || loud_fail "conflict diagnostic unclear: $out"
done

# Missing --profile with advanced overrides explains that profile names existing
# bases and points to the simple path.
if out=$($WORKSGOOD --project "$scratch/dry" setup --profile brand-new \
    --strong-model "$M" --weak-model "$M" --dry-run 2>&1); then
    loud_fail "missing advanced base profile unexpectedly succeeded"
fi
grep -q 'selects an existing reusable base definition' <<<"$out" || loud_fail "missing-profile explanation absent: $out"
grep -q 'worksgood setup --model' <<<"$out" || loud_fail "missing-profile simple-path recommendation absent"

# Cancellation presents exactly one immutable plan and one confirmation, with no
# model/profile picker and no write.
mk_repo "$scratch/cancel"
cancel_before=$(snapshot "$scratch/cancel")
session="one-model-cancel-$$"
tmux new-session -d -s "$session" -x 120 -y 40 \
    "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' XDG_CACHE_HOME='$XDG_CACHE_HOME' PATH='$PATH' '$WORKSGOOD' --project '$scratch/cancel' setup --model '$M'"
tmux set-option -t "$session" remain-on-exit on
wait_for_text "$session" 'Apply this exact plan?'
cancel_screen=$(tmux capture-pane -p -S - -t "$session")
[[ $(grep -c 'Immutable redacted plan:' <<<"$cancel_screen") = 1 ]] || loud_fail "cancel flow did not show exactly one plan"
! grep -q 'Worker/chat model:\|Selection:\|Route choice:' <<<"$cancel_screen" || loud_fail "one-model flow opened an extra picker"
tmux send-keys -t "$session" n Enter
wait_exit "$session"
[[ "$cancel_before" = "$(snapshot "$scratch/cancel")" ]] || loud_fail "cancelled confirmation mutated project"
[[ ! -e "$WG_GLOBAL_DIR" ]] || loud_fail "cancelled confirmation mutated global state"

# Unavailable Pi fails before the first transaction write and does not fall back.
mk_repo "$scratch/no-pi"
no_pi_before=$(snapshot "$scratch/no-pi")
mkdir -p "$scratch/empty-path"
session="one-model-no-pi-$$"
tmux new-session -d -s "$session" -x 240 -y 60
tmux set-option -t "$session" remain-on-exit on
tmux set-option -t "$session" history-limit 10000
tmux send-keys -t "$session" \
    "/usr/bin/env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' XDG_CACHE_HOME='$XDG_CACHE_HOME' PATH='$scratch/empty-path' '$WORKSGOOD' --project '$scratch/no-pi' setup --model '$M'; exit" Enter
wait_exit "$session"
no_pi_screen=$(tmux capture-pane -p -S - -t "$session")
grep -q 'Pi is unavailable' <<<"$no_pi_screen" || loud_fail "unavailable Pi diagnostic missing: $no_pi_screen"
grep -q 'no fallback was selected' <<<"$no_pi_screen" || loud_fail "unavailable Pi did not promise exact refusal"
[[ "$no_pi_before" = "$(snapshot "$scratch/no-pi")" ]] || loud_fail "unavailable Pi mutated project"
[[ ! -e "$WG_GLOBAL_DIR" ]] || loud_fail "unavailable Pi mutated global/plugin state"

# Actual attended command: one pasted model, one confirmation, project-only
# generated association, compatible plugin readiness, and exact daemon identity.
mk_repo "$scratch/repo"
session="one-model-apply-$$"
tmux new-session -d -s "$session" -x 140 -y 45 \
    "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' XDG_CACHE_HOME='$XDG_CACHE_HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' PATH='$PATH' '$WORKSGOOD' --project '$scratch/repo' setup --model '$M'"
tmux set-option -t "$session" remain-on-exit on
wait_for_text "$session" 'Apply this exact plan?'
apply_screen=$(tmux capture-pane -p -S - -t "$session")
apply_flat=$(tr -d '\n' <<<"$apply_screen")
[[ $(grep -c 'Immutable redacted plan:' <<<"$apply_screen") = 1 ]] || loud_fail "apply flow showed more than one plan"
grep -q "$M" <<<"$apply_flat" || loud_fail "plan omitted exact model"
grep -q 'Worker/chat.*effort high' <<<"$apply_flat" || loud_fail "plan omitted Worker/chat high policy"
grep -q 'Eval/assign/FLIP/weak roles.*effort low' <<<"$apply_flat" || loud_fail "plan omitted Eval/assign/FLIP low policy"
! grep -q 'Worker/chat model:\|Selection:\|Route choice:' <<<"$apply_screen" || loud_fail "one-model apply opened an extra picker"
tmux send-keys -t "$session" y Enter
wait_exit "$session"

selection="$scratch/repo/.wg/profile-selection.json"
state="$scratch/repo/.wg/service/state.json"
[[ -f "$selection" && -f "$state" && -f "$scratch/repo/.wg/concierge.json" ]] || loud_fail "committed concierge state missing"
profile=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["profile"])' "$selection")
[[ "$profile" == concierge-pi-one-model-* ]] || loud_fail "unexpected generated profile: $profile"
profile_file="$WG_GLOBAL_DIR/profiles/$profile.toml"
[[ -f "$profile_file" ]] || loud_fail "content-addressed reusable profile missing"
[[ ! -e "$WG_GLOBAL_DIR/active-profile" && ! -e "$WG_GLOBAL_DIR/config.toml" ]] || loud_fail "one-model setup wrote global config/active profile"
python3 - "$profile_file" "$M" <<'PY'
import sys,tomllib
p=tomllib.load(open(sys.argv[1],'rb')); route=sys.argv[2]
assert p['agent']['model']==route and p['dispatcher']['model']==route
for tier in ('fast','standard','premium'): assert p['tiers'][tier]==route
for name,cfg in p['models'].items():
    if isinstance(cfg,dict) and 'model' in cfg: assert cfg['model']==route,(name,cfg)
weak={'evaluator','flip_inference','flip_comparison','assigner','triage','compactor','placer','chat_compactor','reviewer'}
for name,cfg in p['models'].items():
    if isinstance(cfg,dict) and 'reasoning' in cfg:
        assert cfg['reasoning']==('low' if name in weak else 'high'),(name,cfg)
PY
plugin_status=$(HOME="$HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" XDG_CACHE_HOME="$XDG_CACHE_HOME" "$W" pi-plugin status)
grep -q 'build ready:.*yes' <<<"$plugin_status" || loud_fail "Pi plugin build not ready: $plugin_status"
grep -q 'console wired:.*yes' <<<"$plugin_status" || loud_fail "Pi plugin console wiring not ready: $plugin_status"
models=$(HOME="$HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" "$W" --dir "$scratch/repo/.wg" config --models)
[[ $(grep -cF "$M" <<<"$models") -ge 16 ]] || loud_fail "effective role report did not retain exact route: $models"
! grep -Eq 'claude:|codex:|nex:|z-ai/glm|deepseek-chat' <<<"$models" || loud_fail "effective roles contain another route: $models"
status=$($WORKSGOOD --project "$scratch/repo" status)
grep -q 'Service: Healthy' <<<"$status" || loud_fail "service identity not healthy: $status"
grep -q "$profile" <<<"$status" || loud_fail "status omitted selected generated profile"
pid1=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$state")
profile_sha=$(sha256sum "$profile_file")

# Repeating the exact attended invocation is idempotent: same generated bytes,
# same project profile, and authenticated daemon reuse (no duplicate/restart).
session="one-model-repeat-$$"
tmux new-session -d -s "$session" -x 140 -y 45 \
    "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' XDG_CACHE_HOME='$XDG_CACHE_HOME' PATH='$PATH' '$WORKSGOOD' --project '$scratch/repo' setup --model '$M'"
tmux set-option -t "$session" remain-on-exit on
wait_for_text "$session" 'Apply this exact plan?'
tmux send-keys -t "$session" y Enter
wait_exit "$session"
repeat_screen=$(tmux capture-pane -p -S - -t "$session")
grep -q 'Service reconcile: Reuse' <<<"$(tr -d '\n' <<<"$repeat_screen")" || loud_fail "repeat did not reuse exact daemon"
[[ "$profile" = "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["profile"])' "$selection")" ]] || loud_fail "repeat selected a different profile"
[[ "$profile_sha" = "$(sha256sum "$profile_file")" ]] || loud_fail "repeat changed generated profile bytes"
pid2=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$state")
[[ "$pid1" = "$pid2" ]] || loud_fail "repeat replaced/duplicated daemon"

$WORKSGOOD --project "$scratch/repo" stop >/dev/null

echo "PASS: worksgood --model exact one-model dry-run + one-plan/one-confirmation + project-only generated Pi profile + plugin/service identity + idempotent reuse"
