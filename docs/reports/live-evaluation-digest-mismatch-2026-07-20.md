# Live evaluation verdict digest mismatch — 2026-07-20

## Incident evidence

The production failure was preserved byte-for-byte under
`tests/fixtures/live_eval_digest_mismatch/`:

| file | SHA-256 |
|---|---|
| `evaluation.json` | `a993ba776f5700c1262a1024dedf600a36eb0447571021c6e0e488ebfdf6ade0` |
| `verdict.json` | `42d0999c841a898a53864aa7309b5860367d4f52bb618c9868ce5c5937a2c410` |

It is the real `.evaluate-validate-integrated-luca-2` result produced through
`pi:openai-codex:gpt-5.6-terra`: score `0.88`, evaluation id
`eval-validate-integrated-luca-2-2026-07-19T12-38-21.330516412+00-00`, verdict
`verdict-evalp-499fd5ddac13a90963448679-evaluate-97e39ad55786b39b`.
The verdict itself is intact (`verdict_digest =
 b3:351c0fea99e2e11c2104bf58667da321c14517694041b24487b2874acf9d27c9`).
Its evaluation digest is
`b3:cabacc453b0a6b2bb3216e1fa1c3003014e122c1ef27cb306bbacbd342aec2dc`.

## Precise cause

No evaluation field value changed between the writer and the daemon reader.
The differing material was object-member order inside `Evaluation.dimensions`,
a `HashMap<String, f64>`.

The writer did two operations on the same in-memory `Evaluation`:

1. saved pretty JSON, whose dimension order began `correctness`,
   `completeness`, `efficiency`, ...;
2. computed `evaluation_digest` from compact `serde_json::to_vec(evaluation)`.

The restarted daemon deserialized that JSON into a newly seeded `HashMap` and
then called `serde_json::to_vec` again. HashMap iteration order is not a
canonical encoding and changed across the process boundary. The semantic JSON
was identical, but the compact bytes differed, so the reader correctly failed
closed with `WG-EVAL-VERDICT-EVIDENCE` and left the parent `PendingEval`.
Satellite completion did not mutate either durable file; reserialization in the
reader created the false mismatch.

## Repair

New verdicts use `evaluation_digest_schema = 2` and hash the exact durable
evaluation file bytes after its atomic rename. The writer first compares an
ordered semantic representation of the durable file with its in-memory value,
then pins the file bytes. The reader locates exactly one evaluation id and
checks its source, score, digest scheme, and bytes. Duplicate/missing evidence,
unknown schemes, record tampering, or evaluation tampering remain loud
fail-closed errors.

Historical schema-1 verdicts are not guessed or discarded. Their compact writer
bytes are reconstructed losslessly by removing only JSON formatting whitespace
from the durable pretty JSON while preserving member order and string bytes.
That validates the exact incident fixture. Zero or multiple evidence candidates
remain ambiguous and are not selected. An upgrade replay validates both the old
and new digest schemes against the same durable evidence, so changed
observational `created_at` or wrapper run ids do not conflict or cause a second
consumption.

## Regression coverage

- Unit tests pin the exact incident bytes, schema-1 recovery, schema-2 tamper
  rejection, observational replay, pre-schema claimed-evaluator migration, and
  exactly-once graph consumption.
- `live_pi_evaluation_verdict_restart` drives a real persisted-plan
  `wg evaluate run` through the explicit Pi/openai-codex/Terra one-shot handler
  (only the remote response is stubbed), records the same seven-dimension 0.88
  shape, hashes evaluation/verdict files before and after satellite completion,
  starts a fresh service-tick reader, and proves exactly-once consumption across
  repeated restarts. Claude is a failing sentinel; no implicit fallback exists.
  Independent evaluation and verdict tamper branches stay `PendingEval` and emit
  the integrity diagnostic.
