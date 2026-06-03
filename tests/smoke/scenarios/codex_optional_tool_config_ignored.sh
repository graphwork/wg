#!/usr/bin/env bash
# Scenario: codex_optional_tool_config_ignored
#
# Regression for fix-autopoietic-loop-failure: a Codex worker must not inherit
# optional user config that can inject an unavailable image/tool model such as
# gpt-image-2 and abort before the task starts. The fake codex binary below
# exits with the historical startup error unless WG invokes `codex exec` with
# `--ignore-user-config`.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
fake_home="$scratch/home"
fake_bin="$scratch/bin"
argv_log="$scratch/codex-argv.log"
project="$scratch/proj"

mkdir -p "$fake_home/.config/workgraph" "$fake_bin" "$project"
: >"$fake_home/.config/workgraph/config.toml"

cat >"$fake_bin/codex" <<'EOF'
#!/usr/bin/env bash
set -u

: "${WG_FAKE_CODEX_ARGV_LOG:?}"
printf '%s\n' "$@" >"$WG_FAKE_CODEX_ARGV_LOG"

seen_ignore=false
for arg in "$@"; do
    if [[ "$arg" == "--ignore-user-config" ]]; then
        seen_ignore=true
        break
    fi
done

if ! $seen_ignore; then
    printf "The model 'gpt-image-2' does not exist.\nparam: tools\n" >&2
    exit 1
fi

prompt="$(cat || true)"
if [[ -z "$prompt" ]]; then
    printf "No prompt provided via stdin\n" >&2
    exit 2
fi

wg log "$WG_TASK_ID" "fake codex accepted isolated WG config" >/dev/null 2>&1 || true
exit 0
EOF
chmod +x "$fake_bin/codex"

cd "$project"

export HOME="$fake_home"
export XDG_CONFIG_HOME="$fake_home/.config"
export PATH="$fake_bin:$PATH"
export WG_FAKE_CODEX_ARGV_LOG="$argv_log"

run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" \
        PATH="$PATH" WG_FAKE_CODEX_ARGV_LOG="$WG_FAKE_CODEX_ARGV_LOG" \
        wg "$@"
}

if ! run_wg init --route codex-cli --no-agency >init.log 2>&1; then
    loud_fail "wg init --route codex-cli failed: $(tail -10 init.log)"
fi

if ! run_wg config --auto-assign false --no-reload >config.log 2>&1; then
    loud_fail "wg config --auto-assign false failed: $(tail -10 config.log)"
fi

if ! run_wg add "codex optional tool config smoke" \
        --id codex-optional-tool-smoke \
        --no-place \
        -d "Smoke task: fake Codex succeeds only when WG isolates user config." \
        >add.log 2>&1; then
    loud_fail "wg add failed: $(tail -10 add.log)"
fi

if ! start_wg_daemon "$project" --max-agents 1 --no-coordinator-agent --interval 1; then
    loud_fail "start_wg_daemon failed"
fi

graph_dir="$WG_SMOKE_DAEMON_DIR"

status=""
for _ in $(seq 1 80); do
    show_json=$(run_wg show codex-optional-tool-smoke --json 2>/dev/null || true)
    status=$(sed -n 's/.*"status": *"\([^"]*\)".*/\1/p' <<<"$show_json" | head -1)
    if [[ "$status" == "done" || "$status" == "failed" || "$status" == "failed-pending-eval" ]]; then
        break
    fi
    sleep 0.5
done

if [[ ! -f "$argv_log" ]]; then
    loud_fail "fake codex was not invoked. daemon log tail:
$(tail -40 "$graph_dir/service/daemon.log" 2>/dev/null || echo '<no daemon log>')"
fi

if ! grep -q -- "--ignore-user-config" "$argv_log"; then
    loud_fail "codex worker argv did not include --ignore-user-config, so ambient optional tool config can still abort startup. Captured argv:
$(cat "$argv_log")"
fi

if [[ "$status" != "done" ]]; then
    show_out=$(run_wg show codex-optional-tool-smoke 2>&1 || true)
    loud_fail "codex optional tool smoke task did not finish done (status=${status:-unknown}). Task:
$show_out
daemon log tail:
$(tail -40 "$graph_dir/service/daemon.log" 2>/dev/null || echo '<no daemon log>')"
fi

echo "PASS: codex worker spawn isolates user config with --ignore-user-config"
exit 0
