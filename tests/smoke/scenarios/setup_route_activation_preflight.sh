#!/usr/bin/env bash
# Installed/candidate CLI terminal flow for setup route activation + bounded Pi readiness.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v script >/dev/null 2>&1 || loud_skip "MISSING SCRIPT" "script(1) is required"

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
cat >"$scratch/fake-bin/pi" <<'SH'
#!/bin/sh
printf 'unexpected setup invocation: %s\n' "$*" >>"${PI_INVOCATION_LOG:?}"
exit 0
SH
chmod +x "$scratch/fake-bin/pi"
: >"$scratch/pi-invocations.log"
route='pi:openrouter:test/setup-route-activation'

# Real PTY terminal command from a clean HOME. Fake Pi proves executable discovery
# is credential-free and that setup does not pretend to perform a provider call.
cmd="cd '$scratch/project' && env -i HOME='$scratch/home' WG_GLOBAL_DIR='$scratch/home/.wg' XDG_CACHE_HOME='$scratch/home/.cache' USER=test TERM=xterm PATH='$scratch/fake-bin:/usr/bin:/bin' PI_INVOCATION_LOG='$scratch/pi-invocations.log' OPENROUTER_API_KEY='must-not-be-used' HTTP_PROXY='http://127.0.0.1:9' HTTPS_PROXY='http://127.0.0.1:9' ALL_PROXY='http://127.0.0.1:9' NO_PROXY='' '$W' setup --route pi --yes --model '$route'"
script -qec "$cmd" "$scratch/setup.typescript" >/dev/null

[[ "$(cat "$scratch/home/.wg/active-profile")" = pi ]] \
    || loud_fail "setup did not activate the selected pi profile"
grep -qF "model = \"$route\"" "$scratch/home/.wg/config.toml" \
    || loud_fail "setup did not preserve the exact configured route"
grep -q 'Profile: ACTIVE (`pi`' "$scratch/setup.typescript" \
    || loud_fail "terminal output did not report active profile"
grep -q 'Pi handler: AVAILABLE' "$scratch/setup.typescript" \
    || loud_fail "terminal output did not report fake Pi availability"
grep -q 'pi-worksgood: ready (compat' "$scratch/setup.typescript" \
    || loud_fail "noninteractive setup did not ensure the compatible Pi plugin"
grep -q 'Pi auth/model: NOT VERIFIED' "$scratch/setup.typescript" \
    || loud_fail "terminal output silently implied auth/model readiness"
grep -q 'run `pi`, use `/login`' "$scratch/setup.typescript" \
    || loud_fail "terminal output omitted actionable Pi-owned login check"
grep -q 'no cross-provider fallback' "$scratch/setup.typescript" \
    || loud_fail "terminal output omitted exact-route/no-fallback boundary"
[[ ! -s "$scratch/pi-invocations.log" ]] \
    || loud_fail "setup invoked Pi/provider while claiming a bounded preflight: $(cat "$scratch/pi-invocations.log")"

models=$(env -i HOME="$scratch/home" WG_GLOBAL_DIR="$scratch/home/.wg" USER=test PATH="/usr/bin:/bin" \
    "$W" --dir "$scratch/project/.wg" config --models)
default_line=$(grep -E '^  default ' <<<"$models")
task_line=$(grep -E '^  task_agent ' <<<"$models")
grep -qF "$route" <<<"$default_line" || loud_fail "effective default route drifted: $default_line"
grep -qF "$route" <<<"$task_line" || loud_fail "effective task-agent route drifted: $task_line"

# Unavailable handler case remains explicit and action-oriented. Configuration is
# selected exactly (graph/config work can continue), but output never calls it ready.
mkdir -p "$scratch/missing-home" "$scratch/missing-project"
missing_cmd="cd '$scratch/missing-project' && env -i HOME='$scratch/missing-home' WG_GLOBAL_DIR='$scratch/missing-home/.wg' XDG_CACHE_HOME='$scratch/missing-home/.cache' USER=test TERM=xterm PATH='$scratch/empty-path' '$W' setup --route pi --yes --model '$route'"
script -qec "$missing_cmd" "$scratch/missing.typescript" >/dev/null
grep -q 'Pi handler: UNAVAILABLE on PATH' "$scratch/missing.typescript" \
    || loud_fail "unavailable Pi was not reported"
grep -q 'install Pi, then rerun `wg setup`' "$scratch/missing.typescript" \
    || loud_fail "unavailable Pi omitted its recovery command"
grep -q 'Pi auth/model: NOT VERIFIED' "$scratch/missing.typescript" \
    || loud_fail "unavailable Pi output claimed auth/model access"
grep -q 'no fallback was chosen' "$scratch/missing.typescript" \
    || loud_fail "unavailable Pi output omitted no-fallback guarantee"

echo "PASS: setup terminal flow activates pi + exact routes and reports bounded available/unavailable/auth/model readiness without provider access or fallback"
