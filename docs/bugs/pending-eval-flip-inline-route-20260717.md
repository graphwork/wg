# PendingEval / FLIP inline evaluator route failure

Upstream report for a repeatable WG lifecycle stall observed in `spinozans/emender`.

- Full report: https://github.com/spinozans/emender/blob/1e6bb993c26a32eadddcc4fcd437c999fb9d2cde/reports/wg/pending-eval-flip-evaluator-deadlock-20260717.md
- Raw reproduction and evidence: https://github.com/spinozans/emender/blob/1e6bb993c26a32eadddcc4fcd437c999fb9d2cde/reports/wg/pending-eval-flip-evaluator-deadlock-evidence-20260717.md
- Evidence commit: https://github.com/spinozans/emender/commit/1e6bb993c26a32eadddcc4fcd437c999fb9d2cde

The short diagnosis is that scaffolded FLIP tasks persist `model: gpt-5.4-mini`
and `provider: codex` separately, but `spawn_eval_inline` validates only the bare
model as an invocation-scoped route. It rejects the documented `codex-cli`
configuration before claim, trips the five-failure circuit breaker with zero
agent runs, and strands the parent in `PendingEval`. Dispatcher readiness already
has the required system-task PendingEval bypass; generic `wg why-blocked` does
not, so its output misleadingly frames the parent edge as the root cause.

Maintainer intake command:

```bash
wg add 'Fix: inline FLIP evaluator loses provider and strands PendingEval' \
  --id fix-inline-eval-qualified-route \
  -d "$(curl -fsSL https://raw.githubusercontent.com/spinozans/emender/1e6bb993c26a32eadddcc4fcd437c999fb9d2cde/reports/wg/pending-eval-flip-evaluator-deadlock-20260717.md)"
```

