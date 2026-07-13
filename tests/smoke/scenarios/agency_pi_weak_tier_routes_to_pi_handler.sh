#!/usr/bin/env bash
# Scenario: agency_pi_weak_tier_routes_to_pi_handler
#
# Regression (bug-flip-pi-route-uses-claude-cli): FLIP/evaluate tasks
# configured with a handler-first `pi:` weak tier (e.g.
# `pi:openrouter:deepseek/deepseek-chat`) failed because the agency
# one-shot lightweight-LLM path silently fell into the claude-CLI
# catch-all arm of `run_lightweight_llm_call` and errored with
# "Claude CLI call failed ... subscription access disabled" instead of
# honoring the pi route. The fix adds an `ExecutorKind::Pi` arm that
# drives `pi --mode json --print` as a one-shot and parses the NDJSON
# stream. A Pi failure must remain inside Pi: without an explicitly configured
# same-system fallback it fails loudly; with one it tries only the listed Pi
# route and never invokes Claude.
#
# This scenario proves the fix credential-free by stubbing both the
# `pi` and `claude` binaries on PATH:
#   * the stub `pi` records the invocation and emits canned NDJSON that
#     `translate_pi_stream` parses into a valid evaluator JSON reply;
#   * the stub `claude` FAILS loudly if ever invoked (the regression
#     signature), so a regression that routes back through claude fails
#     the scenario instead of silently passing.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

fake_bin="$scratch/fake-bin"
mkdir -p "$fake_bin"
marker="$scratch/invocations"
: >"$marker"

# Stub `pi`: record the selected model, fail the primary Terra route, and emit
# a valid evaluator response only for the explicitly configured Sol fallback.
cat >"$fake_bin/pi" <<'SH'
#!/usr/bin/env bash
set -u
model=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --model) model="${2:-}"; shift 2 ;;
        *) shift ;;
    esac
done
echo "pi:$model" >>"${SMOKE_PI_MARKER:-/dev/null}/pi.log"
cat >/dev/null
if [[ "$model" == "gpt-5.6-terra" ]]; then
    echo "injected Terra transport failure" >&2
    exit 41
fi
if [[ "$model" != "gpt-5.6-sol" ]]; then
    echo "unexpected Pi model: $model" >&2
    exit 42
fi
printf '%s\n' '{"type":"session","id":"smoke","version":3}'
printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","provider":"openai-codex","model":"gpt-5.6-sol","content":[{"type":"text","text":"{\"score\":1.0,\"dimensions\":{\"correctness\":1.0,\"completeness\":1.0},\"notes\":\"smoke pass\"}"}],"usage":{"input":10,"output":5,"cacheRead":0,"cacheWrite":0,"totalTokens":15,"cost":{"total":0.001}}}}'
SH
chmod +x "$fake_bin/pi"

# Stub `claude`: must NEVER be invoked on the happy path. If it is, fail
# loudly with the regression's own error signature so the scenario fails
# instead of silently passing.
cat >"$fake_bin/claude" <<'SH'
#!/usr/bin/env bash
set -u
echo "claude-invoked" >>"${SMOKE_PI_MARKER:-/dev/null}/claude.log"
echo "Claude CLI call failed: Your organization has disabled Claude subscription access for Claude Code" >&2
exit 42
SH
chmod +x "$fake_bin/claude"

export PATH="$fake_bin:$PATH"
export SMOKE_PI_MARKER="$scratch"

# Isolate HOME + XDG so the host's global ~/.wg/config.toml cannot leak into
# the merge (the merge of a global `dispatcher.safety_interval` with the
# local `dispatcher.poll_interval` is a pre-existing duplicate-field bug
# that would force `load_or_default` to fall back to defaults and lose the
# `tiers.fast` override). With no global config, only the local .wg/config
# is loaded and the pi weak tier is honored.
ISOLATED_HOME="$scratch/home"
mkdir -p "$ISOLATED_HOME/.config"
export HOME="$ISOLATED_HOME"
export XDG_CONFIG_HOME="$ISOLATED_HOME/.config"

unset OPENROUTER_API_KEY
unset OPENAI_API_KEY
unset ANTHROPIC_API_KEY
# Pi self-authenticates via env / its own OAuth. Ambient credentials must not
# cause WG to select a different handler or provider after failure.

if ! wg init >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -10 init.log)"
fi

# Configure the agency one-shot evaluator role to a handler-first Pi route —
# the exact configuration that triggered the bug. `wg init` writes explicit
# `[models.evaluator].model = "claude:haiku"` overrides (which would win over
# the weak tier); rewrite them to the pi route so the agency dispatch lands
# on the Pi handler. Also set `tiers.fast` for symmetry with a two-tier Pi
# profile. The bug: `resolve_agency_dispatch` correctly returned
# handler=Pi (so runtime metadata said Executor: pi), but
# `run_lightweight_llm_call`'s catch-all arm shelled out to the claude CLI
# instead, which failed with "Claude CLI call failed ... subscription
# disabled". The fix adds an `ExecutorKind::Pi` arm that drives `pi` as a
# one-shot.
sed -i 's|^fast = .*|fast = "pi:openai-codex:gpt-5.6-terra"|' .wg/config.toml
awk '
    BEGIN { in_eval = 0; in_flip_inf = 0; in_flip_cmp = 0; in_assign = 0 }
    /^\[models\.evaluator\]/    { in_eval = 1; print; next }
    /^\[models\.flip_inference\]/{ in_flip_inf = 1; print; next }
    /^\[models\.flip_comparison\]/{ in_flip_cmp = 1; print; next }
    /^\[models\.assigner\]/     { in_assign = 1; print; next }
    /^\[/ {
        if (in_eval || in_flip_inf || in_flip_cmp || in_assign) {
            in_eval = in_flip_inf = in_flip_cmp = in_assign = 0
        }
    }
    {
        if (in_eval && $1 == "model")      { print "model = \"pi:openai-codex:gpt-5.6-terra\""; in_eval = 0; next }
        if (in_flip_inf && $1 == "model") { print "model = \"pi:openai-codex:gpt-5.6-terra\""; in_flip_inf = 0; next }
        if (in_flip_cmp && $1 == "model") { print "model = \"pi:openai-codex:gpt-5.6-terra\""; in_flip_cmp = 0; next }
        if (in_assign && $1 == "model")  { print "model = \"pi:openai-codex:gpt-5.6-terra\""; in_assign = 0; next }
        print
    }
' .wg/config.toml >config.new
mv config.new .wg/config.toml

if ! grep -q '^model = "pi:openai-codex:gpt-5.6-terra"$' .wg/config.toml; then
    loud_fail "failed to rewrite agency role models to the pi route"
fi
if ! grep -q '^fast = "pi:openai-codex:gpt-5.6-terra"$' .wg/config.toml; then
    loud_fail "failed to set tiers.fast to the pi route in .wg/config.toml"
fi

if ! wg add "Pi weak-tier evaluator smoke" --id pi-weak-tier-target >add.log 2>&1; then
    loud_fail "wg add failed: $(tail -5 add.log)"
fi

if ! wg done pi-weak-tier-target >done.log 2>&1; then
    loud_fail "wg done target failed: $(tail -10 done.log)"
fi

if wg evaluate run pi-weak-tier-target >evaluate-no-fallback.log 2>&1; then
    loud_fail "Pi primary unexpectedly succeeded without a fallback:
$(cat evaluate-no-fallback.log)"
fi

if [[ ! -f "$scratch/pi.log" ]]; then
    loud_fail "pi handler was NOT invoked — agency did not honor the pi: weak tier:
$(cat evaluate-no-fallback.log)"
fi

if [[ -f "$scratch/claude.log" ]]; then
    loud_fail "claude CLI was invoked for a pi: weak tier — the regression is back:
$(cat evaluate-no-fallback.log)"
fi

if ! grep -q 'WG-EXEC-ROUTE-FAILED' evaluate-no-fallback.log \
    || ! grep -q 'role=evaluator' evaluate-no-fallback.log \
    || ! grep -q 'handler=pi' evaluate-no-fallback.log \
    || ! grep -q 'provider=openai-codex' evaluate-no-fallback.log; then
    loud_fail "Pi failure was not surfaced with structured selected-route diagnostics:
$(cat evaluate-no-fallback.log)"
fi

cat >>.wg/config.toml <<'TOML'

[[execution.fallbacks]]
primary = "pi:openai-codex:gpt-5.6-terra"
models = ["pi:openai-codex:gpt-5.6-sol"]
TOML

: >"$scratch/pi.log"
if ! wg evaluate run pi-weak-tier-target >evaluate-fallback.log 2>&1; then
    loud_fail "explicit same-system Pi fallback failed:
$(cat evaluate-fallback.log)"
fi

expected_calls=$'pi:gpt-5.6-terra\npi:gpt-5.6-sol'
actual_calls=$(cat "$scratch/pi.log")
if [[ "$actual_calls" != "$expected_calls" ]]; then
    loud_fail "Pi fallback order mismatch; expected Terra then Sol, got:
$actual_calls"
fi
if [[ -f "$scratch/claude.log" ]]; then
    loud_fail "Claude ran during explicit Pi fallback:
$(cat evaluate-fallback.log)"
fi
if ! grep -q 'fallback_route="pi:openai-codex:gpt-5.6-sol" outcome=success' evaluate-fallback.log; then
    loud_fail "successful same-system fallback diagnostic missing:
$(cat evaluate-fallback.log)"
fi

echo "PASS: failing Pi stayed on Pi; explicit same-system fallback ran Terra -> Sol; Claude never ran"
exit 0
