#!/usr/bin/env bash
# Scenario: pi_plugin_toggle_commands
#
# Pins PLUGIN ON/OFF AS COMMANDS (wg-plugin-toggle): the pi plugin exposes
# `/wg on`, `/wg off`, and `/wg status` so a human NEVER hand-edits
# ~/.pi/agent/settings.json to enable/disable the plugin.
#
# Human-flow proof (credential-free): the built plugin's REAL registered `wg`
# command handler is invoked exactly as pi would invoke it (fake pi + fake ctx),
# driving the REAL `wg` binary (`pi-plugin install` / `pi-plugin disable`) into
# an isolated HOME's settings.json. `on` wires exactly one managed entry, `off`
# removes exactly that entry while preserving every unrelated extension, `status`
# prints one compact truthful line, and both are idempotent.
#
# Candidate binary preferred (WG_SMOKE_CANDIDATE_BIN → $CARGO_TARGET_DIR/debug/wg
# → PATH `wg`); an older binary without `pi-plugin disable` SKIPs LOUDLY.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v node >/dev/null 2>&1 || loud_skip "MISSING NODE" "node is required for the pi-plugin toggle contract"

REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null || echo "$HERE/../..")"
plugin="$REPO_ROOT/worksgood-pi"

if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" && -x "$WG_SMOKE_CANDIDATE_BIN" ]]; then
    WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
elif [[ -x "${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wg" ]]; then
    WG_BIN="${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wg"
else
    WG_BIN="$(command -v wg)"
fi

# The candidate must carry the new `disable` verb. An older installed binary
# cannot exercise this contract: skip LOUDLY rather than assert the wrong thing.
if ! "$WG_BIN" pi-plugin disable --help >/dev/null 2>&1; then
    loud_skip "OLD WG BINARY" \
        "$WG_BIN lacks 'pi-plugin disable'; run 'wg dev-sync' or pass WG_SMOKE_CANDIDATE_BIN"
fi

# Require the built plugin artifact (npm test/build produce pi-worksgood/).
if [ ! -f "$plugin/pi-worksgood/commands.js" ] || [ ! -f "$plugin/pi-worksgood/wg-backend.js" ]; then
    command -v npm >/dev/null 2>&1 || loud_skip "MISSING NPM" "npm needed to build worksgood-pi/pi-worksgood"
    if [ ! -d "$plugin/node_modules" ]; then
        npm --prefix "$plugin" ci >/tmp/pi-worksgood-toggle-npm-ci.log 2>&1 || \
            loud_skip "PI PLUGIN DEPS UNAVAILABLE" "npm ci failed: $(tail -20 /tmp/pi-worksgood-toggle-npm-ci.log)"
    fi
    npm --prefix "$plugin" run build >/tmp/pi-worksgood-toggle-build.log 2>&1 || \
        loud_fail "pi-worksgood build failed: $(tail -40 /tmp/pi-worksgood-toggle-build.log)"
fi

scratch="$(make_scratch)"
fake_home="$scratch/home"
cache="$scratch/cache"
mkdir -p "$fake_home" "$cache"
settings="$fake_home/.pi/agent/settings.json"

log="$scratch/handler.log"
if ! node --input-type=module - "$WG_BIN" "$plugin" "$fake_home" "$cache" \
        >"$log" 2>&1 <<'NODE'
const [wgBin, plugin, fakeHome, cache] = process.argv.slice(2);

const { registerWgCommands } = await import(`${plugin}/pi-worksgood/commands.js`);
const { WgBackend } = await import(`${plugin}/pi-worksgood/wg-backend.js`);

// The human-flow executor: the plugin always shells `wg`; we resolve that to the
// candidate binary and run it with the isolated HOME/cache, exactly as a console
// session would (minus pi's own keystroke dispatch, which is pi's code). The
// child env is MINIMAL (no inherited WG_* worker/admin vars) so the console
// lifecycle verbs are not refused by the worker gate.
const childEnv = {
  PATH: process.env.PATH ?? "",
  HOME: fakeHome,
  XDG_CACHE_HOME: cache,
  TMPDIR: process.env.TMPDIR ?? "/tmp",
  LANG: process.env.LANG ?? "C",
};
const host = {
  exec: (command, args, opts) =>
    new Promise((resolve) => {
      import("node:child_process").then(({ execFile }) => {
        execFile(
          command === "wg" ? wgBin : command,
          args,
          { env: childEnv, maxBuffer: 1 << 22, signal: opts?.signal },
          (err, stdout, stderr) => {
            resolve({ stdout: stdout ?? "", stderr: stderr ?? "", code: err && typeof err.code === "number" ? err.code : err ? 1 : 0, killed: false });
          },
        );
      });
    }),
};
const backend = new WgBackend(host, {});
const commands = new Map();
const pi = {
  registerCommand: (name, opts) => commands.set(name, opts),
  on: () => {},
  sendUserMessage: () => {},
  setModel: async () => true,
};
registerWgCommands(pi, backend);
const wg = commands.get("wg");
const assert = (cond, msg) => { if (!cond) throw new Error(msg); };

// 1. Completion surface includes the lifecycle verbs.
const subs = new Set((wg.getArgumentCompletions?.("") ?? []).map((c) => c.value));
for (const want of ["on", "off", "status"]) assert(subs.has(want), `completions missing ${want}`);

const fs = await import("node:fs/promises");
const settingsPath = `${fakeHome}/.pi/agent/settings.json`;
const notes = [];
const ctx = { hasUI: true, ui: { notify: (t, type) => notes.push({ t, type }) }, signal: undefined };
const last = () => notes[notes.length - 1].t;
const all = () => notes.map((n) => n.t).join("\n");

// 2. `/wg on` wires exactly one managed entry.
await wg.handler("on", ctx);
let raw = await fs.readFile(settingsPath, "utf8");
let val = JSON.parse(raw);
assert(Array.isArray(val.extensions) && val.extensions.length === 1, `on should wire exactly one entry, got ${JSON.stringify(val.extensions)}`);
assert(val.extensions[0].includes("worksgood-pi"), `wired entry is not the managed path: ${val.extensions[0]}`);
assert(/restart pi to load the updated plugin/.test(all()), `on did not instruct a restart: ${all()}`);

// 3. Add an unrelated user extension; `/wg off` must remove ONLY ours.
val.extensions.unshift("/home/u/my-ext/index.ts");
await fs.writeFile(settingsPath, JSON.stringify(val, null, 2));
await wg.handler("off", ctx);
val = JSON.parse(await fs.readFile(settingsPath, "utf8"));
assert(val.extensions.length === 1 && val.extensions[0] === "/home/u/my-ext/index.ts",
  `off did not preserve exactly the unrelated entry: ${JSON.stringify(val.extensions)}`);
assert(/Disabled pi-worksgood/.test(last()), `off output is not a confirmation: ${last()}`);

// 4. `/wg off` is idempotent.
await wg.handler("off", ctx);
assert(/nothing to remove|already disabled/.test(last()), `second off is not an honest no-op: ${last()}`);

// 5. `/wg status` is one compact truthful line.
await wg.handler("status", ctx);
const statusLine = last();
assert(!statusLine.includes("\n"), `status is not one line: ${statusLine}`);
assert(statusLine.startsWith("WG · plugin: "), `status shape wrong: ${statusLine}`);
assert(/plugin: (current|stale|missing|unknown)/.test(statusLine), `status plugin token missing: ${statusLine}`);
assert(/graph: (present|absent)/.test(statusLine), `status graph token missing: ${statusLine}`);
assert(/fleet: /.test(statusLine), `status fleet token missing: ${statusLine}`);

// 6. `/wg on` after an `off` re-wires (a real change), then a SECOND `on` is an
//    honest no-op and never duplicates the managed entry.
await wg.handler("on", ctx);
assert(/restart pi to load the updated plugin/.test(last()), `re-on after off should re-wire + ask for restart: ${last()}`);
val = JSON.parse(await fs.readFile(settingsPath, "utf8"));
assert(val.extensions.length === 2, `re-on must not duplicate the managed entry: ${JSON.stringify(val.extensions)}`);
await wg.handler("on", ctx);
assert(/already up to date|nothing changed/.test(all()), `second consecutive on is not an honest no-op: ${last()}`);
assert(/already current/.test(last()), `second consecutive on should say it is already current: ${last()}`);
val = JSON.parse(await fs.readFile(settingsPath, "utf8"));
assert(val.extensions.length === 2, `idempotent on must not duplicate the managed entry: ${JSON.stringify(val.extensions)}`);

console.log("pi_plugin_toggle_commands: handler flow OK");
NODE
then
    loud_fail "pi /wg on|off|status human-flow contract failed: $(tail -60 "$log")"
fi

[ -f "$settings" ] || loud_fail "settings.json was not created at $settings"
grep -q "handler flow OK" "$log" || loud_fail "handler flow did not complete: $(tail -40 "$log")"

echo "pi_plugin_toggle_commands: PASS (wg=$WG_BIN)"
exit 0
