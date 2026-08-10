# Receipt-backed scored evaluation

`wg evaluate run <task>` is the scored Agency authority. It is deliberately
narrower than task completion. The FLIP/completion-Eval lane described in
[completion-validation-evidence.md](completion-validation-evidence.md) records
advisory manifest review and must not be interpreted as this quality score:

- input is one ordinary reviewed, receipt-backed `Done`;
- current generation, attempt, fence, immutable candidate/review/output bytes,
  terminal observation, and publication are re-verified;
- the exact effective `models.evaluator` **Pi** route and inherited reasoning
  are used after applying the task's named-profile overlay (when present);
- one no-tools, no-session call runs with a bounded prompt, response, notes, and
  timeout;
- one deterministic create-once evaluation is written to
  `.wg/agency/evaluations/` and projected idempotently into performance records;
- no task status, lifecycle, retry, publication, or graph node can be changed.

Failed, Waiting, operator-accepted, unlanded, stale-generation, missing, or
unverifiable inputs fail before the model call. Provider/setup failure is loud
and leaves no score row.

## Operator flow

```bash
wg evaluate run TASK --dry-run
wg --json evaluate run TASK
wg --json evaluate show TASK
wg --json agency stats
```

Dry-run prints the exact route, reasoning, terminal observation, completion
receipt, content digest of the bounded evidence, and prompt byte count without
calling Pi or writing anything.

## One explicit real-model canary

Automated tests use fake Pi. An operator with a configured/login-ready Pi route
may run one canary against an expressly selected ordinary `Done` task. Prefer a
disposable task. A receipt-backed recovery-graph task is appropriate only when
validating that graph is itself the canary objective; snapshot its graph bytes and
status first.

Preflight is read-only and does not count as the live call:

```bash
TASK=receipt-backed-disposable-done
wg --json evaluate run "$TASK" --dry-run \
  | tee /tmp/wg-scored-evaluation-canary-dry-run.json
```

Confirm `eligible=true`, `already_recorded=false`, the intended effective route,
and an unchanged graph digest. Then make **one** provider call:

```bash
wg --json evaluate run "$TASK" \
  > /tmp/wg-scored-evaluation-canary.json \
  2> /tmp/wg-scored-evaluation-canary.err
```

Do not loop it or run a model matrix. Retain both files. If the command fails,
confirm no evaluation row was created, preserve stderr, and stop without retrying
or manufacturing a score. On success, verify the one JSON object locally:

```bash
python3 - <<'PY'
import json
p = "/tmp/wg-scored-evaluation-canary.json"
x = json.load(open(p))
e = x["evaluation"]
assert 0 <= e["score"] <= 1
assert set(e["dimensions"]) == {
  "correctness", "completeness", "efficiency", "style_adherence",
  "downstream_usability", "coordination_overhead", "blocking_impact",
}
assert e["evaluator_route"].startswith("pi:")
assert e["evaluator_reasoning"] in {"off","minimal","low","medium","high","xhigh","max"}
assert e["evidence_digest"].startswith("b3:")
assert e["source_terminal_observation"]["completion_receipt"].startswith("b3:")
assert e["evaluator_usage"]["input_tokens"] >= 0
assert e["evaluator_usage"]["output_tokens"] >= 0
assert e["evaluator_usage"]["cost_usd"] >= 0
print("bounded real-model canary evidence OK")
PY
```

After success only, a second `wg --json evaluate run "$TASK"` must report
`created=false` and `idempotent_replay=true`. The evaluation row count and its
bytes, plus the source graph bytes/status, must remain unchanged. This is an
immutable-row replay check, not a second live model canary: it must not call the
provider again.
