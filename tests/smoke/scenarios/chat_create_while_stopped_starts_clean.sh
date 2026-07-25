#!/usr/bin/env bash
# Scenario: chat_create_while_stopped_starts_clean
#
# fix-chat-coordinator-2 regression: `wg chat create` while the service is
# DOWN writes only the graph task + per-chat CoordinatorState. The chat
# session dir + sessions.json entry are registered by the supervisor on the
# next `wg service start`. Before the fix, the sessions.json atomic `save()`
# used a shared `sessions.json.tmp`; concurrent uncoordinated writes (a TUI
# creating a session while the daemon registered many coordinators on
# restart) collided and the daemon logged
#   `Coordinator-N: register_coordinator_session failed: No such file or directory`
# leaving the chat un-attachable (`wg chat resume` then hung on a dir that
# was never created).
#
# This scenario exercises the real terminal flow with a Pi chat
# (openai-codex:gpt-5.6-sol) and pins:
#   * `wg chat create` while stopped succeeds and creates the .chat-N task
#   * `wg service start` registers the session with NO missing-directory error
#   * the chat dir + sessions.json entry exist after start (attachable)
#   * `wg chat list` returns within a bounded window (no hang)
#
# It is credential-free by design: the supervisor performs registration
# BEFORE spawning the LLM handler, so the assertions hold even though the
# handler itself never reaches a model in the isolated HOME.

set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

# The chat is a Pi chat; the preflight `require_interactive_executor_binary`
# must find the `pi` executable or create refuses before registering
# anything. Skip loud (exit 77) where pi is not installed.
if ! command -v pi >/dev/null 2>&1; then
    loud_skip "PI CLI MISSING" "pi executable not on PATH; run 'wg pi-plugin install' or install pi"
fi

scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME/.wg"
cd "$scratch"

# Graph-only init.
wg init --no-agency >init.log 2>&1 || loud_fail "wg init failed: $(tail -5 init.log)"
wg_dir="$scratch/.wg"

# Lay down a canonical all-Pi role routing so `validate_pi_model_plane` passes
# for every orchestration role. The per-chat `-m` below pins the specific
# Pi route we are regression-testing; the role routing here only needs to be
# all-Pi.
cat > "$wg_dir/config.toml" <<'PI_CFG'
[agent]
model = "pi:openrouter:z-ai/glm-5.2"
[dispatcher]
model = "pi:openrouter:z-ai/glm-5.2"
[tiers]
fast = "pi:openrouter:deepseek/deepseek-chat"
fast_reasoning = "low"
standard = "pi:openrouter:z-ai/glm-5.2"
standard_reasoning = "high"
premium = "pi:openrouter:z-ai/glm-5.2"
premium_reasoning = "xhigh"
[models.default]
model = "pi:openrouter:z-ai/glm-5.2"
reasoning = "high"
[models.task_agent]
model = "pi:openrouter:z-ai/glm-5.2"
reasoning = "high"
[models.evaluator]
model = "pi:openrouter:deepseek/deepseek-chat"
reasoning = "low"
[models.assigner]
model = "pi:openrouter:deepseek/deepseek-chat"
reasoning = "low"
[models.flip_inference]
model = "pi:openrouter:deepseek/deepseek-chat"
reasoning = "low"
[models.flip_comparison]
model = "pi:openrouter:deepseek/deepseek-chat"
reasoning = "low"
[models.reviewer]
model = "pi:openrouter:deepseek/deepseek-chat"
reasoning = "low"
PI_CFG

# Stop any inherited daemon so the create path is the service-DOWN direct
# path (the exact precondition of the bug).
wg --dir "$wg_dir" service stop --force --kill-agents >/dev/null 2>&1 || true

# --- Step 1: create a Pi chat while the service is DOWN -------------------
create_out=$(wg --dir "$wg_dir" chat create --name repro -m pi:openai-codex:gpt-5.6-sol 2>create.err) || \
    loud_fail "wg chat create (service down) failed: $(cat create.err)"
# Resolve the numeric chat id from the task that landed in the graph.
chat_id=$(wg --dir "$wg_dir" chat list --json 2>/dev/null \
    | grep -oE '"chat_id"[[:space:]]*:[[:space:]]*[0-9]+' | head -1 \
    | grep -oE '[0-9]+$') || true
[[ -n "$chat_id" ]] || loud_fail "could not parse chat_id after create: $create_out"

# Precondition: the session is NOT yet registered (supervisor owns that).
if [[ -f "$wg_dir/chat/sessions.json" ]] \
   && grep -q "\"chat-$chat_id\"" "$wg_dir/chat/sessions.json" 2>/dev/null; then
    loud_fail "service-down create must NOT pre-register the session, but sessions.json already lists chat-$chat_id"
fi

# --- Step 2: start the service (supervisor registers on boot) ------------
# Default start enables chat supervisors (do NOT pass --no-chat-agent, which
# would skip registration entirely). --max-agents 0 keeps worker dispatch
# quiet so the only activity is the chat supervisor.
start_wg_daemon "$scratch" --max-agents 0 --interval 5
daemon_log="$wg_dir/service/daemon.log"

# Registration is the FIRST thing the supervisor does each iteration, so it
# must appear quickly. Poll a short window for sessions.json to list the chat.
registered=0
for _ in $(seq 1 50); do
    if [[ -f "$wg_dir/chat/sessions.json" ]] \
       && grep -q "\"chat-$chat_id\"" "$wg_dir/chat/sessions.json" 2>/dev/null; then
        registered=1
        break
    fi
    sleep 0.2
done
[[ "$registered" -eq 1 ]] || \
    loud_fail "chat-$chat_id never registered in sessions.json after service start. daemon.log tail:
$(tail -20 "$daemon_log" 2>/dev/null || true)"

# --- Step 3: NO missing-directory error in the daemon log -----------------
if grep -q "register_coordinator_session failed: No such file or directory" "$daemon_log" 2>/dev/null; then
    loud_fail "daemon logged register_coordinator_session ENOENT (the regression):
$(grep -n 'register_coordinator_session failed' "$daemon_log" | head)"
fi

# --- Step 4: attachable — chat dir exists + resolves ----------------------
# At least one UUID chat dir now exists for this chat.
chat_dir=$(find "$wg_dir/chat" -maxdepth 1 -mindepth 1 -type d 2>/dev/null | head -1)
[[ -n "$chat_dir" && -d "$chat_dir" ]] || \
    loud_fail "no UUID chat dir created under $wg_dir/chat after registration"

# `wg chat list` must return within a bounded window (the wedge symptom was an
# 8s timeout). It must list the chat without a missing-directory error.
if ! timeout 12 wg --dir "$wg_dir" chat list >list.out 2>list.err; then
    loud_fail "wg chat list did not return within 12s or errored:
$(tail -10 list.err 2>/dev/null || true)"
fi
grep -q "repro" list.out || \
    loud_fail "wg chat list did not show the 'repro' chat:
$(cat list.out)"

echo "PASS: Pi chat created while service-down registered on start with no missing-directory error and is attachable"
