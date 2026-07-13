#!/usr/bin/env bash
# Regression: a cleanly-ended idle chat supervisor must not remain a permanent
# stale entry in the daemon's coordinator_agents map.
#
# Human CLI flow (credential-free): start an isolated daemon, create a Pi chat,
# let a fake Pi process exit cleanly while the chat is idle, then run
# `wg chat resume` and `wg chat send`. Before fix-chat-resume, resume was
# accepted but the stale map entry made lazy spawn silently skip; `wg chat show`
# stayed at handler:none and the message was only queued.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
# The smoke gate may itself run inside a WG worker. The isolated fixture models
# a human operator, so do not inherit the worker service-control prohibition.
unset WG_AGENT_ID WG_SPAWN_EPOCH WG_EXECUTOR_TYPE WG_MODEL WG_TIER

scratch=$(make_scratch)
cd "$scratch"

fake_bin="$scratch/fake-bin"
mkdir -p "$fake_bin"
pi_invocations="$scratch/pi-invocations.log"
cat >"$fake_bin/pi" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$WG_FAKE_PI_INVOCATIONS"
# The daemon's direct Pi chat gets null stdin, so this exits successfully and
# deterministically forces the supervisor's clean-idle no-respawn return path.
exit 0
SH
chmod +x "$fake_bin/pi"
export PATH="$fake_bin:$PATH"
export WG_FAKE_PI_INVOCATIONS="$pi_invocations"

if ! wg init -x shell >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -10 init.log)"
fi
start_wg_daemon "$scratch" --max-agents 0
wgd="$WG_SMOKE_DAEMON_DIR"

if ! wg --dir "$wgd" chat create --name idle-pi --exec pi \
        --model pi:openrouter:test/model --json >create.log 2>&1; then
    loud_fail "Pi chat create failed: $(cat create.log)"
fi
cid=$(grep -oE '"chat_id"[[:space:]]*:[[:space:]]*[0-9]+' create.log | grep -oE '[0-9]+$' | head -1)
if [[ -z "$cid" ]]; then
    loud_fail "could not parse chat id: $(cat create.log)"
fi

# The first fake Pi launch exits success. With no inbox/cursor activity the
# supervisor takes its idle no-respawn return path and leaves its handle in the
# daemon map — the exact stale-entry precondition.
ended=""
for _ in $(seq 1 80); do
    if grep -q "Coordinator-$cid: idle .*exiting supervisor" "$wgd/service/daemon.log" 2>/dev/null; then
        ended=1
        break
    fi
    sleep 0.25
done
if [[ -z "$ended" ]]; then
    loud_fail "supervisor did not reach the idle-ended state. daemon=$(tail -60 "$wgd/service/daemon.log" 2>/dev/null)"
fi

before_count=$(wc -l <"$pi_invocations" 2>/dev/null || echo 0)
resume_out=$(wg --dir "$wgd" chat resume "$cid" 2>&1)
resume_rc=$?
if [[ "$resume_rc" -ne 0 ]]; then
    loud_fail "wg chat resume failed: $resume_out"
fi
if ! wg --dir "$wgd" chat send "$cid" "wake after idle" >send.log 2>&1; then
    loud_fail "wg chat send failed: $(cat send.log)"
fi

# Resume itself now queues an urgent wake; send exercises the exact subsequent
# human flow too. The fake handler is intentionally short-lived, so the stable
# evidence of recreation is a new invocation plus the daemon's explicit stale
# eviction breadcrumb. Before the fix the count stays unchanged and no eviction
# appears while `chat show` remains handler:none.
recreated=""
last_show=""
for _ in $(seq 1 80); do
    after_count=$(wc -l <"$pi_invocations" 2>/dev/null || echo 0)
    last_show=$(wg --dir "$wgd" chat show "$cid" 2>&1 || true)
    if [[ "$after_count" -gt "$before_count" ]] \
        && grep -q "Coordinator agent $cid supervisor ended; evicted stale handle" "$wgd/service/daemon.log" 2>/dev/null; then
        recreated=1
        break
    fi
    sleep 0.25
done
if [[ -z "$recreated" ]]; then
    loud_fail "resume/send silently queued behind stale ended supervisor. resume=$resume_out show=$last_show invocations=$(cat "$pi_invocations" 2>/dev/null) daemon=$(tail -60 "$wgd/service/daemon.log" 2>/dev/null)"
fi

if ! find "$wgd/chat" -name inbox.jsonl -type f -exec grep -q 'wake after idle' {} \; -print 2>/dev/null | grep -q .; then
    loud_fail "post-resume message was not persisted to the chat inbox"
fi

echo "PASS: Pi chat supervisor reached idle-ended state; CLI resume/send evicted the stale handle and recreated the handler"
exit 0
