#!/usr/bin/env bash
# Scenario: publish_wcc_releases_paused_subgraph
#
# Regression for wg-publish-wcc: `wg publish <leaf> --wcc` must release the
# entire weakly-connected component containing <leaf>, not just <leaf> +
# downstream. Pre-fix, agents had to loop `wg publish` over every task in a
# paused fan-out batch one-by-one, which is the ergonomic gap the user
# reported on 2026-05-01 ("agents are writing silly for loops").
#
# Asserts (per the task's `## Validation` checklist):
#   1. Linear chain: publish leaf --wcc unpauses every node in the chain.
#   2. Diamond: A→B; A→C; B→D; C→D — publish D --wcc releases A, B, C, D.
#   3. --wcc and --only are mutually exclusive at parse time.
#   4. Topological release order: log-entry timestamps within the component
#      are monotonically non-decreasing in the dep→dependent direction
#      (a task being unpaused has all its `after` deps already unpaused).
#
# No daemon, no LLM — pure graph + `wg add --paused` + `wg publish --wcc`.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

if ! wg init -x shell >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -5 init.log)"
fi

# graph.jsonl lives under .wg/ once `wg init` has run.
graph_file="$(graph_dir_in "$scratch")/graph.jsonl"
[[ -f "$graph_file" ]] || loud_fail "graph.jsonl not found at $graph_file after wg init"

# Helper: assert a task is NOT paused after publish. Reads the JSONL row
# directly to avoid coupling to `wg list` formatting.
assert_unpaused() {
    local id="$1"
    if ! grep -qE "\"id\":\"${id}\"" "$graph_file"; then
        loud_fail "task '$id' missing from graph after publish"
    fi
    # Find the row for $id (kind:task only) and check its paused field.
    local row
    row=$(grep -E "\"kind\":\"task\".*\"id\":\"${id}\"|\"id\":\"${id}\".*\"kind\":\"task\"" "$graph_file" | tail -1)
    if [[ -z "$row" ]]; then
        loud_fail "no task row found for '$id'"
    fi
    if echo "$row" | grep -qE '"paused":true'; then
        loud_fail "task '$id' is still paused after `wg publish ... --wcc` (row: $row)"
    fi
}

# ── Test 1: linear chain — publish from the LEAF must unpause the whole chain ─
for i in 0 1 2 3 4; do
    if [[ $i -eq 0 ]]; then
        wg add "n${i}" --id "chain-n${i}" --paused >/dev/null 2>&1 \
            || loud_fail "wg add n${i} failed"
    else
        prev="chain-n$((i-1))"
        wg add "n${i}" --id "chain-n${i}" --after "$prev" --paused >/dev/null 2>&1 \
            || loud_fail "wg add n${i} --after $prev failed"
    fi
done

if ! wg publish chain-n4 --wcc >publish_chain.log 2>&1; then
    loud_fail "wg publish chain-n4 --wcc failed:
$(cat publish_chain.log)"
fi

for i in 0 1 2 3 4; do
    assert_unpaused "chain-n${i}"
done
echo "PASS (1/4): linear-chain leaf publish --wcc unpaused every node"

# ── Test 2: diamond — A → B; A → C; B → D; C → D, publish D --wcc ─────────
wg add "diamond-a" --id "diamond-a" --paused >/dev/null 2>&1 \
    || loud_fail "wg add diamond-a failed"
wg add "diamond-b" --id "diamond-b" --after "diamond-a" --paused >/dev/null 2>&1 \
    || loud_fail "wg add diamond-b failed"
wg add "diamond-c" --id "diamond-c" --after "diamond-a" --paused >/dev/null 2>&1 \
    || loud_fail "wg add diamond-c failed"
wg add "diamond-d" --id "diamond-d" --after "diamond-b,diamond-c" --paused >/dev/null 2>&1 \
    || loud_fail "wg add diamond-d failed"

if ! wg publish diamond-d --wcc >publish_diamond.log 2>&1; then
    loud_fail "wg publish diamond-d --wcc failed:
$(cat publish_diamond.log)"
fi

for id in diamond-a diamond-b diamond-c diamond-d; do
    assert_unpaused "$id"
done
echo "PASS (2/4): diamond publish from join node --wcc unpaused all four nodes"

# ── Test 3: mutual exclusion of --wcc and --only at parse time ────────────
# Should fail before touching any state. We expect a clap error containing
# "cannot be used with" and exit non-zero.
mx_out=$(wg publish chain-n0 --wcc --only 2>&1) || mx_rc=$? && mx_rc=${mx_rc:-0}
if [[ "$mx_rc" -eq 0 ]]; then
    loud_fail "wg publish chain-n0 --wcc --only should have errored, but exited 0:
$mx_out"
fi
if ! echo "$mx_out" | grep -qE 'cannot be used with|conflict'; then
    loud_fail "expected mutually-exclusive error from --wcc + --only, got:
$mx_out"
fi
echo "PASS (3/4): --wcc and --only correctly rejected at parse time"

# ── Test 4: topological release order via log-entry timestamps ────────────
# The chain release in test 1 wrote one log entry per task with action
# "Task published". Grep timestamps for those entries and assert they are
# monotonically non-decreasing in dep→dependent order: n0 ≤ n1 ≤ … ≤ n4.
extract_publish_ts() {
    local id="$1"
    # Find the task row, then pull the timestamp on the LAST log entry whose
    # message contains "Task published". Tasks are JSONL rows; log entries
    # are objects under `"log":[ ... ]`. We anchor on the message string and
    # walk left to the most recent `"timestamp":"..."` in the same object.
    local row entry
    row=$(grep -E "\"kind\":\"task\".*\"id\":\"${id}\"|\"id\":\"${id}\".*\"kind\":\"task\"" "$graph_file" | tail -1)
    # Find the log entry containing "Task published" (lazy match anything
    # between `{` and the message). Then extract its timestamp.
    entry=$(echo "$row" | grep -oE '\{[^{}]*"message":"Task published"[^{}]*\}' | tail -1)
    if [[ -z "$entry" ]]; then
        return 1
    fi
    echo "$entry" \
        | grep -oE '"timestamp":"[^"]+"' \
        | head -1 \
        | sed -E 's/"timestamp":"//; s/"$//'
}

t0=$(extract_publish_ts chain-n0)
t1=$(extract_publish_ts chain-n1)
t2=$(extract_publish_ts chain-n2)
t3=$(extract_publish_ts chain-n3)
t4=$(extract_publish_ts chain-n4)

if [[ -z "$t0" || -z "$t1" || -z "$t2" || -z "$t3" || -z "$t4" ]]; then
    loud_fail "could not extract publish timestamps from log entries (t0=$t0 t1=$t1 t2=$t2 t3=$t3 t4=$t4). graph row sample:
$(grep -E '\"id\":\"chain-n0\"' "$graph_file" | head -1)"
fi

# RFC3339 UTC timestamps lex-sort the same as time-sort.
prev=""
for label in "n0:$t0" "n1:$t1" "n2:$t2" "n3:$t3" "n4:$t4"; do
    name="${label%%:*}"
    ts="${label#*:}"
    if [[ -n "$prev" ]]; then
        if [[ "$prev" > "$ts" ]]; then
            loud_fail "topological release order violated: $name ($ts) was unpaused BEFORE its upstream ($prev)"
        fi
    fi
    prev="$ts"
done
echo "PASS (4/4): release log timestamps non-decreasing in dep→dependent order"

echo "PASS: wg publish --wcc unpauses the entire weakly-connected component in topological order"
exit 0
