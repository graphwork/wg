<!-- provenance: extracted verbatim from AGENTS.md (sections now stubbed there) by
     task `doc-archive-policy`. Text is unchanged apart from heading level; the
     stub in AGENTS.md/CLAUDE.md links here. See docs/archive/POLICY.md. -->

# WG-Exec — the execution-federation plane (`wg provider`)

*Extracted from `AGENTS.md` § "WG-Exec — the execution-federation plane (`wg provider`, Exec Spark)".*

WG-Exec is the **execution-federation** layer decided in
`docs/execution-federation-study/06` (§4 the spark, §5 Exec-Wave B) and ratified in
`docs/ADR-exec-e1..e4-*.md` (indexed by `docs/ADR-exec-000-acceptance-brief.md`). The
**Exec-Wave B spark** (`src/providers/` + `wg provider` in `src/commands/exec_fed_cmd.rs`)
is the first execution-plane code: the thinnest end-to-end slice that proves *"one task,
a borrowed box, a scoped leash"* — a task placed on a **separately-owned remote provider**
runs under **two scoped UCANs (never the agent's root key)**, reads only its sealed slice,
and signs a result back, with the provider demonstrably **unable to exceed its lease** and
a hostile provider's corrupted result **caught by a disjoint re-run vs a pinned spec**.

It **composes with WG-Fed and invents no second trust system** (NFR-4): identity (`wgid:`),
the sigchain, the custodian-held-root signing boundary, the **attenuating UCAN**
(`identity::custody`), and the **per-recipient sealed envelope** are all reused verbatim.
WG-Exec owns only the **execution wire** (the five envelopes), the **lease-epoch fence**,
and **how the leash dial wires UCAN scope/TTL + lease term/cadence together per task**. Its
prerequisite is the WG-Fed spark (`federation_spark_two_graphs`) passing first.

- **`src/providers/`** — `mod.rs` (`WG_EXEC_COMPAT_VERSION`, an **authenticated loud-fail**
  handshake mirroring `WG_FED_COMPAT_VERSION`; the **five wire envelopes**
  `PlacementOffer` / `Claim` / `RunGrant` / `LeaseRenewal` / `ResultEnvelope`; the
  `ProviderRegistry` whose `ProviderEntry::is_live` is **signed-renewal** liveness, never a
  local PID; `TrustLevel` is `graph::TrustLevel` — the one trust dial). `placement.rs` — the
  matcher (**hard filter** capability+trust-floor → **advisory deterministic rank**) and the
  **fail-closed `leash()` engine** (confidential ⇒ attested-C-or-refuse, **unlabeled ⇒
  refuse, never A**). `bundle.rs` — build / seal / verify the minimal `ContextScope` slice
  over WG-Fed crypto (encryption=ACL). `lease.rs` — the signed `Lease` + the **monotonic
  epoch atomic-CAS fence** at the single canonical-write boundary (closes the double-commit
  / replay). `verify.rs` — attribution (mandatory but **not** integrity) + a **single
  disjoint re-run in a trusted domain (never the producer — X-5) vs a *pinned* spec (not the
  provider's shipped tests — X-6)**.
- **Dispatch seams** — `handler_for_model.rs`'s `ExecutorKind` gains a **`RemoteRunner`** arm
  (driven by the providers plane, not the local spawn-task handler) and `plan_spawn`'s
  `SpawnPlan` gains a **`placement` field** (`Placement::{Local, Provider(wgid:)}`; `Local`
  reproduces today's spawn byte-for-byte). The capability-gated claim is `wg provider claim`
  (the four-part eligibility proof); the local `wg claim` is unchanged (a local worker
  trivially clears the trusted-pool filter).
- **The two scoped UCANs reuse WG-Fed's UCAN** verbatim: an *act-as-agent* UCAN
  (`act-as-agent` on `agent://<G>/task/<T>`) and a *graph-write* UCAN (`graph/write` on
  `graph://task/<T>` only — **never** `graph://*`), both issued via `custody::issue_root`
  with the leash-decided scope/TTL. A `RunGrant::field_scan` asserts **no root key, no
  blanket graph-write** in the delivered bytes.

```
wg provider enroll <wgid> --trust verified --model claude:opus --isolation container
wg provider offer  --as-name <agentG> --task T --model … --isolation … [--sensitivity normal|high|confidential] --provider <wgid> --out offer.json
wg provider claim  --as-name <providerP> --offer offer.json --store <L> --out claim.json
wg provider grant  --as-name <agentG> --claim claim.json --task-input <file> [--after dep=path] [--ucan-ttl-secs N] --store <L> --out grant.json
wg provider run    --as-name <providerP> --grant grant.json --store <L> --out result.json [--target-task U] [--corrupt] [--scope-probe <secret>]
wg provider accept --result result.json --store <L> [--now <rfc3339>]
wg provider reclaim --task T [--new-provider <wgid>]      # bump the lease epoch
wg provider verify --result result.json --verifier <Q> --pinned-spec spec.json --store <L>
wg provider show <…> | wg provider providers              # surface the applied leash + liveness
```

Compatibility is gated by `WG_EXEC_COMPAT_VERSION` in `src/providers/mod.rs` (loud-fail on
incompatible mismatch). The six-step end-to-end proof is pinned by
`tests/smoke/scenarios/exec_spark_borrowed_box.sh` (`owners = [exec-spark]`): place on a
separately-owned provider (no root / no blanket write in the grant) → run under the scoped
UCAN reading only its slice → sign a result back (wrong-signed rejected) → the provider
cannot exceed its lease (wrong-task write / post-expiry sign / replay + stale-after-reclaim
all rejected) → a hostile provider's corrupted + test-poisoned result is caught by a disjoint
re-run vs the pinned spec → a confidential task to a non-attested provider is refused, never
shipped in plaintext (fail-closed). **Spark boundary** (memo §4.3): the attestation slot is
exercised by the **fail-closed refuse**, not a real enclave; the integrity lever is a
**single disjoint re-run**, not quorum; the pinned-spec re-run + `auto_evaluate` eval-gate are
deterministic stubs — the real enclave (Exec-Wave D), the verified-overflow B tier, and the
production weak-tier re-run are later waves.
