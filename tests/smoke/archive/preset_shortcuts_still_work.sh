#!/usr/bin/env bash
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

wg init -m claude:opus >/tmp/preset-shortcuts-init.log 2>&1 || loud_fail "wg init failed: $(tail -5 /tmp/preset-shortcuts-init.log)"

wg chat new --name claude-pane --exec claude --model claude:opus --json >/tmp/claude-chat.json 2>&1 || loud_fail "claude preset failed: $(cat /tmp/claude-chat.json)"
wg chat new --name codex-pane --exec codex --model codex:gpt-5.5 --json >/tmp/codex-chat.json 2>&1 || loud_fail "codex preset failed: $(cat /tmp/codex-chat.json)"
wg chat new --name nex-pane --exec nex --model nex:qwen3-coder --endpoint http://127.0.0.1:8088 --json >/tmp/nex-chat.json 2>&1 || loud_fail "nex preset failed: $(cat /tmp/nex-chat.json)"

graph="$scratch/.wg/graph.jsonl"
for idx in 0 1 2; do
    line=$(grep -E "\"id\":\"\\.chat-$idx\"" "$graph" | head -1)
    [[ -n "$line" ]] || loud_fail "missing .chat-$idx in graph: $(cat "$graph")"
    echo "$line" | grep -qF '"command_argv":' || loud_fail ".chat-$idx missing command_argv: $line"
    echo "$line" | grep -qF '"working_dir":' || loud_fail ".chat-$idx missing working_dir: $line"
done

grep -E '"id":"\.chat-0".*"executor_preset_name":"claude"' "$graph" >/dev/null || loud_fail "claude preset missing: $(grep -E '\"id\":\"\\.chat-0\"' "$graph")"
grep -E '"id":"\.chat-1".*"executor_preset_name":"codex"' "$graph" >/dev/null || loud_fail "codex preset missing: $(grep -E '\"id\":\"\\.chat-1\"' "$graph")"
grep -E '"id":"\.chat-2".*"executor_preset_name":"nex"' "$graph" >/dev/null || loud_fail "nex preset missing: $(grep -E '\"id\":\"\\.chat-2\"' "$graph")"

echo "PASS: claude/codex/nex preset shortcuts still create preset command metadata"
