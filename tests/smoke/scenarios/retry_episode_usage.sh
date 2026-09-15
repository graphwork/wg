#!/usr/bin/env bash
# Real terminal flow: a failed source exec attempt consumes usage, an explicit
# retry succeeds, and show/spend retain both source attempts while keeping the
# completion-review lane separate.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-$(command -v wg)}"
[[ -x "$WG_BIN" ]] || loud_fail "candidate wg binary missing: $WG_BIN"

scratch=$(make_scratch)
cd "$scratch"
"$WG_BIN" init -x shell >/dev/null 2>&1 || loud_fail "wg init failed"
G="$scratch/.wg"
[[ -f "$G/graph.jsonl" ]] || G="$scratch/.workgraph"
[[ -f "$G/graph.jsonl" ]] || loud_fail "graph not created"

export ACCOUNTING_COUNTER="$scratch/attempt-count"
cat >"$scratch/source-attempt.sh" <<'SH'
#!/usr/bin/env bash
set -eu
n=0
[[ -f "$ACCOUNTING_COUNTER" ]] && n=$(cat "$ACCOUNTING_COUNTER")
n=$((n + 1))
printf '%s\n' "$n" >"$ACCOUNTING_COUNTER"
if [[ "$n" -eq 1 ]]; then
  exit 7
fi
exit 0
SH
chmod +x "$scratch/source-attempt.sh"

wgrun() { "$WG_BIN" --dir "$G" "$@"; }
set_usage() {
  python3 - "$G/graph.jsonl" "$1" <<'PY'
import json, sys
path, usage = sys.argv[1:]
rows = []
for line in open(path):
    row = json.loads(line)
    if row.get("kind") == "task" and row.get("id") == "retry-accounting":
        row["token_usage"] = json.loads(usage)
    rows.append(row)
with open(path, "w") as out:
    for row in rows:
        out.write(json.dumps(row, separators=(",", ":")) + "\n")
PY
}
wgrun add "retry accounting fixture" --id retry-accounting --exec "$scratch/source-attempt.sh" >/dev/null
wgrun publish retry-accounting --only >/dev/null
if wgrun exec retry-accounting --actor accounting-worker --shell >first.log 2>&1; then
  loud_fail "first source attempt unexpectedly succeeded: $(tail -20 first.log)"
fi
set_usage '{"cost_usd":1.35,"input_tokens":100,"output_tokens":11,"cache_read_input_tokens":1400}'
first_cost=$(wgrun --json show retry-accounting | python3 -c 'import json,sys; print(json.load(sys.stdin)["token_usage"]["cost_usd"])')
[[ "$first_cost" == "1.35" ]] || loud_fail "failed attempt usage not reported before retry: $first_cost"

wgrun retry retry-accounting --reason "exercise episode accounting" >/dev/null
wgrun exec retry-accounting --actor accounting-worker --shell >second.log 2>&1 \
  || loud_fail "successful retry failed: $(tail -20 second.log)"
set_usage '{"cost_usd":0.23,"input_tokens":27144,"output_tokens":1082,"cache_read_input_tokens":114000}'

show_json=$(wgrun --json show retry-accounting)
spend_json=$(wgrun spend --json)
python3 - "$show_json" "$spend_json" <<'PY'
import json, math, sys
show = json.loads(sys.argv[1])
spend = json.loads(sys.argv[2])
assert show["status"] == "done", show["status"]
assert len(show["source_attempt_usage"]) == 2, show["source_attempt_usage"]
assert [a["attempt_id"] for a in show["source_attempt_usage"]] == ["attempt-0-1", "attempt-1-2"]
usage = show["token_usage"]
assert math.isclose(usage["cost_usd"], 1.58), usage
assert usage["input_tokens"] == 27244, usage
assert usage["output_tokens"] == 1093, usage
assert usage["cache_read_input_tokens"] == 115400, usage
assert math.isclose(spend["total_cost"], 1.58), spend
assert spend["total_input_tokens"] == 27244, spend
assert spend["total_output_tokens"] == 1093, spend
assert spend["source_attempt_count"] == 2, spend
assert spend["accounting_scope"] == "source-workers-only-episode-cumulative", spend
lane = spend["completion_review_lane"]
assert lane["accounting_scope"] == "internal-review-calls-only-not-task-usage", lane
assert lane["total_cost"] == 0.0, lane
PY

human=$(wgrun show retry-accounting)
printf '%s\n' "$human" | grep -q 'Source attempts: 2 (episode cumulative; review lane excluded)' \
  || loud_fail "human task report does not label the two-attempt episode"
printf '%s\n' "$human" | grep -q 'attempt-0-1: \$1.35' \
  || loud_fail "human task report omitted failed attempt usage"
printf '%s\n' "$human" | grep -q 'attempt-1-2: \$0.23' \
  || loud_fail "human task report omitted successful retry usage"

echo "PASS: failed + successful source attempts are episode-cumulative exactly once; review lane remains separate"
