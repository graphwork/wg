#!/usr/bin/env bash
# Live terminal-flow regression for `wg show <task> | head -n 1`.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
if ! (cd "$scratch" && wg init --no-agency >init.log 2>&1); then
    loud_fail "wg init failed: $(tail -5 "$scratch/init.log")"
fi
wg_dir="$scratch/.wg"

# Keep the output larger than a pipe buffer so `head` reliably closes while
# the producer is still writing. The single argv remains below Linux's 128 KiB
# per-argument limit.
description=$(python3 - <<'PY'
for index in range(3000):
    print(f"pipeline regression line {index:05}")
PY
)
if ! wg --dir "$wg_dir" add "Pipeline regression task" \
    --id pipe-task --description "$description" >"$scratch/add.out" 2>"$scratch/add.err"; then
    loud_fail "fixture task creation failed: $(cat "$scratch/add.err")"
fi

# A complete consumer still receives the ordinary, unchanged show output.
if ! wg --dir "$wg_dir" show pipe-task >"$scratch/full.out" 2>"$scratch/full.err"; then
    loud_fail "normal wg show failed: $(cat "$scratch/full.err")"
fi
[[ ! -s "$scratch/full.err" ]] || loud_fail "normal wg show emitted stderr: $(cat "$scratch/full.err")"
grep -q '^Task: pipe-task$' "$scratch/full.out" || loud_fail "normal wg show lost its task header"
grep -q 'pipeline regression line 02999' "$scratch/full.out" || loud_fail "normal wg show was truncated"

# Exact reported human flow. A conventional Unix producer ends on SIGPIPE
# (128 + signal 13 = 141), while `head` succeeds and stderr stays silent.
set +e
wg --dir "$wg_dir" show pipe-task 2>"$scratch/pipe.err" \
    | head -n 1 >"$scratch/first.out" 2>"$scratch/head.err"
statuses=("${PIPESTATUS[@]}")
set -e

[[ "${statuses[0]}" == 141 && "${statuses[1]}" == 0 ]] || {
    loud_fail "unexpected closed-reader pipeline statuses: ${statuses[*]}; stderr: $(cat "$scratch/pipe.err")"
}
[[ "$(cat "$scratch/first.out")" == "Task: pipe-task" ]] || {
    loud_fail "head did not receive the first wg show line: $(cat "$scratch/first.out")"
}
[[ ! -s "$scratch/pipe.err" ]] || loud_fail "wg show emitted panic/backtrace: $(cat "$scratch/pipe.err")"
[[ ! -s "$scratch/head.err" ]] || loud_fail "head emitted stderr: $(cat "$scratch/head.err")"

echo "PASS: wg show exits quietly on a closed pipeline and preserves full output"
