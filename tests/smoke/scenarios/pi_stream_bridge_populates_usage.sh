#!/usr/bin/env bash
# Scenario: pi_stream_bridge_populates_usage
#
# Regression (fix-pi-handler): pi worker tasks (`pi --mode json`) fell into the
# generic `_` wrapper branch in `write_wrapper_script`, which piped ALL output
# to output.log and wrote a HARDCODED stream.jsonl bookend
# `result.usage={input_tokens:0,output_tokens:0}` with NO per-step events. Pi's
# rich per-turn usage (`turn_end.message.usage`, with pi's own field names
# {input,output,cacheRead,cacheWrite,cost}) only landed in output.log, never the
# canonical channel — so the TUI events pane was EMPTY and `wg show`/`wg spend`/
# `wg stats` reported ZERO tokens + cost for every pi task.
#
# This pins the bridge end-to-end on the REAL binary: feed a captured pi event
# stream to `wg pi-stream-bridge` and assert the canonical stream.jsonl carries
# (a) a NONZERO result.usage equal to the SUMMED per-turn `turn_end` totals (no
# double-count of the repeated message_update/message_end usage snapshots),
# (b) per-step events between init and result (the events pane), and (c) a
# session summary. We do NOT call a real LLM — the fixture is the captured
# stream, and the assertions are on the bridge's filesystem output.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

repo_root="$(cd "$HERE/../../.." && pwd)"
fixture="$repo_root/tests/smoke/fixtures/pi_event_stream.jsonl"
[ -f "$fixture" ] || loud_fail "missing fixture: $fixture"

scratch=$(make_scratch)
agent_dir="$scratch/agents/agent-pi-smoke"
mkdir -p "$agent_dir"
cp "$fixture" "$agent_dir/raw_stream.jsonl"
cp "$fixture" "$agent_dir/output.log"
cat >"$agent_dir/metadata.json" <<'JSON'
{"agent_id":"agent-pi-smoke","executor":"pi","model":"openrouter:z-ai/glm-5.2","task_id":"smoke"}
JSON

if ! wg pi-stream-bridge --agent-dir "$agent_dir" --exit-code 0 >"$scratch/bridge.log" 2>&1; then
    loud_fail "wg pi-stream-bridge exited non-zero: $(cat "$scratch/bridge.log")"
fi

stream="$agent_dir/stream.jsonl"
[ -f "$stream" ] || loud_fail "bridge did not write stream.jsonl"

have_jq=0
command -v jq >/dev/null 2>&1 && have_jq=1

result_line="$(grep '"type":"result"' "$stream" | tail -1)"
[ -n "$result_line" ] || loud_fail "stream.jsonl has no result event"

if [ "$have_jq" -eq 1 ]; then
    in_tok="$(echo "$result_line" | jq -r '.usage.input_tokens // 0')"
    out_tok="$(echo "$result_line" | jq -r '.usage.output_tokens // 0')"
    cache_tok="$(echo "$result_line" | jq -r '.usage.cache_read_input_tokens // 0')"
    cost="$(echo "$result_line" | jq -r '.usage.cost_usd // 0')"

    # Summed per-turn turn_end totals from the fixture:
    #   input:  200 + 5   = 205   (NOT inflated by message_update/message_end)
    #   output: 10  + 7   = 17
    #   cacheRead: 50 + 260 = 310
    #   cost:   0.02 + 0.03 = 0.05
    [ "$in_tok" = "205" ] || loud_fail "result.usage.input_tokens=$in_tok, expected 205 (double-count or zero bug)"
    [ "$out_tok" = "17" ] || loud_fail "result.usage.output_tokens=$out_tok, expected 17"
    [ "$cache_tok" = "310" ] || loud_fail "result.usage.cache_read_input_tokens=$cache_tok, expected 310"
    case "$cost" in
        0.05*) : ;;
        *) loud_fail "result.usage.cost_usd=$cost, expected ~0.05" ;;
    esac

    # Per-step events between init and result populate the TUI events pane.
    turns="$(grep -c '"type":"turn"' "$stream" || true)"
    tool_starts="$(grep -c '"type":"tool_start"' "$stream" || true)"
    [ "${turns:-0}" -ge 2 ] || loud_fail "expected >=2 turn events, got ${turns:-0}"
    [ "${tool_starts:-0}" -ge 1 ] || loud_fail "expected >=1 tool_start event, got ${tool_starts:-0}"
else
    # jq unavailable: at minimum the result must NOT be the 0/0 bookend.
    case "$result_line" in
        *'"input_tokens":0,"output_tokens":0'*)
            loud_fail "result.usage is still the 0/0 bookend (jq absent — coarse check)"
            ;;
    esac
    grep -q '"type":"turn"' "$stream" || loud_fail "no per-step turn events in stream.jsonl"
fi

[ -s "$agent_dir/session-summary.md" ] || loud_fail "bridge did not write a non-empty session-summary.md"
grep -q "all done, task complete" "$agent_dir/session-summary.md" \
    || loud_fail "session-summary.md does not carry the final assistant text"

echo "PASS: pi-stream-bridge populates nonzero summed usage + per-step events + session summary"
exit 0
