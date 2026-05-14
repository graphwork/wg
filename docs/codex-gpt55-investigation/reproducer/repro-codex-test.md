# repro-codex-direct: Success Metric Analysis

This reproducer is a direct engagement check for `codex:gpt-5.5`: it is meant
to prove that the executor can read a task, use tools, write artifacts, commit
the result, and leave wg breadcrumbs instead of returning a prose-only
completion. Section 4 of `fix-proposal.md` is useful because it defines success
in terms of observable delivery rather than model self-report. The proposed
metric does not ask whether the model sounded confident. It asks whether a
task produced files, created a git commit, ended in `done`, and logged at least
three progress events.

The core target, `>= 3/4 produce committed deliverables`, is a pragmatic
threshold for a regression-oriented fix. The proposal describes a baseline of
about one successful committed deliverable out of four on `main`, so three out
of four after the fix would show a large improvement without pretending the
system has become perfect. That matters because the suspected failure is not a
normal deterministic code defect. It is an interaction among prompt context,
Codex CLI defaults, model behavior, and wrapper completion semantics. A single
run cannot prove the agent will never bail, but a mixed four-task pack can
show whether the fix meaningfully changes the observed failure rate.

The chosen task mix is also well aligned with the suspected cause. A pure
research/writeup task checks whether Codex can produce a simple artifact and a
commit when no compiler feedback is involved. A mechanical Rust edit checks
basic source mutation and build discipline. A multi-file refactor tests
whether the model can sustain state across several files and finish validation
instead of stopping after a plan. A doc-plus-code CLI flag checks integration
behavior across user-facing text and implementation. Together, these tasks
cover the surface where "lazy completion" is most damaging: the agent may
summarize an approach, leave no files behind, and still exit successfully.

The secondary metric, `count(status=done AND commits_ahead=0) == 0`, is at
least as important as the success ratio. A failed task is recoverable in
WG: it can be retried, escalated, or inspected. A false `done` task is
more dangerous because it tells downstream tasks and evaluators that work
exists when it does not. Fix #3, the wrapper minimum-work gate, is therefore
not only a guardrail for Codex; it is a graph integrity mechanism. It ensures
that an exit code of zero is not treated as sufficient evidence of completion.
The proposal correctly treats "no false done" as a hard quality dimension
rather than merely counting successful completions.

The approach has a sensible defense-in-depth shape. Fix #1 promotes GPT-5
family models into the full knowledge tier so the worker sees the same
completion contract, smoke-gate language, validation discipline, and git
hygiene expected of other strong executors. Fix #2 changes the Codex executor
defaults so the CLI is less biased toward low-verbosity, summary-style output
and receives explicit developer instructions to perform real batch work. Fix
#3 handles the residual case where the model still exits without material
work. These fixes attack the problem at three different points: instruction
availability, model engagement, and wrapper acceptance.

The required web search also supports the broad premise that Codex is evaluated
and marketed as an agentic coding surface, not a chat-only summarizer. OpenAI's
2025 Codex upgrade material described GPT-5-Codex as designed for both
interactive pairing and persistent independent execution on longer tasks. The
official GPT-5.5 announcement later framed GPT-5.5 as useful in Codex for
long-horizon work, including internal examples where Codex analyzed business
data, built frameworks, and processed large document sets. Those sources do
not validate this repository's particular wrapper fix, but they make the
success metric reasonable: a Codex worker assigned through wg should be
expected to complete concrete repository tasks, not merely describe them.

There are still limitations. Four tasks are enough for a minimal reproducer,
but not enough to characterize long-term reliability. The `>= 3/4` target can
be met while one important task class still fails. The pack should therefore be
treated as a smoke gate and directional metric, not a statistical benchmark.
The proposal partly addresses this by making the tasks diverse, but follow-up
runs should preserve the individual task outcomes, not just the aggregate
ratio. If the multi-file refactor is the only failure, the next fix would be
different than if the research/writeup task fails.

The metric also depends on correct interpretation of commits. A commit proves
that something was staged and recorded, but it does not prove the change was
semantically correct. That is why the task-specific acceptance checks and build
or test gates matter. For documentation-only work, word count and path
existence are acceptable. For code work, `cargo build` and `cargo test` are
needed to prevent a superficial commit from satisfying the engagement metric.
The best reading of Section 4 is that committed deliverables are a minimum bar
for engagement, while validation remains the bar for correctness.

For this direct reproducer, the same philosophy applies in miniature. The
markdown file demonstrates that the model read and analyzed the proposal. The
JSON file records the expected executor and model identity with
`engaged=true`. The word-count command turns a qualitative writing task into a
simple measurable check. The git commit requirement proves the worker used the
repository workflow instead of ending at text output. Finally, the required
`wg log` call to `codex-test-fix` leaves an explicit breadcrumb for the
downstream measurement task. That combination is intentionally redundant, and
the redundancy is the point: a real wg completion should leave evidence
in files, git history, validation output, and graph logs.

Sources consulted during the required web search:

- OpenAI, "Introducing GPT-5.5", https://openai.com/index/introducing-gpt-5-5/
- OpenAI, "Introducing upgrades to Codex", https://openai.com/index/introducing-upgrades-to-codex/
- OpenAI Help Center, "Model Release Notes", https://help.openai.com/en/articles/9624314-model-release-notes
