#!/usr/bin/env bash
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch="$(make_scratch)"
bindir="$scratch/bin"
fake_home="$scratch/home"
project="$scratch/project"
mkdir -p "$bindir" "$fake_home/.config/workgraph" "$project"
: >"$fake_home/.config/workgraph/config.toml"

cat >"$bindir/pi" <<'FAKE_PI'
#!/usr/bin/env bash
set -euo pipefail
log="${FAKE_PI_LOG:?}"
prompt="$(cat)"
printf 'ARGS %s\n' "$*" >>"$log"
printf 'STDIN %s\n' "$prompt" >>"$log"
printf 'STDIN_TTY %s STDOUT_TTY %s\n' "$([[ -t 0 ]] && echo yes || echo no)" "$([[ -t 1 ]] && echo yes || echo no)" >>"$log"
if [[ "$*" != *"--mode json"* || "$*" != *" -p "* || "$*" != *"--provider ${EXPECT_PROVIDER:?}"* || "$*" != *"--model ${EXPECT_MODEL:?}"* ]]; then
  echo "bad pi argv" >&2
  exit 2
fi
if [[ -z "${OPENROUTER_API_KEY:-}" ]]; then
  echo "credential error: missing OPENROUTER_API_KEY" >&2
  exit 42
fi
printf 'fake pi reply\n'
FAKE_PI
chmod +x "$bindir/pi"

(
    cd "$project" || exit 1
    env HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" PATH="$bindir:$PATH" \
        wg init -m claude:opus --no-agency >/dev/null 2>&1
) || loud_fail "wg init failed"

(
    cd "$project" || exit 1
    env HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" PATH="$bindir:$PATH" \
        wg config --auto-assign false --no-reload >/dev/null 2>&1
) || loud_fail "wg config failed"

(
    cd "$project" || exit 1
    env HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" PATH="$bindir:$PATH" \
        wg add "pi worker one shot" --id pi-worker-one-shot --no-place \
            --model pi:openrouter/test/model \
            -d "Worker prompt sentinel: PI_WORKER_PROMPT_OK" >/dev/null 2>&1
) || loud_fail "wg add failed"

(
    cd "$project" || exit 1
    env HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" PATH="$bindir:$PATH" \
        wg add "pi custom provider worker one shot" --id pi-custom-provider-one-shot --no-place \
            --model pi:lunaroute:glm-5.2-nvfp4 \
            -d "Worker prompt sentinel: PI_CUSTOM_PROVIDER_PROMPT_OK" >/dev/null 2>&1
) || loud_fail "wg add custom provider failed"

log="$scratch/pi-worker.log"
spawn_out="$scratch/spawn.out"

wait_for_wrapper_failure() {
    local output="$1"
    for _ in $(seq 1 40); do
        grep -q "\[wrapper\] Agent exited with code 42, marking task failed" "$output" 2>/dev/null && return 0
        sleep 0.25
    done
    return 1
}

(
    cd "$project" || exit 1
    env -u OPENROUTER_API_KEY -u OPENAI_API_KEY \
        HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" PATH="$bindir:$PATH" \
        EXPECT_PROVIDER="openrouter" EXPECT_MODEL="test/model" \
        FAKE_PI_LOG="$log" \
        wg spawn pi-worker-one-shot --executor pi --timeout 5s >"$spawn_out" 2>&1
) || loud_fail "wg spawn failed before fake pi could run: $(cat "$spawn_out")"

agent_dir=""
for d in "$project/.wg/agents"/agent-*; do
    [ -d "$d" ] || continue
    if grep -q '"task_id": "pi-worker-one-shot"' "$d/metadata.json" 2>/dev/null; then
        agent_dir="$d"
        break
    fi
done
[ -n "$agent_dir" ] || loud_fail "could not locate pi worker agent dir"

for _ in $(seq 1 40); do
    grep -q "credential error: missing OPENROUTER_API_KEY" "$agent_dir/output.log" 2>/dev/null && break
    sleep 0.25
done

grep -q "ARGS .*--mode json.* -p .*--provider openrouter.*--model test/model" "$log" || \
    loud_fail "fake pi did not receive one-shot -p/json provider/model argv: $(cat "$log" 2>/dev/null)"
grep -q "PI_WORKER_PROMPT_OK" "$log" || \
    loud_fail "fake pi did not receive WG prompt on stdin: $(cat "$log" 2>/dev/null)"
grep -q "STDIN_TTY no STDOUT_TTY no" "$log" || \
    loud_fail "fake pi worker was not run headlessly with piped stdio: $(cat "$log")"
grep -q "credential error: missing OPENROUTER_API_KEY" "$agent_dir/output.log" || \
    loud_fail "credential error did not surface in worker output: $(cat "$agent_dir/output.log" 2>/dev/null)"

wait_for_wrapper_failure "$agent_dir/output.log" || \
    loud_fail "pi worker wrapper did not mark the nonzero fake pi exit as failed: $(cat "$agent_dir/output.log" 2>/dev/null)"

custom_log="$scratch/pi-custom-provider.log"
custom_spawn_out="$scratch/custom-spawn.out"
(
    cd "$project" || exit 1
    env -u OPENROUTER_API_KEY -u OPENAI_API_KEY \
        HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" PATH="$bindir:$PATH" \
        EXPECT_PROVIDER="lunaroute" EXPECT_MODEL="glm-5.2-nvfp4" \
        FAKE_PI_LOG="$custom_log" \
        wg spawn pi-custom-provider-one-shot --executor pi --timeout 5s >"$custom_spawn_out" 2>&1
) || loud_fail "custom-provider wg spawn failed before fake pi could run: $(cat "$custom_spawn_out")"

custom_agent_dir=""
for d in "$project/.wg/agents"/agent-*; do
    [ -d "$d" ] || continue
    if grep -q '"task_id": "pi-custom-provider-one-shot"' "$d/metadata.json" 2>/dev/null; then
        custom_agent_dir="$d"
        break
    fi
done
[ -n "$custom_agent_dir" ] || loud_fail "could not locate pi custom-provider worker agent dir"

for _ in $(seq 1 40); do
    grep -q "credential error: missing OPENROUTER_API_KEY" "$custom_agent_dir/output.log" 2>/dev/null && break
    sleep 0.25
done

grep -q "ARGS .*--mode json.* -p .*--provider lunaroute.*--model glm-5.2-nvfp4" "$custom_log" || \
    loud_fail "fake pi did not receive custom provider/model argv: $(cat "$custom_log" 2>/dev/null)"
grep -q "PI_CUSTOM_PROVIDER_PROMPT_OK" "$custom_log" || \
    loud_fail "fake pi did not receive custom-provider WG prompt on stdin: $(cat "$custom_log" 2>/dev/null)"
if grep -q "not found in config or model cache" "$custom_spawn_out" "$custom_agent_dir/output.log" 2>/dev/null; then
    loud_fail "custom Pi provider was rejected by WG registry/cache before reaching pi"
fi

wait_for_wrapper_failure "$custom_agent_dir/output.log" || \
    loud_fail "pi custom-provider wrapper did not mark the nonzero fake pi exit as failed: $(cat "$custom_agent_dir/output.log" 2>/dev/null)"

echo "PASS: pi worker uses one-shot -p/json, keeps OpenRouter argv, passes custom Pi provider:model argv through, receives prompt via stdin, and fails nonzero on credential error"
