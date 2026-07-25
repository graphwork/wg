#!/usr/bin/env bash
# Meta-regression for Session Log smoke graph isolation. The real target runs
# from below a live sentinel graph with its smoke root deliberately nested
# inside that graph, then exits through pass/fail/signal/early-skip paths.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then
    WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
else
    require_wg
    WG_BIN="$(command -v wg)"
fi
[[ -x "$WG_BIN" ]] || loud_fail "wg binary is not executable: $WG_BIN"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required for the nested target's real TUI flow"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required for graph invariants"
command -v sha256sum >/dev/null 2>&1 || loud_skip "MISSING SHA256SUM" "sha256sum is required for byte invariants"

scratch=$(make_scratch)
# Keep the synthetic live graph's physical path short enough for its nested
# TUI Unix socket (Linux sun_path is 108 bytes). It is still helper-registered
# scratch state and is removed only when this outer meta-scenario exits.
caller=$(mktemp -d /tmp/wglive.XXXXXX)
register_scratch "$caller"
caller_graph="$caller/.wg"
nested="$caller/nested/deeper"
target_root="$caller/smoke-root"
caller_home="$scratch/caller-home"
target="$HERE/tui_log_pane_renders_raw_stream.sh"
mkdir -p "$nested" "$target_root" "$caller_home" "$caller_home/.config" "$caller_home/.wg"

# The negative sentinel is real graph state. A byte-identical graph/task count
# and registry must survive every target exit path.
HOME="$caller_home" XDG_CONFIG_HOME="$caller_home/.config" WG_GLOBAL_DIR="$caller_home/.wg" \
    "$WG_BIN" --dir "$caller_graph" init --no-agency >/dev/null 2>&1 \
    || loud_fail "failed to initialize caller sentinel graph"
HOME="$caller_home" XDG_CONFIG_HOME="$caller_home/.config" WG_GLOBAL_DIR="$caller_home/.wg" \
    "$WG_BIN" --dir "$caller_graph" add "negative Session Log smoke sentinel" \
        --id caller-negative-sentinel >/dev/null 2>&1 \
    || loud_fail "failed to add caller negative sentinel"
mkdir -p "$caller_graph/service"
printf '%s\n' '{"next_agent_id":7401,"agents":{"caller-sentinel-agent":{"id":"caller-sentinel-agent","pid":7401,"task_id":"caller-negative-sentinel","status":"failed"}}}' \
    >"$caller_graph/service/registry.json"

parent_graph_hash=$(sha256sum "$caller_graph/graph.jsonl" | awk '{print $1}')
parent_registry_hash=$(sha256sum "$caller_graph/service/registry.json" | awk '{print $1}')
parent_task_count=$(python3 - "$caller_graph/graph.jsonl" <<'PY'
import json, sys
print(sum(1 for line in open(sys.argv[1]) if line.strip() and json.loads(line).get("kind") == "task"))
PY
)

tmux_fixture_count() {
    tmux list-sessions -F '#{@wg_smoke_owned}|#{@wg_smoke_root}' 2>/dev/null \
        | awk -F'|' -v root="$target_root" '$1 == "wg-smoke-v1" && index($2, root) == 1 {n++} END {print n+0}'
}

assert_caller_unchanged() {
    local path="$1" graph_hash registry_hash task_count leftovers processes
    graph_hash=$(sha256sum "$caller_graph/graph.jsonl" | awk '{print $1}')
    registry_hash=$(sha256sum "$caller_graph/service/registry.json" | awk '{print $1}')
    task_count=$(python3 - "$caller_graph/graph.jsonl" <<'PY'
import json, sys
print(sum(1 for line in open(sys.argv[1]) if line.strip() and json.loads(line).get("kind") == "task"))
PY
)
    [[ "$graph_hash" == "$parent_graph_hash" ]] \
        || loud_fail "$path changed caller graph bytes: before=$parent_graph_hash after=$graph_hash"
    [[ "$task_count" == "$parent_task_count" ]] \
        || loud_fail "$path changed caller task count: before=$parent_task_count after=$task_count"
    [[ "$registry_hash" == "$parent_registry_hash" ]] \
        || loud_fail "$path changed caller service registry: before=$parent_registry_hash after=$registry_hash"
    grep -q '"id":"caller-negative-sentinel"' "$caller_graph/graph.jsonl" \
        || loud_fail "$path removed the caller negative sentinel"
    if grep -Eq '"id":"(smoke-live|\.flip-|\.evaluate-)' "$caller_graph/graph.jsonl"; then
        loud_fail "$path leaked Session Log fixture/agency tasks into caller graph: $(cat "$caller_graph/graph.jsonl")"
    fi
    leftovers=$(find "$target_root" -mindepth 1 -print -quit 2>/dev/null || true)
    [[ -z "$leftovers" ]] || loud_fail "$path left scratch state behind: $leftovers"
    [[ "$(tmux_fixture_count)" -eq 0 ]] \
        || loud_fail "$path left a scratch-owned tmux session behind"
    processes=$(CHECK_PREFIX="$target_root" python3 - <<'PY'
import os
prefix = os.environ["CHECK_PREFIX"].encode()
found = []
for name in os.listdir("/proc"):
    if not name.isdigit():
        continue
    try:
        argv = open(f"/proc/{name}/cmdline", "rb").read()
    except OSError:
        continue
    if prefix in argv:
        found.append(f"{name}:{argv.replace(chr(0).encode(), b' ').decode(errors='replace')}")
print("\n".join(found))
PY
)
    [[ -z "$processes" ]] || loud_fail "$path left scratch processes behind: $processes"
}

run_target() {
    local log="$1" candidate="$2"; shift 2
    (
        cd "$nested" || exit 125
        env HOME="$caller_home" XDG_CONFIG_HOME="$caller_home/.config" WG_GLOBAL_DIR="$caller_home/.wg" \
            WG_DIR="$caller_graph" WG_PROJECT_ROOT="$caller" WG_TASK_ID=caller-negative-sentinel \
            WG_AGENT_ID=caller-sentinel-agent WG_SMOKE_ROOT="$target_root" \
            WG_SMOKE_SCENARIO=tui_log_pane_renders_raw_stream \
            WG_SMOKE_CANDIDATE_BIN="$candidate" "$@" bash "$target"
    ) >"$log" 2>&1
}

# PASS: exercise the complete real tmux/TUI target.
pass_log="$scratch/pass.log"
run_target "$pass_log" "$WG_BIN"
pass_rc=$?
[[ $pass_rc -eq 0 ]] || loud_fail "nested pass path failed rc=$pass_rc: $(tail -30 "$pass_log")"
assert_caller_unchanged pass

# FAIL: let init succeed, then fail the explicitly-directed fixture add.
fail_wg="$scratch/fail-wg"
cat >"$fail_wg" <<'SH'
#!/usr/bin/env bash
for arg in "$@"; do
    [[ "$arg" == add ]] && exit 42
done
exec "$REAL_WG" "$@"
SH
chmod +x "$fail_wg"
fail_log="$scratch/fail.log"
set +e
run_target "$fail_log" "$fail_wg" REAL_WG="$WG_BIN"
fail_rc=$?
set -e
[[ $fail_rc -eq 1 ]] || loud_fail "forced failure path returned $fail_rc, expected 1: $(tail -20 "$fail_log")"
grep -q 'wg add failed during smoke setup' "$fail_log" \
    || loud_fail "forced failure did not reach the post-init fixture boundary: $(cat "$fail_log")"
assert_caller_unchanged failure

# SIGNAL: wait until the target owns a real tmux process, then terminate its
# scenario shell. The shared cleanup trap must reap tmux/process/scratch state.
signal_log="$scratch/signal.log"
(
    cd "$nested" || exit 125
    exec env HOME="$caller_home" XDG_CONFIG_HOME="$caller_home/.config" WG_GLOBAL_DIR="$caller_home/.wg" \
        WG_DIR="$caller_graph" WG_PROJECT_ROOT="$caller" WG_TASK_ID=caller-negative-sentinel \
        WG_AGENT_ID=caller-sentinel-agent WG_SMOKE_ROOT="$target_root" \
        WG_SMOKE_SCENARIO=tui_log_pane_renders_raw_stream \
        WG_SMOKE_CANDIDATE_BIN="$WG_BIN" bash "$target"
) >"$signal_log" 2>&1 &
signal_pid=$!
signal_ready=0
for _ in $(seq 1 160); do
    if [[ "$(tmux_fixture_count)" -gt 0 ]]; then
        signal_ready=1
        break
    fi
    kill -0 "$signal_pid" 2>/dev/null || break
    sleep 0.05
done
[[ $signal_ready -eq 1 ]] || loud_fail "signal path never acquired a scratch-owned tmux session: $(tail -30 "$signal_log")"
kill -TERM "$signal_pid" 2>/dev/null || loud_fail "could not signal target pid $signal_pid"
set +e
wait "$signal_pid"
set -e
# The shared helper deliberately preserves the status active at trap entry;
# cleanup invariants, not a particular shell-specific TERM code, are the bar.
assert_caller_unchanged signal

# EARLY SKIP: hide tmux before target scratch allocation. Even this path must
# preserve the live ancestor and leave no cleanup-owned state.
minimal_path="$scratch/no-tmux-bin"
mkdir -p "$minimal_path"
for tool in bash dirname mktemp rm; do
    tool_path=$(command -v "$tool")
    ln -s "$tool_path" "$minimal_path/$tool"
done
skip_log="$scratch/skip.log"
set +e
run_target "$skip_log" "$WG_BIN" PATH="$minimal_path"
skip_rc=$?
set -e
[[ $skip_rc -eq 77 ]] || loud_fail "early skip returned $skip_rc, expected 77: $(cat "$skip_log")"
grep -q 'SMOKE SKIPPED — MISSING TMUX' "$skip_log" \
    || loud_fail "early skip was not the expected missing-tmux boundary: $(cat "$skip_log")"
assert_caller_unchanged early-skip

echo "PASS: Session Log smoke is graph/user-state isolated and caller sentinel stays byte-identical on pass, failure, signal, and early skip"
