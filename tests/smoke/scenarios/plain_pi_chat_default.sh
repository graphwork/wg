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

plain_out="$scratch/plain-create.out"
if ! run_wg chat create --name plain-pi --exec pi --json >"$plain_out" 2>&1; then
    loud_fail "wg chat create --exec pi failed: $(cat "$plain_out")"
fi

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
explicit = tasks.get(".chat-1")
if plain is None or explicit is None:
    raise SystemExit(f"missing expected chat tasks in graph: {tasks}")
if plain.get("executor_preset_name") != "pi":
    raise SystemExit(f"plain chat executor_preset_name should be pi: {plain}")
if "model" in plain and plain.get("model") not in (None, ""):
    raise SystemExit(f"plain pi chat must not persist a model override: {plain}")
if "endpoint" in plain or "endpoint_override" in plain:
    raise SystemExit(f"plain pi chat must not persist an endpoint override: {plain}")
if plain.get("command_argv") != ["wg", "pi-handler", "--chat", "chat"]:
    raise SystemExit(f"plain pi command_argv must omit model/provider flags: {plain}")
if explicit.get("executor_preset_name") != "pi":
    raise SystemExit(f"explicit chat executor_preset_name should be pi: {explicit}")
if explicit.get("model") != explicit_route:
    raise SystemExit(f"explicit pi model not preserved: {explicit}")
argv = explicit.get("command_argv") or []
if "-m" not in argv or explicit_route not in argv:
    raise SystemExit(f"explicit pi command_argv must carry the model override: {explicit}")
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

plain_spawn="$scratch/plain-spawn.out"
if ! env -u WG_DIR -u WG_MODEL WG_EXECUTOR_TYPE=pi \
        wg --dir "$wgd" spawn-task --dry-run .chat-0 >"$plain_spawn" 2>&1; then
    loud_fail "plain pi spawn-task dry-run failed: $(cat "$plain_spawn")"
fi
plain_msg="$(cat "$plain_spawn")"
echo "$plain_msg" | grep -qF "pi-handler" || \
    loud_fail "plain pi dry-run should dispatch to pi-handler, got: $plain_msg"
if echo "$plain_msg" | grep -Eq -- '(^|[[:space:]])(-m|--model)([[:space:]]|$)|--provider'; then
    loud_fail "plain pi dry-run must not pass model/provider flags, got: $plain_msg"
fi

explicit_spawn="$scratch/explicit-spawn.out"
if ! env -u WG_DIR WG_EXECUTOR_TYPE=pi WG_MODEL="$explicit_route" \
        wg --dir "$wgd" spawn-task --dry-run .chat-1 >"$explicit_spawn" 2>&1; then
    loud_fail "explicit pi spawn-task dry-run failed: $(cat "$explicit_spawn")"
fi
explicit_msg="$(cat "$explicit_spawn")"
echo "$explicit_msg" | grep -qF "pi-handler" || \
    loud_fail "explicit pi dry-run should dispatch to pi-handler, got: $explicit_msg"
echo "$explicit_msg" | grep -qF "$explicit_inner" || \
    loud_fail "explicit pi dry-run must carry the model override, got: $explicit_msg"

echo "PASS: plain Pi chat stores executor=pi with no model and dry-runs without provider/model flags; explicit Pi model still routes"
