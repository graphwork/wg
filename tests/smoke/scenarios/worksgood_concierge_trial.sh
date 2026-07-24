#!/usr/bin/env bash
# Isolated candidate + real attended tmux flow for the profile-first concierge.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v cargo >/dev/null 2>&1 || loud_skip "MISSING CARGO" "cargo is required"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"

scratch=$(make_scratch)
cleanup_worksgood_daemons() {
    local state pid
    while IFS= read -r state; do
        pid=$(python3 -c 'import json,sys
try: print(json.load(open(sys.argv[1])).get("pid", ""))
except Exception: pass' "$state" 2>/dev/null || true)
        [[ "$pid" =~ ^[0-9]+$ ]] || continue
        kill "$pid" 2>/dev/null || true
        sleep 0.05
        kill -9 "$pid" 2>/dev/null || true
    done < <(find "$scratch" -path '*/.wg/service/state.json' -type f 2>/dev/null || true)
}
add_cleanup_hook cleanup_worksgood_daemons
repo_root="$(cd "$HERE/../../.." && pwd)"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
if [[ -n "${WG_SMOKE_CANDIDATE_DIR:-}" ]]; then
    CARGO_TARGET_DIR="$WG_SMOKE_CANDIDATE_DIR"
else
    CARGO_TARGET_DIR="$scratch/candidate-target"
    (cd "$repo_root" && CARGO_TARGET_DIR="$CARGO_TARGET_DIR" CARGO_BUILD_JOBS=1 cargo build --quiet --features worksgood-trial --bin wg --bin worksgood)
fi
export CARGO_TARGET_DIR
WORKSGOOD="$CARGO_TARGET_DIR/debug/worksgood"
W="$CARGO_TARGET_DIR/debug/wg"
[[ -x "$WORKSGOOD" && -x "$W" ]] || loud_fail "isolated candidate bundle missing"

export HOME="$scratch/home"
export WG_GLOBAL_DIR="$HOME/.wg"
export XDG_CACHE_HOME="$HOME/.cache"
export XDG_CONFIG_HOME="$HOME/.config"
unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER TMUX TMUX_TMPDIR
mkdir -p "$HOME" "$WG_GLOBAL_DIR" "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME"

mk_repo() {
    mkdir -p "$1/sub/dir"
    git -C "$1" init -q
}
snapshot() {
    find "$1" -type f -printf '%P %s ' -exec sha256sum {} \; 2>/dev/null | sort
}
assert_no_chat() {
    local graph=$1
    if [[ -f "$graph/graph.jsonl" ]] && grep -qE '"id"[[:space:]]*:[[:space:]]*"\.chat-' "$graph/graph.jsonl"; then
        loud_fail "opening the concierge/TUI implicitly created a chat in $graph"
    fi
}
wait_session_exit() {
    local session=$1
    # Debug candidate executables are large; each authenticated SHA-256 build
    # handshake may take several seconds on contended CI disks.
    for _ in $(seq 1 2400); do
        pane_dead=$(tmux list-panes -t "$session" -F '#{pane_dead}' 2>/dev/null || echo 1)
        [[ "$pane_dead" = 1 ]] && return 0
        sleep 0.05
    done
    loud_fail "tmux session did not exit: $session"
}
wait_tui() {
    local session=$1
    for _ in $(seq 1 2400); do
        tmux capture-pane -p -S - -t "$session" 2>/dev/null | grep -qE '↯|Workspace|New chat' && return 0
        sleep 0.05
    done
    loud_fail "TUI did not render in $session: $(tmux capture-pane -p -S - -t "$session" 2>/dev/null | tr '\n' '|')"
}
run_tui_lifecycle() {
    local session=$1 project=$2; shift 2
    tmux new-session -d -s "$session" -x 40 -y 20 \
        "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' XDG_CACHE_HOME='$XDG_CACHE_HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WORKSGOOD_PI_MODELS_JSON='${WORKSGOOD_PI_MODELS_JSON:-}' '$WORKSGOOD' --project '$project' $*"
    tmux set-option -t "$session" remain-on-exit on
    wait_tui "$session"
    tmux send-keys -t "$session" q
    wait_session_exit "$session"
}

# Help, non-TTY bare, strict dry-run, and default cancel are byte-for-byte
# non-mutating. PATH contains an unknown `wg`; it must never be consulted
# because the authenticated sibling W is absolute.
mk_repo "$scratch/cancel"
mkdir -p "$scratch/fake-bin"
cat >"$scratch/fake-bin/wg" <<EOF
#!/usr/bin/env bash
touch '$scratch/PATH_WG_EXECUTED'
exit 91
EOF
chmod +x "$scratch/fake-bin/wg"
before=$(snapshot "$scratch/cancel")
PATH="$scratch/fake-bin:$PATH" "$WORKSGOOD" --help >/dev/null
after=$(snapshot "$scratch/cancel")
[[ "$before" = "$after" ]] || loud_fail "--help mutated repository"
if PATH="$scratch/fake-bin:$PATH" "$WORKSGOOD" --project "$scratch/cancel" >/tmp/worksgood-nontty.$$ 2>&1; then
    loud_fail "bare non-TTY worksgood unexpectedly succeeded"
fi
grep -q 'ATTENDED_TTY_REQUIRED' /tmp/worksgood-nontty.$$ || loud_fail "stable non-TTY error missing"
rm -f /tmp/worksgood-nontty.$$
[[ ! -e "$scratch/PATH_WG_EXECUTED" ]] || loud_fail "unknown PATH wg was executed"
[[ "$before" = "$(snapshot "$scratch/cancel")" ]] || loud_fail "non-TTY bare mutated repository"
nested_plan=$(PATH="$scratch/fake-bin:$PATH" "$WORKSGOOD" --project "$scratch/cancel/sub/dir" --dry-run --profile codex setup)
grep -q "\"graph\": \"$scratch/cancel/.wg\"" <<<"$nested_plan" || loud_fail "nested repository resolution escaped nearest root"
[[ "$before" = "$(snapshot "$scratch/cancel")" ]] || loud_fail "strict dry-run mutated repository"
[[ ! -e "$WG_GLOBAL_DIR/profile-usage.jsonl" ]] || loud_fail "strict dry-run wrote history"

# A physical Git worktree is its own repository boundary, and a dirty target is
# observed without commit/stash/reset/cleanup.
mk_repo "$scratch/base"
echo base >"$scratch/base/tracked"
git -C "$scratch/base" add tracked
git -C "$scratch/base" -c user.name=Smoke -c user.email=smoke@example.invalid commit -qm base
git -C "$scratch/base" worktree add -q -b smoke-worktree "$scratch/worktree"
echo dirty >"$scratch/worktree/dirty-untracked"
mkdir -p "$scratch/worktree/nested"
worktree_before=$(snapshot "$scratch/worktree")
worktree_plan=$("$WORKSGOOD" --project "$scratch/worktree/nested" --dry-run --profile claude setup)
grep -q "\"graph\": \"$scratch/worktree/.wg\"" <<<"$worktree_plan" || loud_fail "worktree resolved to parent graph"
[[ "$worktree_before" = "$(snapshot "$scratch/worktree")" ]] || loud_fail "dirty worktree dry-run mutated files"

cancel_session="worksgood-cancel-$$"
tmux new-session -d -s "$cancel_session" -x 80 -y 24 \
    "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' '$WORKSGOOD' --project '$scratch/cancel'"
tmux set-option -t "$cancel_session" remain-on-exit on
for _ in $(seq 1 100); do
    tmux capture-pane -p -t "$cancel_session" | grep -q 'Selection:' && break
    sleep 0.05
done
tmux send-keys -t "$cancel_session" Enter
wait_session_exit "$cancel_session"
[[ "$before" = "$(snapshot "$scratch/cancel")" ]] || loud_fail "default cancel mutated repository"

# Absolute executable-identity boundary: relative, symlinked, and unknown external
# candidates are rejected without executing them.
if WORKSGOOD_W_EXECUTABLE=wg "$WORKSGOOD" --project "$scratch/cancel" status >/dev/null 2>&1; then
    loud_fail "relative WorksGood candidate accepted"
fi
ln -s "$W" "$scratch/wg-link"
if WORKSGOOD_W_EXECUTABLE="$scratch/wg-link" "$WORKSGOOD" --project "$scratch/cancel" status >/dev/null 2>&1; then
    loud_fail "symlinked WorksGood candidate accepted"
fi
cp "$W" "$scratch/unknown-wg"; chmod +x "$scratch/unknown-wg"
if WORKSGOOD_W_EXECUTABLE="$scratch/unknown-wg" "$WORKSGOOD" --project "$scratch/cancel" status >/dev/null 2>&1; then
    loud_fail "unknown out-of-bundle candidate accepted without receipt"
fi
unknown_sha="sha256:$(sha256sum "$scratch/unknown-wg" | awk '{print $1}')"
cat >"$scratch/receipt.json" <<EOF
{"product":"WorksGood","executable":"$scratch/unknown-wg","sha256":"$unknown_sha"}
EOF
WORKSGOOD_W_EXECUTABLE="$scratch/unknown-wg" WORKSGOOD_W_RECEIPT="$scratch/receipt.json" \
    "$WORKSGOOD" --project "$scratch/cancel" status >/dev/null || loud_fail "valid absolute package receipt rejected"

# Continue without AI is the user-visible profile choice and opens a real
# Termux-like 40-column setup-neutral TUI with no daemon.
mk_repo "$scratch/no-ai"
run_tui_lifecycle "worksgood-noai-$$" "$scratch/no-ai" "--without-ai --yes"
[[ -f "$scratch/no-ai/.wg/concierge.json" ]] || loud_fail "Continue without AI did not commit"
grep -q 'continue_without_ai' "$scratch/no-ai/.wg/concierge.json" || loud_fail "no-AI state missing"
[[ ! -e "$scratch/no-ai/.wg/service/state.json" ]] || loud_fail "Continue without AI started service"
assert_no_chat "$scratch/no-ai/.wg"

# Nex/local readiness distinguishes the built-in handler from an actually
# configured endpoint; dry-run reports the unavailable endpoint without writes.
mkdir -p "$WG_GLOBAL_DIR/profiles"
cat >"$WG_GLOBAL_DIR/profiles/missing-endpoint.toml" <<'TOML'
description = "endpoint readiness negative control"
[agent]
model = "nex:test-model"
[dispatcher]
model = "nex:test-model"
[tiers]
fast = "nex:test-model"
standard = "nex:test-model"
premium = "nex:test-model"
TOML
mk_repo "$scratch/endpoint"
endpoint_before=$(snapshot "$scratch/endpoint")
endpoint_plan=$("$WORKSGOOD" --project "$scratch/endpoint" --dry-run --profile missing-endpoint setup)
grep -q '"endpoint_status": "not configured"' <<<"$endpoint_plan" || loud_fail "missing endpoint readiness was not honest"
[[ "$endpoint_before" = "$(snapshot "$scratch/endpoint")" ]] || loud_fail "endpoint readiness dry-run mutated project"

# A prerequisite failure after confirmation leaves a redacted recovery marker,
# no committed lifecycle, and no fallback profile/service. Rollback clears only
# the pending exact project effect while preserving initialized graph/auth owner.
mk_repo "$scratch/resume"
failed_session="worksgood-failed-plugin-$$"
tmux new-session -d -s "$failed_session" -x 80 -y 24 \
    "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' XDG_CACHE_HOME='$XDG_CACHE_HOME' WG_PI_PLUGIN_DIR='$scratch/missing-plugin' '$WORKSGOOD' --project '$scratch/resume' --profile pi --strong-model pi:openai-codex:gpt-5.6-sol --weak-model pi:openai-codex:gpt-5.6-sol --strong-reasoning high --weak-reasoning low --yes setup"
tmux set-option -t "$failed_session" remain-on-exit on
wait_session_exit "$failed_session"
[[ -f "$scratch/resume/.wg/concierge-pending.json" ]] || loud_fail "failed setup omitted recovery marker"
[[ ! -e "$scratch/resume/.wg/concierge.json" ]] || loud_fail "failed setup committed lifecycle"
[[ ! -e "$scratch/resume/.wg/service/state.json" ]] || loud_fail "failed plugin setup started service"
rollback_session="worksgood-rollback-$$"
tmux new-session -d -s "$rollback_session" -x 80 -y 24 \
    "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' '$WORKSGOOD' --project '$scratch/resume' setup --rollback"
tmux set-option -t "$rollback_session" remain-on-exit on
for _ in $(seq 1 1200); do
    tmux capture-pane -p -t "$rollback_session" 2>/dev/null | grep -q 'Apply rollback?' && break
    sleep 0.05
done
tmux send-keys -t "$rollback_session" y Enter
wait_session_exit "$rollback_session"
[[ ! -e "$scratch/resume/.wg/concierge-pending.json" ]] || loud_fail "rollback kept pending marker"
[[ ! -e "$scratch/resume/.wg/profile-selection.json" ]] || loud_fail "rollback kept project selection"
[[ -e "$scratch/resume/.wg/graph.jsonl" ]] || loud_fail "rollback removed initialized graph"

# Credential-free Pi picker via Pi-owned RPC-shaped mock. Strong and weak are
# independently explicit, including reasoning; plugin materializes only after
# confirmation. setup intentionally does not open a TUI.
mk_repo "$scratch/pi"
echo seed >"$scratch/pi/tracked"
git -C "$scratch/pi" add tracked
git -C "$scratch/pi" -c user.name=Smoke -c user.email=smoke@example.invalid commit -qm seed
export WORKSGOOD_PI_MODELS_JSON='{"models":[{"provider":"openai-codex","id":"gpt-5.6-sol","name":"GPT 5.6","reasoning":true},{"provider":"openrouter","id":"deepseek/deepseek-chat","name":"DeepSeek","reasoning":true}]}'
tmux new-session -d -s "worksgood-pi-$$" -x 80 -y 24 \
    "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' XDG_CACHE_HOME='$XDG_CACHE_HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WORKSGOOD_PI_MODELS_JSON='$WORKSGOOD_PI_MODELS_JSON' '$WORKSGOOD' --project '$scratch/pi' --profile pi --strong-model pi:openai-codex:gpt-5.6-sol --weak-model pi:openrouter:deepseek/deepseek-chat --strong-reasoning xhigh --weak-reasoning low --yes setup"
tmux set-option -t "worksgood-pi-$$" remain-on-exit on
wait_session_exit "worksgood-pi-$$"
pi_selection="$scratch/pi/.wg/profile-selection.json"
[[ -f "$pi_selection" ]] || loud_fail "Pi project profile missing"
pi_name=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["profile"])' "$pi_selection")
pi_profile="$WG_GLOBAL_DIR/profiles/$pi_name.toml"
grep -q 'pi:openai-codex:gpt-5.6-sol' "$pi_profile" || loud_fail "Pi strong exact route missing"
grep -q 'pi:openrouter:deepseek/deepseek-chat' "$pi_profile" || loud_fail "Pi weak exact route missing"
grep -q 'reasoning = "xhigh"' "$pi_profile" || loud_fail "Pi strong reasoning missing"
grep -q 'reasoning = "low"' "$pi_profile" || loud_fail "Pi weak reasoning missing"
"$WORKSGOOD" --project "$scratch/pi" stop >/dev/null

# The selected project's resolved Worker/chat effort must reach the actual Pi
# process argv as a separate --thinking value (never encoded in the model).
fake_pi_bin="$scratch/fake-pi-bin"; pi_arg_log="$scratch/pi-argv.log"
mkdir -p "$fake_pi_bin"
cat >"$fake_pi_bin/pi" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$@" >"${PI_ARG_LOG:?}"
cat >/dev/null || true
exit 0
SH
chmod +x "$fake_pi_bin/pi"
PATH="$fake_pi_bin:$PATH" PI_ARG_LOG="$pi_arg_log" "$W" --dir "$scratch/pi/.wg" \
    add "WorksGood resolved effort argv probe" --id worksgood-effort-argv \
    -d "No explicit task reasoning: resolve from the selected concierge profile." >/dev/null
PATH="$fake_pi_bin:$PATH" PI_ARG_LOG="$pi_arg_log" "$W" --dir "$scratch/pi/.wg" \
    spawn worksgood-effort-argv --executor pi >/dev/null
for _ in $(seq 1 80); do [[ -s "$pi_arg_log" ]] && break; sleep 0.1; done
[[ -s "$pi_arg_log" ]] || loud_fail "selected concierge profile never reached fake Pi argv"
python3 - "$pi_arg_log" <<'PY'
import sys
args=open(sys.argv[1]).read().splitlines()
def pair(flag, value):
    return any(args[i:i+2] == [flag, value] for i in range(len(args)-1))
assert pair("--provider", "openai-codex"), args
assert pair("--model", "gpt-5.6-sol"), args
assert pair("--thinking", "xhigh"), args
assert not any("gpt-5.6-sol(" in arg for arg in args), args
PY

# Each non-Pi core profile applies through the same reusable project-profile
# owner. The first Codex run starts one daemon + TUI; returning bare reuses the
# authenticated PID. Changing to Claude reloads config without replacing PID.
mk_repo "$scratch/core"
core_first_session="worksgood-core-first-$$"
run_tui_lifecycle "$core_first_session" "$scratch/core" "--profile codex --yes"
tmux capture-pane -p -S - -t "$core_first_session" | grep -q 'Service remains detached and running' || loud_fail "post-TUI running guidance missing"
state="$scratch/core/.wg/service/state.json"
pid1=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$state")
status1=$("$WORKSGOOD" --project "$scratch/core" status)
grep -q 'Service: Healthy' <<<"$status1" || loud_fail "authenticated service status not healthy: $status1"
grep -q 'protocol=worksgood-service-identity-v1' <<<"$status1" || loud_fail "service protocol identity missing"
grep -qF "Repository: $scratch/core" <<<"$status1" || loud_fail "status omitted absolute repository identity"
grep -qF "Graph: $scratch/core/.wg" <<<"$status1" || loud_fail "status omitted absolute graph identity"
grep -qF "WorksGood executable: $W" <<<"$status1" || loud_fail "status omitted absolute executable identity"
python3 - "$state" "$W" "$scratch/core/.wg" <<'PY'
import json,os,sys
s=json.load(open(sys.argv[1])); i=s["identity"]
assert os.path.realpath(i["executable"]) == os.path.realpath(sys.argv[2]), i
assert os.path.realpath(i["canonical_graph"]) == os.path.realpath(sys.argv[3]), i
assert os.path.realpath(s["socket_path"]) == os.path.realpath(sys.argv[3] + "/service/daemon.sock"), s
assert i["protocol"] == "worksgood-service-identity-v1", i
assert i["selected_profile"].startswith("concierge-codex-"), i
assert i["selected_profile_fingerprint"].startswith("b3:"), i
PY
core_return_session="worksgood-core-return-$$"
run_tui_lifecycle "$core_return_session" "$scratch/core" ""
core_return_output=$(tmux capture-pane -p -S - -t "$core_return_session")
grep -q 'Service remains detached and running' <<<"$core_return_output" || loud_fail "returning post-TUI running guidance missing"
grep -q 'Resolved Worker/chat: codex:gpt-5.6-sol (effort high)' <<<"$core_return_output" || loud_fail "returning run omitted resolved Worker/chat effort"
grep -q 'Resolved Agency/FLIP/evaluation: codex:gpt-5.6-luna (effort low)' <<<"$core_return_output" || loud_fail "returning run omitted resolved Agency effort"
pid2=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$state")
[[ "$pid1" = "$pid2" ]] || loud_fail "returning worksgood duplicated/restarted healthy daemon"
kill -0 "$pid2" 2>/dev/null || loud_fail "service did not persist after returning TUI exit"
assert_no_chat "$scratch/core/.wg"

for profile in claude nex opencode; do
    session="worksgood-setup-$profile-$$"
    tmux new-session -d -s "$session" -x 80 -y 24 \
        "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' '$WORKSGOOD' --project '$scratch/core' --profile '$profile' --yes setup"
    tmux set-option -t "$session" remain-on-exit on
    wait_session_exit "$session"
    setup_output=$(tmux capture-pane -p -S - -t "$session")
    setup_flat=$(tr -d '\n' <<<"$setup_output")
    grep -q 'Service reconcile: Reload' <<<"$setup_flat" || loud_fail "$profile config/profile generation change did not reload compatible build: $setup_output"
    grep -q 'generation.*drift' <<<"$setup_flat" || loud_fail "$profile reload omitted exact generation reason: $setup_output"
    selected=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["profile"])' "$scratch/core/.wg/profile-selection.json")
    [[ "$selected" == "concierge-$profile-"* ]] || loud_fail "$profile core profile was not selected as an effort-pinned reusable profile: $selected"
    selected_profile="$WG_GLOBAL_DIR/profiles/$selected.toml"
    grep -q 'reasoning = "high"' "$selected_profile" || loud_fail "$profile Worker/chat effort default was not persisted"
    grep -q 'reasoning = "low"' "$selected_profile" || loud_fail "$profile Agency effort default was not persisted"
done
pid_reload=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$state")
[[ "$pid1" = "$pid_reload" ]] || loud_fail "reloadable profile changes replaced daemon"

# A different absolute path to IDENTICAL bytes is the same authenticated build:
# no restart loop merely because a receipt-authorized copy/hardlink spelling differs.
identity_before=$(sha256sum "$state" "$scratch/core/.wg/profile-selection.json" "$scratch/core/.wg/concierge.json")
alias_plan=$(WORKSGOOD_W_EXECUTABLE="$scratch/unknown-wg" WORKSGOOD_W_RECEIPT="$scratch/receipt.json" \
    "$WORKSGOOD" --project "$scratch/core" --dry-run --profile opencode setup)
grep -q '"service_action": "reuse"' <<<"$alias_plan" || loud_fail "same-build alias dry-run did not plan reuse: $alias_plan"
grep -q 'content build fingerprint all match' <<<"$alias_plan" || loud_fail "same-build reuse omitted exact reason"
[[ "$identity_before" = "$(sha256sum "$state" "$scratch/core/.wg/profile-selection.json" "$scratch/core/.wg/concierge.json")" ]] || loud_fail "reconcile dry-run mutated identity files"
alias_session="worksgood-same-build-alias-$$"
tmux new-session -d -s "$alias_session" -x 80 -y 24 \
    "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WORKSGOOD_W_EXECUTABLE='$scratch/unknown-wg' WORKSGOOD_W_RECEIPT='$scratch/receipt.json' '$WORKSGOOD' --project '$scratch/core'"
tmux set-option -t "$alias_session" remain-on-exit on
wait_tui "$alias_session"
tmux send-keys -t "$alias_session" q
wait_session_exit "$alias_session"
pid_alias=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$state")
[[ "$pid_alias" = "$pid1" ]] || loud_fail "same-build path alias restarted service"

# Same semantic version, DIFFERENT content build: dry-run names the reason and
# writes nothing; attended bare worksgood shows the diff, confirms, restarts,
# verifies the new handshake, then opens TUI.
different_w="$scratch/different-build-wg"
cp "$W" "$different_w"; printf '\n# distinct same-version build\n' >>"$different_w"; chmod +x "$different_w"
different_sha="sha256:$(sha256sum "$different_w" | awk '{print $1}')"
cat >"$scratch/different-receipt.json" <<EOF
{"product":"WorksGood","executable":"$different_w","sha256":"$different_sha"}
EOF
identity_before=$(sha256sum "$state" "$scratch/core/.wg/profile-selection.json" "$scratch/core/.wg/concierge.json")
different_plan=$(WORKSGOOD_W_EXECUTABLE="$different_w" WORKSGOOD_W_RECEIPT="$scratch/different-receipt.json" \
    "$WORKSGOOD" --project "$scratch/core" --dry-run --profile opencode setup)
grep -q '"service_action": "controlled_restart"' <<<"$different_plan" || loud_fail "different build dry-run did not plan restart: $different_plan"
grep -q 'binary/build/protocol mismatch' <<<"$different_plan" || loud_fail "different build dry-run omitted exact reason"
[[ "$identity_before" = "$(sha256sum "$state" "$scratch/core/.wg/profile-selection.json" "$scratch/core/.wg/concierge.json")" ]] || loud_fail "different-build dry-run mutated identity files"
build_session="worksgood-different-build-$$"
tmux new-session -d -s "$build_session" -x 80 -y 24 \
    "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WORKSGOOD_W_EXECUTABLE='$different_w' WORKSGOOD_W_RECEIPT='$scratch/different-receipt.json' '$WORKSGOOD' --project '$scratch/core'"
tmux set-option -t "$build_session" remain-on-exit on
for _ in $(seq 1 1200); do
    tmux capture-pane -p -S - -t "$build_session" 2>/dev/null | grep -q 'Controlled restart this graph' && break
    sleep 0.05
done
build_prompt=$(tmux capture-pane -p -S - -t "$build_session")
grep -q 'Service identity mismatch' <<<"$build_prompt" || loud_fail "same-version/different-build did not show identity diff: $build_prompt"
tmux send-keys -t "$build_session" y Enter
wait_tui "$build_session"
tmux send-keys -t "$build_session" q
wait_session_exit "$build_session"
pid_build=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$state")
[[ "$pid_build" != "$pid1" ]] || loud_fail "confirmed different build did not restart daemon"
python3 - "$state" "$different_w" "$different_sha" <<'PY'
import json,os,sys
s=json.load(open(sys.argv[1])); i=s["identity"]
assert os.path.realpath(i["executable"]) == os.path.realpath(sys.argv[2]), i
assert i["executable_sha256"] == sys.argv[3], i
PY

# Replace the running executable at the SAME path with another valid 0.1.0
# image. The startup fingerprint, not version/path/mtime, forces restart.
cp "$W" "$different_w.new"; chmod +x "$different_w.new"; mv -f "$different_w.new" "$different_w"
replaced_sha="sha256:$(sha256sum "$different_w" | awk '{print $1}')"
cat >"$scratch/different-receipt.json" <<EOF
{"product":"WorksGood","executable":"$different_w","sha256":"$replaced_sha"}
EOF
replaced_session="worksgood-replaced-build-$$"
tmux new-session -d -s "$replaced_session" -x 80 -y 24 \
    "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WORKSGOOD_W_EXECUTABLE='$different_w' WORKSGOOD_W_RECEIPT='$scratch/different-receipt.json' '$WORKSGOOD' --project '$scratch/core'"
tmux set-option -t "$replaced_session" remain-on-exit on
for _ in $(seq 1 1200); do
    tmux capture-pane -p -S - -t "$replaced_session" 2>/dev/null | grep -q 'Controlled restart this graph' && break
    sleep 0.05
done
tmux send-keys -t "$replaced_session" y Enter
wait_tui "$replaced_session"
tmux send-keys -t "$replaced_session" q
wait_session_exit "$replaced_session"
pid_replaced=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$state")
[[ "$pid_replaced" != "$pid_build" ]] || loud_fail "same-path replaced build did not restart"

# Deleted running executable identity is unverifiable: fail loudly, do not
# signal the daemon, and never open TUI. Restoring identical bytes then reuses
# the same live service without a path-only restart loop.
cp "$different_w" "$scratch/different-wg-backup"
rm "$different_w"
deleted_session="worksgood-deleted-running-$$"
tmux new-session -d -s "$deleted_session" -x 80 -y 24 \
    "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' '$WORKSGOOD' --project '$scratch/core'"
tmux set-option -t "$deleted_session" remain-on-exit on
wait_session_exit "$deleted_session"
deleted_output=$(tmux capture-pane -p -S - -t "$deleted_session")
grep -q 'SERVICE_IDENTITY_REFUSED' <<<"$deleted_output" || loud_fail "deleted running executable was not refused: $deleted_output"
grep -q 'TUI was not opened' <<<"$deleted_output" || loud_fail "deleted executable refusal omitted no-TUI guarantee"
kill -0 "$pid_replaced" 2>/dev/null || loud_fail "deleted executable refusal signalled live daemon"
cp "$scratch/different-wg-backup" "$different_w"; chmod +x "$different_w"
restored_alias_session="worksgood-restored-alias-$$"
tmux new-session -d -s "$restored_alias_session" -x 80 -y 24 \
    "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' '$WORKSGOOD' --project '$scratch/core'"
tmux set-option -t "$restored_alias_session" remain-on-exit on
wait_tui "$restored_alias_session"
tmux send-keys -t "$restored_alias_session" q
wait_session_exit "$restored_alias_session"
[[ "$pid_replaced" = "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$state")" ]] || loud_fail "restored same-build alias caused restart loop"

# A failed intended restart delegates stop to the real W, fails start, restores
# the authenticated prior build, and exits without opening stale TUI.
bad_w="$scratch/failing-wg"; bad_tui_marker="$scratch/BAD_TUI_OPENED"
cat >"$bad_w" <<EOF
#!/usr/bin/env bash
case " \$* " in
  *" service stop "*) exec "$W" "\$@" ;;
  *" service start "*) echo 'intentional replacement start failure' >&2; exit 42 ;;
  *" tui "*) touch "$bad_tui_marker"; exit 0 ;;
  *) exec "$W" "\$@" ;;
esac
EOF
chmod +x "$bad_w"
bad_sha="sha256:$(sha256sum "$bad_w" | awk '{print $1}')"
cat >"$scratch/bad-receipt.json" <<EOF
{"product":"WorksGood","executable":"$bad_w","sha256":"$bad_sha"}
EOF
failed_restart_session="worksgood-failed-restart-$$"
tmux new-session -d -s "$failed_restart_session" -x 100 -y 30 \
    "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WORKSGOOD_W_EXECUTABLE='$bad_w' WORKSGOOD_W_RECEIPT='$scratch/bad-receipt.json' '$WORKSGOOD' --project '$scratch/core'"
tmux set-option -t "$failed_restart_session" remain-on-exit on
for _ in $(seq 1 1200); do
    tmux capture-pane -p -S - -t "$failed_restart_session" 2>/dev/null | grep -q 'Controlled restart this graph' && break
    sleep 0.05
done
tmux send-keys -t "$failed_restart_session" y Enter
wait_session_exit "$failed_restart_session"
failed_restart_output=$(tmux capture-pane -p -S - -t "$failed_restart_session")
failed_restart_flat=$(tr -d '\n' <<<"$failed_restart_output")
grep -q 'was.*restored' <<<"$failed_restart_flat" || loud_fail "failed restart did not restore prior build: $failed_restart_output"
grep -q 'TUI was not opened' <<<"$failed_restart_flat" || loud_fail "failed restart omitted no-TUI guarantee: $failed_restart_output"
[[ ! -e "$bad_tui_marker" ]] || loud_fail "failed replacement opened stale TUI"
recovered_pid=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$state")
kill -0 "$recovered_pid" 2>/dev/null || loud_fail "authenticated prior service was not recovered"
assert_no_chat "$scratch/core/.wg"

# A live state/socket handshake that names another graph is foreign. Refuse it
# without signalling the process or entering TUI, then restore exact state.
cp "$state" "$scratch/foreign-state-backup.json"
python3 - "$state" <<'PY'
import json,sys
p=sys.argv[1]; s=json.load(open(p)); i=s["identity"]
i["canonical_graph"]="/foreign/graph"; i["graph_digest"]="sha256:" + "f"*64
json.dump(s,open(p,"w"))
PY
foreign_session="worksgood-foreign-identity-$$"
tmux new-session -d -s "$foreign_session" -x 100 -y 30 \
    "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' '$WORKSGOOD' --project '$scratch/core'"
tmux set-option -t "$foreign_session" remain-on-exit on
wait_session_exit "$foreign_session"
foreign_output=$(tmux capture-pane -p -S - -t "$foreign_session")
grep -q 'SERVICE_IDENTITY_REFUSED' <<<"$foreign_output" || loud_fail "foreign identity was not refused: $foreign_output"
grep -q 'foreign canonical graph identity' <<<"$foreign_output" || loud_fail "foreign refusal omitted exact reason"
kill -0 "$recovered_pid" 2>/dev/null || loud_fail "foreign identity refusal signalled daemon"
cp "$scratch/foreign-state-backup.json" "$state"

# Proven stale PID state is repaired rather than signalled. Then two concurrent
# returning invocations serialize only reconcile, open independent TUI clients,
# and still leave exactly one daemon.
cp "$state" "$scratch/stale-state.json"
"$WORKSGOOD" --project "$scratch/core" stop >/dev/null
python3 - "$scratch/stale-state.json" "$state" <<'PY'
import json,sys
s=json.load(open(sys.argv[1])); s["pid"]=999999; s["pid_start_identity"]="proc-start:impossible"
json.dump(s,open(sys.argv[2],"w"))
PY
run_tui_lifecycle "worksgood-stale-$$" "$scratch/core" ""
stale_pid=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$state")
[[ "$stale_pid" != 999999 ]] || loud_fail "stale PID was not repaired"
"$WORKSGOOD" --project "$scratch/core" stop >/dev/null

concurrent_a="worksgood-concurrent-a-$$"; concurrent_b="worksgood-concurrent-b-$$"
for session in "$concurrent_a" "$concurrent_b"; do
    tmux new-session -d -s "$session" -x 40 -y 20 \
        "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' '$WORKSGOOD' --project '$scratch/core'"
    tmux set-option -t "$session" remain-on-exit on
done
wait_tui "$concurrent_a"; wait_tui "$concurrent_b"
concurrent_pid=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$state")
kill -0 "$concurrent_pid" 2>/dev/null || loud_fail "concurrent lifecycle daemon not alive"
daemon_count=$(ps -eo args= | grep -F "$W --dir $scratch/core/.wg service daemon" | grep -v grep | wc -l)
[[ "$daemon_count" = 1 ]] || loud_fail "concurrent worksgood created $daemon_count daemons"
tmux send-keys -t "$concurrent_a" q; tmux send-keys -t "$concurrent_b" q
wait_session_exit "$concurrent_a"; wait_session_exit "$concurrent_b"

# Explicit restart warns but preserves detached-work policy, then proves a new
# PID/build/config/socket handshake. Finally stop leaves no duplicate daemon.
restart_session="worksgood-restart-$$"
tmux new-session -d -s "$restart_session" -x 80 -y 24 \
    "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' '$WORKSGOOD' --project '$scratch/core' --yes restart"
tmux set-option -t "$restart_session" remain-on-exit on
wait_session_exit "$restart_session"
pid3=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$state")
[[ "$pid3" != "$pid1" ]] || loud_fail "explicit restart did not replace daemon PID"
"$WORKSGOOD" --project "$scratch/core" status | grep -q 'Service: Healthy' || loud_fail "post-restart handshake failed"
"$WORKSGOOD" --project "$scratch/core" stop >/dev/null
kill -0 "$pid3" 2>/dev/null && loud_fail "worksgood stop left daemon alive"

# The candidate target is bounded to the scratch tree and is removed by the
# smoke harness cleanup; no cargo install, PATH install, alias, or release
# artifact was touched.
echo "PASS: worksgood exact/same-build reuse, generation reload, content-build restart, replaced/deleted/foreign fail-closed, failed-restart recovery/no stale TUI, concurrency, explicit effort+Pi argv, strict dry-run/no-chat, absolute identity, and service persistence"
