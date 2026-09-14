# Reliable work: how WG should operate

**Status:** Guiding design and review standard. This describes the intended system, not a claim that every behavior is implemented. It does not change live configuration, waive existing gates, or authorize migrations by itself.

## The promise

> Give WG a bounded piece of work. Receive an inspectable result or a precise request for a necessary decision. Work does not disappear, and the user does not have to repair the coordination machinery.

WG should make a capable agent more dependable, not make it less able to finish. A run that succeeds only after repeated manual retries, contract edits, or daemon restarts is not an autonomous success, even if the task eventually says Done.

This is a work-coordination system, not a ceremony-enforcement system. Its essential responsibilities are continuity, appropriate authority, intelligible acceptance, and honest reporting.

## 1. Make the ordinary path small

The ordinary path is:

**Understand the task → do the work → check the result → repair if appropriate → finish or ask for help.**

The worker should not have to operate internal receipt, lease, manifest, or publication protocols. Those mechanisms may enforce safety underneath the ordinary interface, but using them correctly should be the runtime's responsibility.

One task should normally retain one working context through implementation and repair. Create another task for genuinely separable work or ownership, not to explain a failed command, wait for a process, or perform routine completion recovery.

Cycles are legitimate when their iteration and exit conditions are clear. A cycle is not itself a fault. A graph is operationally stalled when required work has neither an active owner, a valid bounded wait, nor a visible decision request that identifies who can resolve it.

## 2. Separate intent, acceptance, and communication

A task needs three distinct things:

| Part | Purpose | Example |
| --- | --- | --- |
| Intended result | What useful outcome is requested? | A working feature, a supported conclusion, a reviewed document |
| Acceptance | What properties and checks determine whether that outcome is acceptable? | Required behavior, safety constraints, applicable repository checks |
| Coordination | How should participants communicate while doing the work? | Send a brief progress update when the diagnosis changes |

**Coordination requests are not product acceptance criteria.** A request for an early message must not become a requirement to prove the message's timestamp inside the candidate manifest. A preferred work sequence is not automatically a property of the deliverable.

Acceptance should be finite, relevant to the result, and understandable before execution. It must not require evidence that cannot exist until after acceptance, such as the candidate's own future review, publication, or reload receipt. A committed report must not need its own future commit hash.

A reviewer must distinguish defects in the deliverable from incidental omissions in the worker's narrative. If a requirement is ambiguous, request a decision; do not silently choose the most burdensome interpretation and repeatedly reject the work.

## 3. Give workers real judgment within a clear boundary

A worker has three legitimate responses to difficulty:

1. **Repair:** "I understand the failure and can fix it within my authority."
2. **Stop or request help:** "I cannot safely complete this; here is what I tried and the specific blocker."
3. **Propose a contract correction:** "The acceptance conditions appear inconsistent or inappropriate; here is the evidence and proposed change."

These choices must be available for validation failures, semantic review findings, and other relevant blocked states. A worker should not need to manufacture a deterministic test failure merely to request help.

Permission to repair is not permission to redefine success. The worker may choose diagnostic checks and repair code or fixtures within the declared scope. It may not silently remove a required gate, lower a threshold, expand sensitive access, or change the intended outcome to fit its implementation.

An authorized owner can approve a contract correction. WG records that decision, preserves the earlier failures, invalidates stale acceptance bindings where necessary, and validates the resulting candidate under the revised contract. There is no retroactive pass.

An intentional stop is legitimate. The runtime must not turn it into an endless cycle of automatic retries or model escalation.

## 4. Make validation useful without making it a trap

Before work begins, the worker must be able to see:

- The required outcome and applicable validation policy.
- Which executable checks are mandatory, and who authorized them.
- How to run additional checks and attach trustworthy results.
- The permitted repair boundary and how to request a change.

Prose validation criteria and executable hard gates are different. Agent-selected checks do not become mandatory gates merely because an agent ran or recommended them. A hard gate must have explicit user or repository authority; neither the worker nor the attended assistant should invent a stronger gate to appease a reviewer.

**Selecting a useful check, recording its execution, and making it mandatory are three different operations.** WG should support trustworthy capture of agent-selected checks without granting the agent permission to edit the mandatory acceptance contract.

For machine-verifiable checks, capture the command, execution context, result, and relevant candidate identity through a supported runtime path. A managed process result should be usable through that path without rerunning an expensive command solely to create another receipt. If safe reuse cannot be established, explain why revalidation is necessary.

Human-flow evidence, research sources, manual observations, and machine checks are different evidence types. Label their provenance and limitations. Do not pretend every useful observation is a shell command, or every prose claim is independently verified.

A pre-existing failure is neither an automatic waiver nor a reason for unlimited unrelated repair. Reproduce it on the relevant baseline, explain its effect on acceptance, and resolve the scope question explicitly. If a project requires a broad suite to pass, it must provide a feasible repair or decision path for failures in that suite.

Once an authorized mandatory check fails, it remains unsatisfied until repaired or explicitly revised. Neither model approval nor repeated retries can substitute for that decision.

## 5. Use one completion-and-repair loop

Completion should return one of three user-understandable outcomes:

| Outcome | Runtime action |
| --- | --- |
| Accepted | Record the accepted result and perform only the publication authorized by the task contract |
| Repair needed | Return concrete findings and evidence to the same authorized worker, preserving its work |
| Needs a decision | Preserve the candidate and expose one specific request to the responsible person or agent |

These are behavioral outcomes, not a demand for three new database states. Reuse existing lifecycle representations where they are adequate.

Repair feedback should identify the actual failed condition, relevant evidence, and next action. It should not require the worker to reconstruct the failure from several disconnected logs.

Repair must be bounded. Repeated unchanged submissions, duplicate events, incidental output, or restarts must not replenish the budget. A meaningful repair can justify another check, but cannot justify an unbounded episode. Budget exhaustion leads to a visible decision request, not silent abandonment.

Review policy must be explicit before execution. Advisory review remains advisory; strict review is an explicit policy choice. A semantic rejection under strict policy blocks acceptance until resolved. Review unavailability is not a pass. Reviewers supply findings under the declared policy; they do not acquire authority to invent new requirements.

A worker must never retry a reviewer on unchanged work simply to obtain a favorable answer.

## 6. Cooperate with the agent runtime

WG owns task identity, authority, dependencies, and acceptance. The agent runtime owns model turns and its supported tool execution. A process extension owns the processes it starts. Each boundary needs one explicit owner, not competing implementations.

For long-running work, the desired flow is:

**Start a managed operation → register its completion subscription → yield → wake the same authorized worker → inspect the result → continue.**

Turn completion is not task completion. Waiting on a known operation is not proof that an agent is dead. Conversely, a process existing is not proof of useful progress or a reason to wait forever.

The runtime adapter must verify that its launch mode actually supports yielding and waking. Instructions alone cannot make a one-shot invocation into a persistent session. Teach only tools and wakeup behavior that are loaded and supported in that worker.

Keep model-stream inactivity, command deadlines, and task deadlines separate. Do not fix a lifecycle mismatch by increasing every timeout or generating artificial token activity.

Subscribe before yielding, reconcile completion races, deduplicate wakeups, and respect cancellation. Cancellation must terminate and reap the owned process tree without killing unrelated sessions. Reconnection must reattach to a proven identity or request help; it must not blindly execute the command again.

Log matches are observations, not completion verdicts or instructions. Background work must not require polling agents, detached shell folklore, or a parallel WG process scheduler when the runtime already provides suitable ownership and notification.

## 7. Have one authority for configuration

Project configuration is the authority for new model execution. All production paths resolve it through one policy and an identifiable revision.

Ordinary selection is simple:

- One project default.
- Strong inherits that default when unset.
- Weak inherits strong when unset: **strong and weak are equal by default**.
- Explicit tier and role overrides remain available and visible.

Inheritance must not require copying one route into every role. Changing a parent selection updates inherited values, not intentionally explicit overrides. Resetting an override restores inheritance.

The supervisor keeps the service alive; it is not a second store of model preferences. It must not resurrect stale launch overrides after a configuration change. Running attempts retain their exact route binding; new attempts resolve the current revision.

Status must explain configured selection, effective selection, provenance, and intentional running-attempt pins. Missing configuration, unsupported provider capability, unavailable credentials, and a transient provider outage are different failures and must not share an ambiguous error.

A pre-execution configuration problem should block admission with a corrective action, not consume and fail a work attempt that never started. Correcting configuration should be observed without requiring a ritual of retries and restarts.

## 8. Distinguish failure classes and preserve the real blocker

| Situation | Expected response |
| --- | --- |
| Clearly transient provider failure | Bounded retry under the same route and authority, honoring provider timing |
| Missing configuration or credentials | Actionable admission or attention state; no aggressive retry loop |
| Failed required check | Same-worker repair or a scope/contract decision |
| Strict semantic rejection | Repair the candidate or request help; no random reviewer reruns |
| Authority or integrity mismatch | Fail closed, retain evidence, name the exact mismatch |
| Explicit user pause or worker stop | Respect the stop; no automatic revival |
| Ambiguous interrupted operation | Reconcile ownership and effects before any retry |

A retry delay cap is not a retry budget. Automatic retries need bounded attempts or elapsed time and a truthful exhaustion outcome. Restarts must not reset that accounting.

A later timeout must not erase an earlier pending contract decision. Preserve multiple facts when necessary: "review blocked; worker subsequently exited" is more informative than relabeling the entire episode "provider failure."

Likewise, do not escalate to a more capable model merely because configuration, fixtures, or lifecycle authority are broken. Model escalation should be an explicit policy responding to an appropriate failure class, not a substitute for diagnosis.

## 9. Make blocked work visible without creating more work

For every blocked chain, WG should show:

- The root blocking condition and evidence reference.
- Whether a worker is actively repairing it.
- Any bounded wait and its deadline.
- The affected downstream work.
- The one next action and who may take it.

Compute this from existing graph and runtime evidence, with cycle-safe traversal and deduplication. Do not create a new agent task just to determine that an existing task is blocked.

Use the existing originating conversation or notification bridge when supported. A log line that nobody sees is not sufficient operational escalation. Delivery limitations must be explicit; do not claim a user was notified because an event was written to disk.

Attention notifications should be deduplicated and meaningful, not repeated on every tick. The objective is a decision, not noise.

## 10. Keep product policy out of generic machinery

WG can coordinate code, research, documents, and other bounded work. Core completion logic must not assume Rust, Cargo, a particular model provider, or WG's own repository conventions.

The boundary is:

- **WG core:** ownership, continuity, declared checks, evidence, repair/help, dependencies, and acceptance.
- **Project policy:** what useful work means, applicable commands, risk, and required review.
- **Runtime/build adapters:** Pi tools, process notifications, Cargo caches, or other specific capabilities.
- **WG contributor guidance:** how to develop, test, package, and deploy WG itself.

Infrastructure state such as build-baseline readiness must refresh through its actual owner. It must not require changing model configuration or restarting an unrelated control component.

## 11. Treat deployment as part of reliability

Source landed, checks passed, review accepted, remotely published, and deployed are separate facts. Show them separately. The task's declared finish line determines which are required; do not silently add publication or deployment at the end.

Workers must not replace the global runtime underneath active work. Deployments are coordinated, version-aware operations with explicit ownership and a known recovery path. Verify the running process identity, not merely the executable on PATH.

A new candidate may be tested using an explicit candidate binary. That does not mean its behavior is active in the installed daemon. A failed deployment gate leaves the previous deployment in place and must be reported plainly.

Keep the runtime contract stable for an attempt. Schema or policy changes must have explicit compatibility handling; historical evidence cannot be relabeled to fit a new verifier.

## 12. Judge the whole system, not the number of completed patches

Autonomous development can accumulate locally reasonable safeguards into a globally unusable system. A change is not justified merely because it fixes its own unit test or produces another Done task.

When reliability regresses:

1. Stop adding speculative control mechanisms.
2. Preserve failed runs, versions, configuration, and worktrees.
3. Reproduce representative work in direct Pi (or the relevant agent runtime) as a control, using comparable models, tools, and tasks.
4. Introduce WG's launch, wait, validation, review, and publication layers incrementally. Identify the first boundary that changes the outcome.
5. Compare with a demonstrated reliable WG revision where available; do not infer the regression point from memory alone.
6. Repair or remove the implicated complexity and rerun the complete path.

Do not respond to every stall by creating a new recovery subsystem. Do not silently broaden the scope of every worker until it owns the whole platform. Assign explicit ownership for integration health and regression diagnosis.

Track at least:

- Useful accepted results per human intervention, alongside cost and elapsed time.
- Tasks needing manual lifecycle repair or repeated launch.
- Completion repair rounds, including repeated unchanged review.
- Time spent blocked without an active owner or delivered decision request.
- Lost wakeups, duplicate execution/publication, and orphan processes.
- First-user success against the actual deployed version.

A task that eventually finishes after repeated rescue counts as recovered work, not evidence of an unattended success.

## The release-level proof

Before claiming that the operating loop is reliable, demonstrate a representative batch through the actual production entry points:

| Case | What must be observed |
| --- | --- |
| Ordinary useful task | Clear contract, appropriate checks, one authorized finish |
| Repairable test failure | Same worker repairs and completes without manual lifecycle editing |
| Semantic blocker | Worker can repair, stop, or request a decision without bypassing rejection |
| Incorrect acceptance contract | Explicit proposal and approval; old evidence retained; new binding validated |
| Long managed command | Yield and wake without model busy-wait or premature task failure |
| Completion/cancellation race | No lost result, duplicate action, or revival after cancellation |
| Temporary provider outage | Bounded recovery or actionable exhaustion, not an infinite loop |
| Configuration change | New attempts use the new route; active attempts retain their pins |
| Restart | Ownership and pending decisions survive; no duplicate publication |
| Non-code or non-Rust task | No accidental Cargo or code-only acceptance requirements |
| Cyclic dependency flow | Bounded iteration and accurate blockers, not a false DAG assumption |
| Runtime upgrade | Installed and running identities verified; actual user flow still works |

The standard is not that nothing ever fails. It is that expected failures lead to proportionate repair or a necessary decision, without the user becoming the system's repair loop.

## The design test

Before adding a rule or mechanism, ask:

> What concrete failure does this prevent? Who owns the resulting state? How does work continue when this check refuses? Can we get the same safety with less machinery?

**Make safe completion the ordinary path. Make stopping honest. Make asking for help possible. Keep the machinery subordinate to the work.**
