#!/usr/bin/env bash
# Explicit native Codex live-chat regression.
#
# Drives the real human surfaces with a credential-free fake Codex CLI:
#   1. a real tmux-hosted `wg tui` native composer submits turn one;
#   2. terminal `wg chat send` submits turn two;
#   3. the TUI submits a failing third turn.
# The daemon-supervised codex-handler must invoke `codex exec` first, persist
# its canonical UUID-backed thread id, use `codex exec resume <id>` thereafter,
# keep exit-42 text visible in the outbox/TUI, and never invoke Pi.
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

cat >"$bin/codex" <<'SH'
#!/usr/bin/env bash
set -u
counter="$HOME/codex-call-count"
count=0
[[ -f "$counter" ]] && count=$(cat "$counter")
count=$((count + 1))
printf '%s\n' "$count" >"$counter"
printf '%s\n' "$@" >"$HOME/codex-args-$count"
prompt=$(cat)
printf '%s' "$prompt" >"$HOME/codex-prompt-$count"

# A direct interactive `codex` launch is a routing regression: live WG Codex
# chat must always arrive through codex-handler's `codex exec` adapter.
if [[ "${1:-}" != "exec" ]]; then
  printf 'DIRECT_CODEX_TUI_REGRESSION: %s\n' "$*" >&2
  exit 96
fi

case "$prompt" in
  *TUI_FIRST_NATIVE_TURN*)
    printf '%s\n' '{"type":"thread.started","thread_id":"native-thread-chat-874"}'
    printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"CODEX_FIRST_VISIBLE"}}'
    printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":11,"cached_input_tokens":0,"output_tokens":3}}'
    ;;
  *TERMINAL_SECOND_NATIVE_TURN*)
    printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"CODEX_RESUME_VISIBLE"}}'
    printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":7,"cached_input_tokens":2,"output_tokens":4}}'
    ;;
  *TUI_FAIL_NATIVE_TURN*)
    echo 'FAIL42_VISIBLE_NATIVE_CODEX' >&2
    exit 42
    ;;
  *)
    echo "unexpected fake Codex prompt" >&2
    exit 43
    ;;
esac
SH
chmod +x "$bin/codex"

cat >"$bin/pi" <<'SH'
#!/usr/bin/env bash
printf 'PI_CROSSOVER %s\n' "$*" >>"$HOME/pi-crossover.log"
exit 91
SH
chmod +x "$bin/pi"

export HOME="$home"
export XDG_CONFIG_HOME="$home/.config"
export PATH="$bin:$PATH"
unset WG_EXECUTOR_TYPE WG_MODEL WG_REASONING WG_TIER WG_AGENT_ID

cd "$project"
wg init --no-agency >init.log 2>&1 \
  || loud_fail "graph-only init failed: $(tail -30 init.log)"
wg profile use codex --no-reload >profile.log 2>&1 \
  || loud_fail "explicit Codex profile failed: $(cat profile.log)"
route='codex:future/opaque:native-chat-v11'
wg config --local --model "$route" --reasoning high --no-reload >>profile.log 2>&1 \
  || loud_fail "explicit Codex route config failed: $(cat profile.log)"

start_wg_daemon "$project" --max-agents 0 --interval 1 \
  || loud_fail "Codex chat daemon failed to start"
G="$WG_SMOKE_DAEMON_DIR"

wg --dir "$G" chat create --name native-codex --executor codex --model "$route" \
  >create.log 2>&1 || loud_fail "explicit Codex chat create failed: $(cat create.log)"

# Creation must preserve one atomic route generation. A Pi profile/provider
# spelling is not equivalent to this direct Codex route.
python3 - "$G/graph.jsonl" "$G/service/coordinator-state-0.json" "$route" <<'PY' \
  || loud_fail "Codex chat route metadata crossed or drifted"
import json, sys
rows=[json.loads(x) for x in open(sys.argv[1]) if x.strip()]
task=next(x for x in rows if x.get("id")==".chat-0")
state=json.load(open(sys.argv[2]))
route=sys.argv[3]
assert task.get("executor_preset_name")=="codex", task
assert task.get("model")==route, task
assert state.get("executor_override")=="codex", state
assert state.get("model_override")==route, state
assert not route.startswith("pi:"), route
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
wait_for_handler || loud_fail "codex-handler never became live: $(tail -80 "$G/service/daemon.log" 2>/dev/null)"

session="wgsmoke-explicit-codex-chat-$$"
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

# Normal-mode c opens the native composer. Before this fix Codex owned a
# direct vendor PTY, so this exact key went to Codex instead of ChatInput.
for _ in $(seq 1 80); do
  if wg --dir "$G" --json tui-dump >/dev/null 2>&1; then break; fi
  sleep 0.1
done
tmux send-keys -t "$session" c
wait_input_mode ChatInput \
  || loud_fail "Codex TUI did not enter native ChatInput: $(tmux capture-pane -p -t "$session" -S -80)"
tmux send-keys -l -t "$session" 'TUI_FIRST_NATIVE_TURN'
tmux send-keys -t "$session" Enter

wait_outbox_text() {
  local marker="$1"
  for _ in $(seq 1 160); do
    if grep -Rqs -- "$marker" "$G/chat" 2>/dev/null; then return 0; fi
    sleep 0.1
  done
  return 1
}
wait_screen_text() {
  local marker="$1"
  for _ in $(seq 1 100); do
    if tmux capture-pane -p -t "$session" -S -120 2>/dev/null | grep -q -- "$marker"; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

wait_outbox_text CODEX_FIRST_VISIBLE \
  || loud_fail "first Codex reply missing: $(find "$G/chat" -type f -maxdepth 3 -print 2>/dev/null)"
wait_screen_text CODEX_FIRST_VISIBLE \
  || loud_fail "first reply reached outbox but was not visible in TUI: $(tmux capture-pane -p -t "$session" -S -120)"

# Restart the actual supervisor between turns. The replacement codex-handler
# must find the UUID-backed marker written by generation one and continue the
# same native thread; process lifetime is never conversation lifetime.
wg --dir "$G" service stop --force >/dev/null 2>&1 \
  || loud_fail "could not stop Codex supervisor between turns"
start_wg_daemon "$project" --max-agents 0 --interval 1 \
  || loud_fail "Codex chat daemon failed to restart"
G="$WG_SMOKE_DAEMON_DIR"
wait_for_handler \
  || loud_fail "replacement codex-handler did not resume supervision: $(tail -80 "$G/service/daemon.log" 2>/dev/null)"

# The operator's ordinary terminal flow supplies turn two. The replacement
# handler must reuse the exact thread started by turn one, not replay through
# Pi or start a new native Codex conversation.
wg --dir "$G" chat send 0 'TERMINAL_SECOND_NATIVE_TURN' >/dev/null 2>&1 \
  || loud_fail "terminal wg chat send failed"
wait_outbox_text CODEX_RESUME_VISIBLE \
  || loud_fail "resumed Codex reply missing"
wait_screen_text CODEX_RESUME_VISIBLE \
  || loud_fail "resumed reply was not visible in TUI"

# ChatInput remains active after submit. A failing explicit Codex turn must be
# rendered as a durable assistant error, while the long-lived handler stays in
# Codex and remains available for retry.
tmux send-keys -l -t "$session" 'TUI_FAIL_NATIVE_TURN'
tmux send-keys -t "$session" Enter
wait_outbox_text FAIL42_VISIBLE_NATIVE_CODEX \
  || loud_fail "Codex exit-42 detail was swallowed: $(tail -100 "$G/service/daemon.log" 2>/dev/null)"
wait_screen_text FAIL42_VISIBLE_NATIVE_CODEX \
  || loud_fail "Codex failure reached outbox but was not visible in TUI: $(tmux capture-pane -p -t "$session" -S -120)"
wait_for_handler || loud_fail "turn failure killed or crossed the Codex handler"

python3 - "$HOME" "$G" "$trace" "$route" <<'PY' \
  || loud_fail "fake-Codex first/resume/failure assertions failed"
import json, pathlib, sys
home=pathlib.Path(sys.argv[1]); graph=pathlib.Path(sys.argv[2]); trace=pathlib.Path(sys.argv[3]); route=sys.argv[4]
args=[]
for n in (1,2,3):
    p=home/f"codex-args-{n}"
    assert p.exists(), f"missing invocation {n}"
    args.append(p.read_text().splitlines())
model=route.split(":",1)[1]
assert args[0][0]=="exec" and "resume" not in args[0], args[0]
assert "--model" in args[0] and args[0][args[0].index("--model")+1]==model, args[0]
assert args[1][:3]==["exec","resume","native-thread-chat-874"], args[1]
assert args[2][:3]==["exec","resume","native-thread-chat-874"], args[2]
for argv in args:
    assert "--dangerously-bypass-approvals-and-sandbox" in argv, argv
    assert "--json" in argv, argv
    assert "--provider" not in argv, argv
assert "STOP — You Are A Chat Agent" in (home/"codex-prompt-1").read_text(), "chat contract missing first turn"
assert "STOP — You Are A Chat Agent" not in (home/"codex-prompt-2").read_text(), "first-turn contract replayed"
assert "TERMINAL_SECOND_NATIVE_TURN" in (home/"codex-prompt-2").read_text()
# The thread marker belongs to the canonical UUID directory, never literal
# chat/chat-0 storage.
markers=list((graph/"chat").glob("*/.codex-session-id"))
assert len(markers)==1, markers
assert markers[0].parent.name != "chat-0", markers[0]
assert markers[0].read_text().strip()=="native-thread-chat-874", markers[0].read_text()
# The real TUI Enter event proves this was the native composer human path.
events=[]
if trace.exists():
    for line in trace.read_text(errors="replace").splitlines():
        try: events.append(json.loads(line))
        except Exception: pass
assert any(e.get("event",{}).get("code")=="Enter" and e.get("state",{}).get("chat_input_route")=="native_composer" for e in events), "no native_composer Enter trace"
PY

[[ ! -e "$HOME/pi-crossover.log" ]] \
  || loud_fail "explicit Codex chat invoked Pi: $(cat "$HOME/pi-crossover.log")"
if find "$G" -type f \( -name '*.json' -o -name '*.jsonl' -o -name '*.log' \) \
    -exec grep -l 'PI_CROSSOVER' {} + 2>/dev/null | grep -q .; then
  loud_fail "Pi crossover evidence appeared in Codex chat state"
fi

# No direct vendor pane is allowed for this chat; TUI stays file-backed while
# the daemon's codex-handler owns the canonical adapter lock.
if tmux list-sessions -F '#{session_name}' 2>/dev/null \
    | grep -q "$(wg --dir "$G" chat show 0 --json 2>/dev/null | python3 -c 'import json,sys; print((json.load(sys.stdin).get("tmux") or {}).get("session", "__none__"))')"; then
  loud_fail "TUI spawned a direct Codex vendor tmux pane beside codex-handler"
fi

echo "PASS: fake native Codex chat used TUI composer -> codex-handler first turn, terminal resume, visible TUI failure, canonical thread persistence, and zero Pi crossover"
