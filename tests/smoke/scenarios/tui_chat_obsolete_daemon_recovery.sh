#!/usr/bin/env bash
# Candidate-binary real TUI recovery from an authenticated obsolete project daemon.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "real TUI flow requires tmux"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "identity assertions require python3"

CANDIDATE="${WG_SMOKE_CANDIDATE_BIN:-${WG_BIN:-$(command -v wg)}}"
CANDIDATE="$(readlink -f "$CANDIDATE")"
export WG_SMOKE_ROOT="/tmp/wgsmoke-obsolete-chat"
scratch=$(make_scratch)
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export WG_GLOBAL_DIR="$HOME/.wg"
export TMUX_TMPDIR=/tmp
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$scratch/bin"
ln -s "$CANDIDATE" "$scratch/bin/wg"
export PATH="$scratch/bin:/usr/bin:/bin"
G="$scratch/project/.wg"
SIBLING="$scratch/sibling/.wg"
PI_LOG="$scratch/pi.log"
export PI_LOG
mkdir -p "$scratch/project" "$scratch/sibling"

cat >"$scratch/bin/pi" <<'SH'
#!/usr/bin/env bash
set -u
printf 'chat=%s argv=' "${WG_CHAT_ID:-missing}" >>"$PI_LOG"
printf ' <%s>' "$@" >>"$PI_LOG"
printf '\n' >>"$PI_LOG"
echo "OBSOLETE_RECOVERY_PI_READY:${WG_CHAT_ID:-missing}"
while IFS= read -r line; do echo "ECHO:$line"; done
SH
chmod +x "$scratch/bin/pi"

clean=(env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" PATH="$PATH")
"${clean[@]}" "$CANDIDATE" --dir "$G" init --no-agency >/dev/null || loud_fail "project init failed"
"${clean[@]}" "$CANDIDATE" --dir "$SIBLING" init --no-agency >/dev/null || loud_fail "sibling init failed"
"${clean[@]}" "$CANDIDATE" --dir "$G" config --local \
    --model pi:openai-codex:gpt-5.6-sol --reasoning high \
    --set-model evaluator pi:openai-codex:gpt-5.6-luna \
    --set-reasoning evaluator low --max-coordinators 4 --no-reload >/dev/null \
    || loud_fail "project route setup failed"

# Preserve a pre-existing archived chat, route generation, and history across
# the controlled daemon replacement. The new attended chat must allocate .chat-1.
"${clean[@]}" "$CANDIDATE" --dir "$G" chat create --json --exec pi >/dev/null \
    || loud_fail "existing chat seed failed"
python3 - "$G/graph.jsonl" <<'PY'
import json,sys
p=sys.argv[1]
rows=[]
for line in open(p):
    row=json.loads(line)
    if row.get('id')=='.chat-0':
        row['status']='done'
        tags=row.setdefault('tags',[])
        if 'archived' not in tags: tags.append('archived')
    rows.append(row)
with open(p,'w') as f:
    for row in rows: f.write(json.dumps(row,separators=(',',':'))+'\n')
PY
mkdir -p "$G/chat/chat-0"
printf '%s\n' '{"id":1,"timestamp":"2026-08-01T00:00:00Z","role":"user","content":"preserve-me","request_id":"existing-history"}' >"$G/chat/chat-0/inbox.jsonl"
history_before=$(sha256sum "$G/chat/chat-0/inbox.jsonl" | awk '{print $1}')
sibling_before=$(sha256sum "$SIBLING/graph.jsonl" | awk '{print $1}')

# Appending bytes leaves a runnable ELF but gives it a distinct authenticated
# content build. Unlink chat.sock after bind to model the old compatibility
# daemon that exposes only the general IPC lane.
OLD="$scratch/wg-obsolete-compat"
cp "$CANDIDATE" "$OLD"
printf '\nOBSOLETE-COMPAT-NO-CHAT-SOCKET\n' >>"$OLD"
chmod +x "$OLD"
"${clean[@]}" "$OLD" --dir "$G" service start --max-agents 0 --no-chat-agent >/dev/null \
    || loud_fail "obsolete compat daemon failed to start"
old_pid=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$G/service/state.json")
old_build=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["identity"]["build_id"])' "$G/service/state.json")
# Snapshot existing durable chat identity immediately before the human action;
# daemon runtime counters are intentionally excluded from the route signature.
existing_before=$(python3 - "$G/graph.jsonl" <<'PY'
import json,sys
for line in open(sys.argv[1]):
 x=json.loads(line)
 if x.get('id')=='.chat-0': print(json.dumps(x,sort_keys=True,separators=(',',':')))
PY
)
route_before=$(python3 - "$G/service/coordinator-state-0.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1]))
print(json.dumps({k:x.get(k) for k in ('executor_override','model_override','endpoint_override','route_generation')},sort_keys=True))
PY
)
rm -f "$G/service/chat.sock"

TM_SOCK="wgsmoke-obsolete-chat-$$"
TM() { tmux -L "$TM_SOCK" "$@"; }
cleanup_tmux() { tmux -L "$TM_SOCK" kill-server 2>/dev/null || true; }
cleanup_service() { "${clean[@]}" "$CANDIDATE" --dir "$G" service stop --force >/dev/null 2>&1 || true; }
add_cleanup_hook cleanup_tmux
add_cleanup_hook cleanup_service
outer=obsolete-chat-tui
TM new-session -d -s "$outer" -x 180 -y 50 \
    "cd '$scratch/project' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WG_DIR='$SIBLING' PATH='$PATH' PI_LOG='$PI_LOG' TERM=xterm-256color '$CANDIDATE' --dir '$G' tui --no-mouse"
capture() { TM capture-pane -p -t "$outer" 2>/dev/null || true; }
wait_for() {
    local pattern="$1" tries="${2:-240}"
    for _ in $(seq 1 "$tries"); do capture | grep -qE "$pattern" && return 0; sleep 0.05; done
    return 1
}

wait_for 'New chat|No chat selected' || loud_fail "TUI did not reach attended surface: $(capture)"
TM send-keys -t "$outer" n
wait_for 'Pi \(choose model in Pi\)' || loud_fail "New-chat launcher did not open: $(capture)"
TM send-keys -t "$outer" Enter
for _ in $(seq 1 300); do
    grep -q '^chat=.chat-1 ' "$PI_LOG" 2>/dev/null && break
    sleep 0.05
done
grep -q '^chat=.chat-1 ' "$PI_LOG" 2>/dev/null \
    || loud_fail "TUI did not recover/create .chat-1: screen=$(capture) pi=$(cat "$PI_LOG" 2>/dev/null) daemon=$(tail -80 "$G/service/daemon.log" 2>/dev/null)"
wait_for 'OBSOLETE_RECOVERY_PI_READY:.chat-1' \
    || loud_fail "recovered chat was not visibly attached: $(capture)"

python3 - "$G/graph.jsonl" "$G/service/state.json" "$CANDIDATE" "$old_pid" "$old_build" <<'PY'
import hashlib,json,os,sys
graph,state_path,candidate,old_pid,old_build=sys.argv[1:]
rows=[json.loads(x) for x in open(graph)]
chats={x['id']:x for x in rows if x.get('id','').startswith('.chat-')}
assert set(chats)=={'.chat-0','.chat-1'},chats
new=chats['.chat-1']
assert new.get('model') in (None,''),new
assert new.get('command_argv')==['pi'],new
actor=new.get('log',[{}])[0].get('actor','')
assert actor.startswith('chat-create-request:chat-create-'),new
state=json.load(open(state_path))
assert str(state['pid']) != old_pid,state
assert state['identity']['build_id'] != old_build,state
expected='sha256:'+hashlib.sha256(open(candidate,'rb').read()).hexdigest()
assert state['identity']['executable_sha256']==expected,(state['identity'],expected)
PY

existing_after=$(python3 - "$G/graph.jsonl" <<'PY'
import json,sys
for line in open(sys.argv[1]):
 x=json.loads(line)
 if x.get('id')=='.chat-0': print(json.dumps(x,sort_keys=True,separators=(',',':')))
PY
)
[[ "$existing_after" == "$existing_before" ]] || loud_fail "existing chat row changed across recovery"
route_after=$(python3 - "$G/service/coordinator-state-0.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1]))
print(json.dumps({k:x.get(k) for k in ('executor_override','model_override','endpoint_override','route_generation')},sort_keys=True))
PY
)
[[ "$route_after" == "$route_before" ]] \
    || loud_fail "existing chat route generation changed: before=$route_before after=$route_after"
[[ "$(sha256sum "$G/chat/chat-0/inbox.jsonl" | awk '{print $1}')" == "$history_before" ]] \
    || loud_fail "existing chat history changed"
[[ "$(sha256sum "$SIBLING/graph.jsonl" | awk '{print $1}')" == "$sibling_before" ]] \
    || loud_fail "inherited sibling WG_DIR was mutated"
[[ ! -e "$SIBLING/service/state.json" ]] || loud_fail "restart was redirected to inherited sibling WG_DIR"
live_pi=$(pgrep -fc "^bash $scratch/bin/pi " || true)
[[ $live_pi -eq 1 ]] || loud_fail "chat has $live_pi concurrent Pi children: $(pgrep -af "$scratch/bin/pi" || true) log=$(cat "$PI_LOG")"
if capture | grep -q 'chat creation did not produce a reconcilable graph commit'; then
    loud_fail "TUI still showed the obsolete generic reconciliation error: $(capture)"
fi

# Lost before commit: both bounded same-ID attempts are deliberately dropped.
# The client must emit the exact project action and leave zero chat residue.
PRE="$scratch/precommit/.wg"
mkdir -p "$scratch/precommit"
"${clean[@]}" "$CANDIDATE" --dir "$PRE" init --no-agency >/dev/null
"${clean[@]}" "$CANDIDATE" --dir "$PRE" config --local \
    --model pi:openai-codex:gpt-5.6-sol --reasoning high \
    --set-model evaluator pi:openai-codex:gpt-5.6-luna \
    --set-reasoning evaluator low --no-reload >/dev/null
pre_env=(env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" PATH="$PATH" WG_TEST_CHAT_CREATE_DROP_BEFORE_COMMIT=1)
"${pre_env[@]}" "$CANDIDATE" --dir "$PRE" service start --max-agents 0 --no-chat-agent >/dev/null \
    || loud_fail "precommit daemon failed to start"
set +e
pre_out=$(env -u WG_TASK_ID -u WG_AGENT_ID WG_DIR="$SIBLING" HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" PATH="$PATH" \
    "$CANDIDATE" --dir "$PRE" chat create --json --exec pi 2>&1)
pre_rc=$?
set -e
[[ $pre_rc -ne 0 ]] || loud_fail "precommit loss incorrectly claimed success: $pre_out"
grep -Fq "wg --dir '$PRE' service restart" <<<"$pre_out" \
    || loud_fail "precommit loss omitted exact recovery action: $pre_out"
! grep -q 'chat creation did not produce a reconcilable graph commit' <<<"$pre_out" \
    || loud_fail "precommit loss used obsolete generic error: $pre_out"
python3 - "$PRE/graph.jsonl" <<'PY'
import json,sys
chats=[json.loads(x) for x in open(sys.argv[1]) if json.loads(x).get('id','').startswith('.chat-')]
assert chats==[],chats
PY
"${clean[@]}" "$CANDIDATE" --dir "$PRE" service stop >/dev/null \
    || loud_fail "precommit daemon stop failed"

# Lost after commit: a deliberately late response must reconcile the exact
# durable receipt, not allocate a second row or report uncertainty.
POST="$scratch/postcommit/.wg"
mkdir -p "$scratch/postcommit"
"${clean[@]}" "$CANDIDATE" --dir "$POST" init --no-agency >/dev/null
"${clean[@]}" "$CANDIDATE" --dir "$POST" config --local \
    --model pi:openai-codex:gpt-5.6-sol --reasoning high \
    --set-model evaluator pi:openai-codex:gpt-5.6-luna \
    --set-reasoning evaluator low --no-reload >/dev/null
post_env=(env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" PATH="$PATH" WG_TEST_CHAT_CREATE_RESPONSE_DELAY_MS=2600)
"${post_env[@]}" "$CANDIDATE" --dir "$POST" service start --max-agents 0 --no-chat-agent >/dev/null \
    || loud_fail "postcommit daemon failed to start"
post_out=$(env -u WG_TASK_ID -u WG_AGENT_ID WG_DIR="$SIBLING" HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" PATH="$PATH" \
    "$CANDIDATE" --dir "$POST" chat create --json --exec pi 2>"$scratch/post.err") \
    || loud_fail "postcommit response loss did not reconcile: $(cat "$scratch/post.err")"
python3 - "$POST/graph.jsonl" "$post_out" <<'PY'
import json,sys
chats=[json.loads(x) for x in open(sys.argv[1]) if json.loads(x).get('id','').startswith('.chat-')]
out=json.loads(sys.argv[2])
assert len(chats)==1,chats
assert out['chat_id']==0 and out['durable_receipt'] is True and out['reconciled'] is True,out
rid=out['request_id']
assert chats[0]['log'][0]['actor']=='chat-create-request:'+rid,(chats,out)
PY
"${clean[@]}" "$CANDIDATE" --dir "$POST" service stop >/dev/null \
    || loud_fail "postcommit daemon stop failed"

echo "PASS: TUI obsolete-daemon recovery plus pre/post-commit response loss kept exact graph/service/request identity, zero duplicates, and preserved sibling/history/route state"
