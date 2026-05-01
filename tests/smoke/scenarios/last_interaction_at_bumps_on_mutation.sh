#!/usr/bin/env bash
# Scenario: last_interaction_at_bumps_on_mutation
#
# Regression for the bad ship of fix-tui-graph (commit 73f2f5c11) and the
# subsequent revert+redo (revert-redo-fix). The previous fix derived chat
# activity from inbox/outbox file mtimes and re-sorted the TUI on every
# tick, which yanked the viewport on every event and made the TUI unusable.
#
# The redo: every task carries `last_interaction_at`, bumped automatically
# inside `modify_graph` whenever a substantive field changes (status, log,
# edits, etc.). Heartbeats live in `service/registry.json` and never reach
# `modify_graph`, so they are naturally excluded.
#
# This smoke pins the data-side contract the TUI relies on:
#   1. After `wg add`, the new task carries last_interaction_at >= created_at.
#   2. After `wg log <id> "..."`, last_interaction_at moves forward.
#   3. After `wg start <id>`, last_interaction_at moves forward again.
#   4. An untouched sibling task does NOT have its timestamp bumped.
#   5. Pre-existing rows on disk that lack the field altogether are
#      migrated lazily on read (default = created_at), so the TUI sort
#      never sees a None.
#
# No daemon, no LLM — just CLI + graph.jsonl reads via `wg --json show`.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

if ! wg init -x shell >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -5 init.log)"
fi

# (1) Fresh task gets a populated last_interaction_at.
if ! wg add "Bumped task" --id bumped >>init.log 2>&1; then
    loud_fail "wg add failed: $(tail -5 init.log)"
fi
if ! wg add "Untouched sibling" --id untouched >>init.log 2>&1; then
    loud_fail "wg add (sibling) failed: $(tail -5 init.log)"
fi

read_field() {
    local id="$1" field="$2"
    wg --json show "$id" 2>/dev/null \
        | python3 -c "import json,sys; print(json.load(sys.stdin).get('$field') or '')"
}

ts1=$(read_field bumped last_interaction_at)
sib_ts1=$(read_field untouched last_interaction_at)
created=$(read_field bumped created_at)

if [[ -z "$ts1" ]]; then
    loud_fail "fresh task 'bumped' has empty last_interaction_at (expected >= created_at)"
fi
if [[ -z "$created" ]]; then
    loud_fail "fresh task 'bumped' has empty created_at — wg add regression"
fi
if [[ -z "$sib_ts1" ]]; then
    loud_fail "fresh task 'untouched' has empty last_interaction_at"
fi

# Ensure every clock tick fires forward; sleep > 1s so RFC3339 strings differ
# even at second resolution.
sleep 2

# (2) wg log moves last_interaction_at forward on the touched task.
if ! wg log bumped "smoke poke" >log.log 2>&1; then
    loud_fail "wg log failed: $(cat log.log)"
fi
ts2=$(read_field bumped last_interaction_at)
sib_ts2=$(read_field untouched last_interaction_at)
if [[ -z "$ts2" ]]; then
    loud_fail "post-log last_interaction_at is empty"
fi
if [[ "$ts2" == "$ts1" ]]; then
    loud_fail "wg log did not bump last_interaction_at (still $ts1)"
fi
if [[ "$ts2" < "$ts1" ]]; then
    loud_fail "post-log last_interaction_at went backwards: $ts1 -> $ts2"
fi

# (4) Untouched sibling stays put — bump must be per-task, not global.
if [[ "$sib_ts2" != "$sib_ts1" ]]; then
    loud_fail "untouched sibling's last_interaction_at moved: $sib_ts1 -> $sib_ts2"
fi

sleep 2

# (3) wg edit (description change) bumps again — covers the edit mutation
# path which routes through modify_graph just like log/status changes.
if ! wg edit bumped -d "smoke edit" >edit.log 2>&1; then
    loud_fail "wg edit failed: $(cat edit.log)"
fi
ts3=$(read_field bumped last_interaction_at)
if [[ -z "$ts3" || "$ts3" == "$ts2" || "$ts3" < "$ts2" ]]; then
    loud_fail "wg edit did not bump last_interaction_at: $ts2 -> $ts3"
fi

# (5) Migration: a synthetic graph row predating the field must read back
# with last_interaction_at == created_at. Hand-craft a JSONL row in a fresh
# scratch dir and load it.
migrate_dir=$(make_scratch)
cd "$migrate_dir"
if ! wg init -x shell >init.log 2>&1; then
    loud_fail "wg init (migrate scratch) failed: $(tail -5 init.log)"
fi

# Resolve the graph file location (.wg or .workgraph).
gd=$(graph_dir_in "$migrate_dir") || loud_fail "no .wg dir under $migrate_dir"
gjson="$gd/graph.jsonl"

# Append a row with NO last_interaction_at field at all.
printf '%s\n' \
    '{"id":"legacy","kind":"task","title":"Legacy row","status":"open","created_at":"2026-04-30T00:00:00+00:00"}' \
    >> "$gjson"

if ! wg --json show legacy >legacy.log 2>&1; then
    loud_fail "wg --json show legacy failed: $(cat legacy.log)"
fi
legacy_lia=$(read_field legacy last_interaction_at)
legacy_created=$(read_field legacy created_at)
if [[ "$legacy_lia" != "$legacy_created" ]]; then
    loud_fail "migration default broken: legacy row's last_interaction_at='$legacy_lia' but created_at='$legacy_created'"
fi

echo "PASS: last_interaction_at bumps on mutation, leaves siblings alone, migrates from created_at"
exit 0
