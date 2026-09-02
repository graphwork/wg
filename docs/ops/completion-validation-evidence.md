# Deterministic completion-validation evidence

Ordinary `wg done TASK` uses the task's `## Validation` prose as its default
acceptance contract. Workers choose relevant checks, report what they ran, and
can distinguish candidate regressions from pre-existing or environmental
failures. FLIP and completion Eval are advisory by default.

An exact executable command is a separate, optional surface:

1. **An explicitly configured command is authoritative.** It must exit
   successfully on one unchanged candidate before completion can continue.
2. **Only the operator or checked-in repository policy should grant that
   authority.** Agent-selected checks belong in `## Validation`; agents must not
   invent or broaden a hard gate merely to be “safer” or satisfy model review.

Completion Eval is not an Agency quality score. `wg evaluate run TASK` remains
that separate, post-`Done`, scored authority.

## Optional operator-authorized exact command

The normal task needs only prose criteria:

```bash
wg add "Implement parser" --id parser \
  --description $'Implement the parser.\n\n## Validation\n- [ ] focused parser regression passes\n- [ ] report failures and whether they reproduce on the base revision'
wg publish parser --only
```

Use `--validation-command` only when the human explicitly requests the exact
command or a checked-in project policy declares it authoritative:

```bash
wg add "Implement parser" --id parser \
  --description $'Implement the parser.\n\n## Validation\n- [ ] focused tests pass' \
  --validation-command "./scripts/check-parser"
```

Do not silently append `--all-features`, `-D warnings`, full-suite scope, or
similar strengthening to a repository command. A configured command is stored
in the task's canonical requirements; editing it changes the requirements
digest and invalidates any earlier candidate:

```bash
wg edit parser --validation-command "cargo test parser::tests && cargo clippy"
wg edit parser --validation-command ""   # clear
```

A single shell expression may compose multiple deterministic checks. WG records
that exact expression as one command identity; it does not infer commands from
worker prose or pretend that a `wg log` line is test evidence.

## What one-step `wg done` records

WG executes the configured command itself with a bounded timeout and captures a
canonical `deterministic-validation/configured/v1` object containing:

- exact `bash -lc` argv and its BLAKE3 command-identity digest;
- task, requirements, generation, attempt, and fence;
- digests identifying the Git repository, worktree, and cwd, plus the
  repository-relative cwd;
- candidate HEAD, tree, integrated-main OID, and before/after status digests;
- RFC3339 start/finish times and monotonic duration;
- exit code, signal, success, and timeout state;
- full stdout/stderr BLAKE3 digests, total byte counts, and at most 32 KiB of
  exact review content per stream.

Land completion also records the same structured envelope for WG's baseline
`git diff --check refs/heads/main..HEAD` check. The completion manifest names
those immutable objects; its digest names the exact evidence set. Each WG-run
capture also has a create-once marker in the protected completion control plane,
so worker-authored JSON passed through diagnostic `completion-object` cannot
masquerade as a host execution. FLIP/Eval receipts then bind that manifest and
requirements to the selected candidate sequence and attempt/fence.

Before either model call—and again before publication-derived `Done`—WG checks
that every configured command has one passing envelope, its command identity
matches the immutable task configuration, the task/requirements/attempt/fence
are current, the repository identity is exact, and the candidate commit/tree
and worktree status did not change during execution. Missing worker-prose-only,
failing, stale, digest-mismatched, or structurally tampered evidence is
`IncompleteEvidence` and cannot reach semantic review.

## Failure behavior

- A nonzero/timeout result is retained by digest, shown on stderr, logged on the
  task, and leaves the task `InProgress` without a review call.
- Missing or stale evidence submitted through diagnostic manifest commands
  creates a visible FLIP `incomplete_evidence` receipt and leaves lifecycle
  unchanged.
- A model rejection after valid deterministic evidence is visible but advisory
  by default; FLIP Pass is required before completion Eval is called.
- Review activity is an append-only receipt projection. Landing, `Done`, daemon
  restart, and stale full-graph saves cannot erase an existing activity row.
