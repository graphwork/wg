#!/usr/bin/env bash
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

wg init -m claude:opus >/tmp/custom-command-chat-init.log 2>&1 || loud_fail "wg init failed: $(tail -5 /tmp/custom-command-chat-init.log)"
printf 'first line\n' > /tmp/wg-custom-command-chat.log

cmd='tail -f /tmp/wg-custom-command-chat.log'
out=$(wg chat new --name tail --command "$cmd" --json 2>&1) || loud_fail "wg chat new --command tail failed: $out"

graph="$scratch/.wg/graph.jsonl"
line=$(grep -E '"id":"\.chat-0"' "$graph" | head -1)
[[ -n "$line" ]] || loud_fail "missing .chat-0 after tail chat create: $(cat "$graph")"

echo "$line" | grep -qF '"command_argv":["bash","-lc","tail -f /tmp/wg-custom-command-chat.log"]' || loud_fail "tail command_argv missing/wrong: $line"
echo "$line" | grep -qF '"working_dir":' || loud_fail "tail chat missing working_dir: $line"

echo "PASS: arbitrary command chat persists tail -f command line"
