#!/usr/bin/env bash
# Scenario: provider_completion_refusal_breaker
#
# Live daemon/spawn regression for Luca 02204b19's provider-pause diagnosis.
# A fake Codex worker reaches the real `wg done` boundary and is refused after
# a dependency is inserted post-spawn. Its blocker title contains HTTP 401/quota.
# Three such dead wrapper runs must leave provider health untouched. Then three
# genuine fake provider-auth crashes must still trip exactly the spawned Codex
# route's breaker. No provider credential is required.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
# The smoke harness may itself be launched from a worker. Service control is
# the human/daemon boundary under test, so do not leak the parent worker role.
unset WG_AGENT_ID WG_TASK_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER
if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON3" "python3 is required to inspect provider health fixtures"
fi

scratch=$(make_scratch)
fake_home="$scratch/home"
fake_bin="$scratch/bin"
project="$scratch/project"
mkdir -p "$fake_home/.config/workgraph" "$fake_bin" "$project"
: >"$fake_home/.config/workgraph/config.toml"

cat >"$fake_bin/codex" <<'SH'
#!/usr/bin/env bash
set -u

# Find and kill the spawned run.sh wrapper without letting its safety-net
# `wg fail` rewrite the registry status. This exercises the dead-agent triage
# and breaker path rather than only the pure classifier.
kill_wrapper() {
    local pid="$PPID" args parent
    for _ in 1 2 3 4 5 6; do
        args=$(ps -o args= -p "$pid" 2>/dev/null || true)
        if [[ "$args" == *"/run.sh"* ]]; then
            kill -9 "$pid" 2>/dev/null || true
            return 0
        fi
        parent=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ' || true)
        [[ -n "$parent" && "$parent" != "0" && "$parent" != "$pid" ]] || break
        pid="$parent"
    done
    echo "fake codex could not locate run.sh ancestor" >&2
    return 1
}

case "${WG_TASK_ID:-}" in
    refusal-*)
        : "${WG_FAKE_SYNC_DIR:?}"
        touch "$WG_FAKE_SYNC_DIR/started-$WG_TASK_ID"
        while [[ ! -f "$WG_FAKE_SYNC_DIR/release" ]]; do sleep 0.05; done
        wg log "$WG_TASK_ID" "fake worker reached completion boundary" >/dev/null 2>&1 || true
        # The smoke adds a blocked dependency only after this process starts.
        # `wg done` must write typed completion-refused provenance even though
        # the embedded blocker title contains provider-looking words.
        wg done "$WG_TASK_ID" >/dev/null 2>&1 || true
        wg pause "$WG_TASK_ID" >/dev/null 2>&1 || true
        kill_wrapper
        exit 0
        ;;
    auth-*)
        echo "authentication failed (HTTP 401): quota exhausted" >&2
        kill_wrapper
        exit 1
        ;;
    *)
        echo "unexpected fake task ${WG_TASK_ID:-unset}" >&2
        exit 2
        ;;
esac
SH
chmod +x "$fake_bin/codex"

cd "$project"
export HOME="$fake_home"
export XDG_CONFIG_HOME="$fake_home/.config"
export PATH="$fake_bin:$PATH"
export WG_FAKE_SYNC_DIR="$scratch/sync"
mkdir -p "$WG_FAKE_SYNC_DIR"

run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" PATH="$PATH" \
        WG_FAKE_SYNC_DIR="$WG_FAKE_SYNC_DIR" wg "$@"
}

if ! run_wg init --route codex-cli --no-agency >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -10 init.log)"
fi
wg_dir="$project/.wg"
# Make dead-wrapper triage deterministic and fast for the smoke.
python3 - "$wg_dir/config.toml" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1])
s = p.read_text()
s = s.replace("reaper_grace_seconds = 30", "reaper_grace_seconds = 0")
s = s.replace("settling_delay_ms = 2000", "settling_delay_ms = 0")
p.write_text(s)
PY

for n in 1 2 3; do
    if ! run_wg add "Authentication failed HTTP 401 quota blocker $n" \
            --id "blocker-$n" --paused --no-place \
            -d "Paused graph blocker; it must never be interpreted as provider stderr." \
            >"add-blocker-$n.log" 2>&1; then
        loud_fail "failed to add blocker-$n: $(tail -10 "add-blocker-$n.log")"
    fi
    if ! run_wg add "graph refusal worker $n" --id "refusal-$n" --no-place \
            -d "Worker starts ready; the smoke inserts its blocker before wg done." \
            >"add-refusal-$n.log" 2>&1; then
        loud_fail "failed to add refusal-$n: $(tail -10 "add-refusal-$n.log")"
    fi
done

if ! start_wg_daemon "$project" --max-agents 3 --no-coordinator-agent --interval 1; then
    loud_fail "failed to start refusal-phase daemon"
fi
graph_dir="$WG_SMOKE_DAEMON_DIR"
health_file="$graph_dir/service/provider_health.json"

started=false
for _ in $(seq 1 80); do
    if [[ -f "$WG_FAKE_SYNC_DIR/started-refusal-1" \
       && -f "$WG_FAKE_SYNC_DIR/started-refusal-2" \
       && -f "$WG_FAKE_SYNC_DIR/started-refusal-3" ]]; then
        started=true
        break
    fi
    sleep 0.25
done
if ! $started; then
    loud_fail "fake refusal workers did not all start: $(ls -la "$WG_FAKE_SYNC_DIR" 2>/dev/null || true)"
fi
for n in 1 2 3; do
    run_wg add-dep "refusal-$n" "blocker-$n" >/dev/null 2>&1 \
        || loud_fail "could not insert blocker-$n after refusal-$n was spawned"
done
touch "$WG_FAKE_SYNC_DIR/release"

refusal_ok=false
for _ in $(seq 1 80); do
    if [[ -f "$health_file" ]] && python3 - "$graph_dir" "$health_file" <<'PY' >/dev/null 2>&1
import glob, json, os, sys
graph, health_path = sys.argv[1:]
outcomes = []
for path in glob.glob(os.path.join(graph, "agents", "agent-*", "outcome.json")):
    try:
        row = json.load(open(path))
    except Exception:
        continue
    if row.get("task_id", "").startswith("refusal-") and row.get("outcome", {}).get("type") == "completion-refused":
        outcomes.append(row)
health = json.load(open(health_path))
assert len({r["task_id"] for r in outcomes}) == 3
assert health.get("providers", {}) == {}
assert health.get("service_paused") is False
PY
    then
        refusal_ok=true
        break
    fi
    sleep 0.5
done

if ! $refusal_ok; then
    loud_fail "three live wg-done refusals did not remain breaker-neutral. health:\n$(cat "$health_file" 2>/dev/null || echo '<missing>')\ndaemon tail:\n$(tail -80 "$graph_dir/service/daemon.log" 2>/dev/null || true)"
fi

echo "PASS (1/2): three typed graph-policy refusals left provider counters at zero"
run_wg service stop --force >/dev/null 2>&1 || true

for n in 1 2 3; do
    if ! run_wg add "real provider auth failure $n" --id "auth-$n" --no-place \
            -d "Fake Codex emits a genuine HTTP 401 and crashes its wrapper." \
            >"add-auth-$n.log" 2>&1; then
        loud_fail "failed to add auth-$n: $(tail -10 "add-auth-$n.log")"
    fi
done

if ! start_wg_daemon "$project" --max-agents 3 --no-coordinator-agent --interval 1; then
    loud_fail "failed to start auth-phase daemon"
fi
graph_dir="$WG_SMOKE_DAEMON_DIR"
health_file="$graph_dir/service/provider_health.json"

auth_ok=false
for _ in $(seq 1 80); do
    if [[ -f "$health_file" ]] && python3 - "$health_file" <<'PY' >/dev/null 2>&1
import json, sys
h = json.load(open(sys.argv[1]))
providers = h.get("providers", {})
assert len(providers) == 1, providers
(key, value), = providers.items()
assert key.startswith("codex|openai-codex-cli|self-authenticated"), key
assert value.get("consecutive_failures") == 3, value
assert value.get("is_paused") is True, value
assert h.get("service_paused") is True, h
PY
    then
        auth_ok=true
        break
    fi
    sleep 0.5
done

if ! $auth_ok; then
    loud_fail "real provider 401 crashes did not trip exactly one route breaker. health:\n$(cat "$health_file" 2>/dev/null || echo '<missing>')\ndaemon tail:\n$(tail -100 "$graph_dir/service/daemon.log" 2>/dev/null || true)"
fi

echo "PASS (2/2): real HTTP 401 failures still reached threshold for exactly the Codex route"
echo "PASS: completion-refusal provenance protects the live provider breaker"
exit 0
