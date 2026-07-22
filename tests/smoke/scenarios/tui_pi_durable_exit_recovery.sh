#!/usr/bin/env bash
# Real human-flow regression for fix-durable-pi-chat-exit.
#
# A fake interactive Pi receives one submitted turn, then its exact INNER PID
# is SIGKILLed.  The live TUI must show the inner reason (not `tmux attach`), a
# restarted TUI must reconstruct the same panel without spawning, and one
# explicit R must reopen the exact UUID/session/route with zero replay bytes.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v tmux >/dev/null 2>&1 \
    || loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive wg tui"
command -v python3 >/dev/null 2>&1 \
    || loud_skip "MISSING PYTHON" "python3 is required for deterministic ledger assertions"

WG_BIN="$(command -v wg)"
scratch=$(make_scratch)
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
mkdir -p "$HOME/.wg" "$XDG_CONFIG_HOME"
G="$scratch/.wg"
fakebin="$scratch/fakebin"
mkdir -p "$fakebin"
stdin_log="$scratch/pi-stdin.log"
launch_log="$scratch/pi-launches.jsonl"

cat >"$fakebin/pi" <<'PY'
#!/usr/bin/env python3
import json, os, pathlib, sys
args=sys.argv[1:]
base=pathlib.Path(__file__).resolve().parent.parent
stdin_log=base/"pi-stdin.log"
launch_log=base/"pi-launches.jsonl"
def value(flag):
    try: return args[args.index(flag)+1]
    except (ValueError, IndexError): return ""
sid=value("--session-id")
sdir=pathlib.Path(value("--session-dir"))
sdir.mkdir(parents=True, exist_ok=True)
transcript=sdir/("fake_"+sid+".jsonl")
if not transcript.exists():
    transcript.write_text(json.dumps({"type":"session","id":sid})+"\n")
with open(launch_log,"a") as f:
    f.write(json.dumps({
        "argv":args,"sid":sid,"sdir":str(sdir),"pid":os.getpid(),
        "writeback":{
            "graph":os.environ.get("WG_DIR"),
            "task":os.environ.get("WG_CHAT_ID"),
            "ref":os.environ.get("WG_CHAT_REF"),
            "executor":os.environ.get("WG_EXECUTOR_TYPE"),
            "model":os.environ.get("WG_MODEL"),
            "reasoning":os.environ.get("WG_REASONING"),
        }
    })+"\n")
print("FAKE_PI_IDLE pid=%d sid=%s"%(os.getpid(),sid), flush=True)
for line in sys.stdin:
    text=line.rstrip("\r\n")
    with open(stdin_log,"a") as f: f.write(text+"\n")
    with open(transcript,"a") as f:
        f.write(json.dumps({"type":"message","role":"user","text":text})+"\n")
        f.write(json.dumps({"type":"message","role":"assistant","text":"ACK "+text})+"\n")
    print("FAKE_PI_ACK "+text, flush=True)
PY
chmod +x "$fakebin/pi"
export PATH="$fakebin:$PATH"
export FAKE_PI_STDIN_LOG="$stdin_log"
export FAKE_PI_LAUNCH_LOG="$launch_log"

if ! wg profile init-starters >"$scratch/profile-init.log" 2>&1; then
    loud_fail "profile init failed: $(tail -20 "$scratch/profile-init.log")"
fi
if ! wg --dir "$G" init >"$scratch/init.log" 2>&1; then
    loud_fail "wg init failed: $(tail -20 "$scratch/init.log")"
fi
if ! wg --dir "$G" profile use pi --no-reload >"$scratch/profile.log" 2>&1; then
    loud_fail "pi profile failed: $(tail -20 "$scratch/profile.log")"
fi
cp "$HOME/.wg/config.toml" "$G/config.toml"
route="pi:openai-codex:gpt-5.6-sol"
if ! wg --dir "$G" chat new --name durable --executor pi --model "$route" >"$scratch/create.log" 2>&1; then
    loud_fail "Pi chat create failed: $(cat "$scratch/create.log")"
fi
wg --dir "$G" edit .chat-0 --reasoning xhigh >"$scratch/reasoning.log" 2>&1 \
    || loud_fail "could not pin reasoning: $(cat "$scratch/reasoning.log")"

outer="wgsmoke-durable-pi-outer-$$"
inner_session="$(wg --dir "$G" --json chat show .chat-0 | python3 -c 'import json,sys; print(json.load(sys.stdin)["tmux"]["session"])')"
TM() { tmux "$@"; }
cleanup_tmux() {
    tmux kill-session -t "$outer" 2>/dev/null || true
    tmux kill-session -t "$inner_session" 2>/dev/null || true
}
add_cleanup_hook cleanup_tmux
capture() { TM capture-pane -p -t "$outer" 2>/dev/null || true; }
wait_screen() {
    local needle="$1" limit="${2:-100}"
    for _ in $(seq 1 "$limit"); do
        capture | grep -qF "$needle" && return 0
        sleep 0.1
    done
    return 1
}

cd "$scratch"
TM new-session -d -s "$outer" -x 180 -y 50 \
    "env FAKE_PI_STDIN_LOG='$stdin_log' FAKE_PI_LAUNCH_LOG='$launch_log' PATH='$PATH' HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' '$WG_BIN' --dir '$G' tui"
wait_screen "FAKE_PI_IDLE" 150 || loud_fail "initial fake Pi did not become interactive: $(capture)"

ledger="$(find "$G/chat" -mindepth 2 -maxdepth 2 -name runtime.jsonl -print -quit)"
[[ -n "$ledger" && -f "$ledger" ]] || loud_fail "canonical UUID runtime ledger was not created"
chat_dir="$(dirname "$ledger")"
uuid="$(basename "$chat_dir")"
[[ "$uuid" =~ ^[0-9a-f]{8}-[0-9a-f-]{27}$ ]] || loud_fail "runtime ledger is not under UUID dir: $chat_dir"

prompt="ONE_DURABLE_TURN_$$"
TM send-keys -t "$outer" -l "$prompt"
TM send-keys -t "$outer" Enter
wait_screen "FAKE_PI_ACK $prompt" 100 || loud_fail "submitted turn did not reach fake Pi: $(capture)"

python3 - "$ledger" "$G" "$uuid" "$route" <<'PY' || loud_fail "initial runtime identity is incomplete"
import json, pathlib, sys
ledger,g,uuid,route=sys.argv[1:]
ev=[json.loads(x) for x in open(ledger) if x.strip()]
s=[x for x in ev if x["kind"]=="start"]
assert len(s)==1, s
x=s[0]; i=x["identity"]
assert i["graph_path"]==str(pathlib.Path(g).resolve()/"graph.jsonl"), i
assert i["task_id"]==".chat-0" and i["chat_ref"]=="chat-0" and i["uuid"]==uuid, i
assert i["executor"]=="pi" and i["route"]==route and i["reasoning"]=="xhigh", i
assert i["session_dir"]==str(pathlib.Path(ledger).parent/"pi-sessions"), i
assert "--session-id" in x["argv"] and "chat-0" in x["argv"], x
assert "--session-dir" in x["argv"] and "--thinking" in x["argv"] and "xhigh" in x["argv"], x
assert x.get("wrapper_pid") and x.get("inner_pid"), x
PY

inner_pid="$(python3 - "$ledger" <<'PY'
import json,sys
print([json.loads(x) for x in open(sys.argv[1]) if x.strip() and json.loads(x).get("kind")=="start"][-1]["inner_pid"])
PY
)"
kill -KILL "$inner_pid" || loud_fail "could not SIGKILL recorded inner pid $inner_pid"

reason="inner pi terminated by SIGKILL (9)"
wait_screen "$reason" 150 || loud_fail "TUI did not surface durable INNER reason '$reason': $(capture)"
show_before="$(wg --dir "$G" chat show .chat-0 2>&1)"
grep -qF "$reason" <<<"$show_before" || loud_fail "wg chat show disagrees with TUI reason: $show_before"

start_count_before="$(python3 - "$ledger" <<'PY'
import json,sys
print(sum(json.loads(x).get("kind")=="start" for x in open(sys.argv[1]) if x.strip()))
PY
)"
[[ "$start_count_before" == 1 ]] || loud_fail "unexpected pre-recovery starts: $start_count_before"

# Kill only the outer TUI. The inner session is already gone. Reopening the
# TUI must show the same durable panel and MUST NOT auto-create Pi.
TM kill-session -t "$outer"
TM new-session -d -s "$outer" -x 180 -y 50 \
    "env FAKE_PI_STDIN_LOG='$stdin_log' FAKE_PI_LAUNCH_LOG='$launch_log' PATH='$PATH' HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' '$WG_BIN' --dir '$G' tui"
wait_screen "$reason" 150 || loud_fail "TUI restart lost durable reason: $(capture)"
sleep 0.5
[[ "$(wc -l <"$launch_log")" == 1 ]] || loud_fail "TUI restart implicitly relaunched Pi before R: $(cat "$launch_log")"

# Explicit human recovery. R itself must send zero stdin; fake Pi opens idle on
# the same named branch and sees no duplicate turn.
TM send-keys -t "$outer" R
wait_screen "FAKE_PI_IDLE" 150 || loud_fail "explicit R did not reopen exact Pi branch: $(capture)"
sleep 0.5
[[ "$(wc -l <"$stdin_log")" == 1 ]] || loud_fail "recovery replayed/sent stdin: $(cat "$stdin_log")"

python3 - "$ledger" "$launch_log" "$stdin_log" "$prompt" <<'PY' || loud_fail "recovery identity/history was not byte-stable"
import json, pathlib, sys
ledger,launch,stdin_log,prompt=sys.argv[1:]
ev=[json.loads(x) for x in open(ledger) if x.strip()]
starts=[x for x in ev if x["kind"]=="start"]
assert len(starts)==2, starts
# Ignore only process/time fields; exact identity and sanitized argv are stable.
assert starts[0]["identity"]==starts[1]["identity"], starts
assert starts[0]["argv"]==starts[1]["argv"], starts
assert starts[0]["inner_pid"]!=starts[1]["inner_pid"], starts
restarts=[x for x in ev if x["kind"]=="decision" and x.get("decision")=="explicit-restart"]
assert len(restarts)==1 and restarts[0]["attempt"]==1, restarts
launches=[json.loads(x) for x in open(launch)]
assert len(launches)==2 and launches[0]["sid"]==launches[1]["sid"]=="chat-0", launches
assert launches[0]["sdir"]==launches[1]["sdir"], launches
assert launches[0]["writeback"]==launches[1]["writeback"], launches
assert launches[0]["writeback"]=={
    "graph":str(pathlib.Path(starts[0]["identity"]["graph_path"]).parent),
    "task":".chat-0","ref":"chat-0","executor":"pi",
    "model":starts[0]["identity"]["route"],"reasoning":"xhigh",
}, launches
lines=pathlib.Path(stdin_log).read_text().splitlines()
assert lines==[prompt], lines
transcripts=list(pathlib.Path(launches[0]["sdir"]).glob("*_chat-0.jsonl"))
assert len(transcripts)==1, transcripts
entries=[json.loads(x) for x in open(transcripts[0])]
users=[x for x in entries if x.get("role")=="user"]
assistants=[x for x in entries if x.get("role")=="assistant"]
assert users==[{"type":"message","role":"user","text":prompt}], users
assert assistants==[{"type":"message","role":"assistant","text":"ACK "+prompt}], assistants
PY

# Terminal authority wins over fresh/stale runtime metadata. Archiving kills
# the exact tmux session; a later TUI cannot relaunch it.
TM kill-session -t "$outer"
wg --dir "$G" chat archive .chat-0 >"$scratch/archive.log" 2>&1 \
    || loud_fail "archive failed: $(cat "$scratch/archive.log")"
TM new-session -d -s "$outer" -x 180 -y 50 \
    "env FAKE_PI_STDIN_LOG='$stdin_log' FAKE_PI_LAUNCH_LOG='$launch_log' PATH='$PATH' HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' '$WG_BIN' --dir '$G' tui"
sleep 1.5
[[ "$(wc -l <"$launch_log")" == 2 ]] || loud_fail "archived chat relaunched from runtime metadata: $(cat "$launch_log")"
if TM has-session -t "$inner_session" 2>/dev/null; then
    loud_fail "archived chat retained/recreated its exact inner tmux session $inner_session"
fi

echo "PASS: durable Pi inner SIGKILL reason survived TUI restart; one exact explicit recovery sent zero stdin and duplicated no turn; archived chat stayed terminal"
