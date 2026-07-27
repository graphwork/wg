#!/usr/bin/env bash
#
# Real clean-room canary for WG's native Codex 5.6 worker, evaluator, and
# resumable chat paths. This intentionally does not stub or wrap `codex`.
#
# Runtime state is always removed. Output is limited to redacted assertions;
# Codex authentication bytes, model responses other than fixed canary tokens,
# and raw agent logs are never printed.
set -euo pipefail

case "$(basename "$0")" in
    pi | claude)
        printf '%s\n' "$(basename "$0")" >>"${WG_CANARY_SENTINEL_LOG:?}"
        exit 97
        ;;
esac

die() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

WG_BIN=$(command -v wg)
CODEX_BIN=$(command -v codex)
STRACE_BIN=$(command -v strace)
ORIGINAL_HOME=${HOME:?}
AUTH_SOURCE="$ORIGINAL_HOME/.codex/auth.json"
SCRIPT_PATH=$(cd "$(dirname "$0")" && pwd)/$(basename "$0")

[[ "$WG_BIN" = /* && -x "$WG_BIN" ]] || die "installed wg binary not found"
[[ "$CODEX_BIN" = /* && -x "$CODEX_BIN" ]] || die "real codex binary not found"
[[ "$STRACE_BIN" = /* && -x "$STRACE_BIN" ]] || die "strace not found"
[[ -e "$AUTH_SOURCE" ]] || die "Codex auth file not found"
"$CODEX_BIN" login status >/dev/null 2>&1 || die "codex login status failed"

CANARY_ROOT=$(mktemp -d /tmp/wg-native-codex56-canary.XXXXXX)
CANARY_HOME="$CANARY_ROOT/home"
CANARY_REPO="$CANARY_ROOT/repo"
CANARY_GRAPH="$CANARY_REPO/.wg"
CANARY_BIN="$CANARY_ROOT/sentinel-bin"
SENTINEL_LOG="$CANARY_ROOT/cross-runtime-sentinel.log"
MODELS_JSON="$CANARY_ROOT/models.json"
WORKER_JSON="$CANARY_ROOT/worker.json"
WORKER_PID_FILE="$CANARY_ROOT/worker-pid"
EVALUATE_OUT="$CANARY_ROOT/evaluate.out"
EVALUATE_ERR="$CANARY_ROOT/evaluate.err"
EVALUATOR_TRACE="$CANARY_ROOT/evaluator-exec.trace"
CHAT_CREATE_JSON="$CANARY_ROOT/chat-create.json"
CHAT_SHOW_JSON="$CANARY_ROOT/chat-show.json"
PROFILE_NAME=codex-56-high
TASK_ID=native-codex56-worker
CANARY_FILE=CANARY_NATIVE_CODEX56.txt
CANARY_CONTENT=native-codex-5.6-sol-canary
CHAT_NAME=native-sol-resume
CHAT_CONTEXT_TOKEN=NATIVE_RESUME_56_ALPHA
CHAT_FIRST_REPLY=CODEX56_TURN1_OK

# Every nested command starts without inherited parent-graph state. Preserve
# the normal host environment (proxy/cert configuration can be required for a
# real Codex login), but remove every inherited WG variable before setting the
# one sentinel-only variable used by this canary.
CLEAN_ENV=(env)
while IFS= read -r name; do
    [[ "$name" == WG_* ]] && CLEAN_ENV+=(-u "$name")
done < <(compgen -e)

run_wg() {
    "${CLEAN_ENV[@]}" \
        HOME="$CANARY_HOME" \
        XDG_CONFIG_HOME="$CANARY_HOME/.config" \
        PATH="$CANARY_BIN:$PATH" \
        WG_CANARY_SENTINEL_LOG="$SENTINEL_LOG" \
        "$WG_BIN" --dir "$CANARY_GRAPH" "$@"
}

cleanup() {
    if [[ -n "${CANARY_ROOT:-}" && "$CANARY_ROOT" == /tmp/wg-native-codex56-canary.* ]]; then
        run_wg service stop --kill-agents >/dev/null 2>&1 || true
        # `service stop` can return just before its daemon finishes one final
        # registry write. Repeated deletion closes that bounded race without
        # ever broadening the target beyond this validated mktemp directory.
        for _ in $(seq 1 100); do
            find "$CANARY_ROOT" -depth -delete 2>/dev/null || true
            [[ ! -e "$CANARY_ROOT" ]] && return
            sleep 0.1
        done
    fi
}
trap cleanup EXIT INT TERM

wait_task_status() {
    local task=$1
    local expected=$2
    local status=
    local _
    for _ in $(seq 1 600); do
        if run_wg --json show "$task" >"$WORKER_JSON" 2>/dev/null; then
            status=$(python3 - "$WORKER_JSON" <<'PY'
import json, sys
print(json.load(open(sys.argv[1])).get("status", ""))
PY
)
            [[ "$status" == "$expected" ]] && return 0
            case "$status" in
                failed | abandoned | failed-pending-eval)
                    return 1
                    ;;
            esac
        fi
        sleep 0.5
    done
    return 1
}

wait_chat_handler() {
    local _
    for _ in $(seq 1 360); do
        if run_wg --json chat show "$CHAT_NAME" >"$CHAT_SHOW_JSON" 2>/dev/null &&
            python3 - "$CHAT_SHOW_JSON" <<'PY' >/dev/null 2>&1
import json, sys
value = json.load(open(sys.argv[1]))
raise SystemExit(0 if (value.get("handler") or {}).get("kind") == "adapter" else 1)
PY
        then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

find_outbox() {
    find "$CANARY_GRAPH/chat" -type f -name outbox.jsonl -print -quit 2>/dev/null
}

wait_outbox_count() {
    local expected=$1
    local outbox=
    local count=
    local _
    for _ in $(seq 1 720); do
        outbox=$(find_outbox)
        if [[ -n "$outbox" ]]; then
            count=$(python3 - "$outbox" <<'PY'
import json, sys
print(sum(1 for line in open(sys.argv[1]) if line.strip() and json.loads(line)))
PY
)
            [[ "$count" == "$expected" ]] && return 0
        fi
        sleep 0.25
    done
    return 1
}

mkdir -m 700 -p "$CANARY_HOME/.codex" "$CANARY_HOME/.config" "$CANARY_REPO" "$CANARY_BIN"
ln -s "$AUTH_SOURCE" "$CANARY_HOME/.codex/auth.json"
ln -s "$SCRIPT_PATH" "$CANARY_BIN/pi"
ln -s "$SCRIPT_PATH" "$CANARY_BIN/claude"

git -C "$CANARY_REPO" init -q
git -C "$CANARY_REPO" config user.name "WG native Codex canary"
git -C "$CANARY_REPO" config user.email "wg-canary@example.invalid"

cd "$CANARY_REPO"
run_wg init >/dev/null 2>&1
run_wg profile create "$PROFILE_NAME" --from codex >/dev/null
run_wg profile pi \
    --profile "$PROFILE_NAME" \
    --strong codex:gpt-5.6-sol \
    --weak codex:gpt-5.6-luna \
    --strong-reasoning high \
    --weak-reasoning high \
    --no-reload >/dev/null
run_wg profile select "$PROFILE_NAME" --no-reload >/dev/null
run_wg config \
    --auto-assign false \
    --auto-evaluate false \
    --flip-enabled false \
    --eval-gate-threshold 0 \
    --eval-gate-all false \
    --no-reload >/dev/null
run_wg --json config --models >"$MODELS_JSON"

python3 - "$MODELS_JSON" <<'PY' || die "fresh project route assertion failed"
import json, sys
models = json.load(open(sys.argv[1]))
worker = models["task_agent"]
evaluator = models["evaluator"]
assert worker["handler"] == "codex", worker
assert worker["route"] == "codex:gpt-5.6-sol", worker
assert worker["model"] == "gpt-5.6-sol", worker
assert worker["reasoning"] == "high", worker
assert evaluator["handler"] == "codex", evaluator
assert evaluator["route"] == "codex:gpt-5.6-luna", evaluator
assert evaluator["model"] == "gpt-5.6-luna", evaluator
assert evaluator["reasoning"] == "high", evaluator
assert all(not value["route"].startswith("pi:openai-codex:") for value in models.values())
PY

git -C "$CANARY_REPO" add .gitignore AGENTS.md CLAUDE.md
git -C "$CANARY_REPO" commit -qm "chore: initialize disposable WG canary"

TASK_DESCRIPTION=$(printf '%s\n' \
    "Create a file named $CANARY_FILE at the repository root with exactly this one line:" \
    "$CANARY_CONTENT" \
    "" \
    "This is a disposable repository with no remote. Do not push." \
    "Record the artifact with: $WG_BIN --dir $CANARY_GRAPH artifact $TASK_ID $CANARY_FILE" \
    "Commit only $CANARY_FILE locally with message: canary: native Codex 5.6 Sol" \
    "Then complete with: $WG_BIN --dir $CANARY_GRAPH done $TASK_ID" \
    "" \
    "## Validation" \
    "- [ ] test \"\$(cat $CANARY_FILE)\" = \"$CANARY_CONTENT\"" \
    "- [ ] Exactly one local commit contains $CANARY_FILE")

run_wg add "Native Codex 5.6 Sol deterministic canary" \
    --id "$TASK_ID" \
    --independent \
    --scope disposable \
    --context-scope task \
    --timeout 8m \
    --description "$TASK_DESCRIPTION" >/dev/null
run_wg publish "$TASK_ID" --only >/dev/null
run_wg service start --max-agents 1 --interval 1 --no-chat-agent >/dev/null

wait_task_status "$TASK_ID" done || die "Sol worker did not complete successfully within the bound"

# The wrapper can still be finalizing the isolated worktree after `wg done`.
for _ in $(seq 1 120); do
    [[ -f "$CANARY_REPO/$CANARY_FILE" ]] && break
    sleep 0.25
done

run_wg --json show "$TASK_ID" >"$WORKER_JSON"
python3 - "$CANARY_GRAPH" "$WORKER_JSON" "$CANARY_REPO" "$CANARY_FILE" "$CANARY_CONTENT" "$WORKER_PID_FILE" <<'PY' \
    || die "Sol worker metadata/artifact assertion failed"
import json, pathlib, sys
graph = pathlib.Path(sys.argv[1])
shown = json.load(open(sys.argv[2]))
repo = pathlib.Path(sys.argv[3])
filename = sys.argv[4]
expected = sys.argv[5] + "\n"
metadata = []
for path in (graph / "agents").glob("*/metadata.json"):
    value = json.load(open(path))
    if value.get("task_id") == "native-codex56-worker":
        metadata.append((path, value))
assert len(metadata) == 1, metadata
_, meta = metadata[0]
assert meta["executor"] == "codex", meta
assert meta["model"] == "codex:gpt-5.6-sol", meta
assert meta["native_model"] == "gpt-5.6-sol", meta
assert meta["reasoning"] == "high", meta
spawn_logs = [
    entry.get("message", "")
    for entry in shown.get("log", [])
    if "Spawned by " in entry.get("message", "")
]
assert len(spawn_logs) == 1, spawn_logs
assert "--executor codex" in spawn_logs[0], spawn_logs
assert "--model codex:gpt-5.6-sol" in spawn_logs[0], spawn_logs
candidates = [repo / filename]
if meta.get("effective_cwd"):
    candidates.append(pathlib.Path(meta["effective_cwd"]) / filename)
artifact = next((path for path in candidates if path.is_file()), None)
assert artifact is not None, candidates
assert artifact.read_text() == expected, artifact.read_text()
pathlib.Path(sys.argv[6]).write_text(str(meta["pid"]) + "\n")
PY

# A task can become immutable/done just before its agent wrapper finishes
# accounting and exits. Waiting on the recorded wrapper PID prevents a late
# usage write from recreating disposable graph state during cleanup.
WORKER_PID=$(<"$WORKER_PID_FILE")
for _ in $(seq 1 1200); do
    kill -0 "$WORKER_PID" 2>/dev/null || break
    sleep 0.05
done
kill -0 "$WORKER_PID" 2>/dev/null && die "Sol worker wrapper did not exit within the bound"

run_wg service stop --kill-agents >/dev/null 2>&1 || die "could not stop worker service"

# Trace only process launches around the real one-shot evaluator. There is no
# Codex wrapper or shadow in PATH: the successful execve must target the exact
# host Codex path discovered before the clean room was created. The raw trace
# stays inside disposable state and is never printed or preserved.
set +e
"$STRACE_BIN" -f -s 1048576 -e trace=execve -o "$EVALUATOR_TRACE" \
    "${CLEAN_ENV[@]}" \
    HOME="$CANARY_HOME" \
    XDG_CONFIG_HOME="$CANARY_HOME/.config" \
    PATH="$CANARY_BIN:$PATH" \
    WG_CANARY_SENTINEL_LOG="$SENTINEL_LOG" \
    "$WG_BIN" --dir "$CANARY_GRAPH" evaluate run "$TASK_ID" \
    >"$EVALUATE_OUT" 2>"$EVALUATE_ERR"
EVALUATE_RC=$?
set -e
[[ "$EVALUATE_RC" == 0 ]] || die "manual Luna evaluation failed"

python3 - "$EVALUATOR_TRACE" "$CANARY_GRAPH" "$TASK_ID" "$CODEX_BIN" <<'PY' \
    || die "Luna evaluator invocation/verdict assertion failed"
import json, pathlib, re, sys
trace = pathlib.Path(sys.argv[1]).read_text(errors="replace").splitlines()
codex = sys.argv[4]
launches = [
    line for line in trace
    if f'execve("{codex}", ' in line and line.rstrip().endswith("= 0")
]
assert len(launches) == 1, len(launches)
launch = launches[0]
assert '"exec"' in launch
assert re.search(r'"--model", "gpt-5\.6-luna"', launch)
assert 'model_reasoning_effort=\\"high\\"' in launch
graph = pathlib.Path(sys.argv[2])
task_id = sys.argv[3]
records = []
for path in (graph / "agency" / "evaluations").glob("*.json"):
    value = json.load(open(path))
    if value.get("task_id") == task_id:
        records.append(value)
assert len(records) == 1, records
record = records[0]
assert record["evaluator"] == "codex:gpt-5.6-luna", record
assert isinstance(record["score"], (int, float)), record
assert 0.0 <= record["score"] <= 1.0, record
assert isinstance(record["dimensions"], dict) and record["dimensions"], record
PY

run_wg --json chat create \
    --name "$CHAT_NAME" \
    --executor codex \
    --model codex:gpt-5.6-sol >"$CHAT_CREATE_JSON"
run_wg service start --max-agents 1 --interval 1 >/dev/null
wait_chat_handler || die "native Codex chat handler did not start"

FIRST_PROMPT="Memorize context token $CHAT_CONTEXT_TOKEN. Reply with exactly $CHAT_FIRST_REPLY and nothing else."
run_wg chat send "$CHAT_NAME" "$FIRST_PROMPT" >/dev/null
wait_outbox_count 1 || die "first native Codex chat reply timed out"

OUTBOX=$(find_outbox)
SESSION_MARKER=$(find "$CANARY_GRAPH/chat" -type f -name .codex-session-id -print -quit)
[[ -n "$SESSION_MARKER" && -s "$SESSION_MARKER" ]] || die "native Codex session marker missing"
SESSION_HASH_BEFORE=$(sha256sum "$SESSION_MARKER" | cut -d' ' -f1)
python3 - "$OUTBOX" "$CHAT_FIRST_REPLY" <<'PY' || die "first chat reply was not exact"
import json, sys
messages = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
assert len(messages) == 1, messages
assert messages[0]["content"].strip() == sys.argv[2], messages[0]["content"]
PY

run_wg service stop --force --kill-agents >/dev/null 2>&1 || die "chat daemon restart stop failed"
run_wg service start --max-agents 1 --interval 1 >/dev/null
wait_chat_handler || die "native Codex chat handler did not resume after daemon restart"

SECOND_PROMPT="Reply with exactly the context token I told you to memorize in the previous turn and nothing else."
run_wg chat send "$CHAT_NAME" "$SECOND_PROMPT" >/dev/null
wait_outbox_count 2 || die "resumed native Codex chat reply timed out"
SESSION_HASH_AFTER=$(sha256sum "$SESSION_MARKER" | cut -d' ' -f1)
[[ "$SESSION_HASH_BEFORE" == "$SESSION_HASH_AFTER" ]] || die "Codex session identity changed across restart"

python3 - "$OUTBOX" "$CHAT_FIRST_REPLY" "$CHAT_CONTEXT_TOKEN" "$CANARY_GRAPH/service/daemon.log" <<'PY' \
    || die "native Codex chat resume assertion failed"
import json, sys
messages = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
assert len(messages) == 2, messages
assert messages[0]["content"].strip() == sys.argv[2], messages[0]["content"]
assert messages[1]["content"].strip() == sys.argv[3], messages[1]["content"]
log = open(sys.argv[4], errors="replace").read()
assert log.count("codex-handler: spawning `codex exec` ") == 1, log.count("codex-handler: spawning `codex exec` ")
assert log.count("codex-handler: spawning `codex exec resume` ") == 1, log.count("codex-handler: spawning `codex exec resume` ")
assert "model=codex:gpt-5.6-sol" in log, "missing exact native chat route"
PY

[[ ! -s "$SENTINEL_LOG" ]] || die "Pi or Claude sentinel was invoked"
run_wg service stop --force --kill-agents >/dev/null 2>&1 || die "final nested service stop failed"

printf '%s\n' \
    "codex_login_status=success" \
    "parent_contamination_guard=all_inherited_WG_variables_removed" \
    "profile_task_agent=handler:codex,route:codex:gpt-5.6-sol,reasoning:high" \
    "profile_evaluator=handler:codex,route:codex:gpt-5.6-luna,reasoning:high" \
    "worker_attempts=1" \
    "worker_metadata=executor:codex,model:codex:gpt-5.6-sol,native_model:gpt-5.6-sol,reasoning:high" \
    "worker_artifact=deterministic" \
    "evaluator_invocations=1" \
    "evaluator_record=route:codex:gpt-5.6-luna,reasoning:high,verdict:parseable" \
    "chat_turn1=exact" \
    "chat_restart_resume=session_identity_preserved,context_reply_exact" \
    "pi_sentinel_invocations=0" \
    "claude_sentinel_invocations=0" \
    "nested_service=stopped"

cleanup
trap - EXIT INT TERM
[[ ! -e "$CANARY_ROOT" ]] || die "temporary state removal failed"
printf '%s\n' "temporary_state=removed" "PASS: native Codex 5.6 clean-room canary"
