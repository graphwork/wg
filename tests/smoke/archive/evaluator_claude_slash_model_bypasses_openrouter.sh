#!/usr/bin/env bash
# Scenario: evaluator_claude_slash_model_bypasses_openrouter
#
# Regression: an agency evaluator configured with the registry/OpenRouter-style
# Claude model ID `anthropic/claude-haiku-4-5` routed through native OpenRouter
# without a key, then fell back to the Claude CLI with that same invalid model
# string. The evaluator should call the Claude CLI directly with `--model haiku`.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

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

if [[ "$model" != "haiku" ]]; then
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

if ! wg init --route openrouter >init.log 2>&1; then
    loud_fail "wg init --route openrouter failed: $(tail -10 init.log)"
fi

awk '
    $0 == "[models.evaluator]" { in_evaluator = 1; print; next }
    in_evaluator && $1 == "model" {
        print "model = \"anthropic/claude-haiku-4-5\""
        in_evaluator = 0
        next
    }
    in_evaluator && $0 ~ /^\[/ { in_evaluator = 0 }
    { print }
' .wg/config.toml >config.new
mv config.new .wg/config.toml

if ! wg add "Evaluator slash smoke" --id evaluator-slash-target >add.log 2>&1; then
    loud_fail "wg add failed: $(tail -5 add.log)"
fi

if ! wg done evaluator-slash-target >done.log 2>&1; then
    loud_fail "wg done target failed: $(tail -10 done.log)"
fi

if ! wg evaluate run evaluator-slash-target >evaluate.log 2>&1; then
    loud_fail "wg evaluate failed:
$(cat evaluate.log)"
fi

if grep -qi "native openrouter call failed" evaluate.log; then
    loud_fail "evaluation detoured through native OpenRouter:
$(cat evaluate.log)"
fi

if grep -qi "unexpected claude model" evaluate.log; then
    loud_fail "Claude CLI received an unnormalized model:
$(cat evaluate.log)"
fi

echo "PASS: evaluator slash-form Claude model used claude CLI alias without OpenRouter"
exit 0
