#!/usr/bin/env bash
# Credential-free attended-chat behavioral regression. A fake native Claude
# model is driven through the real daemon + TUI composer. It obeys the injected
# first-turn contract and records/executes the tool decisions a model would
# make: read a known file, leave discussion/read-only state untouched, perform
# an explicitly requested harmless edit/check, delegate a WG task, and clarify
# an ambiguous destructive discussion without inventing a chat-only denylist.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v python3 >/dev/null 2>&1 \
  || loud_skip "MISSING PYTHON" "python3 is required for assertions"
command -v tmux >/dev/null 2>&1 \
  || loud_skip "MISSING TMUX" "tmux is required for the attended TUI flow"

scratch=$(make_scratch)
home="$scratch/home"
bin="$scratch/bin"
project="$scratch/project"
mkdir -p "$home/.config/workgraph" "$bin" "$project"
: >"$home/.config/workgraph/config.toml"
printf 'KNOWN_REPOSITORY_VALUE_937\n' >"$project/known-repository-file.txt"
printf 'MUST_REMAIN_UNCHANGED\n' >"$project/read-only-guard.txt"

cat >"$bin/claude" <<'SH'
#!/usr/bin/env bash
set -u
if [[ "${1:-}" == "--version" ]]; then
  echo '2.1.0 (fake attended authority model)'
  exit 0
fi
printf '%s\n' "$@" >"$HOME/claude-attended-argv"
argv=" $* "
if [[ "$argv" == *" --allowedTools "* ]]; then
  echo 'ATTENDED_TOOLS_WERE_NARROWED' >&2
  exit 90
fi
prompt=""
args=("$@")
for ((i=0; i<${#args[@]}; i++)); do
  if [[ "${args[$i]}" == "--system-prompt" && $((i+1)) -lt ${#args[@]} ]]; then
    prompt="${args[$((i+1))]}"
  fi
done
printf '%s' "$prompt" >"$HOME/attended-system-prompt"
for required in \
  "human's attended repository assistant" \
  "Follow the human's request" \
  "normal tool surface: read, search, write, edit, execute, test" \
  "no role-based operation denylist" \
  "Never say that the chat contract prohibits repository reads"; do
  if [[ "$prompt" != *"$required"* ]]; then
    echo "MISSING_OPERATOR_CONTRACT: $required" >&2
    exit 91
  fi
done
if [[ "$prompt" == *"You do NOT read source files"* \
   || "$prompt" == *"The ONLY files you may read are WG state"* ]]; then
  echo 'RETIRED_BLANKET_PROHIBITION_PRESENT' >&2
  exit 92
fi

printf '%s\n' '{"type":"system","subtype":"init","session_id":"attended-authority-session"}'
while IFS= read -r line; do
  case "$line" in
    *READ_KNOWN_FILE_937*)
      path="$PROJECT/known-repository-file.txt"
      value=$(cat "$path") || exit 93
      printf 'action=Read path=%s result=%s\n' "$path" "$value" >>"$HOME/tool-events.log"
      printf '%s\n' "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"read-1\",\"name\":\"Read\",\"input\":{\"file_path\":\"$path\"}},{\"type\":\"text\",\"text\":\"READ_ANSWER_937: $value\"}]}}"
      printf '%s\n' "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"read-1\",\"content\":\"$value\"}]}}"
      printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"READ_ANSWER_937"}'
      ;;
    *DISCUSS_ONLY_937*)
      printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"DISCUSSION_ANSWER_937: two options, no mutation performed"}]}}'
      printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"DISCUSSION_ANSWER_937"}'
      ;;
    *HARMLESS_EDIT_AND_CHECK_937*)
      path="$PROJECT/harmless-user-directed.txt"
      printf 'USER_DIRECTED_EDIT_937\n' >"$path"
      grep -q '^USER_DIRECTED_EDIT_937$' "$path" || exit 94
      printf 'action=Edit path=%s result=written\n' "$path" >>"$HOME/tool-events.log"
      printf 'action=Bash command=grep-check result=pass\n' >>"$HOME/tool-events.log"
      printf '%s\n' "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"edit-1\",\"name\":\"Edit\",\"input\":{\"file_path\":\"$path\"}},{\"type\":\"tool_use\",\"id\":\"bash-1\",\"name\":\"Bash\",\"input\":{\"command\":\"grep -q USER_DIRECTED_EDIT_937 $path\"}},{\"type\":\"text\",\"text\":\"EDIT_AND_CHECK_PASS_937\"}]}}"
      printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"EDIT_AND_CHECK_PASS_937"}'
      ;;
    *DELEGATE_WG_TASK_937*)
      wg --dir "$WG_DIR" add 'Attended delegated task' --id attended-delegated-937 \
        -d $'Delegated at explicit human request.\n\n## Validation\n- [ ] report completion' >/dev/null || exit 95
      wg --dir "$WG_DIR" publish attended-delegated-937 --only >/dev/null || exit 96
      printf 'action=WG command=add+publish task=attended-delegated-937\n' >>"$HOME/tool-events.log"
      printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"DELEGATION_PASS_937: attended-delegated-937 published"}]}}'
      printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"DELEGATION_PASS_937"}'
      ;;
    *AMBIGUOUS_DELETION_DISCUSSION_937*)
      # This is discussion, not an explicit operation request. Clarification is
      # warranted because intent is ambiguous—not because chat has a denylist.
      printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"CLARIFICATION_REQUIRED_937: are you asking me to delete it, or only explaining the consequence?"}]}}'
      printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"CLARIFICATION_REQUIRED_937"}'
      ;;
    *)
      echo "unexpected attended prompt: $line" >&2
      exit 97
      ;;
  esac
done
SH
chmod +x "$bin/claude"

export HOME="$home"
export XDG_CONFIG_HOME="$home/.config"
export PATH="$bin:$PATH"
export PROJECT="$project"
unset WG_EXECUTOR_TYPE WG_MODEL WG_REASONING WG_TIER WG_AGENT_ID
cd "$project"
wg init --no-agency >init.log 2>&1 || loud_fail "init failed: $(tail -30 init.log)"
wg profile use claude --no-reload >profile.log 2>&1 || loud_fail "profile failed: $(cat profile.log)"
wg config --local --model claude:opus --reasoning high --no-reload >>profile.log 2>&1 \
  || loud_fail "route config failed: $(cat profile.log)"
start_wg_daemon "$project" --max-agents 0 --interval 1 \
  || loud_fail "attended chat daemon failed"
G="$WG_SMOKE_DAEMON_DIR"
wg --dir "$G" chat create --name authority --executor claude --model claude:opus \
  >create.log 2>&1 || loud_fail "chat create failed: $(cat create.log)"

session="wgsmoke-attended-authority-$$"
trace="$scratch/tui-trace.jsonl"
tmux new-session -d -s "$session" -x 220 -y 60 \
  "wg --dir '$G' tui --no-mouse --trace '$trace'"
cleanup_tmux() { tmux kill-session -t "$session" 2>/dev/null || true; }
add_cleanup_hook cleanup_tmux

for _ in $(seq 1 120); do
  if wg --dir "$G" chat show 0 --json 2>/dev/null \
    | python3 -c 'import json,sys; raise SystemExit(0 if (json.load(sys.stdin).get("handler") or {}).get("kind")=="adapter" else 1)' 2>/dev/null; then
    break
  fi
  sleep 0.1
done

tmux send-keys -t "$session" c
for _ in $(seq 1 80); do
  mode=$(wg --dir "$G" --json tui-dump 2>/dev/null \
    | python3 -c 'import json,sys; print(json.load(sys.stdin).get("input_mode", ""))' 2>/dev/null || true)
  [[ "$mode" == "ChatInput" ]] && break
  sleep 0.1
done
[[ "${mode:-}" == "ChatInput" ]] || loud_fail "TUI did not enter attended chat input"

send_turn() {
  local text="$1" marker="$2"
  tmux send-keys -l -t "$session" "$text"
  tmux send-keys -t "$session" Enter
  for _ in $(seq 1 160); do
    grep -RFqs -- "$marker" "$G/chat" 2>/dev/null && return 0
    sleep 0.1
  done
  loud_fail "chat turn '$text' did not produce '$marker'"
}

before_known=$(sha256sum known-repository-file.txt | cut -d' ' -f1)
before_guard=$(sha256sum read-only-guard.txt | cut -d' ' -f1)
before_tasks=$(grep -c '"kind":"task"' "$G/graph.jsonl" 2>/dev/null || true)
send_turn 'READ_KNOWN_FILE_937: Read known-repository-file.txt and answer with its actual content. Do not edit anything.' READ_ANSWER_937
send_turn 'DISCUSS_ONLY_937: Discuss two possible naming approaches. Do not change files or create tasks.' DISCUSSION_ANSWER_937
[[ "$(sha256sum known-repository-file.txt | cut -d' ' -f1)" == "$before_known" ]] \
  || loud_fail "read-only request mutated the known file"
[[ "$(sha256sum read-only-guard.txt | cut -d' ' -f1)" == "$before_guard" ]] \
  || loud_fail "read-only/discussion request caused unsolicited mutation"
after_discussion_tasks=$(grep -c '"kind":"task"' "$G/graph.jsonl" 2>/dev/null || true)
[[ "$after_discussion_tasks" -eq "$before_tasks" ]] \
  || loud_fail "read-only/discussion request created unsolicited WG tasks"

send_turn 'HARMLESS_EDIT_AND_CHECK_937: Create harmless-user-directed.txt with USER_DIRECTED_EDIT_937 and run a focused grep check.' EDIT_AND_CHECK_PASS_937
grep -qx 'USER_DIRECTED_EDIT_937' harmless-user-directed.txt \
  || loud_fail "explicit harmless edit was not performed"
send_turn 'DELEGATE_WG_TASK_937: Create and publish a WG task named Attended delegated task.' DELEGATION_PASS_937
wg --dir "$G" show attended-delegated-937 >delegated.show 2>&1 \
  || loud_fail "delegated task was not created"
grep -q 'Status: open' delegated.show \
  || loud_fail "delegated task was not published: $(cat delegated.show)"

printf 'DO_NOT_DELETE_937\n' >irreversible-target.txt
send_turn 'AMBIGUOUS_DELETION_DISCUSSION_937: I am thinking about permanently deleting irreversible-target.txt; what would that do?' CLARIFICATION_REQUIRED_937
[[ -f irreversible-target.txt ]] || loud_fail "discussion was mistaken for an explicit deletion request"

python3 - "$HOME" "$G" "$project" <<'PY' || loud_fail "attended authority evidence failed"
import json, pathlib, sys
home=pathlib.Path(sys.argv[1]); graph=pathlib.Path(sys.argv[2]); project=pathlib.Path(sys.argv[3])
prompt=(home/'attended-system-prompt').read_text()
assert "human's attended repository assistant" in prompt
assert "You do NOT read source files" not in prompt
argv=(home/'claude-attended-argv').read_text().splitlines()
assert '--dangerously-skip-permissions' in argv, argv
assert '--allowedTools' not in argv, argv
events=(home/'tool-events.log').read_text()
known=str(project/'known-repository-file.txt')
assert f'action=Read path={known} result=KNOWN_REPOSITORY_VALUE_937' in events, events
assert f'action=Edit path={project/"harmless-user-directed.txt"} result=written' in events, events
assert 'action=Bash command=grep-check result=pass' in events, events
assert 'action=WG command=add+publish task=attended-delegated-937' in events, events
assert 'delete' not in events.lower(), events
outboxes=list((graph/'chat').glob('*/outbox.jsonl'))
assert len(outboxes)==1, outboxes
rows=[json.loads(x) for x in outboxes[0].read_text().splitlines() if x.strip()]
read=next(x for x in rows if 'READ_ANSWER_937' in x.get('content',''))
full=read.get('full_response','')
assert 'Read' in full and known in full and 'KNOWN_REPOSITORY_VALUE_937' in full, full
# Chat metadata explicitly carries the attended full-context/full-tool posture.
tasks=[json.loads(x) for x in (graph/'graph.jsonl').read_text().splitlines() if x.strip()]
chat=next(x for x in tasks if x.get('id')=='.chat-0')
assert chat.get('exec_mode')=='full', chat
assert chat.get('context_scope')=='full', chat
assert pathlib.Path(chat.get('working_dir')).resolve()==project.resolve(), chat
PY

echo "PASS: attended chat read a real repository file, made only the explicitly requested harmless edit/check, delegated through WG, made no discussion-time write, and clarified ambiguous destructive discussion"
