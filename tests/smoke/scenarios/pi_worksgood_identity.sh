#!/usr/bin/env bash
# Public-identity and migration contract for the WorksGood Pi integration.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v node >/dev/null 2>&1 || loud_skip "MISSING NODE" "node is required to inspect package metadata"

repo="$(cd "$HERE/../../.." && pwd)"
component="$repo/worksgood-pi"
scratch=$(make_scratch)

# Fresh package metadata and lockfile use the canonical npm identity. The
# extension entry's parent is the exact label Pi renders at startup.
node --input-type=module - "$component" <<'NODE' || loud_fail "canonical package identity check failed"
import { readFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";

const root = process.argv[2];
const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
const lock = JSON.parse(readFileSync(join(root, "package-lock.json"), "utf8"));
if (pkg.name !== "@worksgood/pi") throw new Error(`package name: ${pkg.name}`);
if (lock.name !== "@worksgood/pi" || lock.packages?.[""]?.name !== "@worksgood/pi") {
  throw new Error("package-lock identity drift");
}
if (pkg.description !== "Connect Pi agents to WorksGood graphs, tools, and context.") {
  throw new Error(`description: ${pkg.description}`);
}
if (pkg.pi?.extensions?.length !== 1) throw new Error("expected exactly one Pi extension");
const entry = pkg.pi.extensions[0];
if (basename(dirname(entry)) !== "pi-worksgood" || basename(entry) !== "index.js") {
  throw new Error(`Pi display entry: ${entry}`);
}
NODE

compat=$(wg pi-plugin compat-version 2>/dev/null) || loud_fail "compat-version failed"
fresh_home="$scratch/fresh-home"
fresh_cache="$scratch/fresh-cache"
mkdir -p "$fresh_home" "$fresh_cache"

fresh_out=$(env HOME="$fresh_home" XDG_CACHE_HOME="$fresh_cache" \
    WG_PI_PLUGIN_FORCE_CACHE=1 wg pi-plugin install 2>&1) \
    || loud_fail "fresh pi-worksgood install failed: $fresh_out"
entry="$fresh_cache/wg/worksgood-pi/$compat/pi-worksgood/index.js"
[ -f "$entry" ] || loud_fail "missing canonical embedded entry: $entry"
grep -qF "$entry" "$fresh_home/.pi/agent/settings.json" \
    || loud_fail "fresh settings do not wire pi-worksgood"
grep -q "Installed pi-worksgood (npm: @worksgood/pi" <<<"$fresh_out" \
    || loud_fail "fresh install output leaked the old product identity: $fresh_out"

# Seed both legacy installation forms. One Console ensure must preserve package
# records/version pins and unrelated object configuration, disable package
# extension copies, replace old managed paths with one compat-locked path, and
# report the accepted legacy install once. This keeps an offline console
# functional without duplicate loading. A second ensure is byte-identical and
# does not repeat the compatibility notice.
legacy_home="$scratch/legacy-home"
legacy_cache="$scratch/legacy-cache"
mkdir -p "$legacy_home/.pi/agent" "$legacy_cache"
cat >"$legacy_home/.pi/agent/settings.json" <<'JSON'
{
  "packages": [
    {
      "source": "npm:@worksgood/wg-pi-plugin@0.1.0",
      "autoload": false,
      "extensions": ["pi-worksgood/index.js"]
    },
    "npm:@worksgood/pi@0.1.0",
    { "source": "npm:pi-web-access@2.0.0", "extensions": [] }
  ],
  "extensions": [
    "/tmp/user-extension/index.ts",
    "/tmp/cache/wg/pi-plugin/0.1.1/dist/index.js"
  ]
}
JSON

legacy_out=$(env HOME="$legacy_home" XDG_CACHE_HOME="$legacy_cache" \
    WG_PI_PLUGIN_FORCE_CACHE=1 wg pi-plugin install 2>&1) \
    || loud_fail "legacy migration failed: $legacy_out"
[ "$(grep -c "Compatibility: retained the legacy" <<<"$legacy_out")" -eq 1 ] \
    || loud_fail "legacy compatibility notice must appear exactly once: $legacy_out"
grep -q "pi remove npm:@worksgood/wg-pi-plugin" <<<"$legacy_out" \
    || loud_fail "legacy notice is not actionable: $legacy_out"

legacy_entry="$legacy_cache/wg/worksgood-pi/$compat/pi-worksgood/index.js"
[ -f "$legacy_entry" ] || loud_fail "missing legacy migration cache entry: $legacy_entry"

node --input-type=module - "$legacy_home/.pi/agent/settings.json" "$legacy_entry" <<'NODE' \
    || loud_fail "legacy settings migration contract failed"
import { readFileSync } from "node:fs";
const value = JSON.parse(readFileSync(process.argv[2], "utf8"));
const managedEntry = process.argv[3];
if (value.packages.length !== 3) throw new Error(`package records lost: ${JSON.stringify(value.packages)}`);
if (value.packages[0].source !== "npm:@worksgood/wg-pi-plugin@0.1.0") throw new Error("legacy version pin was not retained");
if (value.packages[0].autoload !== false || value.packages[0].extensions.length !== 0) {
  throw new Error("legacy package config was not preserved/disabled");
}
if (value.packages[1].source !== "npm:@worksgood/pi@0.1.0" || value.packages[1].extensions.length !== 0) {
  throw new Error("canonical npm package copy was not disabled");
}
if (value.packages[2].source !== "npm:pi-web-access@2.0.0" || value.packages[2].extensions.length !== 0) {
  throw new Error("unrelated package configuration changed");
}
if (value.extensions.join("\n") !== `/tmp/user-extension/index.ts\n${managedEntry}`) {
  throw new Error(`duplicate or lost extension: ${JSON.stringify(value.extensions)}`);
}
if (JSON.stringify(value.extensions).includes("/wg/pi-plugin/")) {
  throw new Error("legacy managed path remains in settings");
}
NODE

before=$(sha256sum "$legacy_home/.pi/agent/settings.json" | cut -d' ' -f1)
legacy_second=$(env HOME="$legacy_home" XDG_CACHE_HOME="$legacy_cache" \
    WG_PI_PLUGIN_FORCE_CACHE=1 wg pi-plugin install 2>&1) \
    || loud_fail "second migration install failed: $legacy_second"
after=$(sha256sum "$legacy_home/.pi/agent/settings.json" | cut -d' ' -f1)
[ "$before" = "$after" ] || loud_fail "second install changed migrated settings"
! grep -q "Compatibility: migrated legacy" <<<"$legacy_second" \
    || loud_fail "compatibility notice repeated after migration: $legacy_second"
! grep -q "Compatibility: retained the legacy" <<<"$legacy_second" \
    || loud_fail "legacy compatibility notice repeated after migration: $legacy_second"

# Drive the real human Pi startup flow. Pi derives the compact extension label
# from the parent of index.js; the collapsed startup list must therefore be one
# exact `pi-worksgood` item, never an implementation directory or old brand.
if command -v pi >/dev/null 2>&1 && command -v tmux >/dev/null 2>&1; then
    legacy_rpc=$(printf '{"type":"get_commands"}\n' | timeout 20 \
        env HOME="$legacy_home" XDG_CACHE_HOME="$legacy_cache" PI_OFFLINE=1 \
            WG_PI_PLUGIN_COMPAT_VERSION="$compat" \
            pi --mode rpc --no-session 2>&1) \
        || loud_fail "offline legacy Pi startup failed after migration: $legacy_rpc"
    [ "$(grep -o '"name":"wg"' <<<"$legacy_rpc" | wc -l)" -eq 1 ] \
        || loud_fail "offline legacy Pi must load exactly one /wg command: $legacy_rpc"
    [ "$(grep -o '"name":"wg-model"' <<<"$legacy_rpc" | wc -l)" -eq 1 ] \
        || loud_fail "offline legacy Pi must load exactly one /wg-model command: $legacy_rpc"

    session="pi-worksgood-identity-$$"
    tmux new-session -d -s "$session" \
        "env HOME='$fresh_home' XDG_CACHE_HOME='$fresh_cache' PI_OFFLINE=1 WG_PI_PLUGIN_COMPAT_VERSION='$compat' pi --no-session"
    startup=""
    for _ in $(seq 1 40); do
        startup=$(tmux capture-pane -p -t "$session" -S -120 2>/dev/null || true)
        grep -q "\[Extensions\]" <<<"$startup" && break
        sleep 0.1
    done
    tmux send-keys -t "$session" C-d 2>/dev/null || true
    tmux kill-session -t "$session" 2>/dev/null || true

    extension_line=$(awk '/\[Extensions\]/{getline; gsub(/^[[:space:]]+|[[:space:]]+$/, ""); print; exit}' <<<"$startup")
    [ "$extension_line" = "pi-worksgood" ] \
        || loud_fail "Pi startup extension list must display exactly pi-worksgood; got '$extension_line'. Screen: $startup"
    ! grep -Eqi '^([[:space:]]*)(dist|wg-pi-plugin|worksgood)([[:space:]]*)$' <<<"$startup" \
        || loud_fail "Pi startup leaked an implementation/legacy label: $startup"

    mismatch=$(printf '{"type":"shutdown"}\n' | timeout 20 \
        env HOME="$fresh_home" XDG_CACHE_HOME="$fresh_cache" PI_OFFLINE=1 \
            WG_PI_PLUGIN_COMPAT_VERSION="9.9.9-identity-smoke" \
            pi --mode rpc -e "$entry" -ne 2>&1 || true)
    grep -q "WorksGood Pi integration compat mismatch" <<<"$mismatch" \
        || loud_fail "compat mismatch did not name WorksGood clearly: $mismatch"
else
    echo "pi_worksgood_identity: live Pi startup sub-check skipped (pi or tmux missing)" >&2
fi

echo "pi_worksgood_identity: PASS"
