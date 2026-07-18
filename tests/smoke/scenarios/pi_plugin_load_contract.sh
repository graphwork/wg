#!/usr/bin/env bash
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v npm >/dev/null 2>&1 || loud_skip "MISSING NPM" "npm is required for the pi-plugin load contract"
command -v node >/dev/null 2>&1 || loud_skip "MISSING NODE" "node is required for the pi-plugin load contract"

repo="$(cd "$HERE/../../.." && pwd)"
plugin="$repo/worksgood-pi"
[ -f "$plugin/package-lock.json" ] || loud_fail "missing worksgood-pi/package-lock.json"

if [ ! -d "$plugin/node_modules" ]; then
    npm --prefix "$plugin" ci >/tmp/pi-worksgood-npm-ci.log 2>&1 || \
        loud_skip "PI PLUGIN DEPS UNAVAILABLE" "npm ci failed: $(tail -20 /tmp/pi-worksgood-npm-ci.log)"
fi

npm --prefix "$plugin" test >/tmp/pi-worksgood-test.log 2>&1 || \
    loud_fail "pi-worksgood npm test failed: $(tail -80 /tmp/pi-worksgood-test.log)"

npm --prefix "$plugin" run selftest >/tmp/pi-worksgood-selftest.log 2>&1 || \
    loud_fail "pi-worksgood host selftest failed: $(tail -80 /tmp/pi-worksgood-selftest.log)"

node --input-type=module - "$plugin" <<'NODE' >/tmp/pi-worksgood-contract.log 2>&1 || \
    loud_fail "pi-worksgood registration contract smoke failed: $(cat /tmp/pi-worksgood-contract.log)"
const plugin = process.argv[2];
const mod = await import(`${plugin}/pi-worksgood/index.js`);
const lines = mod.renderWidget([
  { id: "task-a", title: "Alpha" },
  { id: "task-b", title: "Beta" },
]);
if (Array.isArray(lines) && lines.length !== 0) {
  throw new Error(`passive ready-task widget should be disabled, got: ${JSON.stringify(lines)}`);
}
for (const mode of ["rpc", "tui", "print"]) {
  const pi = {
    registerTool: (tool) => { pi.tools.push(tool.name); },
    registerCommand: (name) => { pi.commands.push(name); },
    registerProvider: () => {},
    on: (event) => { pi.events.push(event); },
    exec: async () => ({ stdout: "[]", stderr: "", code: 0, killed: false }),
    tools: [],
    commands: [],
    events: [],
  };
  process.env.PI_MODE = mode;
  mod.default(pi);
  for (const tool of ["wg_ready", "wg_show", "wg_add", "wg_done", "wg_fail", "wg_msg_send", "wg_msg_read", "wg_run"]) {
    if (!pi.tools.includes(tool)) throw new Error(`${mode}: missing tool ${tool}`);
  }
  for (const command of ["wg", "wg-model"]) {
    if (!pi.commands.includes(command)) throw new Error(`${mode}: missing command ${command}`);
  }
  for (const event of ["session_start", "model_select", "session_shutdown"]) {
    if (!pi.events.includes(event)) throw new Error(`${mode}: missing event ${event}`);
  }
  for (const event of ["turn_end"]) {
    if (pi.events.includes(event)) throw new Error(`${mode}: should not subscribe to passive ready UI event ${event}`);
  }
}
NODE

echo "PASS: pi plugin loads/registers tools commands and model bridge without passive ready-task UI hooks"
