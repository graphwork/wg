#!/usr/bin/env bash
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch="$(make_scratch)"
bindir="$scratch/bin"
fake_home="$scratch/home"
project="$scratch/project"
mkdir -p "$bindir" "$fake_home/.config/workgraph" "$project"
ln -s "$(command -v wg)" "$bindir/wg"
: >"$fake_home/.config/workgraph/config.toml"

(
    cd "$project" || exit 1
    env HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" PATH="$bindir" \
        wg init -m claude:opus --no-agency >/dev/null 2>&1
) || loud_fail "wg init failed"

cat >"$project/.wg/config.toml" <<'TOML'
[agent]
model = "pi:openrouter/test/model"

[models]
default = { model = "pi:openrouter/test/model" }
task_agent = { model = "pi:openrouter/test/model" }
TOML

out="$scratch/lint.out"
(
    cd "$project" || exit 1
    env HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" PATH="$bindir" \
        WG_PI_PLUGIN_DIR="$scratch/does-not-exist" \
        wg config lint --local >"$out" 2>&1
) || loud_fail "wg config lint command failed unexpectedly: $(cat "$out")"

grep -q "configured model route targets the .*pi.* executor" "$out" || \
    loud_fail "config lint did not reject/warn for unsatisfied pi route: $(cat "$out")"
grep -q "neither a .*pi.* binary nor the Node host bundle" "$out" || \
    loud_fail "config lint warning did not mention both missing transports: $(cat "$out")"

echo "PASS: config lint rejects an unsatisfied pi route when no pi binary or Node host bundle is present"
