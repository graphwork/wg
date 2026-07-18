#!/usr/bin/env bash
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

run_case() {
    local topology="$1"
    local scratch bindir fake_home project create_out cid handler_out outbox reply log
    scratch="$(make_scratch)"
    bindir="$scratch/bin"
    fake_home="$scratch/home"
    project="$scratch/project"
    mkdir -p "$bindir" "$fake_home/.config/workgraph" "$project"
    : >"$fake_home/.config/workgraph/config.toml"

    cat >"$bindir/pi" <<'FAKE_PI'
#!/usr/bin/env bash
set -euo pipefail
log="${FAKE_PI_LOG:?}"
printf 'PI_START args=%s stdin_tty=%s stdout_tty=%s\n' "$*" "$([[ -t 0 ]] && echo yes || echo no)" "$([[ -t 1 ]] && echo yes || echo no)" >>"$log"
if [[ "$*" != *"--mode rpc"* ]]; then
  printf 'PI_BAD_MODE\n' >>"$log"
  exit 2
fi
while IFS= read -r line; do
  printf 'PI_LINE %s\n' "$line" >>"$log"
  if [[ "$line" == *'"type":"shutdown"'* ]]; then
    exit 0
  fi
  if [[ "$line" == *'"type":"prompt"'* ]]; then
    printf '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"rpc plugin reply"}}\n'
    printf '{"type":"agent_end"}\n'
  fi
done
FAKE_PI
    chmod +x "$bindir/pi"

    cat >"$bindir/node" <<'FAKE_NODE'
#!/usr/bin/env bash
set -euo pipefail
script="$1"
shift || true
exec "$script" "$@"
FAKE_NODE
    chmod +x "$bindir/node"

    plugin_dir="$scratch/worksgood-pi"
    # The explicit override models a checked-out development component. The
    # Node-host topology intentionally requires peer dependencies to exist.
    mkdir -p "$plugin_dir/host" "$plugin_dir/pi-worksgood" "$plugin_dir/node_modules"
    : >"$plugin_dir/pi-worksgood/index.js"
    cat >"$plugin_dir/host/wg-pi-host.mjs" <<'FAKE_HOST'
#!/usr/bin/env bash
set -euo pipefail
log="${FAKE_PI_LOG:?}"
printf 'NODE_HOST_START provider=%s model=%s\n' "${WG_PI_PROVIDER:-}" "${WG_PI_MODEL:-}" >>"$log"
printf '{"type":"ready"}\n'
while IFS= read -r line; do
  printf 'NODE_HOST_LINE %s\n' "$line" >>"$log"
  if [[ "$line" == *'"type":"prompt"'* ]]; then
    printf '{"type":"delta","text":"node plugin reply"}\n'
    printf '{"type":"turn_end"}\n'
  fi
done
FAKE_HOST
    chmod +x "$plugin_dir/host/wg-pi-host.mjs"

    (
        cd "$project" || exit 1
        env HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" PATH="$bindir:$PATH" \
            wg init -m claude:opus --no-agency >/dev/null 2>&1
    ) || loud_fail "$topology: wg init failed"

    create_out="$scratch/create.out"
    (
        cd "$project" || exit 1
        env HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" PATH="$bindir:$PATH" \
            wg chat create >"$create_out" 2>&1
    ) || loud_fail "$topology: wg chat create failed: $(cat "$create_out")"
    cid="$(python3 -c "import json;print(json.load(open('$create_out'))['coordinator_id'])" 2>/dev/null)"
    [ -n "$cid" ] || cid="$(grep -oE 'Created chat [0-9]+' "$create_out" | grep -oE '[0-9]+' | head -1)"
    [ -n "$cid" ] || loud_fail "$topology: could not parse chat id: $(cat "$create_out")"

    (
        cd "$project" || exit 1
        env HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" PATH="$bindir:$PATH" \
            wg chat send "$cid" "exercise pi plugin transport" >/dev/null 2>&1
    ) || loud_fail "$topology: wg chat send failed"

    log="$scratch/$topology.log"
    handler_out="$scratch/$topology-handler.out"
    (
        cd "$project" || exit 1
        env HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" PATH="$bindir:$PATH" \
            WG_PI_PLUGIN_DIR="$plugin_dir" WG_PI_TOPOLOGY="$topology" FAKE_PI_LOG="$log" \
            timeout 8s wg pi-handler --chat "$cid" --model pi:openrouter/test/model \
            >"$handler_out" 2>&1
    )
    rc=$?
    if [ "$rc" -ne 124 ] && [ "$rc" -ne 0 ]; then
        loud_fail "$topology: pi-handler exited unexpectedly ($rc): $(cat "$handler_out") log: $(cat "$log" 2>/dev/null)"
    fi

    outbox="$(find "$project/.wg/chat" -maxdepth 3 -name outbox.jsonl -type f -size +0c 2>/dev/null | head -1)"
    [ -n "$outbox" ] || loud_fail "$topology: no outbox reply. handler: $(cat "$handler_out") log: $(cat "$log" 2>/dev/null)"
    reply="$(cat "$outbox")"
    case "$topology" in
        rpc)
            grep -q "rpc plugin reply" <<<"$reply" || loud_fail "rpc: missing fake RPC reply: $reply"
            grep -q "PI_START .*--mode rpc" "$log" || loud_fail "rpc: fake pi did not receive --mode rpc: $(cat "$log")"
            grep -q "stdin_tty=no stdout_tty=no" "$log" || loud_fail "rpc: fake pi was not headless: $(cat "$log")"
            ;;
        node)
            grep -q "node plugin reply" <<<"$reply" || loud_fail "node: missing fake node reply: $reply"
            grep -q "NODE_HOST_START provider=openrouter model=test/model" "$log" || loud_fail "node: host did not receive provider/model env: $(cat "$log")"
            ;;
    esac
}

run_case rpc
run_case node

echo "PASS: wg pi-handler loads plugin transports in RPC and Node-host modes and writes replies"
