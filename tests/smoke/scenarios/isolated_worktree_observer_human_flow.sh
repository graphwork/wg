#!/usr/bin/env bash
# Installed-binary terminal flow for implement-isolated-worktree.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

scratch="$(make_scratch)"
project="$scratch/project"
home="$scratch/home"
fakebin="$scratch/fakebin"
sync="$scratch/sync"
mkdir -p "$project" "$home/.config/workgraph" "$fakebin" "$sync"
: >"$home/.config/workgraph/config.toml"

cat >"$fakebin/pi" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
cat >/dev/null
mkdir -p docs target/deep/a/b/c
printf '%s\n' "$PWD" >"${FAKE_SYNC:?}/worktree"
printf 'candidate-part-1\n' >docs/draft.md
: >"$FAKE_SYNC/churn-started"
# 100,000 create/write/unlink cycles produce at least 200,000 nested target/
# events. Interleaved source writes prove ignored coalescing cannot mask source.
python3 - <<'PY'
import os
sync = os.environ["FAKE_SYNC"]
for i in range(100_000):
    path = "target/deep/a/b/c/churn"
    with open(path, "wb") as f:
        f.write(str(i).encode())
    os.unlink(path)
    if i % 20_000 == 0:
        with open("docs/draft.md", "ab") as f:
            f.write(("candidate-part-%s\n" % (i // 20_000 + 2)).encode())
PY
mv docs/draft.md docs/final.md
# Same-content replacement must not manufacture a sequence.
cp docs/final.md docs/same && mv docs/same docs/final.md
printf '{"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","delta":"still reasoning"}}\n'
printf '%s\n' ready >"$FAKE_SYNC/wrote"
# Keep the real wrapper/observer alive across stress sampling and daemon restart.
sleep 30
exit 42
SH
chmod +x "$fakebin/pi"

(
  cd "$project"
  git init -q
  git config user.email observer-smoke@test.invalid
  git config user.name 'Observer Smoke'
  mkdir -p docs
  printf 'MAIN-INERT\n' >docs/final.md
  git add docs/final.md
  git commit -qm baseline
  env HOME="$home" XDG_CONFIG_HOME="$home/.config" PATH="$fakebin:$PATH" \
    wg init -m pi:openrouter:test/model --no-agency >/dev/null
  env HOME="$home" XDG_CONFIG_HOME="$home/.config" PATH="$fakebin:$PATH" \
    wg config --auto-assign false --no-reload >/dev/null
  env HOME="$home" XDG_CONFIG_HOME="$home/.config" PATH="$fakebin:$PATH" \
    wg add 'observe isolated worktree' --id observe-wt \
      --model pi:openrouter:test/model \
      -d $'Write docs/final.md in the leased worktree.\n\n## Validation\n- observer evidence stays diagnostic' >/dev/null
  env HOME="$home" XDG_CONFIG_HOME="$home/.config" PATH="$fakebin:$PATH" \
    wg publish observe-wt --only >/dev/null
  env HOME="$home" XDG_CONFIG_HOME="$home/.config" PATH="$fakebin:$PATH" FAKE_SYNC="$sync" OPENROUTER_API_KEY=fake \
    wg service start --max-agents 1 --model pi:openrouter:test/model --no-coordinator-agent --no-supervise >/dev/null
)

for _ in $(seq 1 120); do
  [[ -f "$sync/churn-started" ]] && break
  sleep 0.1
done
[[ -f "$sync/churn-started" ]] || loud_fail "fake Pi never started target stress"
worktree="$(cat "$sync/worktree")"
state_dir="$(find "$project/.wg/attempts" -path '*/worktree-observer/state.json' -print -quit | xargs dirname)"
[[ -n "$state_dir" && -f "$state_dir/baseline.json" ]] || loud_fail "baseline was not persisted before execution"
observer_pid="$(tr -d '[:space:]' <"$state_dir/watcher.lock")"
max_rss_kib=0
for _ in $(seq 1 1200); do
  if [[ -r "/proc/$observer_pid/status" ]]; then
    rss="$(awk '/^VmRSS:/ {print $2}' "/proc/$observer_pid/status")"
    (( rss > max_rss_kib )) && max_rss_kib="$rss"
  fi
  [[ -f "$sync/wrote" ]] && break
  sleep 0.02
done
[[ -f "$sync/wrote" ]] || loud_fail "fake Pi never finished 200k-event target stress"
(( max_rss_kib < 262144 )) || loud_fail "observer RSS was unbounded under ignored churn: ${max_rss_kib}KiB"
sleep 1
if [[ ! -r "/proc/$observer_pid/stat" ]]; then
  observer_failure="$(python3 - "$state_dir/state.json" <<'PY'
import json,sys
p=json.load(open(sys.argv[1]))["projection"]
print(p["health"], p.get("degraded_reason"), p.get("classification_hold"), p["next_safe_action"])
PY
)"
  loud_fail "observer exited during ignored stress: $observer_failure"
fi
cpu_before="$(awk '{print $14+$15}' "/proc/$observer_pid/stat")"
sleep 1
cpu_after="$(awk '{print $14+$15}' "/proc/$observer_pid/stat")"
(( cpu_after - cpu_before < 50 )) || loud_fail "observer CPU did not settle after churn: $cpu_before -> $cpu_after"

[[ "$worktree" == *".wg-worktrees/"* ]] || loud_fail "worker did not receive an isolated worktree: $worktree"
[[ "$(cat "$project/docs/final.md")" == 'MAIN-INERT' ]] || loud_fail "integration/main checkout was modified"

show1="$(cd "$project" && env HOME="$home" XDG_CONFIG_HOME="$home/.config" PATH="$fakebin:$PATH" wg show observe-wt)"
grep -q 'Worktree activity: observed/unproven seq=' <<<"$show1" || loud_fail "wg show omitted observed/unproven activity: $show1"
grep -q 'Pi progress: proven' <<<"$show1" || loud_fail "wg show conflated/omitted the separate proven clock: $show1"
grep -q 'proof default=300s; observed grace=120s / 600s hard cap' <<<"$show1" || loud_fail "production 300/120/600 bounds not visible: $show1"
show_json="$(cd "$project" && wg show observe-wt --json)"
python3 -c 'import json,sys; p=json.load(sys.stdin)["activity_clocks"]; assert p["worktree_authority"]=="observed-unproven"; assert p["last_proven_progress"] is None; assert p["meaningful_silence_secs"]==300; assert p["observed_activity_grace_secs"]==120; assert p["max_observed_only_extension_secs"]==600' <<<"$show_json" || loud_fail "stable JSON did not keep observed/proven clocks separate"
seq1="$(python3 - "$state_dir/state.json" <<'PY'
import json,sys
print(json.load(open(sys.argv[1]))['projection']['content_seq'])
PY
)"
[[ "$seq1" -ge 1 ]] || loud_fail "candidate writes did not advance content sequence"
ignored="$(python3 - "$state_dir/state.json" <<'PY'
import json,sys
print(json.load(open(sys.argv[1]))['projection']['ignored_churn'].get('volatile-target',0))
PY
)"
[[ "$ignored" -ge 1 ]] || loud_fail "target subtree exclusion was not visibly/accountably recorded: $ignored"
python3 - "$state_dir/state.json" <<'PY' || loud_fail "source manifest retained target paths or lost exact final source"
import json,sys
entries=json.load(open(sys.argv[1]))["current_manifest"]["entries"]
assert "docs/final.md" in entries, entries.keys()
assert entries["docs/final.md"]["path"] == "docs/final.md"
assert not any(p == "target" or p.startswith("target/") for p in entries), entries.keys()
PY

# Killing the exact auxiliary observer must not touch the writer or worktree,
# and every operator surface must immediately stop claiming it is active.
kill -TERM "$observer_pid"
for _ in $(seq 1 50); do
  ! kill -0 "$observer_pid" 2>/dev/null && break
  sleep 0.05
done
! kill -0 "$observer_pid" 2>/dev/null || loud_fail "exact observer PID survived TERM"
[[ -d "$worktree" && -f "$worktree/docs/final.md" ]] || loud_fail "observer TERM damaged retained worktree bytes"
status_after_kill="$(cd "$project" && wg show observe-wt)"
grep -q 'Observer: PollOnly' <<<"$status_after_kill" || loud_fail "dead observer still reported active: $status_after_kill"
grep -q 'recover with argv: wg worktree-observer-run --state-dir' <<<"$status_after_kill" || loud_fail "dead observer omitted safe recovery command: $status_after_kill"

# A daemon crash/restart must not refresh the activity timestamp or sequence.
before_ts="$(python3 - "$state_dir/state.json" <<'PY'
import json,sys
p=json.load(open(sys.argv[1]))['projection']; print(p['last_activity']['observed_at'])
PY
)"
observer_epoch_before="$(python3 - "$state_dir/state.json" <<'PY'
import json,sys
print(json.load(open(sys.argv[1]))['projection']['source']['observer_epoch'])
PY
)"
pkill -f "worktree-observer-run --state-dir $state_dir" 2>/dev/null || true
sleep 0.2
service_json="$(cd "$project" && env HOME="$home" XDG_CONFIG_HOME="$home/.config" PATH="$fakebin:$PATH" wg service status --json)"
service_pid="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["pid"])' <<<"$service_json")"
kill -9 "$service_pid" 2>/dev/null || true
sleep 0.2
(
  cd "$project"
  env HOME="$home" XDG_CONFIG_HOME="$home/.config" PATH="$fakebin:$PATH" FAKE_SYNC="$sync" OPENROUTER_API_KEY=fake \
    wg service start --force --max-agents 1 --model pi:openrouter:test/model --no-coordinator-agent --no-supervise >/dev/null
)
for _ in $(seq 1 50); do
  observer_epoch_after="$(python3 - "$state_dir/state.json" <<'PY'
import json,sys
print(json.load(open(sys.argv[1]))['projection']['source']['observer_epoch'])
PY
)"
  [[ "$observer_epoch_after" -gt "$observer_epoch_before" ]] && break
  sleep 0.1
done
[[ "$observer_epoch_after" -gt "$observer_epoch_before" ]] || loud_fail "daemon startup did not reattach/reconcile the missing observer"
after_ts="$(python3 - "$state_dir/state.json" <<'PY'
import json,sys
p=json.load(open(sys.argv[1]))['projection']; print(p['last_activity']['observed_at'])
PY
)"
[[ "$before_ts" == "$after_ts" ]] || loud_fail "daemon restart invented a fresh activity timestamp: $before_ts -> $after_ts"

# Overflow is a hint followed by full reconciliation, never an activity record.
# Stop the restarted auxiliary first: the manual adapter is a test-only writer
# and must not race the live observer's exact-state atomic projection writes.
pkill -f "worktree-observer-run --state-dir $state_dir" 2>/dev/null || true
sleep 0.1
overflows_before="$(python3 - "$state_dir/state.json" <<'PY'
import json,sys
print(json.load(open(sys.argv[1]))['projection']['watcher_overflows'])
PY
)"
(cd "$project" && env HOME="$home" XDG_CONFIG_HOME="$home/.config" PATH="$fakebin:$PATH" \
  wg worktree-observer-reconcile --state-dir "$state_dir" --overflow --json >/dev/null)
read -r seq2 overflows_after < <(python3 - "$state_dir/state.json" <<'PY'
import json,sys
p=json.load(open(sys.argv[1]))['projection']; print(p['content_seq'], p['watcher_overflows'])
PY
)
[[ "$seq2" == "$seq1" && "$overflows_after" -eq $((overflows_before + 1)) ]] || loud_fail "overflow reconciliation manufactured activity or was not recorded: seq $seq1 -> $seq2, overflow $overflows_before -> $overflows_after"

# Wait for nonzero Fake-Pi exit. Existing wrapper lifecycle may fail the task;
# the observer itself must never make it Done.
for _ in $(seq 1 100); do
  status="$(cd "$project" && wg show observe-wt --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])')"
  [[ "$status" != 'in-progress' ]] && break
  sleep 0.1
done
[[ "$status" != 'done' ]] || loud_fail "file mutation made the task Done"

# Stop the detached native watcher, declare the exact reap boundary through the
# read-only evidence adapter, then prove a late write is quarantined and inert.
pkill -f "worktree-observer-run --state-dir $state_dir" 2>/dev/null || true
sleep 0.1
(cd "$project" && wg worktree-observer-reconcile --state-dir "$state_dir" --preservation --after-reap >/dev/null)
printf 'late-after-reap\n' >>"$worktree/docs/final.md"
late_json="$(cd "$project" && wg worktree-observer-reconcile --state-dir "$state_dir" --preservation --after-reap --json)"
python3 -c 'import json,sys; p=json.load(sys.stdin); assert p["quarantine_required"] is True; assert any(x["reason"]=="late-write-after-reap" for x in p["late_mutations"])' <<<"$late_json" || loud_fail "late post-reap write was not quarantined"
final_status="$(cd "$project" && wg show observe-wt --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])')"
[[ "$final_status" != 'done' ]] || loud_fail "late evidence woke/completed the task"

service_human="$(cd "$project" && wg service status)"
grep -q 'Worktree activity: observed/unproven' <<<"$service_human" || loud_fail "service status omitted observer projection: $service_human"
(cd "$project" && wg service stop >/dev/null 2>&1 || true)

echo "PASS: 200k+ ignored target events keep observer RSS/CPU/queue bounded, source evidence stays exact, dead exact-PID health degrades truthfully, restart/overflow converge, and post-reap writes quarantine without Done authority"
