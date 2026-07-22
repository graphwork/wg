#!/usr/bin/env bash
# Generated evaluator tasks must keep the handler-qualified route in both
# task metadata and the invocation they execute. This covers an implementation
# task and a merge task scaffolded later in the same project.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

wg init --route codex-cli >init.log 2>&1 \
    || loud_fail "wg init failed: $(tail -10 init.log)"

python3 - <<'PY'
from pathlib import Path
p = Path('.wg/config.toml')
s = p.read_text()
start = s.index('[models.evaluator]')
end = s.find('\n[', start + 1)
if end < 0:
    end = len(s)
section = s[start:end]
lines = section.splitlines()
for i, line in enumerate(lines):
    if line.startswith('model = '):
        lines[i] = 'model = "codex:gpt-5.6-luna"'
        break
else:
    raise SystemExit('models.evaluator has no model')
p.write_text(s[:start] + '\n'.join(lines) + s[end:])
PY

for spec in \
    "implementation:Implementation" \
    "merge-rank-level-containment:Merge rank-level containment"
do
    id=${spec%%:*}
    title=${spec#*:}
    wg add "$title" --id "$id" >"add-$id.log" 2>&1 \
        || loud_fail "wg add $id failed: $(tail -10 "add-$id.log")"

    for generated in ".flip-$id" ".evaluate-$id"
    do
        shown=$(wg show "$generated" --json 2>&1) \
            || loud_fail "generated task $generated missing: $shown"
        python3 - "$generated" "$shown" <<'PY'
import json, sys
task_id, raw = sys.argv[1:]
task = json.loads(raw)
if task.get('model') != 'codex:gpt-5.6-luna':
    raise SystemExit(f"{task_id}: model lost provider prefix: {task.get('model')!r}")
command = task.get('exec') or ''
if '--evaluator-model codex:gpt-5.6-luna' not in command:
    raise SystemExit(f"{task_id}: invocation route not pinned: {command!r}")
PY
    done
done

echo "PASS: implementation and later-generated merge evaluators retain codex:gpt-5.6-luna"
