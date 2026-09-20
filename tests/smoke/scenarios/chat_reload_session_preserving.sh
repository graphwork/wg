#!/usr/bin/env bash
# Scenario: chat_reload_session_preserving
#
# End-to-end (credential-free) proof that `wg chat reload` is the one verb for
# a session-preserving reload of the pi plugin bundle + handler:
#
#   1. A live pi chat is started under an isolated daemon with a fake `pi`.
#   2. The embedded plugin cache is deliberately corrupted (stale bytes +
#      wrong digest stamp) AFTER the handler is live, modelling the six-week
#      stale cache that silently served old extensions.
#   3. `wg chat reload <cid>` must re-materialize the cache (embed digest
#      validation), stop + respawn the handler resuming the SAME session
#      file, become live again, and print a before/after delta showing the
#      plugin digest CHANGED and the session preserved.
#   4. Removing the transcript makes reload refuse loudly
#      (WG-CHAT-RELOAD-SESSION-MISSING) without touching the live handler.
#   5. A fake pi that exits immediately makes reload refuse loudly
#      (WG-CHAT-RELOAD-NOT-LIVE), leaving the chat resumable.
#
# Everything is credential-free: the fake `pi` never contacts a provider.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
# The smoke gate may itself run inside a WG worker. The isolated fixture models
# a human operator, so do not inherit the worker service-control prohibition.
unset WG_AGENT_ID WG_SPAWN_EPOCH WG_EXECUTOR_TYPE WG_MODEL WG_TIER
# Force the embedded→cache plugin path (the worker env may point
# WG_PI_PLUGIN_DIR at a pre-materialized real cache).
unset WG_PI_PLUGIN_DIR

scratch=$(make_scratch)
cd "$scratch"

# --- fake pi ------------------------------------------------------------------
# Writes the transcript pi would own (`<ts>_<session-id>.jsonl` under
# --session-dir), then stays live. When the dead sentinel exists it exits
# immediately after touching the transcript, exercising the not-live refusal.
fake_bin="$scratch/fake-bin"
mkdir -p "$fake_bin"
cat >"$fake_bin/pi" <<'SH'
#!/usr/bin/env bash
sid=""
sdir=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --session-id) sid="$2"; shift 2 ;;
        --session-dir) sdir="$2"; shift 2 ;;
        *) shift ;;
    esac
done
if [[ -n "$sdir" && -n "$sid" ]]; then
    mkdir -p "$sdir"
    printf '{"type":"turn"}\n' >>"$sdir/2026-01-01T00-00-00-000Z_${sid}.jsonl"
fi
if [[ -n "${WG_FAKE_PI_DEAD_SENTINEL:-}" && -f "${WG_FAKE_PI_DEAD_SENTINEL}" ]]; then
    exit 0
fi
sleep 30
SH
chmod +x "$fake_bin/pi"
export PATH="$fake_bin:$PATH"
export WG_FAKE_PI_DEAD_SENTINEL="$scratch/pi-dead"

# Force the embedded→cache plugin path even though this repo checkout has a dev
# tree; that is the installed-binary behavior the reload must validate.
export WG_PI_PLUGIN_FORCE_CACHE=1
export XDG_CACHE_HOME="$scratch/cache"
export HOME="$scratch/home"
mkdir -p "$XDG_CACHE_HOME" "$HOME"

if ! wg init -x shell >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -10 init.log)"
fi
start_wg_daemon "$scratch" --max-agents 0
wgd="$WG_SMOKE_DAEMON_DIR"

if ! wg --dir "$wgd" chat create --name reload-pi --exec pi \
        --model pi:openrouter:test/model --json >create.log 2>&1; then
    loud_fail "Pi chat create failed: $(cat create.log)"
fi
cid=$(grep -oE '"chat_id"[[:space:]]*:[[:space:]]*[0-9]+' create.log | grep -oE '[0-9]+$' | head -1)
if [[ -z "$cid" ]]; then
    loud_fail "could not parse chat id: $(cat create.log)"
fi

chat_show() {
    wg --dir "$wgd" chat show "$cid" --json 2>/dev/null
}

# Wait for the first handler to become live (and create its transcript).
live=""
for _ in $(seq 1 120); do
    if chat_show | grep -q '"pid"'; then
        live=1
        break
    fi
    sleep 0.25
done
if [[ -z "$live" ]]; then
    loud_fail "chat $cid never became live. show=$(chat_show) daemon=$(tail -80 "$wgd/service/daemon.log" 2>/dev/null)"
fi

# The canonical chat dir is the UUID dir (registry-resolved), not chat/chat-N.
# The fake pi creates the transcript a moment after the handler lock is taken.
transcript=""
for _ in $(seq 1 80); do
    transcript=$(find "$wgd/chat" -type f -name "*_chat-${cid}.jsonl" 2>/dev/null | head -1)
    if [[ -n "$transcript" ]]; then
        break
    fi
    sleep 0.25
done
if [[ -z "$transcript" || ! -f "$transcript" ]]; then
    loud_fail "fake pi did not create a *_chat-$cid.jsonl transcript under $wgd/chat (tree=$(find "$wgd/chat" -maxdepth 3 2>&1))"
fi
session_dir=$(dirname "$transcript")
before_lines=$(wc -l <"$transcript")
if [[ "$before_lines" -lt 1 ]]; then
    loud_fail "transcript unexpectedly empty: $transcript"
fi

# --- corrupt the embedded plugin cache (the stale-cache failure) --------------
compat=$(wg pi-plugin compat-version)
cache_version_dir="$XDG_CACHE_HOME/wg/worksgood-pi/$compat"
mkdir -p "$cache_version_dir/pi-worksgood" "$cache_version_dir/host"
printf '/* six-week-old stale build */\n' >"$cache_version_dir/pi-worksgood/index.js"
printf '// stale host\n' >"$cache_version_dir/host/wg-pi-host.mjs"
printf '{"compat":"%s"}\n' "$compat" >"$cache_version_dir/version.json"
printf 'b3:stale-not-the-embed\n' >"$cache_version_dir/.wg-embed-digest"
: >"$cache_version_dir/.wg-ok"

# --- 1. successful reload -----------------------------------------------------
reload_out="$scratch/reload.out"
if ! wg --dir "$wgd" chat reload "$cid" >"$reload_out" 2>&1; then
    loud_fail "wg chat reload failed: $(cat "$reload_out") daemon=$(tail -80 "$wgd/service/daemon.log" 2>/dev/null)"
fi
cat "$reload_out"

grep -q 'plugin-digest=CHANGED' "$reload_out" || \
    loud_fail "reload must report the plugin digest changed (stale cache re-materialized), got: $(cat "$reload_out")"
grep -q 'session=preserved' "$reload_out" || \
    loud_fail "reload must report the session preserved, got: $(cat "$reload_out")"
grep -q 'cache=Current' "$reload_out" || \
    loud_fail "reload must leave the cache Current, got: $(cat "$reload_out")"

if [[ ! -f "$transcript" ]]; then
    loud_fail "reload did not preserve the session file $transcript"
fi
after_lines=$(wc -l <"$transcript")
if [[ "$after_lines" -le "$before_lines" ]]; then
    loud_fail "session did not continue across reload ($before_lines -> $after_lines lines)"
fi

# Cache really is current with the binary's embed again.
stale_digest='b3:stale-not-the-embed'
if grep -q "$stale_digest" "$cache_version_dir/.wg-embed-digest" 2>/dev/null; then
    loud_fail "stale cache digest stamp survived reload"
fi

# --- 2. missing-session refusal leaves the live handler alone -----------------
rm -f "$transcript"
missing_out="$scratch/reload-missing.out"
missing_rc=0
wg --dir "$wgd" chat reload "$cid" >"$missing_out" 2>&1 || missing_rc=$?
if [[ "$missing_rc" -eq 0 ]]; then
    loud_fail "reload must refuse when the session transcript is missing: $(cat "$missing_out")"
fi
grep -q 'WG-CHAT-RELOAD-SESSION-MISSING' "$missing_out" || \
    loud_fail "missing-session refusal must be loud and named, got: $(cat "$missing_out")"
if ! chat_show | grep -q '"pid"'; then
    loud_fail "refusal must leave the live handler running: $(chat_show)"
fi
# Restore the transcript for the not-live case.
printf '{"type":"turn"}\n' >"$transcript"

# --- 3. not-live-within-window refusal ----------------------------------------
: >"$WG_FAKE_PI_DEAD_SENTINEL"
not_live_out="$scratch/reload-not-live.out"
not_live_rc=0
wg --dir "$wgd" chat reload "$cid" >"$not_live_out" 2>&1 || not_live_rc=$?
if [[ "$not_live_rc" -eq 0 ]]; then
    loud_fail "reload must refuse when the respawn never becomes live: $(cat "$not_live_out")"
fi
grep -q 'WG-CHAT-RELOAD-NOT-LIVE' "$not_live_out" || \
    loud_fail "not-live refusal must be loud and named, got: $(cat "$not_live_out")"

exit 0
