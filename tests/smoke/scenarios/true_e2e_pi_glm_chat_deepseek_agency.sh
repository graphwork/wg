#!/usr/bin/env bash
# Scenario: true_e2e_pi_glm_chat_deepseek_agency
#
# The single TRUE end-to-end smoke for the Pi/OpenRouter two-tier profile:
# drive the system from a PTY-based Pi GLM-5.2 chat, dispatch real work that
# runs on a Pi worker via openrouter:z-ai/glm-5.2, observe completion, and
# confirm the model split wired by `update-pi-starter` is honored —
# GLM-5.2 for worker/chat roles, DeepSeek (`openrouter:deepseek/deepseek-chat`)
# for the agency one-shot roles (.evaluate / .flip / .assign), NOT claude:haiku.
#
# It pins four things at once (the four validation items of true-e2e-smoke):
#
#   1. PTY chat boot — a Pi GLM-5.2 chat created from `wg chat new --executor pi`
#      and rendered in `wg tui` MUST attach its pane to a live interactive `pi`
#      PTY (here a fake `pi` that prints a unique marker), NOT hang on
#      "Booting agent..." / route through `pi --mode rpc` on pipes. If the
#      marker never appears the scenario FAILS loudly — this is the
#      'Booting agent...' hang regression guard (fix-tui-pi).
#
#   2. Worker routing — a task created AND dispatched FROM that PTY chat runs on
#      a Pi worker whose inner argv is `--provider openrouter --model z-ai/glm-5.2`
#      (the GLM-5.2 worker route). Asserted from the fake-pi worker's recorded
#      argv.
#
#   3. Agency routing — the agency one-shot roles resolve to the configured
#      cheap DeepSeek model under the Pi profile, NOT claude:haiku. Asserted
#      credential-free via `wg evaluate run <task> --dry-run` (evaluator role)
#      and `--flip --dry-run` (inference/comparison roles), which print the
#      role-resolved model without making any network call.
#
#   4. End-to-end completion driven from the PTY chat — typing a trigger into
#      the live chat composer makes the (fake) Pi agent run `wg add` + `wg spawn`,
#      the GLM-5.2 worker completes the task (`wg done` → status `done`), and the
#      chat pane reflects the completion. Asserted from the worker task status
#      AND a reflection marker echoed back into the tmux pane.
#
# ── Why this is CI-capable / credential-free ──────────────────────────────
# A fake `pi` binary stands in for both the interactive chat agent and the
# one-shot worker (per the existing pi smoke conventions: see
# pi_worker_one_shot_prompt_and_cred_error.sh and tui_pi_chat_pty_embedded.sh).
# No OpenRouter / DeepSeek / Anthropic key is needed: the worker route is
# proven by the fake-pi *argv*, and the DeepSeek agency route is proven by the
# config-resolved `--dry-run` output. A live `pi`+OpenRouter run would,
# additionally, exercise the real model — but a real DeepSeek agency call
# requires OPENROUTER_API_KEY; without a key `resolve_agency_dispatch` falls
# back to claude:haiku on purpose (the fix-route-agency credential safety net),
# so a credential-free smoke asserts the *configured* routing rather than
# forcing a live DeepSeek call.
#
# ── Why a PRIVATE tmux server (`tmux -L`) ─────────────────────────────────
# `wg tui` resolves the chat pane's executor from the loaded config's
# coordinator/[dispatcher] model. A SHARED tmux server reuses the env of
# whichever client started it first, so a developer/CI box that already has a
# tmux server running leaks its HOME (and therefore a *different* ~/.wg/config),
# which made `wg tui` spawn the wrong executor entirely (observed: a real
# `claude` pane). We start a dedicated, throwaway tmux server on a private
# socket so the TUI inherits THIS scenario's isolated HOME + PATH every time.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive a PTY for the TUI"
fi

WG_BIN="$(command -v wg)"

scratch=$(make_scratch)
# Fully isolate HOME/XDG so `wg profile use pi` writes the starter profile and
# active-profile pointer into the scratch, never the real ~/.wg.
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
mkdir -p "$HOME/.wg" "$XDG_CONFIG_HOME"
bindir="$scratch/bin"
mkdir -p "$bindir"
# Put the fake `pi` FIRST on PATH so both the TUI chat pane and the dispatched
# worker resolve `pi` to our stand-in. `wg` stays reachable via the rest of PATH.
export PATH="$bindir:$PATH"

G="$scratch/proj/.wg"
marker="TRUE_E2E_PI_BOOT_$$"
reflect="TRUE_E2E_CHAT_REFLECTS_DONE"
worker_log="$scratch/worker.log"
inv_log="$scratch/pi-invocations.log"
: >"$worker_log"
: >"$inv_log"

# Private tmux server — see header. Registered for teardown.
TM_SOCK="wgsmoke-true-e2e-$$"
TM() { tmux -L "$TM_SOCK" "$@"; }
tmux_kill_server() { tmux -L "$TM_SOCK" kill-server 2>/dev/null || true; }
add_cleanup_hook tmux_kill_server

# ── Fake `pi` binary (chat-interactive + one-shot worker in one) ──────────
# Interactive (no `-p`): print the boot marker to the PTY, then read stdin
#   lines. On a line containing GODISPATCH, create + dispatch a GLM-5.2 worker
#   task and echo a reflection line once it reaches `done`.
# Worker (`-p`): record argv (so the test can assert the GLM-5.2 route), mark
#   the WG task done so the graph reaches completion, print a reply, exit 0.
cat >"$bindir/pi" <<PIEOF
#!/usr/bin/env bash
G="$G"
WG="$WG_BIN"
printf 'INV args=[%s]\n' "\$*" >>"$inv_log"
is_worker=0
for a in "\$@"; do [[ "\$a" == "-p" ]] && is_worker=1; done
if [[ \$is_worker -eq 1 ]]; then
    printf 'WORKER_ARGS %s\n' "\$*" >>"$worker_log"
    cat >/dev/null
    if [[ -n "\${WG_TASK_ID:-}" ]]; then
        "\$WG" --dir "\$G" done "\$WG_TASK_ID" >>"$worker_log" 2>&1 \
            || echo "WORKER_DONE_FAILED rc=\$?" >>"$worker_log"
    fi
    printf 'fake glm-5.2 worker reply\n'
    exit 0
fi
echo "$marker"
while IFS= read -r line; do
    printf 'GOT %s\n' "\$line" >>"$inv_log"
    case "\$line" in
        *GODISPATCH*)
            "\$WG" --dir "\$G" add "glm worker from chat" --id glm-from-chat \
                --no-place --model pi:openrouter/z-ai/glm-5.2 \
                -d "CHAT_DISPATCHED_WORKER_PROMPT" >>"$inv_log" 2>&1
            "\$WG" --dir "\$G" spawn glm-from-chat --executor pi --timeout 30s \
                >>"$inv_log" 2>&1
            st=""
            for i in \$(seq 1 60); do
                st=\$("\$WG" --dir "\$G" show glm-from-chat 2>/dev/null \
                    | grep -i '^Status:' | head -1)
                [[ "\$st" == *done* ]] && break
                sleep 0.5
            done
            echo "$reflect \$st"
            ;;
    esac
done
PIEOF
chmod +x "$bindir/pi"

# ── Config: activate the real `pi` starter profile, then pin it project-local ─
# The pi starter (update-pi-starter) sets [dispatcher].model = pi:openrouter/
# z-ai/glm-5.2 (so effective_executor = pi) and the agency/meta roles to
# openrouter:deepseek/deepseek-chat. `wg init` writes a claude default project
# config; we overwrite it with the activated pi config so BOTH the CLI and the
# TUI's own Config::load resolve the Pi routing deterministically (no daemon /
# CoordinatorState handoff required).
if ! wg profile init-starters >"$scratch/init-starters.log" 2>&1; then
    loud_fail "wg profile init-starters failed: $(tail -5 "$scratch/init-starters.log")"
fi
if ! wg --dir "$G" init >"$scratch/init.log" 2>&1; then
    loud_fail "wg --dir init failed: $(tail -5 "$scratch/init.log")"
fi
if ! wg --dir "$G" profile use pi --no-reload >"$scratch/profile-use.log" 2>&1; then
    loud_fail "wg profile use pi failed: $(tail -5 "$scratch/profile-use.log")"
fi
if [[ ! -f "$HOME/.wg/config.toml" ]]; then
    loud_fail "wg profile use pi did not write $HOME/.wg/config.toml"
fi
cp "$HOME/.wg/config.toml" "$G/config.toml"

# Sanity: the Pi profile is honored in the resolved config (worker=GLM-5.2,
# agency=DeepSeek). This is the config-side half of validation item #3.
models_out="$(wg --dir "$G" config --models 2>&1 || true)"
if ! grep -q "pi:openrouter/z-ai/glm-5.2" <<<"$models_out"; then
    loud_fail "pi profile not honored: default/worker model is not pi:openrouter/z-ai/glm-5.2.
$models_out"
fi
if ! grep -q "deepseek/deepseek-chat" <<<"$models_out"; then
    loud_fail "pi profile not honored: agency roles not on openrouter:deepseek/deepseek-chat.
$models_out"
fi

# Create the Pi GLM-5.2 chat (the conversation we will drive).
if ! wg --dir "$G" chat new --name piroom --executor pi \
        --model "openrouter:z-ai/glm-5.2" >"$scratch/chat-new.log" 2>&1; then
    loud_fail "wg chat new --executor pi failed: $(cat "$scratch/chat-new.log")"
fi

# ── Launch the TUI in the private tmux server ─────────────────────────────
session="wgsmoke-true-e2e-$$"
TM new-session -d -s "$session" -x 180 -y 50 "$WG_BIN --dir $G tui"

# ── Assertion 1: PTY chat boots (no 'Booting agent...' hang) ──────────────
# The fake-pi marker must appear in the live pane. If the pane were stuck on
# "Booting agent..." (the fix-tui-pi regression) or spawned the wrong executor
# (env leak), the marker would never show.
saw_marker=""
for _ in $(seq 1 60); do
    cap=$(TM capture-pane -t "$session" -p 2>/dev/null || true)
    if grep -qF "$marker" <<<"$cap"; then
        saw_marker=1
        break
    fi
    sleep 0.25
done
if [[ -z "$saw_marker" ]]; then
    cap=$(TM capture-pane -t "$session" -p 2>/dev/null || true)
    loud_fail "Pi GLM-5.2 chat pane never showed the live interactive-pi marker '$marker' \
within 15s — the 'Booting agent...' PTY hang regression (pane not attached to a live \
interactive pi). Pane content:
$cap
pi invocations:
$(cat "$inv_log" 2>/dev/null)"
fi

# ── Drive the dispatch FROM the chat ──────────────────────────────────────
# After auto-enable the chat pane owns input focus, so typed bytes forward to
# the fake-pi child's stdin. The trigger makes it run `wg add` + `wg spawn`.
sleep 1
TM send-keys -t "$session" "GODISPATCH" Enter

# ── Assertions 2 + 4: GLM-5.2 worker completes, chat reflects it ──────────
worker_done=""
reflect_seen=""
for _ in $(seq 1 100); do
    st=$(wg --dir "$G" show glm-from-chat 2>/dev/null | grep -i '^Status:' | head -1)
    [[ "$st" == *done* ]] && worker_done=1
    cap=$(TM capture-pane -t "$session" -p 2>/dev/null || true)
    grep -qF "$reflect" <<<"$cap" && reflect_seen=1
    [[ -n "$worker_done" && -n "$reflect_seen" ]] && break
    sleep 0.5
done

if [[ -z "$worker_done" ]]; then
    loud_fail "GLM-5.2 worker task 'glm-from-chat' (dispatched from the PTY chat) never \
reached 'done'. Last status: $(wg --dir "$G" show glm-from-chat 2>/dev/null | grep -i '^Status:' | head -1)
pi invocations:
$(cat "$inv_log" 2>/dev/null)
worker log:
$(cat "$worker_log" 2>/dev/null)"
fi

# The chat-created worker must have run as a Pi worker on the GLM-5.2 route.
if ! grep -q "WORKER_ARGS .*--provider openrouter --model z-ai/glm-5.2" "$worker_log"; then
    loud_fail "worker task did NOT run on the GLM-5.2 route (openrouter:z-ai/glm-5.2). \
fake-pi worker argv:
$(cat "$worker_log" 2>/dev/null)"
fi

# The chat must have observed the completion (reflection echoed into the pane).
if [[ -z "$reflect_seen" ]]; then
    cap=$(TM capture-pane -t "$session" -p 2>/dev/null || true)
    loud_fail "chat pane did not reflect the dispatched task's completion (no '$reflect' \
line) — the PTY chat did not observe end-to-end completion. Pane content:
$cap"
fi

# Confirm the trigger genuinely traversed the PTY (defensive: the worker could
# in principle be dispatched some other way; require the typed line to have
# reached the fake-pi chat's stdin).
if ! grep -q "GOT GODISPATCH" "$inv_log"; then
    loud_fail "dispatch trigger never reached the interactive pi chat's stdin via the \
PTY composer. pi invocations:
$(cat "$inv_log" 2>/dev/null)"
fi

# ── Assertion 3: agency one-shot roles route to DeepSeek, not claude:haiku ──
# Credential-free: --dry-run prints the role-resolved model without a network
# call. Run against the chat-dispatched worker task so the assertion is tied to
# the very task whose evaluation/flip would run under this profile.
eval_dry="$(wg --dir "$G" evaluate run glm-from-chat --dry-run 2>&1 || true)"
eval_model_line="$(grep -i 'Evaluator model' <<<"$eval_dry" | head -1)"
if ! grep -q "deepseek/deepseek-chat" <<<"$eval_model_line"; then
    loud_fail "agency evaluator did NOT route to the configured DeepSeek model under the \
Pi profile. Evaluator model line: '$eval_model_line'
full dry-run:
$eval_dry"
fi
if grep -qiE "haiku|claude" <<<"$eval_model_line"; then
    loud_fail "agency evaluator leaked claude:haiku instead of DeepSeek: '$eval_model_line'"
fi

flip_dry="$(wg --dir "$G" evaluate run glm-from-chat --flip --dry-run 2>&1 || true)"
if ! grep -qi "Inference model: openrouter:deepseek/deepseek-chat" <<<"$flip_dry"; then
    loud_fail "FLIP inference role did NOT route to openrouter:deepseek/deepseek-chat.
$flip_dry"
fi
if ! grep -qi "Comparison model: openrouter:deepseek/deepseek-chat" <<<"$flip_dry"; then
    loud_fail "FLIP comparison role did NOT route to openrouter:deepseek/deepseek-chat.
$flip_dry"
fi

echo "PI_GLM_CHAT_BOOTED: yes (marker '$marker' on live PTY, no 'Booting agent...' hang)"
echo "WORKER_RAN_ON_GLM_5_2: yes ($(grep 'WORKER_ARGS' "$worker_log" | head -1))"
echo "AGENCY_ON_DEEPSEEK: yes ($eval_model_line; FLIP=openrouter:deepseek/deepseek-chat)"
echo "E2E_COMPLETION_FROM_CHAT: yes (glm-from-chat -> done; chat reflected '$reflect')"
echo "PASS: PTY Pi GLM-5.2 chat drove a GLM-5.2 worker to completion with DeepSeek agency routing"
exit 0
