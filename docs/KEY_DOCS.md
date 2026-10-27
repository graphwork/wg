# Key Documentation Files

**Generated — do not hand-edit.** This file is rendered from
`docs/key-docs.tsv` by `scripts/gen-key-docs.py`.

The curated zones below are the docs a reader is expected to reach; the census at the
end accounts for the rest of the tree. Archive/keep rules live in
`docs/archive/POLICY.md`; the gate for moving a doc is
`scripts/doc-archive-audit.sh`.

- curated docs: **72**
- tracked markdown in the repo: **788**
- regenerate: `scripts/gen-key-docs.py` · verify: `scripts/gen-key-docs.py --check`

---

## Start here (`core`)

The entry points and the policy that governs the doc tree.

| File | Purpose | Audience |
|---|---|---|
| `README.md` | Project overview, install, setup, usage patterns, feature summary | Everyone (entry point) |
| `AGENTS.md` | Layer-2 project guide for this repo: build/dev loop, cargo-install rule, service configuration, profiles/secrets. Universal role contract comes from `wg agent-guide`. Byte-identical twin of `CLAUDE.md` | AI agents (Codex, Cursor, Claude Code) |
| `CLAUDE.md` | Same bytes as `AGENTS.md`; exists because Claude Code looks for this filename. Divergence is a bug (guard-tested) | AI agents (Claude Code) |
| `docs/KEY_DOCS.md` | This file. Generated from `docs/key-docs.tsv` — do not hand-edit | Everyone |
| `docs/archive/POLICY.md` | Documentation archive policy: keep/archive rule, the grep-before-archive gate, archive layout and waves | Contributors |
| `docs/survey/survey-synthesis-subtraction-plan.md` | The ranked subtraction plan that the archive waves execute; also the do-not-cut list | Operators |
| `worksgood.toml` | This repo's project-local route/configuration document (model plane, tiers, reasoning) | Operators, contributors |

## Rules for AI agents (`agent`)

Read before claiming or dispatching work.

| File | Purpose | Audience |
|---|---|---|
| `docs/AGENT-GUIDE.md` | How spawned agents should think about task graphs: patterns, structures, retry/kill mechanics, anti-patterns | AI agents, advanced users |
| `docs/agent-git-hygiene.md` | Git hygiene rules for agents sharing one repo (surgical staging, no stash, no force-push) | AI agents, operators |
| `docs/WORKTREE-ISOLATION.md` | Worktree-based isolation for concurrent agents | Operators, contributors |
| `docs/design/agents-md-slimming.md` | Proposal (unapplied patch) to move the spark-stage narrative out of the always-loaded AGENTS.md/CLAUDE.md | Contributors |

## Using `wg` (`user`)

Install, configure a route, run the graph.

| File | Purpose | Audience |
|---|---|---|
| `docs/README.md` | Documentation overview: core concepts, quick start, storage format, JSON output | Users, contributors |
| `docs/quickstart-pi-openrouter.md` | Verified pushbutton quickstart: install, OpenRouter auth, free-model discovery, plugin install, route selection (canonical quickstart) | New users |
| `docs/COMMANDS.md` | Complete CLI command reference with examples | Users, agents |
| `docs/models.md` | Model, endpoint and API-key management (companion: `docs/config-ux-design.md`) | Users, agents |
| `docs/config-precedence.md` | Config precedence: which layer wins (worksgood.toml vs global vs flags) | Users, operators |
| `docs/config-ux-design.md` | Config UX: `wg config init` / `wg setup`, named profiles, secrets, migration strategy | Users, operators, contributors |
| `docs/AGENCY.md` | Agency system: roles, tradeoffs, evaluation, evolution, skills | Users setting up agency |
| `docs/guides/install.md` | Install paths: operator `cargo install`, npm, `wg dev-sync` dev loop | Users, operators |
| `docs/guides/openrouter-setup.md` | OpenRouter provider setup | Users, operators |
| `docs/guides/server-setup.md` | Server setup guide | Operators |
| `docs/guides/frontier-build.md` | Frontier build profile | Operators |
| `docs/guides/standalone-nex.md` | The standalone `nex` binary | Advanced users |
| `docs/guides/executor-arena.md` | External executor adapter matrix, OpenRouter env setup, init templates | Operators |
| `docs/manual/README.md` | Manual build instructions (Typst source → Markdown) | Contributors |
| `docs/manual/wg-manual.md` | Full assembled manual (derived from the Typst source) | Deep readers |

## Running it (`operator`)

Daemon, lifecycle, recovery, security, logging.

| File | Purpose | Audience |
|---|---|---|
| `docs/AGENT-SERVICE.md` | Service daemon architecture: dispatcher tick, dispatch cycle, agent lifecycle, handler routing | Operators, contributors |
| `docs/AGENT-LIFECYCLE.md` | Hardened agent lifecycle: spawn → work → complete → die, target-dir reaper, dispatcher persistence | Operators, contributors |
| `docs/ops/runbook.md` | Operator runbook: stand-up, verify, teardown | Operators |
| `docs/ops/completion-validation-evidence.md` | The completion-evidence contract a fail-closed `wg done` enforces | Operators, contributors |
| `docs/ops/maze-free-recovery.md` | Recovery when the graph is stuck | Operators |
| `docs/ops/pi-evaluation-rollout.md` | Pi evaluation rollout | Operators |
| `docs/ops/review-projection-recovery.md` | Recovery for the completion-review projection | Operators |
| `docs/ops/scored-evaluation.md` | Scored evaluation | Operators |
| `docs/LOGGING.md` | Logging and provenance: operation log, agent archives, rotation | Operators, integrators |
| `docs/SECURITY.md` | Security guide: pre-commit hooks, GitGuardian, secret management | Operators, contributors |
| `docs/DEV.md` | Development notes: build, test, reusable functions, common pitfalls | Contributors |
| `docs/config-canonical.md` | Canonical config-key audit: what to keep, what is stale, what each key does | Operators, contributors |
| `docs/MODEL_REGISTRY.md` | Model provider registry with quality tiers | Contributors |
| `docs/AGENCY_AUDIT.md` | Agency system audit | Contributors, operators |
| `docs/README-windows.md` | Windows-specific notes | Users on Windows, contributors |

## Design of record (`design`)

Why a shipping behaviour is shaped the way it is.

| File | Purpose | Audience |
|---|---|---|
| `docs/design-handler-first-model-spec.md` | Handler-first model specs: why the leading token of a model spec is always a handler | Contributors |
| `docs/design-project-local-pi-config.md` | Project-local Pi config: `worksgood.toml` as the sole project authority | Contributors |
| `docs/design-two-tier-pi-profile.md` | Two-tier (strong/weak) Pi profile projection | Contributors |
| `docs/design-pi-plugin-install.md` | The idempotent `ensure-pi-plugin` primitive and the embedded plugin cache | Contributors |
| `docs/design-retire-model-registry.md` | Why the model registry was retired in favour of the Pi catalog | Contributors |
| `docs/design-cow-worktrees.md` | Copy-on-write worktrees for agent isolation | Contributors |
| `docs/build-artifact-storage.md` | Build artifact storage / target-cache layout | Contributors |
| `docs/automatic-task-archival.md` | Automatic task-archival behaviour in the graph | Contributors |
| `docs/design/claim-lifecycle.md` | Claim lifecycle for `wg reset` / `wg retry` / the dispatcher reconciler ("Both": eager + lazy) | Contributors |

## Decisions of record (ADR) (`adr`)

Ratified decisions. Never archived because a feature went quiet.

| File | Purpose | Audience |
|---|---|---|
| `docs/ADR-actor-vs-agent-identity.md` | ADR: actor vs agent identity | Contributors |
| `docs/ADR-fed-000-acceptance-brief.md` | ADR series brief: WG-Fed acceptance criteria | Contributors |
| `docs/ADR-fed-001-identity-key-model.md` | ADR-fed-001: identity and key model | Contributors |
| `docs/ADR-fed-002-transport.md` | ADR-fed-002: transport rungs (dumb store, node inbox) | Contributors |
| `docs/ADR-fed-003-custody-delegation-recovery.md` | ADR-fed-003: custody, delegation (UCAN), recovery | Contributors |
| `docs/ADR-fed-004-loadable-state-safety.md` | ADR-fed-004: loadable-state safety (S-5 pipeline) | Contributors |
| `docs/ADR-content-safety-001-review-gate.md` | ADR-CS-001: the inbound-content review gate | Contributors |
| `docs/ADR-content-safety-002-reviewer-hardening.md` | ADR-CS-002: reviewer hardening (no-scope, spotlight, quorum) | Contributors |
| `docs/ADR-content-safety-003-verdict-audit-revoke.md` | ADR-CS-003: verdict audit, digest-pinned consumption, revoke | Contributors |
| `docs/ADR-exec-000-acceptance-brief.md` | ADR series brief: WG-Exec acceptance criteria | Contributors |
| `docs/ADR-exec-e1-placement.md` | ADR-exec-E1: placement matcher | Contributors |
| `docs/ADR-exec-e2-confidentiality.md` | ADR-exec-E2: confidentiality / attested providers | Contributors |
| `docs/ADR-exec-e3-capability-lease.md` | ADR-exec-E3: capability envelope and lease fencing | Contributors |
| `docs/ADR-exec-e4-verification.md` | ADR-exec-E4: disjoint verification of a remote result | Contributors |

## Spark-stage subsystems (`spark`)

Live, smoke-pinned, off the daily path. Read before touching that code.

| File | Purpose | Audience |
|---|---|---|
| `docs/spark/README.md` | Index + reading rules for the spark-stage subsystem docs | Contributors |
| `docs/spark/wg-fed.md` | WG-Fed (Waves 3–6): `wgid:` identity, sigchain, transport, portable state, encryption=ACL, UCAN delegation | Contributors |
| `docs/spark/wg-review.md` | WG-Review: `wg review` inbound-content review gate | Contributors |
| `docs/spark/wg-exec.md` | WG-Exec: `wg provider` remote placement, lease fence, disjoint re-run | Contributors |
| `docs/spark/e2e-family-team.md` | The three sparks composing across two FS-independent instances | Contributors |
| `docs/spark/wg-pilot.md` | WG-Pilot (**RETIRED** — `wg pilot` deleted by `retire-wg-pilot`; historical design record) | Contributors |

## Bundled / generated (`embedded`)

Content shipped inside the binary or derived from another source.

| File | Purpose | Audience |
|---|---|---|
| `src/text/agent_guide.md` | Universal role contract bundled into the `wg` binary and emitted by `wg agent-guide` (chat agent / dispatcher / worker, validation, smoke gate, cycles) | AI agents (any project) |
| `src/commands/quickstart.rs` | `wg quickstart` onboarding text — the "read me first" for an agent in a WG project | AI agents, new users |

## Not indexed — historical zones (census)

These trees are historical (or not-yet-curated) and are deliberately **not**
enumerated here: an index that lists 600 stale files is how this file rotted
last time. Counts are live from `git ls-files`.

| Directory | Markdown files | What it holds |
|---|---:|---|
| `docs/archive/` | 209 | Archived: root reports, historical research, audits, superseded designs |
| `docs/reports/` | 16 | Dated reports and closeouts (wave-2 archive candidates) |
| `docs/research/` | 121 | Research notes and studies |
| `docs/audit/` | 7 | Dated audit trees |
| `docs/plans/` | 13 | Implementation plans for work in flight |
| `docs/design/` | 84 | Additional design docs (subset curated above) |
| `docs/designs/` | 7 | Earlier design drafts |
| `docs/prod-audit/` | 7 | Production-readiness audits |
| `docs/studies/` | 7 | Study documents |

To find a specific doc in those trees:

```bash
git ls-files 'docs/reports/*.md' | xargs grep -l '<topic>'
scripts/doc-archive-audit.sh docs/reports/<file>.md   # is it safe to archive?
```

## Adding a doc to this index

1. Add one row to `docs/key-docs.tsv` (`zone`, `path`, `purpose`, `audience`).
2. Run `scripts/gen-key-docs.py`.
3. Commit the TSV **and** the regenerated `docs/KEY_DOCS.md` together.

A doc that is not canonical does **not** belong in the TSV — it belongs in the
census, or in `docs/archive/` if `scripts/doc-archive-audit.sh` says so.
