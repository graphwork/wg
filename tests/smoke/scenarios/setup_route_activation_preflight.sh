#!/usr/bin/env bash
# Installed/candidate CLI terminal flow for setup route activation + bounded Pi readiness.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v script >/dev/null 2>&1 || loud_skip "MISSING SCRIPT" "script(1) is required"
command -v strace >/dev/null 2>&1 || loud_skip "MISSING STRACE" "strace is required to prove setup makes no provider/network request"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required for the real worker isolation flow"

repo_root="$(cd "$HERE/../../.." && pwd)"
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then
    W="$WG_SMOKE_CANDIDATE_BIN"
elif [[ -x "$repo_root/target/debug/wg" ]]; then
    W="$repo_root/target/debug/wg"
else
    loud_skip "MISSING CANDIDATE" "set WG_SMOKE_CANDIDATE_BIN or build target/debug/wg"
fi
[[ -x "$W" ]] || loud_fail "candidate wg is not executable: $W"

scratch=$(make_scratch)
mkdir -p "$scratch/home" "$scratch/project" "$scratch/fake-bin" "$scratch/empty-path"
cat >"$scratch/fake-bin/pi" <<SH
#!/bin/sh
printf 'unexpected setup invocation: %s\\n' "\$*" >>"$scratch/pi-invocations.log"
exit 0
SH
chmod +x "$scratch/fake-bin/pi"
: >"$scratch/pi-invocations.log"
route='pi:openrouter:test/setup-route-activation'
git -C "$scratch/project" init -q
git -C "$scratch/project" config user.email smoke@example.com
git -C "$scratch/project" config user.name Smoke
printf 'setup activation fixture\n' >"$scratch/project/README.md"
git -C "$scratch/project" add README.md
git -C "$scratch/project" commit -qm init
base_env=(env -i HOME="$scratch/home" WG_GLOBAL_DIR="$scratch/home/.wg" XDG_CACHE_HOME="$scratch/home/.cache" USER=test TERM=xterm PATH="$scratch/fake-bin:/usr/bin:/bin" PI_INVOCATION_LOG="$scratch/pi-invocations.log" WG_SMOKE_RUN_ID="$WG_SMOKE_RUN_ID" WG_SMOKE_SCENARIO="$WG_SMOKE_SCENARIO")
"${base_env[@]}" "$W" --dir "$scratch/project/.wg" init --no-agency >/dev/null
cleanup_service() {
    "${base_env[@]}" "$W" --dir "$scratch/project/.wg" service stop --force >/dev/null 2>&1 || true
}
add_cleanup_hook cleanup_service

# Real PTY terminal command from a clean HOME. Fake Pi proves executable discovery
# is credential-free and that setup does not pretend to perform a provider call.
cmd="cd '$scratch/project' && env -i HOME='$scratch/home' WG_GLOBAL_DIR='$scratch/home/.wg' XDG_CACHE_HOME='$scratch/home/.cache' USER=test TERM=xterm PATH='$scratch/fake-bin:/usr/bin:/bin' PI_INVOCATION_LOG='$scratch/pi-invocations.log' WG_SMOKE_RUN_ID='$WG_SMOKE_RUN_ID' WG_SMOKE_SCENARIO='$WG_SMOKE_SCENARIO' OPENROUTER_API_KEY='must-not-be-used' HTTP_PROXY='http://127.0.0.1:9' HTTPS_PROXY='http://127.0.0.1:9' ALL_PROXY='http://127.0.0.1:9' NO_PROXY='' strace -f -qq -e trace=connect -o '$scratch/network.trace' '$W' setup --route pi --yes --model '$route'"
script -qec "$cmd" "$scratch/setup.typescript" >/dev/null

[[ ! -e "$scratch/home/.wg/active-profile" ]] \
    || loud_fail "project-local setup unexpectedly rewrote legacy global active-profile state"
[[ ! -e "$scratch/home/.wg/config.toml" ]] \
    || loud_fail "project-local setup unexpectedly wrote legacy global routing"
grep -qF "model = \"$route\"" "$scratch/project/worksgood.toml" \
    || loud_fail "setup did not preserve the exact project-local route"
grep -q 'Profile: project-local route is effective' "$scratch/setup.typescript" \
    || loud_fail "terminal output did not report the effective project-local route"
grep -q 'Pi handler: AVAILABLE' "$scratch/setup.typescript" \
    || loud_fail "terminal output did not report fake Pi availability"
grep -q 'pi-worksgood: hermetic JIT at worker spawn; Console settings unchanged' "$scratch/setup.typescript" \
    || loud_fail "noninteractive setup did not report hermetic candidate-scoped Pi wiring"
grep -q 'Pi auth/model: NOT VERIFIED' "$scratch/setup.typescript" \
    || loud_fail "terminal output silently implied auth/model readiness"
grep -q 'run `pi`, use `/login`' "$scratch/setup.typescript" \
    || loud_fail "terminal output omitted actionable Pi-owned login check"
grep -q 'no cross-provider fallback' "$scratch/setup.typescript" \
    || loud_fail "terminal output omitted exact-route/no-fallback boundary"
[[ ! -s "$scratch/pi-invocations.log" ]] \
    || loud_fail "setup invoked Pi/provider while claiming a bounded preflight: $(cat "$scratch/pi-invocations.log")"
if grep -Eq 'sa_family=AF_INET6?|sin6?_family=AF_INET6?' "$scratch/network.trace"; then
    loud_fail "setup made an IP network/provider request during bounded preflight: $(cat "$scratch/network.trace")"
fi

models=$(cd "$scratch/project" && env -i HOME="$scratch/home" WG_GLOBAL_DIR="$scratch/home/.wg" USER=test PATH="/usr/bin:/bin" \
    WG_SMOKE_RUN_ID="$WG_SMOKE_RUN_ID" WG_SMOKE_SCENARIO="$WG_SMOKE_SCENARIO" \
    "$W" --dir "$scratch/project/.wg" config --models)
for tier_label in 'project default' 'effective strong' 'effective weak'; do
    tier_line=$(grep -E "^  ${tier_label}[[:space:]]" <<<"$models")
    grep -qF "$route" <<<"$tier_line" \
        || loud_fail "${tier_label} crossed away from the one approved route: $tier_line"
done
for role_name in default task_agent evaluator flip_inference flip_comparison assigner evolver verification triage creator compactor coordinator_eval placer chat_compactor reviewer merger; do
    role_line=$(grep -E "^  ${role_name}[[:space:]]" <<<"$models")
    grep -qF "$route" <<<"$role_line" \
        || loud_fail "first-task/review role ${role_name} crossed away from the one approved route: $role_line"
done
if grep -E '^  (project default|effective strong|effective weak|default|task_agent|evaluator|flip_inference|flip_comparison|assigner|evolver|verification|triage|creator|compactor|coordinator_eval|placer|chat_compactor|reviewer|merger)[[:space:]]' <<<"$models" | grep -vF "$route"; then
    loud_fail "setup projected at least one first-task/review role onto a second provider: $models"
fi

# A real terminal invocation must refuse a split-brain project target before
# it can write usage/config bytes to either project. This is the human typo:
# the shell is in project A while --dir names project B.
mkdir -p "$scratch/project-a" "$scratch/project-b"
"${base_env[@]}" "$W" --dir "$scratch/project-a/.wg" init --no-agency >/dev/null
"${base_env[@]}" "$W" --dir "$scratch/project-b/.wg" init --no-agency >/dev/null
before_b=$(find "$scratch/project-b" -type f -print0 | sort -z | xargs -0 sha256sum)
mismatch_cmd="cd '$scratch/project-a' && env -i HOME='$scratch/home' WG_GLOBAL_DIR='$scratch/home/.wg' XDG_CACHE_HOME='$scratch/home/.cache' USER=test TERM=xterm PATH='$scratch/fake-bin:/usr/bin:/bin' WG_SMOKE_RUN_ID='$WG_SMOKE_RUN_ID' WG_SMOKE_SCENARIO='$WG_SMOKE_SCENARIO' '$W' --dir '$scratch/project-b/.wg' setup --route pi --yes --model '$route'"
if script -qec "$mismatch_cmd" "$scratch/mismatch.typescript" >/dev/null; then
    loud_fail "setup accepted disagreeing --dir/CWD project targets"
fi
grep -q 'WG-SETUP-PROJECT-TARGET-MISMATCH' "$scratch/mismatch.typescript" \
    || loud_fail "setup mismatch did not fail with the stable actionable diagnostic: $(cat "$scratch/mismatch.typescript")"
grep -qF "$scratch/project-a" "$scratch/mismatch.typescript" \
    || loud_fail "setup mismatch omitted the CWD project"
grep -qF "$scratch/project-b" "$scratch/mismatch.typescript" \
    || loud_fail "setup mismatch omitted the --dir project"
[[ ! -e "$scratch/project-a/worksgood.toml" ]] \
    || loud_fail "setup mismatch wrote the CWD project's config"
[[ ! -e "$scratch/project-b/worksgood.toml" ]] \
    || loud_fail "setup mismatch wrote the --dir project's config"
after_b=$(find "$scratch/project-b" -type f -print0 | sort -z | xargs -0 sha256sum)
[[ "$after_b" == "$before_b" ]] \
    || loud_fail "setup mismatch mutated the wrong --dir project before refusing"

# Exercise checked reload against a real running daemon (max-agents=0 keeps
# this phase deterministic), then drive the first LLM-backed command manually.
(
    cd "$scratch/project"
    "${base_env[@]}" "$W" --dir "$scratch/project/.wg" service start --max-agents 0 \
        --no-coordinator-agent --no-supervise >/dev/null
)
(
    cd "$scratch/project"
    "${base_env[@]}" "$W" setup --route pi --yes --model "$route"
) >"$scratch/live-reload.log" 2>&1 || loud_fail "setup could not reload its running daemon: $(cat "$scratch/live-reload.log")"
grep -q 'Daemon reloaded' "$scratch/live-reload.log" \
    || loud_fail "setup did not confirm checked live reload: $(cat "$scratch/live-reload.log")"

"${base_env[@]}" "$W" --dir "$scratch/project/.wg" add "setup activation probe" \
    --id setup-activation-probe --model "$route" -d $'Runtime route probe.\n\n## Validation\n- fake Pi receives the exact provider/model' >/dev/null
"${base_env[@]}" "$W" --dir "$scratch/project/.wg" publish setup-activation-probe --only >/dev/null
"${base_env[@]}" "$W" --dir "$scratch/project/.wg" spawn-task setup-activation-probe >/dev/null
for _ in $(seq 1 400); do
    [[ -s "$scratch/pi-invocations.log" ]] && break
    sleep 0.05
done
[[ -s "$scratch/pi-invocations.log" ]] || loud_fail "first LLM-backed task never reached fake Pi"
grep -q -- '--provider openrouter' "$scratch/pi-invocations.log" \
    || loud_fail "first worker did not retain provider: $(cat "$scratch/pi-invocations.log")"
grep -q -- '--model test/setup-route-activation' "$scratch/pi-invocations.log" \
    || loud_fail "first worker did not retain model: $(cat "$scratch/pi-invocations.log")"
cleanup_service

# Unavailable handler case remains explicit and action-oriented. Configuration is
# selected exactly (graph/config work can continue), but output never calls it ready.
mkdir -p "$scratch/missing-home" "$scratch/missing-project"
missing_cmd="cd '$scratch/missing-project' && env -i HOME='$scratch/missing-home' WG_GLOBAL_DIR='$scratch/missing-home/.wg' XDG_CACHE_HOME='$scratch/missing-home/.cache' USER=test TERM=xterm PATH='$scratch/empty-path' WG_SMOKE_RUN_ID='$WG_SMOKE_RUN_ID' WG_SMOKE_SCENARIO='$WG_SMOKE_SCENARIO' '$W' setup --route pi --yes --model '$route'"
script -qec "$missing_cmd" "$scratch/missing.typescript" >/dev/null
grep -q 'Pi handler: UNAVAILABLE on PATH' "$scratch/missing.typescript" \
    || loud_fail "unavailable Pi was not reported"
grep -q 'install Pi, then rerun `wg setup`' "$scratch/missing.typescript" \
    || loud_fail "unavailable Pi omitted its recovery command"
grep -q 'Pi auth/model: NOT VERIFIED' "$scratch/missing.typescript" \
    || loud_fail "unavailable Pi output claimed auth/model access"
grep -q 'no fallback was chosen' "$scratch/missing.typescript" \
    || loud_fail "unavailable Pi output omitted no-fallback guarantee"

echo "PASS: setup terminal flow keeps every first-task/review role on one exact Pi route, rejects --dir/CWD project disagreement before writes, and reports bounded readiness without provider access or fallback"
