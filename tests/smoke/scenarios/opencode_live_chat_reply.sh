#!/usr/bin/env bash
# Live human-flow smoke for the OpenCode chat handler (verify-opencode-chat).
#
# Drives the REAL `wg opencode-handler --chat` against a seeded chat inbox and
# asserts a genuine assistant reply lands in the outbox — NOT the
# "encountered an error running opencode" fallback. This is the regression
# guard for two bugs that shipped green from routing-only tests but broke the
# live path against opencode >=1.16:
#
#   1. `opencode run`'s `--file` is a *variadic* option that greedily swallowed
#      the trailing positional message ("File not found: <message>", exit 1).
#      Fix: the positional message must precede `--file`.
#   2. `opencode run --format json` does NOT stream the assistant text on stdout
#      (only `step_start`); the reply is recovered via `opencode export
#      <sessionID>`. Fix: capture the session id and export it.
#
# SKIPs loudly (exit 77) when the opencode binary or OpenRouter credentials are
# absent — no false FAIL in credential-less CI.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

command -v opencode >/dev/null 2>&1 || \
    loud_skip "MISSING OPENCODE BINARY" "opencode not on PATH; install opencode CLI to run the live chat smoke"

# OpenRouter creds: either opencode's own auth store or the env var.
oc_auth="${HOME}/.local/share/opencode/auth.json"
if [ -n "${OPENROUTER_API_KEY:-}" ]; then
    :
elif [ -f "$oc_auth" ] && grep -q '"openrouter"' "$oc_auth" 2>/dev/null; then
    :
else
    loud_skip "MISSING OPENROUTER CREDENTIALS" \
        "no OPENROUTER_API_KEY and no openrouter entry in $oc_auth"
fi

MODEL="opencode:openrouter/stepfun/step-3.7-flash"

scratch=$(make_scratch)
cd "$scratch"

# Init with a plain claude route — the opencode model is passed to the handler
# explicitly via `--model` below, so the project's configured model is moot
# here (and `wg init` does not accept executor-qualified `opencode:` routes).
init_out="$scratch/init.out"
wg init -m claude:opus --no-agency >"$init_out" 2>&1 || \
    loud_fail "wg init failed: $(tail -20 "$init_out")"

# Create a chat (no daemon in scratch → nothing competes for the session lock).
# With the service down, `wg chat create` prints a human line ("Created chat N
# (task .chat-N). ...") rather than JSON; with it up it prints JSON. Parse both.
create_out="$scratch/create.out"
wg chat create >"$create_out" 2>&1 || \
    loud_fail "wg chat create failed: $(cat "$create_out")"
cid="$(python3 -c "import json;print(json.load(open('$create_out'))['coordinator_id'])" 2>/dev/null)"
if [ -z "$cid" ]; then
    cid="$(grep -oE 'Created chat [0-9]+' "$create_out" | grep -oE '[0-9]+' | head -1)"
fi
[ -n "$cid" ] || loud_fail "could not parse chat id from: $(cat "$create_out")"

# Seed the inbox with a deterministic, trivially-checkable prompt.
# NOTE: send and handler must use the SAME chat ref so they resolve to the same
# chat dir. With the service down there is no session registry, so refs are
# taken literally (`coordinator-N` and `N` would land in different dirs); use
# the bare chat id for both.
wg chat send "$cid" "Reply with exactly the single word OK and nothing else." >/dev/null 2>&1 || \
    loud_fail "wg chat send failed"

# Run the real handler. It loops forever polling the inbox, so cap it with a
# timeout; once it has answered, the outbox carries the reply.
handler_out="$scratch/handler.out"
timeout 90 wg opencode-handler --chat "$cid" --model "$MODEL" \
    >"$handler_out" 2>&1
rc=$?
# 124 (timeout) is the expected exit — the handler answered then kept polling.
if [ "$rc" -ne 124 ] && [ "$rc" -ne 0 ]; then
    loud_fail "opencode-handler exited unexpectedly ($rc): $(tail -20 "$handler_out")"
fi

# Locate the chat dir and read the outbox.
outbox=""
for d in "$scratch"/.wg/chat/*/; do
    if [ -f "$d/outbox.jsonl" ] && [ -s "$d/outbox.jsonl" ]; then
        outbox="$d/outbox.jsonl"
        break
    fi
done
[ -n "$outbox" ] || loud_fail "no non-empty outbox written by the handler under $scratch/.wg/chat"

reply="$(python3 -c "
import sys,json
last=None
for ln in open('$outbox'):
    ln=ln.strip()
    if not ln: continue
    last=json.loads(ln)
print((last or {}).get('content',''))
" 2>/dev/null)"

# Hard gate: the handler must NOT have fallen back to the opencode error reply.
case "$reply" in
    *"encountered an error running opencode"*|*"File not found"*)
        loud_fail "handler returned the opencode ERROR fallback, not a model reply: $reply"
        ;;
esac
[ -n "$reply" ] || loud_fail "handler wrote an empty reply to the outbox"

# The model was asked for a one-word OK; accept it case-insensitively.
echo "$reply" | grep -qiE '\bok\b' || \
    loud_fail "expected an 'OK' reply from the opencode handler, got: $reply"

echo "PASS: opencode chat handler produced a live reply: $reply" 1>&2
exit 0
