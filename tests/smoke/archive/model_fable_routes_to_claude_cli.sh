#!/usr/bin/env bash
# Scenario: model_fable_routes_to_claude_cli
#
# Pins enable-fable: Fable 5 is a first-class claude model.
#
#   1. `wg model list` and `wg models list` both surface the model — the
#      config builtin registry exposes `fable` / `claude:fable` with the full
#      CLI id `claude-fable-5`, and the models.yaml registry exposes
#      `anthropic/claude-fable-5` in the frontier tier.
#   2. `claude:fable` routes to the claude CLI handler (self-auth, no key) and
#      the friendly alias is expanded to `--model claude-fable-5` — the claude
#      CLI has no bare `fable` shortcut, so wg must expand it. Verified with a
#      fake `claude` binary that fails loudly on any other model string.
#
# Credential-free: the fake `claude` on PATH stands in for the real CLI, so
# this runs deterministically in CI with no Claude Max / API key.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

# ── Fake claude that asserts the expanded Fable CLI id ───────────────
fake_bin="$scratch/fake-bin"
mkdir -p "$fake_bin"
cat >"$fake_bin/claude" <<'SH'
#!/usr/bin/env bash
set -u

model=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --model)
            shift
            model="${1:-}"
            ;;
    esac
    shift || true
done

cat >/dev/null

if [[ "$model" != "claude-fable-5" ]]; then
    echo "unexpected claude model: $model" >&2
    exit 42
fi

printf '%s\n' '{"result":"{\"score\":1.0,\"dimensions\":{\"correctness\":1.0},\"notes\":\"smoke pass\"}","usage":{"input_tokens":1,"output_tokens":1}}'
SH
chmod +x "$fake_bin/claude"
export PATH="$fake_bin:$PATH"

unset OPENROUTER_API_KEY
unset OPENAI_API_KEY
unset ANTHROPIC_API_KEY

if ! wg init >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -10 init.log)"
fi

# ── (1) Registry surfaces ────────────────────────────────────────────
if ! wg model list >model_list.log 2>&1; then
    loud_fail "wg model list failed: $(tail -10 model_list.log)"
fi
if ! grep -q "claude-fable-5" model_list.log; then
    loud_fail "wg model list does not show claude-fable-5:
$(cat model_list.log)"
fi
if ! grep -Eq "^ *(claude:)?fable\b" model_list.log; then
    loud_fail "wg model list does not show the fable / claude:fable aliases:
$(cat model_list.log)"
fi

if ! wg models list >models_list.log 2>&1; then
    loud_fail "wg models list failed: $(tail -10 models_list.log)"
fi
if ! grep -q "anthropic/claude-fable-5" models_list.log; then
    loud_fail "wg models list does not show anthropic/claude-fable-5:
$(cat models_list.log)"
fi

# ── (2) Routing + alias expansion through the real wg dispatch path ──
# An explicit evaluator override pins claude:fable; agency routes it to the
# claude CLI and MUST hand the fake claude `--model claude-fable-5`.
awk '
    $0 == "[models.evaluator]" { in_evaluator = 1; print; next }
    in_evaluator && $1 == "model" {
        print "model = \"claude:fable\""
        in_evaluator = 0
        next
    }
    in_evaluator && $0 ~ /^\[/ { in_evaluator = 0 }
    { print }
' .wg/config.toml >config.new
if ! grep -q '\[models.evaluator\]' config.new; then
    printf '\n[models.evaluator]\nmodel = "claude:fable"\n' >>config.new
fi
mv config.new .wg/config.toml

if ! wg add "Fable routing smoke" --id fable-route-target >add.log 2>&1; then
    loud_fail "wg add failed: $(tail -5 add.log)"
fi

if ! wg done fable-route-target >done.log 2>&1; then
    loud_fail "wg done target failed: $(tail -10 done.log)"
fi

if ! wg evaluate run fable-route-target >evaluate.log 2>&1; then
    loud_fail "wg evaluate failed:
$(cat evaluate.log)"
fi

if grep -qi "unexpected claude model" evaluate.log; then
    loud_fail "Claude CLI received an unexpected model (expected claude-fable-5):
$(cat evaluate.log)"
fi

echo "PASS: fable is a first-class claude model — registries show claude-fable-5 and claude:fable expands to --model claude-fable-5"
exit 0
