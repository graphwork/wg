#!/usr/bin/env bash
# Explicit native Codex worker regression. Proves the built-in direct profile
# and a per-task override both reach `codex exec`, preserve exact route
# provenance/usage, and propagate native failure without crossing to Pi/Claude.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

scratch=$(make_scratch)
home="$scratch/home"
bin="$scratch/bin"
project="$scratch/project"
mkdir -p "$home/.config/workgraph" "$bin" "$project"
: >"$home/.config/workgraph/config.toml"

cat >"$bin/codex" <<'SH'
#!/usr/bin/env bash
set -u
{
  printf '%s\n' '--- invocation ---'
  printf 'arg=%s\n' "$@"
  printf 'WG_EXECUTOR_TYPE=%s\n' "${WG_EXECUTOR_TYPE:-}"
  printf 'WG_MODEL=%s\n' "${WG_MODEL:-}"
  printf 'WG_REASONING=%s\n' "${WG_REASONING:-}"
} >>"$HOME/codex-invocations.log"
cat >/dev/null
case " $* " in
  *" --model fail-native-opaque "*)
    echo 'intentional native Codex failure' >&2
    exit 42
    ;;
esac
artifact="$HOME/${WG_TASK_ID:-codex}-artifact.txt"
printf 'native codex worker artifact\n' >"$artifact"
wg artifact "${WG_TASK_ID}" "$artifact" >/dev/null 2>&1 || true
printf '%s\n' '{"type":"thread.started","thread_id":"fake-codex-thread"}'
printf '%s\n' '{"type":"item.completed","item":{"id":"item-1","type":"command_execution","command":"wg artifact","aggregated_output":"recorded","exit_code":0,"status":"completed"}}'
printf '%s\n' '{"type":"item.completed","item":{"id":"item-2","type":"agent_message","text":"native codex complete"}}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":17,"cached_input_tokens":3,"output_tokens":5,"reasoning_output_tokens":2}}'
SH
chmod +x "$bin/codex"

cat >"$bin/pi" <<'SH'
#!/usr/bin/env bash
printf 'PI INVOKED: %s\n' "$*" >>"$HOME/cross-system.log"
exit 91
SH
cat >"$bin/claude" <<'SH'
#!/usr/bin/env bash
printf 'CLAUDE INVOKED: %s\n' "$*" >>"$HOME/cross-system.log"
exit 92
SH
chmod +x "$bin/pi" "$bin/claude"

export HOME="$home"
export XDG_CONFIG_HOME="$home/.config"
export PATH="$bin:$PATH"
unset WG_EXECUTOR_TYPE WG_MODEL WG_TIER WG_AGENT_ID

cd "$project"
wg init --no-agency >init.log 2>&1 || loud_fail "graph-only init failed: $(tail -30 init.log)"
wg config --auto-assign false --auto-evaluate false --no-reload \
  >config.log 2>&1 || loud_fail "safe recovery config failed: $(cat config.log)"
wg config set dispatcher.max_incomplete_retries 0 >>config.log 2>&1 \
  || loud_fail "failed to disable incomplete retries: $(cat config.log)"
wg config set agency.flip_enabled false >>config.log 2>&1 \
  || loud_fail "failed to keep FLIP disabled during Codex recovery: $(cat config.log)"
wg config set dispatcher.worktree_isolation false >>config.log 2>&1 \
  || loud_fail "failed to disable fixture worktrees: $(cat config.log)"

wait_status() {
  local task="$1" expected="$2" status=""
  for _ in $(seq 1 160); do
    status=$(wg show "$task" --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status", ""))' 2>/dev/null || true)
    [[ "$status" == "$expected" ]] && return 0
    sleep 0.25
  done
  local detail
  detail=$(wg show "$task" --json 2>&1 || true)
  loud_fail "task $task did not reach $expected (last=$status); detail=$detail; daemon=$(tail -80 "$WG_SMOKE_DAEMON_DIR/service/daemon.log" 2>/dev/null)"
}

# Direct profile selection is built in and must render native Codex distinctly
# from Pi's openai-codex provider route.
wg profile use codex --no-reload >profile-codex.log 2>&1 \
  || loud_fail "direct Codex profile activation failed: $(cat profile-codex.log)"
models=$(wg config --models 2>&1) || loud_fail "Codex config --models failed: $models"
grep -q 'codex:gpt-5.6-sol' <<<"$models" || loud_fail "direct Codex route missing: $models"
grep -q 'codex.*codex:gpt-5.6-sol' <<<"$models" || loud_fail "handler/route identity is ambiguous: $models"
if grep -q 'pi:openai-codex:gpt-5.6-sol' <<<"$models"; then
  loud_fail "direct Codex profile was rendered as Pi Codex: $models"
fi

wg add 'direct Codex profile worker' --id codex-profile-worker --reasoning high --independent \
  -d $'Exercise fake native Codex.\n\n## Validation\n- [ ] fake adapter records an artifact' >/dev/null \
  || loud_fail "failed to add profile worker"
wg publish codex-profile-worker --only >/dev/null || loud_fail "failed to publish profile worker"
start_wg_daemon "$project" --max-agents 1 --no-coordinator-agent --interval 1 \
  || loud_fail "Codex-profile daemon failed to start"
wait_status codex-profile-worker done
wg service stop --force >/dev/null 2>&1 || true

# Pi remains selectable/recommended. An explicit per-task native Codex route
# must override it without executing Pi and without WG validating the opaque ID.
wg profile use pi --no-reload >profile-pi.log 2>&1 \
  || loud_fail "Pi profile re-activation failed: $(cat profile-pi.log)"
# Preserve Pi's OpenAI Codex provider dialect as a distinct execution system;
# this is not native `codex:*` and must continue to render as handler=pi.
wg config --local --model 'pi:openai-codex:gpt-pi-opaque' --reasoning high --no-reload \
  >>profile-pi.log 2>&1 || loud_fail "Pi Codex route pin failed: $(cat profile-pi.log)"
pi_models=$(wg config --models 2>&1) || loud_fail "Pi config --models failed: $pi_models"
grep -Eq '^  default +pi +pi:openai-codex:gpt-pi-opaque' <<<"$pi_models" \
  || loud_fail "Pi Codex route no longer renders distinctly as Pi: $pi_models"

wg add 'per-task native Codex worker' --id codex-task-worker \
  --model 'codex:future/opaque:model-v9' --reasoning xhigh --independent \
  -d $'Exercise explicit task override.\n\n## Validation\n- [ ] exact opaque native model reaches Codex' >/dev/null \
  || loud_fail "explicit codex task override was rejected"
wg publish codex-task-worker --only >/dev/null || loud_fail "failed to publish task override"
start_wg_daemon "$project" --max-agents 1 --no-coordinator-agent --interval 1 \
  || loud_fail "Pi-default daemon with Codex override failed to start"
wait_status codex-task-worker done

# A native Codex failure is terminal/retryable within Codex. It must not invoke
# Pi or Claude as an implicit fallback.
wg add 'failing native Codex worker' --id codex-failure-worker \
  --model 'codex:fail-native-opaque' --reasoning high --independent \
  -d $'Propagate fake Codex exit 42.\n\n## Validation\n- [ ] failure remains a Codex failure' >/dev/null \
  || loud_fail "failing codex task override was rejected"
wg publish codex-failure-worker --only >/dev/null || loud_fail "failed to publish failure task"
wait_status codex-failure-worker failed

[[ ! -e "$HOME/cross-system.log" ]] \
  || loud_fail "explicit Codex crossed execution systems: $(cat "$HOME/cross-system.log")"

# Invocation: native opaque ID goes to --model unchanged, while WG identity
# remains the exact handler-first route in the child environment.
log=$(cat "$HOME/codex-invocations.log")
grep -q '^arg=exec$' <<<"$log" \
  || loud_fail "native adapter did not invoke codex exec: $log"
grep -q '^arg=--json$' <<<"$log" \
  || loud_fail "native adapter did not request Codex JSON streaming: $log"
grep -q '^arg=future/opaque:model-v9$' <<<"$log" \
  || loud_fail "opaque native model did not reach Codex unchanged: $log"
if grep -Eq '^arg=--(provider|endpoint|api-key)$' <<<"$log"; then
  loud_fail "WG injected provider/endpoint/auth ownership into native Codex: $log"
fi
grep -q '^WG_EXECUTOR_TYPE=codex$' <<<"$log" \
  || loud_fail "child executor identity is not Codex: $log"
grep -q '^WG_MODEL=codex:future/opaque:model-v9$' <<<"$log" \
  || loud_fail "exact WG route provenance missing from child env: $log"
grep -q '^WG_REASONING=xhigh$' <<<"$log" \
  || loud_fail "per-task reasoning missing from Codex invocation: $log"

agent_dir=$(find "$project/.wg/agents" -mindepth 1 -maxdepth 1 -type d \
  -exec grep -l 'codex:future/opaque:model-v9' '{}/metadata.json' \; 2>/dev/null | head -1 | xargs -r dirname)
[[ -n "$agent_dir" ]] || loud_fail "no Codex task agent metadata with exact route"
python3 - "$agent_dir/metadata.json" <<'PY' || loud_fail "Codex metadata provenance mismatch"
import json, sys
m=json.load(open(sys.argv[1]))
assert m["executor"] == "codex", m
assert m["model"] == "codex:future/opaque:model-v9", m
assert m["native_model"] == "future/opaque:model-v9", m
PY
raw="$agent_dir/raw_stream.jsonl"
grep -q '"type":"item.completed"' "$raw" \
  || loud_fail "Codex raw stream was not captured: $(cat "$raw" 2>/dev/null)"
grep -q '"type":"turn.completed"' "$raw" \
  || loud_fail "Codex usage stream was not captured: $(cat "$raw" 2>/dev/null)"

# Completion translates Codex turn.completed usage into the task accounting
# surface while preserving executor/model provenance in the task log.
show=$(wg show codex-task-worker --json 2>&1) || loud_fail "wg show failed: $show"
python3 - "$show" <<'PY' || loud_fail "Codex usage/provenance accounting mismatch: $show"
import json, sys
v=json.loads(sys.argv[1])
u=v.get("token_usage") or {}
# WG reports novel input (17 total - 3 cached) and includes Codex reasoning
# output (5 visible + 2 reasoning) in accounted output.
assert u.get("input_tokens") == 14, u
assert u.get("output_tokens") == 7, u
assert u.get("cache_read_input_tokens") == 3, u
logs="\n".join(x.get("message", "") for x in v.get("log", []))
assert "--executor codex" in logs, logs
assert "--model codex:future/opaque:model-v9" in logs, logs
PY

failure=$(wg show codex-failure-worker --json 2>&1) || true
python3 - "$failure" <<'PY' || loud_fail "native Codex failure was not propagated: $failure"
import json, sys
v=json.loads(sys.argv[1])
assert v.get("status") == "failed", v
assert "42" in (v.get("failure_reason") or ""), v
PY

# Retry starts a fresh worker attempt, remains explicitly native Codex, and
# accepts a newly pinned opaque Codex model rather than resuming/crossing over.
wg service stop --force >/dev/null 2>&1 || true
wg retry codex-failure-worker --reason 'fake adapter recovered' >/dev/null \
  || loud_fail "Codex failure was not retryable"
wg edit codex-failure-worker --model 'codex:retry/opaque:model-v10' >/dev/null \
  || loud_fail "retry Codex model override was rejected"
start_wg_daemon "$project" --max-agents 1 --no-coordinator-agent --interval 1 \
  || loud_fail "Codex retry daemon failed to start"
wait_status codex-failure-worker done
retry_log=$(cat "$HOME/codex-invocations.log")
grep -q '^arg=retry/opaque:model-v10$' <<<"$retry_log" \
  || loud_fail "retry did not reach native Codex with the new opaque model: $retry_log"
grep -q '^WG_MODEL=codex:retry/opaque:model-v10$' <<<"$retry_log" \
  || loud_fail "retry lost exact native Codex provenance: $retry_log"
[[ ! -e "$HOME/cross-system.log" ]] \
  || loud_fail "Codex retry crossed execution systems: $(cat "$HOME/cross-system.log")"

echo "PASS: explicit Codex profile/task workers use native adapter with exact provenance, stream usage, retry, and no cross-system fallback"
