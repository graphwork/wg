#!/usr/bin/env bash
# Explicit native Claude worker regression. Proves the built-in direct profile
# and a per-task override both reach `claude --print --output-format stream-json`,
# preserve exact route/native-model identity and usage, retry natively, and
# propagate failure without crossing to Pi or Codex. Claude chat is deliberately
# out of scope and remains owned by its separate restoration task.
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

cat >"$bin/claude" <<'SH'
#!/usr/bin/env bash
set -u
{
  printf '%s\n' '--- invocation ---'
  printf 'arg=%s\n' "$@"
  printf 'WG_EXECUTOR_TYPE=%s\n' "${WG_EXECUTOR_TYPE:-}"
  printf 'WG_MODEL=%s\n' "${WG_MODEL:-}"
  printf 'WG_REASONING=%s\n' "${WG_REASONING:-}"
} >>"$HOME/claude-invocations.log"
cat >/dev/null
case " $* " in
  *" --model fail-native-opaque "*)
    echo 'intentional native Claude failure' >&2
    exit 42
    ;;
esac
artifact="$HOME/${WG_TASK_ID:-claude}-artifact.txt"
printf 'native claude worker artifact\n' >"$artifact"
wg artifact "${WG_TASK_ID}" "$artifact" >/dev/null 2>&1 || true
printf '%s\n' '{"type":"system","subtype":"init","session_id":"fake-claude-session"}'
printf '%s\n' '{"type":"assistant","message":{"id":"msg-1","type":"message","role":"assistant","content":[{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"wg artifact"}},{"type":"text","text":"native claude complete"}],"usage":{"input_tokens":17,"output_tokens":5,"cache_read_input_tokens":3,"cache_creation_input_tokens":2}}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"native claude complete","session_id":"fake-claude-session","total_cost_usd":0.123,"usage":{"input_tokens":17,"output_tokens":5,"cache_read_input_tokens":3,"cache_creation_input_tokens":2}}'
SH
chmod +x "$bin/claude"

cat >"$bin/pi" <<'SH'
#!/usr/bin/env bash
printf 'PI INVOKED: %s\n' "$*" >>"$HOME/cross-system.log"
exit 91
SH
cat >"$bin/codex" <<'SH'
#!/usr/bin/env bash
printf 'CODEX INVOKED: %s\n' "$*" >>"$HOME/cross-system.log"
exit 92
SH
chmod +x "$bin/pi" "$bin/codex"

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
  || loud_fail "failed to keep FLIP disabled during Claude recovery: $(cat config.log)"
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

# Direct Claude profile is built in, explicit, and distinct from a Pi route
# whose provider happens to be Anthropic.
wg profile use claude --no-reload >profile-claude.log 2>&1 \
  || loud_fail "direct Claude profile activation failed: $(cat profile-claude.log)"
models=$(wg config --models 2>&1) || loud_fail "Claude config --models failed: $models"
grep -q 'claude:opus' <<<"$models" || loud_fail "direct Claude route missing: $models"
grep -Eq '^  default +claude +claude:opus' <<<"$models" \
  || loud_fail "handler/route identity is ambiguous: $models"
if grep -q 'pi:anthropic:opus' <<<"$models"; then
  loud_fail "direct Claude profile was rendered as Pi Anthropic: $models"
fi

wg add 'direct Claude profile worker' --id claude-profile-worker --reasoning high --independent \
  -d $'Exercise fake native Claude.\n\n## Validation\n- [ ] fake adapter records an artifact' >/dev/null \
  || loud_fail "failed to add profile worker"
wg publish claude-profile-worker --only >/dev/null || loud_fail "failed to publish profile worker"
start_wg_daemon "$project" --max-agents 1 --no-coordinator-agent --interval 1 \
  || loud_fail "Claude-profile daemon failed to start"
wait_status claude-profile-worker done
wg service stop --force >/dev/null 2>&1 || true

# Pi stays recommended/selectable. An explicit per-task Claude route overrides
# the Pi worker floor without invoking Pi or consulting a WG model catalog.
wg profile use pi --no-reload >profile-pi.log 2>&1 \
  || loud_fail "Pi profile re-activation failed: $(cat profile-pi.log)"
wg config --local --model 'pi:anthropic:pi-opaque-model' --reasoning high --no-reload \
  >>profile-pi.log 2>&1 || loud_fail "Pi Anthropic route pin failed: $(cat profile-pi.log)"
pi_models=$(wg config --models 2>&1) || loud_fail "Pi config --models failed: $pi_models"
grep -Eq '^  default +pi +pi:anthropic:pi-opaque-model' <<<"$pi_models" \
  || loud_fail "Pi Anthropic route no longer renders distinctly as Pi: $pi_models"

wg add 'per-task native Claude worker' --id claude-task-worker \
  --model 'claude:future/opaque:model-v9' --reasoning xhigh --independent \
  -d $'Exercise explicit Claude task override.\n\n## Validation\n- [ ] exact opaque native model reaches Claude' >/dev/null \
  || loud_fail "explicit Claude task override was rejected"
wg publish claude-task-worker --only >/dev/null || loud_fail "failed to publish task override"
start_wg_daemon "$project" --max-agents 1 --no-coordinator-agent --interval 1 \
  || loud_fail "Pi-default daemon with Claude override failed to start"
wait_status claude-task-worker done

# A native Claude failure must remain a Claude failure. Never retry through Pi
# or Codex as a hidden cross-system fallback.
wg add 'failing native Claude worker' --id claude-failure-worker \
  --model 'claude:fail-native-opaque' --reasoning high --independent \
  -d $'Propagate fake Claude exit 42.\n\n## Validation\n- [ ] failure remains a Claude failure' >/dev/null \
  || loud_fail "failing Claude task override was rejected"
wg publish claude-failure-worker --only >/dev/null || loud_fail "failed to publish failure task"
wait_status claude-failure-worker failed

[[ ! -e "$HOME/cross-system.log" ]] \
  || loud_fail "explicit Claude crossed execution systems: $(cat "$HOME/cross-system.log")"

# Invocation and exact identity: only the handler token is stripped for Claude;
# native model spelling stays opaque, while WG_MODEL keeps the full route.
log=$(cat "$HOME/claude-invocations.log")
grep -q '^arg=--print$' <<<"$log" \
  || loud_fail "native adapter did not invoke Claude print mode: $log"
grep -q '^arg=stream-json$' <<<"$log" \
  || loud_fail "native adapter did not request Claude stream-json: $log"
grep -q '^arg=future/opaque:model-v9$' <<<"$log" \
  || loud_fail "opaque native model did not reach Claude unchanged: $log"
if grep -Eq '^arg=--(provider|endpoint|api-key)$' <<<"$log"; then
  loud_fail "WG injected provider/endpoint/auth ownership into native Claude: $log"
fi
grep -q '^WG_EXECUTOR_TYPE=claude$' <<<"$log" \
  || loud_fail "child executor identity is not Claude: $log"
grep -q '^WG_MODEL=claude:future/opaque:model-v9$' <<<"$log" \
  || loud_fail "exact WG route provenance missing from child env: $log"
grep -q '^WG_REASONING=xhigh$' <<<"$log" \
  || loud_fail "per-task reasoning missing from Claude invocation: $log"

agent_dir=$(find "$project/.wg/agents" -mindepth 1 -maxdepth 1 -type d \
  -exec grep -l 'claude:future/opaque:model-v9' '{}/metadata.json' \; 2>/dev/null | head -1 | xargs -r dirname)
[[ -n "$agent_dir" ]] || loud_fail "no Claude task agent metadata with exact route"
python3 - "$agent_dir/metadata.json" <<'PY' || loud_fail "Claude metadata provenance mismatch"
import json, sys
m=json.load(open(sys.argv[1]))
assert m["executor"] == "claude", m
assert m["model"] == "claude:future/opaque:model-v9", m
assert m["native_model"] == "future/opaque:model-v9", m
PY
raw="$agent_dir/raw_stream.jsonl"
grep -q '"type":"assistant"' "$raw" \
  || loud_fail "Claude raw assistant stream was not captured: $(cat "$raw" 2>/dev/null)"
grep -q '"type":"result"' "$raw" \
  || loud_fail "Claude raw result/usage stream was not captured: $(cat "$raw" 2>/dev/null)"

show=$(wg show claude-task-worker --json 2>&1) || loud_fail "wg show failed: $show"
python3 - "$show" <<'PY' || loud_fail "Claude usage/provenance accounting mismatch: $show"
import json, sys
v=json.loads(sys.argv[1])
u=v.get("token_usage") or {}
assert u.get("input_tokens") == 17, u
assert u.get("output_tokens") == 5, u
assert u.get("cache_read_input_tokens") == 3, u
assert u.get("cache_creation_input_tokens") == 2, u
logs="\n".join(x.get("message", "") for x in v.get("log", []))
assert "--executor claude" in logs, logs
assert "--model claude:future/opaque:model-v9" in logs, logs
PY

failure=$(wg show claude-failure-worker --json 2>&1) || true
python3 - "$failure" <<'PY' || loud_fail "native Claude failure was not propagated: $failure"
import json, sys
v=json.loads(sys.argv[1])
assert v.get("status") == "failed", v
assert "42" in (v.get("failure_reason") or ""), v
PY

# Retry is a fresh worker attempt on the newly pinned native Claude model.
wg service stop --force >/dev/null 2>&1 || true
wg retry claude-failure-worker --reason 'fake adapter recovered' >/dev/null \
  || loud_fail "Claude failure was not retryable"
wg edit claude-failure-worker --model 'claude:retry/opaque:model-v10' >/dev/null \
  || loud_fail "retry Claude model override was rejected"
start_wg_daemon "$project" --max-agents 1 --no-coordinator-agent --interval 1 \
  || loud_fail "Claude retry daemon failed to start"
wait_status claude-failure-worker done
retry_log=$(cat "$HOME/claude-invocations.log")
grep -q '^arg=retry/opaque:model-v10$' <<<"$retry_log" \
  || loud_fail "retry did not reach native Claude with the new opaque model: $retry_log"
grep -q '^WG_MODEL=claude:retry/opaque:model-v10$' <<<"$retry_log" \
  || loud_fail "retry lost exact native Claude provenance: $retry_log"
[[ ! -e "$HOME/cross-system.log" ]] \
  || loud_fail "Claude retry crossed execution systems: $(cat "$HOME/cross-system.log")"

echo "PASS: explicit Claude profile/task workers use native adapter with exact provenance, stream usage, retry, and no cross-system fallback; chat remains a separate follow-up"
