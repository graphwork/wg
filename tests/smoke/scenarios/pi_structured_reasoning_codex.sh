#!/usr/bin/env bash
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
fake_home="$scratch/home"
fake_bin="$scratch/bin"
mkdir -p "$fake_home/.config/workgraph" "$fake_bin"
: >"$fake_home/.config/workgraph/config.toml"

cat >"$fake_bin/pi" <<'SH'
#!/usr/bin/env bash
{
    printf '%s\n' "$0"
    printf '%s\n' "$@"
} >>"${PI_ARG_LOG:?}"
cat >/dev/null || true
exit 0
SH
chmod +x "$fake_bin/pi"

cd "$scratch"
wgd="$scratch/.wg"
arg_log="$scratch/pi-args.log"

run_wg() {
    env -u WG_DIR -u WG_MODEL -u WG_EXECUTOR_TYPE -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" \
        PATH="$fake_bin:$PATH" PI_ARG_LOG="$arg_log" \
        wg --dir "$wgd" "$@"
}

if ! run_wg init --no-agency >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -20 init.log)"
fi

if ! run_wg config --local -m pi:openai-codex:gpt-5.6-sol --reasoning high --no-reload \
        >config-set.log 2>&1; then
    loud_fail "wg config model/reasoning failed: $(cat config-set.log)"
fi

models_out="$(run_wg config --models 2>&1)" || \
    loud_fail "wg config --models failed: $models_out"
grep -qF "pi:openai-codex:gpt-5.6-sol" <<<"$models_out" || \
    loud_fail "config --models did not show canonical Pi Codex route:\n$models_out"
grep -qF "high" <<<"$models_out" || \
    loud_fail "config --models did not show resolved reasoning high:\n$models_out"

if ! run_wg add "Pi Codex reasoning probe" --id pi-codex-reasoning \
        --model pi:openai-codex:gpt-5.6-sol --reasoning high \
        -d "Smoke probe for structured Pi reasoning routing." >add.log 2>&1; then
    loud_fail "wg add failed: $(cat add.log)"
fi

show_out="$(run_wg show pi-codex-reasoning 2>&1)" || \
    loud_fail "wg show failed: $show_out"
grep -qF "Model: pi:openai-codex:gpt-5.6-sol" <<<"$show_out" || \
    loud_fail "wg show did not preserve configured model:\n$show_out"
grep -qF "Reasoning: high" <<<"$show_out" || \
    loud_fail "wg show did not expose reasoning high:\n$show_out"

spawn_out="$scratch/spawn.out"
if ! run_wg spawn pi-codex-reasoning --executor pi >"$spawn_out" 2>&1; then
    loud_fail "wg spawn failed: $(cat "$spawn_out")"
fi

for _ in $(seq 1 40); do
    [[ -s "$arg_log" ]] && break
    sleep 0.25
done
[[ -s "$arg_log" ]] || \
    loud_fail "fake pi was not invoked. spawn output:\n$(cat "$spawn_out")"

python3 - "$arg_log" <<'PY'
import sys
from pathlib import Path

args = Path(sys.argv[1]).read_text().splitlines()[1:]

def require_pair(flag, value):
    for i, arg in enumerate(args[:-1]):
        if arg == flag and args[i + 1] == value:
            return
    raise SystemExit(f"missing {flag} {value!r} in pi argv: {args!r}")

require_pair("--provider", "openai-codex")
require_pair("--model", "gpt-5.6-sol")
require_pair("--thinking", "high")

if "pi:openai-codex:gpt-5.6-sol(high)" in args:
    raise SystemExit(f"reasoning leaked into model string: {args!r}")
PY

echo "PASS: Pi Codex route kept model/reasoning separate and spawned pi with --provider openai-codex --model gpt-5.6-sol --thinking high"
