#!/usr/bin/env bash
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"
wgd="$scratch/.wg"
run_wg() {
    env -u WG_DIR -u WG_MODEL -u WG_EXECUTOR_TYPE wg --dir "$wgd" "$@"
}

init_out="$scratch/init.out"
run_wg init -m claude:opus --no-agency >"$init_out" 2>&1 || \
    loud_fail "wg init failed: $(tail -20 "$init_out")"

assert_plain_chat_contract() {
    local cid="$1"
    local forbidden_model="$2"
    local spawn_out="$scratch/plain-spawn-${cid}.out"
    if ! env -u WG_DIR -u WG_MODEL WG_EXECUTOR_TYPE=pi \
            wg --dir "$wgd" spawn-task --dry-run ".chat-${cid}" >"$spawn_out" 2>&1; then
        loud_fail "plain pi spawn-task dry-run failed for .chat-${cid}: $(cat "$spawn_out")"
    fi
    local msg
    msg="$(cat "$spawn_out")"
    local preview
    preview="$(printf '%s\n' "$msg" | tail -n 1)"
    echo "$preview" | grep -Eq '^pi([[:space:]]|$)' || \
        loud_fail "plain pi dry-run should launch pi CLI for .chat-${cid}, got: $msg"
    if echo "$preview" | grep -Eq -- '(^|[[:space:]])--mode[[:space:]]+rpc([[:space:]]|$)|(^|[[:space:]])(-m|--model)([[:space:]]|$)|--provider'; then
        loud_fail "plain pi dry-run command must not pass rpc/model/provider flags for .chat-${cid}, got: $msg"
    fi
    if [[ -n "$forbidden_model" ]] && echo "$preview" | grep -qF "$forbidden_model"; then
        loud_fail "plain pi dry-run command leaked configured model '$forbidden_model' for .chat-${cid}: $msg"
    fi
}

plain_out="$scratch/plain-create.out"
if ! run_wg chat create --name plain-pi --exec pi --json >"$plain_out" 2>&1; then
    loud_fail "wg chat create --exec pi failed: $(cat "$plain_out")"
fi
assert_plain_chat_contract 0 ""

codex_cfg="$wgd/config.toml"
cat >"$codex_cfg" <<'TOML'
[coordinator]
executor = "codex"
model = "codex:gpt-5.5"

[agent]
model = "codex:gpt-5.5"
TOML

codex_plain_out="$scratch/plain-codex-profile-create.out"
if ! run_wg chat create --name plain-pi-codex-default --exec pi --json >"$codex_plain_out" 2>&1; then
    loud_fail "wg chat create --exec pi under codex default failed: $(cat "$codex_plain_out")"
fi
assert_plain_chat_contract 1 "codex:gpt-5.5"

cat >"$codex_cfg" <<'TOML'
[coordinator]
executor = "pi"
model = "pi:lunaroute:glm-5.2-nvfp4"

[agent]
model = "pi:lunaroute:glm-5.2-nvfp4"
TOML

pi_plain_out="$scratch/plain-pi-profile-create.out"
if ! run_wg chat create --name plain-pi-pi-default --exec pi --json >"$pi_plain_out" 2>&1; then
    loud_fail "wg chat create --exec pi under pi default failed: $(cat "$pi_plain_out")"
fi
assert_plain_chat_contract 2 "pi:lunaroute:glm-5.2-nvfp4"

explicit_route="pi:lunaroute:glm-5.2-nvfp4"
explicit_inner="lunaroute:glm-5.2-nvfp4"
explicit_out="$scratch/explicit-create.out"
if ! run_wg chat create --name explicit-pi --exec pi --model "$explicit_route" --json >"$explicit_out" 2>&1; then
    loud_fail "wg chat create --exec pi --model failed: $(cat "$explicit_out")"
fi

graph="$wgd/graph.jsonl"
python3 - "$graph" "$explicit_route" <<'PY'
import json
import sys
from pathlib import Path

graph = Path(sys.argv[1])
explicit_route = sys.argv[2]
rows = [json.loads(line) for line in graph.read_text().splitlines() if line.strip()]
tasks = {row.get("id"): row for row in rows if row.get("kind", "task") == "task"}
plain = tasks.get(".chat-0")
plain_codex = tasks.get(".chat-1")
plain_pi = tasks.get(".chat-2")
explicit = tasks.get(".chat-3")
if plain is None or plain_codex is None or plain_pi is None or explicit is None:
    raise SystemExit(f"missing expected chat tasks in graph: {tasks}")
for label, task in [("plain", plain), ("plain_codex", plain_codex), ("plain_pi", plain_pi)]:
    if task.get("executor_preset_name") != "pi":
        raise SystemExit(f"{label} chat executor_preset_name should be pi: {task}")
    if "model" in task and task.get("model") not in (None, ""):
        raise SystemExit(f"{label} pi chat must not persist a model override: {task}")
    if "endpoint" in task or "endpoint_override" in task:
        raise SystemExit(f"{label} pi chat must not persist an endpoint override: {task}")
    if task.get("command_argv") != ["pi"]:
        raise SystemExit(f"{label} pi command_argv must be plain CLI metadata: {task}")
if explicit.get("executor_preset_name") != "pi":
    raise SystemExit(f"explicit chat executor_preset_name should be pi: {explicit}")
if explicit.get("model") != explicit_route:
    raise SystemExit(f"explicit pi model not preserved: {explicit}")
argv = explicit.get("command_argv") or []
if "--model" not in argv or explicit_route not in argv:
    raise SystemExit(f"explicit pi command_argv must carry the explicit model marker: {explicit}")
PY

show_out="$scratch/show.out"
if ! run_wg chat show 0 >"$show_out" 2>&1; then
    loud_fail "wg chat show 0 failed: $(cat "$show_out")"
fi
show_msg="$(cat "$show_out")"
echo "$show_msg" | grep -qF "executor : pi" || \
    loud_fail "wg chat show should display executor pi, got: $show_msg"
if echo "$show_msg" | grep -qF "$explicit_route"; then
    loud_fail "wg chat show for plain pi leaked explicit route: $show_msg"
fi
if echo "$show_msg" | grep -Eq 'Model:[[:space:]]*pi:'; then
    loud_fail "wg chat show for plain pi forced a pi model route: $show_msg"
fi

explicit_spawn="$scratch/explicit-spawn.out"
if ! env -u WG_DIR WG_EXECUTOR_TYPE=pi WG_MODEL="$explicit_route" \
        wg --dir "$wgd" spawn-task --dry-run .chat-3 >"$explicit_spawn" 2>&1; then
    loud_fail "explicit pi spawn-task dry-run failed: $(cat "$explicit_spawn")"
fi
explicit_msg="$(cat "$explicit_spawn")"
explicit_preview="$(printf '%s\n' "$explicit_msg" | tail -n 1)"
echo "$explicit_preview" | grep -Eq '^pi([[:space:]]|$)' || \
    loud_fail "explicit pi dry-run should launch pi CLI, got: $explicit_msg"
echo "$explicit_preview" | grep -qF -- "--provider lunaroute --model glm-5.2-nvfp4" || \
    loud_fail "explicit pi dry-run should pass explicit provider/model, got: $explicit_msg"
if echo "$explicit_preview" | grep -Eq -- '(^|[[:space:]])--mode[[:space:]]+rpc([[:space:]]|$)'; then
    loud_fail "explicit pi dry-run must not use rpc mode, got: $explicit_msg"
fi

echo "PASS: plain Pi chat stores executor=pi with no model and dry-runs as pi CLI without rpc/provider/model flags; explicit Pi model still passes provider/model"
