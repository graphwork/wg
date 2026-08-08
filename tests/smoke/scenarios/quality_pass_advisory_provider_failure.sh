#!/usr/bin/env bash
# Optional quality-pass provider failure releases unchanged; required stays closed.
set -euo pipefail
source "$(dirname "$0")/_helpers.sh"
: "${WG_BIN:?smoke harness must provide candidate WG_BIN}"
[[ -x $WG_BIN ]] || loud_fail "candidate WG_BIN is not executable: $WG_BIN"

scratch=$(mktemp -d "${TMPDIR:-/tmp}/wg-quality-advisory.XXXXXX")
cleanup() {
  for p in "$scratch"/*/project; do
    [[ -d $p ]] || continue
    env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC \
      WG_DIR="$p/.wg" "$WG_BIN" service stop --force --kill-agents >/dev/null 2>&1 || true
  done
  [[ ${WG_SMOKE_KEEP_TMP:-0} == 1 ]] || rm -rf "$scratch"
}
trap cleanup EXIT

run_case() {
  local name=$1 required=$2 mutated=${3:-no} admission=${4:-coordinator}
  local root="$scratch/$name" project="$scratch/$name/project" home="$scratch/$name/home"
  mkdir -p "$project" "$home" "$root/bin"
  ln -s "$WG_BIN" "$root/bin/wg"
  cat >"$root/bin/pi" <<'SH'
#!/usr/bin/env bash
# Pi/OpenRouter-shaped provider outage, credential-free and deterministic.
# The mutated case proves baseline integrity: trusted cross-task edits are real,
# but a worker cannot then call its failed batch "unchanged".
if [[ ${WG_TASK_ID:-} == .quality-pass-mutated ]]; then
  wg edit downstream-mutated --description 'mutated before provider failure'
fi
printf '%s\n' '{"type":"error","error":{"code":402,"message":"Insufficient credits","metadata":{"error_type":"payment_required"}}}'
exit 1
SH
  chmod +x "$root/bin/pi"
  (
    export PATH="$root/bin:$PATH" HOME="$home" XDG_CONFIG_HOME="$home/.config"
    unset WG_AGENT_ID WG_TASK_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_PROJECT_ROOT WG_WORKTREE_PATH WG_DIR
    git -C "$project" init -q -b main
    git -C "$project" config user.email quality-pass@test.invalid
    git -C "$project" config user.name QualityPass
    printf 'base\n' >"$project/README.md"
    git -C "$project" add README.md
    git -C "$project" commit -qm base
    cd "$project"
    "$WG_BIN" init --no-agency --route pi --model pi:openrouter:test/model >/dev/null
    wgrun() { env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC WG_DIR="$project/.wg" "$WG_BIN" "$@"; }
    if [[ $admission == manual ]]; then
      wgrun config set dispatcher.settling_delay_ms 60000 >/dev/null
    else
      wgrun config set dispatcher.settling_delay_ms 5000 >/dev/null
    fi
    local tag=()
    [[ $required == yes ]] && tag=(--tag quality-pass:required)
    wgrun add "Quality pass $name" --id ".quality-pass-$name" "${tag[@]}" >/dev/null
    wgrun add "Downstream $name" --id "downstream-$name" --after ".quality-pass-$name" >/dev/null
    wgrun publish ".quality-pass-$name" --wcc >/dev/null
    wgrun service start --max-agents 1 --no-coordinator-agent --no-supervise >/dev/null
    if [[ $admission == manual ]]; then
      wgrun spawn ".quality-pass-$name" --executor pi --model pi:openrouter:test/model >/dev/null
    fi
    for _ in $(seq 1 300); do
      status=$(wgrun show ".quality-pass-$name" --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))' 2>/dev/null || true)
      [[ $status == failed ]] && break
      sleep 0.05
    done
    [[ ${status:-} == failed ]] || loud_fail "$name quality pass did not fail through fake provider"
    # The long settling window keeps the downstream from being dispatched while
    # this fixture checks the dependency disposition.
    wgrun service stop --force --kill-agents >/dev/null
    detail=$(wgrun show "downstream-$name" --json)
    if [[ $required == yes || $mutated == yes ]]; then
      printf '%s' "$detail" | grep -Eq '"satisfied"[[:space:]]*:[[:space:]]*false' \
        || loud_fail "$name quality pass did not remain fail-closed: $detail"
      if wgrun ready --json | grep -q "downstream-$name"; then
        loud_fail "$name quality pass released downstream"
      fi
      if [[ $mutated == yes ]]; then
        printf '%s' "$detail" | grep -q 'mutated before provider failure' \
          || loud_fail "mutated fixture did not perform its trusted cross-task edit"
      fi
    else
      printf '%s' "$detail" | grep -Eq '"satisfied"[[:space:]]*:[[:space:]]*true' \
        || loud_fail "optional quality pass did not release unchanged downstream: $detail"
      printf '%s' "$detail" | grep -q 'released unchanged batch' \
        || loud_fail "optional release omitted visible advisory warning: $detail"
      wgrun ready --json | grep -q "downstream-$name" \
        || loud_fail "optional quality pass left downstream stranded"
    fi
  )
}

run_case optional no
run_case manual no no manual
run_case mutated no yes
run_case required yes

echo "PASS: fake-provider failure released only a baseline-verified unchanged optional batch; a trusted pass that edited the batch and a quality-pass:required task both stayed fail-closed"
