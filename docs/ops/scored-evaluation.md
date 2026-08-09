# Receipt-backed scored evaluation

`wg evaluate run <task>` is the scored Agency authority. It is deliberately
narrower than task completion. The FLIP/completion-Eval lane described in
[completion-validation-evidence.md](completion-validation-evidence.md) records
advisory manifest review and must not be interpreted as this quality score:

- input is one ordinary reviewed, receipt-backed `Done`;
- current generation, attempt, fence, immutable candidate/review/output bytes,
  terminal observation, and publication are re-verified;
- the exact configured `[models.evaluator]` **Pi** route and inherited reasoning
  are used;
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
may run **exactly this one live canary command** against a disposable ordinary
`Done` task:

```bash
TASK=receipt-backed-disposable-done
wg --json evaluate run "$TASK" | tee /tmp/wg-scored-evaluation-canary.json
```

Do not loop it or run a model matrix. The bounded evidence to retain is the one
JSON object. Verify locally:

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

A second `wg --json evaluate run "$TASK"` must report
`idempotent_replay=true` and must not call the model again.
