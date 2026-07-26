#!/usr/bin/env bash
# Explicit native Claude live-chat regression.
#
# Drives the actual human surfaces with a credential-free fake Claude CLI:
#   1. a real tmux-hosted `wg tui` native composer submits turn one;
#   2. the daemon is restarted and terminal `wg chat send` submits turn two;
#   3. the TUI submits a failing third turn.
# The daemon-supervised claude-handler must preserve the exact handler-first
# route, persist Claude's native session id, use `claude --resume <id>` after
# restart, project stream/tool/usage events into the TUI, keep native failure
# text visible, and never invoke Pi or Codex.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

command -v python3 >/dev/null 2>&1 \
  || loud_skip "MISSING PYTHON" "python3 is required for JSON assertions"
command -v tmux >/dev/null 2>&1 \
  || loud_skip "MISSING TMUX" "tmux is required for the live TUI flow"

scratch=$(make_scratch)
home="$scratch/home"
bin="$scratch/bin"
project="$scratch/project"
mkdir -p "$home/.config/workgraph" "$bin" "$project"
: >"$home/.config/workgraph/config.toml"

cat >"$bin/claude" <<'SH'
#!/usr/bin/env bash
set -u
if [[ "${1:-}" == "--version" ]]; then
  echo '2.1.0 (fake Claude)'
  exit 0
fi
counter="$HOME/claude-call-count"
count=0
[[ -f "$counter" ]] && count=$(cat "$counter")
count=$((count + 1))
printf '%s\n' "$count" >"$counter"
printf '%s\n' "$@" >"$HOME/claude-args-$count"

# The WG chat adapter is the only allowed launch edge. A direct interactive
# Claude PTY, a worker-shaped one-shot, or a provider rewrite is a regression.
argv=" $* "
if [[ "$argv" != *" --print "* \
   || "$argv" != *" --input-format stream-json "* \
   || "$argv" != *" --output-format stream-json "* ]]; then
  printf 'DIRECT_CLAUDE_TUI_REGRESSION: %s\n' "$*" >&2
  exit 96
fi

printf '%s\n' '{"type":"system","subtype":"init","session_id":"native-claude-chat-session-912"}'
while IFS= read -r line; do
  printf '%s\n' "$line" >>"$HOME/claude-stdin-$count"
  case "$line" in
    *TUI_FIRST_NATIVE_CLAUDE_TURN*)
      printf '%s\n' '{"type":"assistant","session_id":"native-claude-chat-session-912","message":{"role":"assistant","content":[{"type":"text","text":"CLAUDE_STREAM_VISIBLE CLAUDE_FIRST_VISIBLE"},{"type":"tool_use","name":"Bash","input":{"command":"wg status"}}],"usage":{"input_tokens":13,"output_tokens":5,"cache_read_input_tokens":7,"cache_creation_input_tokens":2},"stop_reason":null}}'
      sleep 1
      printf '%s\n' '{"type":"user","session_id":"native-claude-chat-session-912","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":"CLAUDE_TOOL_OUTPUT_VISIBLE"}]}}'
      printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"session_id":"native-claude-chat-session-912","result":"CLAUDE_STREAM_VISIBLE CLAUDE_FIRST_VISIBLE","total_cost_usd":0.0123,"usage":{"input_tokens":13,"output_tokens":5,"cache_read_input_tokens":7,"cache_creation_input_tokens":2}}'
      ;;
    *TERMINAL_SECOND_NATIVE_CLAUDE_TURN*)
      printf '%s\n' '{"type":"assistant","session_id":"native-claude-chat-session-912","message":{"role":"assistant","content":[{"type":"text","text":"CLAUDE_RESUME_VISIBLE"}],"usage":{"input_tokens":17,"output_tokens":4,"cache_read_input_tokens":11,"cache_creation_input_tokens":0},"stop_reason":null}}'
      printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"session_id":"native-claude-chat-session-912","result":"CLAUDE_RESUME_VISIBLE","total_cost_usd":0.0234,"usage":{"input_tokens":17,"output_tokens":4,"cache_read_input_tokens":11,"cache_creation_input_tokens":0}}'
      ;;
    *TUI_FAIL_NATIVE_CLAUDE_TURN*)
      printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"native-claude-chat-session-912","result":"FAIL42_VISIBLE_NATIVE_CLAUDE","total_cost_usd":0.0001,"usage":{"input_tokens":3,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}'
      echo 'FAIL42_VISIBLE_NATIVE_CLAUDE' >&2
      exit 42
      ;;
    *)
      echo "unexpected fake Claude prompt: $line" >&2
      exit 43
      ;;
  esac
done
SH
chmod +x "$bin/claude"

cat >"$bin/pi" <<'SH'
#!/usr/bin/env bash
printf 'PI_CROSSOVER %s\n' "$*" >>"$HOME/pi-crossover.log"
exit 91
SH
chmod +x "$bin/pi"

cat >"$bin/codex" <<'SH'
#!/usr/bin/env bash
printf 'CODEX_CROSSOVER %s\n' "$*" >>"$HOME/codex-crossover.log"
exit 92
SH
chmod +x "$bin/codex"

export HOME="$home"
export XDG_CONFIG_HOME="$home/.config"
export PATH="$bin:$PATH"
unset WG_EXECUTOR_TYPE WG_MODEL WG_REASONING WG_TIER WG_AGENT_ID

cd "$project"
wg init --no-agency >init.log 2>&1 \
  || loud_fail "graph-only init failed: $(tail -30 init.log)"
wg profile use claude --no-reload >profile.log 2>&1 \
  || loud_fail "explicit Claude profile failed: $(cat profile.log)"
route='claude:future/opaque:native-chat-v12'
wg config --local --model "$route" --reasoning high --no-reload >>profile.log 2>&1 \
  || loud_fail "explicit Claude route config failed: $(cat profile.log)"

start_wg_daemon "$project" --max-agents 0 --interval 1 \
  || loud_fail "Claude chat daemon failed to start"
G="$WG_SMOKE_DAEMON_DIR"

wg --dir "$G" chat create --name native-claude --executor claude --model "$route" \
  >create.log 2>&1 || loud_fail "explicit Claude chat create failed: $(cat create.log)"

python3 - "$G/graph.jsonl" "$route" <<'PY' \
  || loud_fail "Claude chat route metadata crossed or drifted"
import json, sys
rows=[json.loads(x) for x in open(sys.argv[1]) if x.strip()]
task=next(x for x in rows if x.get("id")==".chat-0")
route=sys.argv[2]
assert task.get("executor_preset_name")=="claude", task
assert task.get("model")==route, task
assert not route.startswith("pi:") and not route.startswith("codex:"), route
PY

wait_for_handler() {
  for _ in $(seq 1 120); do
    if wg --dir "$G" chat show 0 --json 2>/dev/null \
      | python3 -c 'import json,sys; v=json.load(sys.stdin); raise SystemExit(0 if (v.get("handler") or {}).get("kind")=="adapter" else 1)' \
      2>/dev/null; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}
wait_for_handler || loud_fail "claude-handler never became live: $(tail -80 "$G/service/daemon.log" 2>/dev/null)"

session="wgsmoke-explicit-claude-chat-$$"
trace="$scratch/tui-trace.jsonl"
tmux new-session -d -s "$session" -x 220 -y 60 \
  "wg --dir '$G' tui --no-mouse --trace '$trace'"

wait_input_mode() {
  local want="$1"
  for _ in $(seq 1 80); do
    mode=$(wg --dir "$G" --json tui-dump 2>/dev/null \
      | python3 -c 'import json,sys; print(json.load(sys.stdin).get("input_mode", ""))' \
      2>/dev/null || true)
    [[ "$mode" == "$want" ]] && return 0
    sleep 0.1
  done
  return 1
}
wait_outbox_text() {
  local marker="$1"
  for _ in $(seq 1 180); do
    if grep -RFqs -- "$marker" "$G/chat" 2>/dev/null; then return 0; fi
    sleep 0.1
  done
  return 1
}
wait_screen_text() {
  local marker="$1"
  for _ in $(seq 1 120); do
    if tmux capture-pane -p -t "$session" -S -160 2>/dev/null | grep -Fq -- "$marker"; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}
wait_streaming_text() {
  local marker="$1"
  for _ in $(seq 1 80); do
    if find "$G/chat" -name .streaming -type f -exec grep -Fq -- "$marker" {} \; 2>/dev/null; then
      return 0
    fi
    sleep 0.05
  done
  return 1
}

for _ in $(seq 1 80); do
  if wg --dir "$G" --json tui-dump >/dev/null 2>&1; then break; fi
  sleep 0.1
done
# Supervised Claude uses WG's native composer; it must not swallow `c` inside
# a direct vendor PTY.
tmux send-keys -t "$session" c
wait_input_mode ChatInput \
  || loud_fail "Claude TUI did not enter native ChatInput: $(tmux capture-pane -p -t "$session" -S -80)"
tmux send-keys -l -t "$session" 'TUI_FIRST_NATIVE_CLAUDE_TURN'
tmux send-keys -t "$session" Enter

wait_streaming_text CLAUDE_STREAM_VISIBLE \
  || loud_fail "Claude partial stream was not projected into canonical chat storage"
wait_screen_text CLAUDE_STREAM_VISIBLE \
  || loud_fail "Claude partial stream was not visible in the TUI"
wait_outbox_text CLAUDE_FIRST_VISIBLE \
  || loud_fail "first Claude reply missing"
wait_screen_text CLAUDE_FIRST_VISIBLE \
  || loud_fail "first Claude reply reached outbox but not the TUI"
wait_outbox_text CLAUDE_TOOL_OUTPUT_VISIBLE \
  || loud_fail "Claude tool result was not preserved in the full response"
wait_screen_text CLAUDE_TOOL_OUTPUT_VISIBLE \
  || loud_fail "Claude tool result was not rendered in the TUI"
wait_outbox_text '[usage: 13 input' \
  || loud_fail "Claude usage was not preserved in the full response"
wait_screen_text '[usage: 13 input' \
  || loud_fail "Claude usage was not rendered in the TUI"

# Restart the real supervisor between turns. The replacement handler must use
# Claude's persisted native session id, not history replay through another
# execution system and not a fresh Claude conversation.
wg --dir "$G" service stop --force >/dev/null 2>&1 \
  || loud_fail "could not stop Claude supervisor between turns"
start_wg_daemon "$project" --max-agents 0 --interval 1 \
  || loud_fail "Claude chat daemon failed to restart"
G="$WG_SMOKE_DAEMON_DIR"
wait_for_handler \
  || loud_fail "replacement claude-handler did not resume supervision: $(tail -80 "$G/service/daemon.log" 2>/dev/null)"

wg --dir "$G" chat send 0 'TERMINAL_SECOND_NATIVE_CLAUDE_TURN' >/dev/null 2>&1 \
  || loud_fail "terminal wg chat send failed"
wait_outbox_text CLAUDE_RESUME_VISIBLE || loud_fail "resumed Claude reply missing"
wait_screen_text CLAUDE_RESUME_VISIBLE \
  || loud_fail "resumed Claude reply was not visible in TUI"

# ChatInput remains active. A native Claude error result + exit 42 must be a
# durable assistant error. A replacement handler may restart, but no different
# executor is allowed to answer.
tmux send-keys -l -t "$session" 'TUI_FAIL_NATIVE_CLAUDE_TURN'
tmux send-keys -t "$session" Enter
wait_outbox_text FAIL42_VISIBLE_NATIVE_CLAUDE \
  || loud_fail "Claude exit-42 detail was swallowed: $(tail -100 "$G/service/daemon.log" 2>/dev/null)"
wait_screen_text FAIL42_VISIBLE_NATIVE_CLAUDE \
  || loud_fail "Claude failure reached outbox but was not visible in TUI"
wait_for_handler || loud_fail "Claude failure crossed away from or permanently killed its handler"

python3 - "$HOME" "$G" "$trace" "$route" <<'PY' \
  || loud_fail "fake-Claude first/resume/failure assertions failed"
import json, pathlib, sys
home=pathlib.Path(sys.argv[1]); graph=pathlib.Path(sys.argv[2]); trace=pathlib.Path(sys.argv[3]); route=sys.argv[4]
args=[]
for n in (1,2,3):
    p=home/f"claude-args-{n}"
    assert p.exists(), f"missing invocation {n}"
    args.append(p.read_text().splitlines())
model=route.split(":",1)[1]
assert "--print" in args[0] and "--resume" not in args[0], args[0]
assert "--system-prompt" in args[0], args[0]
assert "--model" in args[0] and args[0][args[0].index("--model")+1]==model, args[0]
for argv in args[1:3]:
    assert "--resume" in argv, argv
    assert argv[argv.index("--resume")+1]=="native-claude-chat-session-912", argv
    assert "--system-prompt" not in argv, argv
    assert "--model" in argv and argv[argv.index("--model")+1]==model, argv
for argv in args:
    assert "--input-format" in argv and argv[argv.index("--input-format")+1]=="stream-json", argv
    assert "--output-format" in argv and argv[argv.index("--output-format")+1]=="stream-json", argv
    assert "--dangerously-skip-permissions" in argv, argv
    assert "--provider" not in argv, argv
markers=list((graph/"chat").glob("*/.claude-session-id"))
assert len(markers)==1, markers
assert markers[0].parent.name != "chat-0", markers[0]
assert markers[0].read_text().strip()=="native-claude-chat-session-912", markers[0].read_text()
# Exact chat identity remains handler-first after live execution.
rows=[json.loads(x) for x in (graph/"graph.jsonl").read_text().splitlines() if x.strip()]
task=next(x for x in rows if x.get("id")==".chat-0")
assert task.get("executor_preset_name")=="claude", task
assert task.get("model")==route, task
# Both Enter events traversed the real native-composer human path.
events=[]
if trace.exists():
    for line in trace.read_text(errors="replace").splitlines():
        try: events.append(json.loads(line))
        except Exception: pass
enters=[e for e in events if e.get("event",{}).get("code")=="Enter" and e.get("state",{}).get("chat_input_route")=="native_composer"]
assert len(enters)>=2, "missing native_composer Enter traces"
# Stream/tool/usage all survived as one visible full response.
outboxes=list((graph/"chat").glob("*/outbox.jsonl"))
assert len(outboxes)==1, outboxes
messages=[json.loads(x) for x in outboxes[0].read_text().splitlines() if x.strip()]
first=next(x for x in messages if "CLAUDE_FIRST_VISIBLE" in x.get("content",""))
full=first.get("full_response","")
assert "┌─ Bash" in full and "$ wg status" in full and "CLAUDE_TOOL_OUTPUT_VISIBLE" in full, full
assert "[usage: 13 input · 5 output · 7 cache-read · 2 cache-write · $0.0123]" in full, full
assert any("Claude chat failed: FAIL42_VISIBLE_NATIVE_CLAUDE" in x.get("content","") for x in messages), messages
PY

[[ ! -e "$HOME/pi-crossover.log" ]] \
  || loud_fail "explicit Claude chat invoked Pi: $(cat "$HOME/pi-crossover.log")"
[[ ! -e "$HOME/codex-crossover.log" ]] \
  || loud_fail "explicit Claude chat invoked Codex: $(cat "$HOME/codex-crossover.log")"
if find "$G" -type f \( -name '*.json' -o -name '*.jsonl' -o -name '*.log' \) \
    -exec grep -l -E 'PI_CROSSOVER|CODEX_CROSSOVER' {} + 2>/dev/null | grep -q .; then
  loud_fail "cross-executor evidence appeared in Claude chat state"
fi

# The TUI must not create a direct Claude vendor tmux owner beside the adapter.
inner_tmux=$(wg --dir "$G" chat show 0 --json 2>/dev/null \
  | python3 -c 'import json,sys; print((json.load(sys.stdin).get("tmux") or {}).get("session", ""))')
if [[ -n "$inner_tmux" ]] && tmux has-session -t "$inner_tmux" 2>/dev/null; then
  loud_fail "TUI spawned direct Claude vendor tmux session $inner_tmux beside claude-handler"
fi

echo "PASS: fake native Claude chat used TUI composer -> claude-handler first turn, native --resume after daemon restart, stream/tool/usage projection, visible failure, exact identity, and zero Pi/Codex crossover"
