#!/usr/bin/env bash
# Scenario: task_timeout_edit_publish_recover
#
# Pins the recovery flow fixed by bug-task-timeout-edit-publish-block: a task
# carrying a hidden/stale per-task `timeout` field can be INSPECTED via
# `wg show`, REPAIRED via `wg edit --timeout`/`--verify-timeout`, and CLEARED
# with an empty string — without abandoning or superseding the task. Also pins
# that an invalid timeout value is rejected with an actionable message naming
# the field and the clear escape hatch.
#
# This is the live human-flow simulation (real `wg` CLI binary in a scratch
# graph), not a unit test — it exercises the exact terminal surface a user
# hits when a task is stuck on a bad timeout value.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)

# Initialize a scratch graph (no executor needed; we never spawn workers).
# Use the project-local `.wg` form by running from inside the scratch dir.
if ! (cd "$scratch" && wg init --no-agency >init.log 2>&1); then
    loud_fail "wg init failed: $(tail -5 init.log)"
fi
wg_dir="$scratch/.wg"

# 1. Create a task WITH a per-task timeout — the "hidden field" the user
#    report said was unrecoverable.
if ! wg --dir "$wg_dir" add "Stuck Task" --id stuck-task --timeout 4h >"$scratch"/add.log 2>&1; then
    loud_fail "wg add --timeout 4h failed: $(cat "$scratch"/add.log)"
fi

# 2. INSPECT: `wg show` must surface the timeout field clearly enough to
#    diagnose the stuck task.
show_out=$(wg --dir "$wg_dir" show stuck-task 2>&1) || loud_fail "wg show failed"
if ! grep -q "^Timeout: 4h" <<<"$show_out"; then
    loud_fail "wg show does not expose the timeout field for diagnosis:\n$show_out"
fi
# The repair hint must name the exact `wg edit` command.
if ! grep -q "wg edit stuck-task --timeout" <<<"$show_out"; then
    loud_fail "wg show timeout line does not include an actionable repair command:\n$show_out"
fi

# 3. EDIT the timeout to a new value.
if ! wg --dir "$wg_dir" edit stuck-task --timeout 90m >"$scratch"/edit1.log 2>&1; then
    loud_fail "wg edit --timeout 90m failed: $(cat "$scratch"/edit1.log)"
fi
show_out=$(wg --dir "$wg_dir" show stuck-task 2>&1) || loud_fail "wg show failed after edit"
if ! grep -q "^Timeout: 90m" <<<"$show_out"; then
    loud_fail "wg show did not reflect the edited timeout value:\n$show_out"
fi

# 4. Set verify_timeout too and confirm both display.
if ! wg --dir "$wg_dir" edit stuck-task --verify-timeout 15m >"$scratch"/edit2.log 2>&1; then
    loud_fail "wg edit --verify-timeout 15m failed: $(cat "$scratch"/edit2.log)"
fi
show_out=$(wg --dir "$wg_dir" show stuck-task --json 2>&1) || loud_fail "wg show --json failed"
if ! grep -q '"timeout": "90m"' <<<"$show_out"; then
    loud_fail "wg show --json missing timeout field:\n$show_out"
fi
if ! grep -q '"verify_timeout": "15m"' <<<"$show_out"; then
    loud_fail "wg show --json missing verify_timeout field:\n$show_out"
fi

# 5. RECOVERY: clear the stale timeout with an empty string. This is the
#    exact repair the user could not perform before — the task is recovered
#    IN PLACE, not abandoned/superseded.
if ! wg --dir "$wg_dir" edit stuck-task --timeout "" --verify-timeout "" >"$scratch"/edit3.log 2>&1; then
    loud_fail "wg edit --timeout '' (clear) failed: $(cat "$scratch"/edit3.log)"
fi
show_out=$(wg --dir "$wg_dir" show stuck-task --json 2>&1) || loud_fail "wg show --json failed after clear"
if grep -q '"timeout"' <<<"$show_out"; then
    loud_fail "timeout field was not cleared:\n$show_out"
fi
if grep -q '"verify_timeout"' <<<"$show_out"; then
    loud_fail "verify_timeout field was not cleared:\n$show_out"
fi
# The task must still be present and recoverable — NOT abandoned/superseded.
# `superseded_by`/`supersedes` are skipped when empty, so absence = good.
if ! grep -q '"status": "open"' <<<"$show_out"; then
    loud_fail "task status changed unexpectedly after timeout clear:\n$show_out"
fi
if grep -qE '"superseded_by": "\[[^]]' <<<"$show_out" || grep -qE '"supersedes": "[^n]' <<<"$show_out"; then
    loud_fail "task was marked superseded instead of recovered in place:\n$show_out"
fi

# 6. Invalid timeout value must be rejected with an actionable message that
#    names the field and the clear escape hatch — no silent corruption.
if wg --dir "$wg_dir" edit stuck-task --timeout "not-a-duration" >"$scratch"/edit4.log 2>&1; then
    loud_fail "wg edit --timeout not-a-duration should have been rejected"
fi
err=$(cat "$scratch"/edit4.log)
if ! grep -qi "timeout" <<<"$err"; then
    loud_fail "rejection error must name the timeout field:\n$err"
fi
if ! grep -qi "empty string to clear" <<<"$err"; then
    loud_fail "rejection error must mention the clear escape hatch:\n$err"
fi
# The invalid edit must NOT have corrupted the field.
show_out=$(wg --dir "$wg_dir" show stuck-task --json 2>&1) || loud_fail "wg show --json failed"
if grep -q '"timeout"' <<<"$show_out"; then
    loud_fail "invalid timeout edit corrupted the field:\n$show_out"
fi

echo "PASS: task timeout can be inspected, edited, and cleared in place (no abandon/supersede)"
exit 0
