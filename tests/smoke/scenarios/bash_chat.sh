#!/usr/bin/env bash
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

wg init -m claude:opus >/tmp/bash-chat-init.log 2>&1 || loud_fail "wg init failed: $(tail -5 /tmp/bash-chat-init.log)"

out=$(wg chat new --name shell --command 'bash' --json 2>&1) || loud_fail "wg chat new --command bash failed: $out"

graph="$scratch/.wg/graph.jsonl"
line=$(grep -E '"id":"\.chat-0"' "$graph" | head -1)
[[ -n "$line" ]] || loud_fail "missing .chat-0 after bash chat create: $(cat "$graph")"

echo "$line" | grep -qF '"command_argv":["bash","-lc","bash"]' || loud_fail "bash chat command_argv missing/wrong: $line"
if echo "$line" | grep -qF '"executor_preset_name"'; then
    loud_fail "custom bash chat should not have executor_preset_name: $line"
fi
echo "$line" | grep -qF '"working_dir":' || loud_fail "bash chat missing working_dir: $line"

echo "PASS: bash custom chat stores generic command metadata"
