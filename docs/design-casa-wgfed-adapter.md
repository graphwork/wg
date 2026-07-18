# Casa adapter over the WG federation substrate

**Status:** Proposed design; **implementation release blocked pending explicit maintainer approval**

**Date:** 2026-07-18

**Owner task:** `design-casa-adapter`

**Decision channel:** [graphwork/wg#58](https://github.com/graphwork/wg/issues/58)

**Inputs:** [Casa/WG overlap study](reports/luca-casa-wgfed-overlap-2026-07-18.md), [Luca coordination record](reports/luca-coordination-2026-07-18.md), WG-Fed/Review/Exec ADRs and current source

## 1. Decision and approval state

Casa SHALL be a household product/channel adapter over WG-Fed, WG-Review, and
WG-Exec. It SHALL NOT introduce a second principal, trust, capability, message,
replay, recovery, or remote-result authority plane.

The boundary is:

- **WG-Fed is authoritative** for cryptographic principals (`wgid:`), key
  continuity, signatures, recipient ACL encryption, offline transport, freshness,
  UCAN authority, and revocation/recovery.
- **WG-Review is authoritative at the consumption edge.** Authenticated inbound is
  still untrusted content. Casa election, composition, conversation memory, task
  creation, and fast writes may see only an accepted, digest-pinned item whose trust
  was derived by WG.
- **WG-Exec is authoritative for remote execution.** Casa cannot invent a provider,
  lease, grant, result, or completion shortcut.
- **Casa is authoritative only for Casa product policy and connector operation:**
  household presentation/roles, persona aliases/voice/emoji, election and routing
  policy, conversation panes, reminders/pacing, channel update cursors, and UI copy.
  That authority never establishes WG identity, trust, or graph-write permission.
- Casa feed/thread files are **rebuildable read models**, not event, identity,
  delivery, or task ledgers.

This document is **not accepted merely by being committed**. As of
`2026-07-18T10:12:53Z`, issue #58 is open with **zero comments**. Luca has neither
accepted nor rejected this boundary. No implementation task may be published,
made ready, or dispatched until a `graphwork/wg` maintainer explicitly approves
this document (or a named revision) in issue #58 or a design PR. Any task drafted
before that approval must remain unpublished/paused. External Casa gateway changes
also require the gateway source and owner confirmation in the stage-0 gate (§12).

### Luca position ledger (facts, not inference)

| Topic | Evidence | Agreement/disagreement recorded |
|---|---|---|
| Rust per-agent ledger in fork PR #2 | Luca closed [fork PR #2](https://github.com/lucapinello/workgraph/pull/2#issuecomment-4984501851) at `78e09978`, saying the gateway-side `claw3d-bridge ledger.mjs` “superseded” it and was live with threads, dedupe, mirror-out, and replay. | **Confirmed direction:** the Rust PR is obsolete relative to Luca's gateway ledger. This says nothing about whether that ledger is authoritative or a projection under this design. |
| Upstream-vs-Casa split and WG-Fed/Review/Exec boundary | Erik asked Luca to confirm the split in [issue #58](https://github.com/graphwork/wg/issues/58). | **No response; no agreement or disagreement.** |
| Reviewable source after `integration/casa-pinello@64f2cba8` and the external gateway | Issue #58 asks for the newer branch/repository/PR. | **No response; source availability unresolved.** |
| Casa commits implementing roster, persona, election, gateway auth, feed, lifecycle, and report-back | Verified by the overlap study. | Evidence of implemented product intent, **not consent to this authority boundary**. |

A later Luca response must be appended to this table verbatim or accurately
summarized with a permalink. Silence MUST NOT be converted into acceptance.

## 2. Existing substrate and compatibility baseline

This design reuses the current primitives rather than restating their cryptography:

- `WG_FED_COMPAT_VERSION = 0.4.0` (`src/identity/mod.rs:141`) and envelope version 1.
- `IdentityRecord`, `StateSnapshot`, per-recipient `SealedEnvelope`, and
  `SignedEvent` (`src/identity/envelope.rs:61,115,203,293`).
- untrusted at-least-once `FedStore` (`src/identity/transport.rs:132`) over file or
  WG node HTTP, with content-addressed objects, inbox events, ack/delete, and
  freshness attestations.
- recipient-scoped, signature-pinned consume dedupe (`DedupStore`,
  `src/identity/dedup.rs:33`).
- attenuating, expiring UCAN `Capability` and signed revocation
  (`src/identity/custody.rs:294,493`).
- canonical author-trust resolution (`resolve_author_trust`, `src/trust.rs:117`),
  sourced from peer trust and only tightened by provider trust.
- the IC1–IC4 review pipeline and exact-byte digest pin (`review_inbound`,
  `src/review/mod.rs:311`) plus verdict audit/revoke.
- `RunGrant` and `ResultEnvelope` (`src/providers/mod.rs:487,560`),
  `WG_EXEC_COMPAT_VERSION = 0.1.0`, and the lease epoch atomic commit fence.

The existing `SignedEvent.kind` and typed JSON body/`refs` are sufficient for Casa
v1. New Casa payload kinds do **not** change `SignedEvent` semantics and therefore do
not, by themselves, justify a WG-Fed compat bump. Each new payload has its own
`schema` major and unknown majors fail closed. If implementation changes envelope
signing, sealed-sender binding, recipient meaning, or transport behavior, it MUST
bump `WG_FED_COMPAT_VERSION` and pass the existing loud-fail handshake. Any change
to the five WG-Exec envelopes or acceptance semantics MUST bump
`WG_EXEC_COMPAT_VERSION`; Casa must otherwise use them unchanged.

No `WG_REVIEW_COMPAT_VERSION` is introduced. Review verdicts continue to ride the
existing WG-Fed/audit substrate.

## 3. Canonical identity and resource mapping

### Invariant

A thing that can author, receive confidential content, issue authority, or hold a
recoverable continuity identity gets a distinct `wgid:` and sigchain. A product
collection or data store is a **resource**, not automatically a principal. Channel
IDs and display labels are aliases/evidence, never cryptographic identities.

| Casa/WG thing | Canonical mapping | Custody / authority | Explicit non-equivalences |
|---|---|---|---|
| Human (for example Luca) | One human `wgid:`. A local `human-luca`/agent row is a projection keyed to it. | Human/root custodian and configured recovery policy. Human or authorized household admin issues narrow device/channel capabilities. | Not a Telegram numeric ID, username, browser cookie, Casa `humanId`, local agent ID, owner string, or trust level. |
| Persistent agent/persona (Nora, Bruno, Otto, Mira) | One agent `wgid:` per durable persona, with `IdentityRecord.agent_fields`; name, emoji, voice, role, and model are aliases/policy. | Agent signer/encryption keys under the chosen custodian; capabilities determine actions. | Not a bot token/ID, LLM model, execution provider, local process, or human operator. One persona may use many channels; one bot may deliver many personas. |
| Telegram bot / Casa bridge | The external bot account remains a channel endpoint. The process that controls it is a **service `wgid:`**. Use separate service identities when credentials/custody or revocation domains differ; otherwise multiple bot endpoints may map to one narrowly scoped gateway service. | Bot token remains in `wg secret`; service signer asserts what the external channel delivered and receives only `channel/assert`/`message/deliver` capabilities. | A bot does not become the persona whose text it renders, and its numeric bot ID/token is not a `wgid:`. |
| Browser/tablet/device | A device `wgid:` or device signer bound to a short-lived session capability; display label stays presentation-only. | Human/service authorizes a scoped, expiring UCAN. Revoke device/session separately from the human. | Not the human, household, gateway, cookie, or IP address. |
| Household | A Casa resource `casa://household/<household_id>` plus a versioned, signed roster update. `household_id` is an opaque stable UUID, never a name/chat ID. The household has no signing key by default. | Named human/agent/service `wgid`s sign or act through UCANs on concrete household resources. Content is sealed to the current individual recipient `wgid`s. If an inbox actor is needed, it is a distinct household-ingress **service** `wgid`, not “the household key.” | Not a shared secret, Telegram group chat, roster filename, collective display name, or implicit global ACL. |
| WG graph | A resource, not a person. Casa stores `GraphRefV1 { instance_id, authorizer_wgid }`; `instance_id` is an opaque stable local instance UUID, not a filesystem path. In v1, current WG-Exec resources remain `graph://task/<task_id>` and are interpreted under the issuing authorizer/graph. | The authorizer `wgid:` signs placement/result decisions and issues task-scoped `graph/write`; graph state itself does not sign. | Not the household, gateway, repository path, Git remote, task author, or provider. A future globally qualified graph URI is a separate WG design and compat decision. |
| Remote provider/borrowed box | Provider `wgid:` in `ProviderRegistry`. | WG-Exec trust/leash, signed renewals, two UCANs, and lease epoch. | Not a persona or author-trust endorsement. Verified compute cannot promote content author trust. |

### Casa roster schema

Roster and election remain Casa policy, but all references are canonical:

```json
{
  "schema": "casa.household.v1",
  "household_id": "9d3d…",
  "graph": {"instance_id": "45ab…", "authorizer_wgid": "wgid:z…"},
  "revision": 7,
  "prev_cid": "b3:…",
  "admins": ["wgid:zHuman…"],
  "members": [
    {"wgid": "wgid:zHuman…", "kind": "human", "aliases": ["Luca"]},
    {"wgid": "wgid:zAgent…", "kind": "agent", "aliases": ["Nora"],
     "presentation": {"emoji": "…", "voice": "…"}}
  ],
  "ingress_services": ["wgid:zGateway…"]
}
```

A roster revision is the body of a signed `casa.household-update.v1` event from an
admin (or a holder of `household/roster-write` on that exact household), linked by
`prev_cid`. It chooses Casa candidates and recipients; it cannot set peer trust,
authorize graph writes by itself, or recover a key. Aliases are escaped display data.

## 4. Generic channel evidence seam

Telegram/browser infrastructure authenticates an external account or device, not a
human's WG key. Preserve that honest distinction with two generic schemas. These may
live in a small `src/identity/channel.rs` only after maintainer approval; until then
they belong in the Casa adapter crate/module and use WG signing/CAS helpers.

```rust
struct ChannelBindingV1 {
    schema: "wg.channel-binding.v1",
    issuer_wgid: String,        // subject or authorized household admin
    subject_wgid: String,       // human principal being associated
    service_wgid: String,       // gateway/listener allowed to assert delivery
    channel: String,            // "telegram" | "casa-web"
    external_subject_cid: String,
    ceremony_cid: String,
    created_at: String,
    expires_at: Option<String>,
    sequence: u64,
    authority_capability_cid: Option<String>, // required when issuer != subject
    service_capability_cid: String,
    sig: String                 // verified against issuer_wgid's sigchain
}

struct ChannelAssertionV1 {
    schema: "wg.channel-assertion.v1",
    service_wgid: String,
    subject_wgid: Option<String>,
    binding_cid: Option<String>,
    native_event_ids: Vec<String>,
    received_at: String,
    channel_context_cid: String,
    content_cid: String,
    capability_cid: String,
    sig: String                 // service/device signer
}
```

`external_subject_cid` is a domain-separated BLAKE3 correlation of canonical channel
evidence. It is not authentication by itself and, for low-entropy numeric IDs, is not
claimed to provide secrecy. Raw Telegram/user/chat IDs live only in a protected local
locator store or a sealed object. The signed binding, capability, and assertion are
the proof.

Required APIs:

```rust
verify_channel_binding(binding, issuer_auth, authority_cap, now,
                       revocations) -> VerifiedBinding
verify_channel_assertion(assertion, service_auth, service_cap, binding, body, now,
                         revocations) -> VerifiedChannelProvenance
casa_ingest(event: SignedEvent, assertion: ChannelAssertionV1) -> HeldInbound
```

Verification checks schema major, all `wgid`s, subject/service consistency, body CID,
binding/capability scope and time, sigchain status/freshness, and revocation. The
subject may bind itself; an admin issuer additionally proves a capability that permits
`channel/bind` on that exact subject/household. It never accepts a name match. The
canonical `SignedEvent.from` is the service/device signer; `subject_wgid` records whose
external account supplied the content. Review trust is the strictest of
`resolve_author_trust(service_wgid)` and, when present,
`resolve_author_trust(subject_wgid)`, further tightened by review revoke overrides.
Onboarding/binding never auto-sets that local peer trust; it must be asserted separately
by the consumer. A service/provider enrollment can never upgrade the human author.

### External-gateway bootstrap exception

There is one genuine boundary WG-Fed cannot remove: Telegram and a not-yet-enrolled
browser do not initially possess a WG signer. Therefore:

- TLS, the Telegram bot token/webhook secret, browser session auth, Origin/CSRF
  controls, and a high-entropy one-use `login_`/`join_` challenge may authenticate the
  **bootstrap ceremony** at the external gateway.
- The challenge is subject/device-bound, expires, is atomically single-use, and only
  its digest/transcript CID is retained. Raw nonce, token, and cookie are never a
  principal ID or logged.
- Completion must yield a subject/admin-signed `ChannelBindingV1` and/or a narrow,
  expiring UCAN. Subsequent actions use signatures and capabilities, not the nonce.
- A local shared secret may remain mandatory defense-in-depth against browser-to-
  loopback or listener-to-gateway request forgery. It is stored by `wg secret`,
  rotated, and fails closed. It authenticates a process hop only; holders cannot be
  treated as Luca/Nora/the household or granted blanket action authority.
- The current optional `x-casa-auth-secret` downgrade and authoritative
  `--sender <name>` path are forbidden after cutover. Legacy absence is an error,
  never unauthenticated compatibility.

This exception does **not** create a parallel federation trust root. It ends when the
gateway emits a verifiable assertion under its service key and scoped capability.

## 5. Canonical inbound pipeline

The required order is structural:

```text
external bytes
  → connector limits/normalization and native update cursor
  → verify service/device SignedEvent + ChannelAssertion + binding/capability
  → signature-pinned receive/replay record
  → derive strictest WG author trust + revoke override
  → WG-Review IC4 over exact bytes
  → if directive/task/write: WG-Review IC1 + taint/sensitivity
  → atomically mark exact digest consumable once
  → Casa recipient election/routing
  → compose or perform a typed UCAN-authorized action
```

Rules:

1. Transport and channel authentication run before Review; forged/tampered content is
   rejected, not “reviewed.” A valid signature proves author/provenance, never safety.
2. `Verdict::Quarantine` and `Reject` withhold the body from election prompts,
   conversation state, composers, task creation, `TASK_CREATE:`, and plan writers.
3. `Accept` permits only the exact digest (`VerdictStore` consumption pin). It does
   not turn content into system instructions. Prompts receive accepted text as
   spotlighted data with bounded provenance.
4. A directive is parsed into a typed operation only after IC4/IC1 acceptance. The
   operation then requires a UCAN such as:
   - `message/send` on `casa://household/H/thread/T`;
   - `household/plan-edit` on `casa://household/H/plan/<date>`;
   - `task/create` on `graph://task/<new-id>` or the approved graph creation scope;
   - `household/roster-write` on exactly `casa://household/H`.
   Channel binding alone grants none of these.
5. Casa election returns agent `wgid`s; aliases and bot endpoints are selected only
   for rendering/delivery after the principal decision.

### Replay and dedupe state

`FedStore` is at-least-once and the signed event ID (or sealed-sender inner CID) is the
canonical replay key. Telegram update/message IDs and Casa `srcId` are retained only
as connector evidence.

The adapter needs a crash-safe receipt state keyed by `(recipient_wgid, dedup_key)`:

```rust
enum IngressState { Received, Reviewed, Consumable, Consumed, Quarantined, Rejected }
struct IngressReceiptV1 {
    event_cid: String,
    recipient_wgid: String,
    state: IngressState,
    review_record_cid: Option<String>,
    first_seen_at: String,
    consumed_at: Option<String>,
}
```

The transition to `Consumed` must be an atomic create/CAS so concurrent pollers cannot
both act. A quarantine must remain re-reviewable; “first seen” is not synonymous with
“consumed.” Its authenticated bytes/CAS reference remain held, and the FedStore inbox
is not ack-deleted as consumed, until resolution or an explicit retention decision.
Current `DedupStore::check_and_record` is single-process and records first sight before
the live poll review. Casa SHALL wrap/extend it with the above states rather than use a
pre-review marker as proof of consumption. Re-delivery of an accepted event never
re-consumes; two genuinely distinct signed events with identical text/time remain
distinct.

## 6. Signed/encrypted lifecycle and delivery

Casa lifecycle messages are projections of canonical graph/Exec state, not completion
claims from a worker or chat sink.

1. An accepted request records `request_event_cid`, requester subject/service
   `wgid`s, selected persona `wgid`, `review_record_cid`, and protected report-back
   locator reference on the created WG task/provenance record.
2. A graph-authorizer service observes canonical task transitions and emits one signed
   `casa.lifecycle.v1` event per transition. The event references the request CID,
   `GraphRefV1`, task ID, monotonic transition sequence, and (for remote completion)
   accepted `ResultEnvelope.cid` and lease epoch.
3. `Done` may be emitted only after local WG completion gates succeed, or for remote
   work after result signature/UCAN attribution, IC2 Review, integrity policy, and the
   lease ledger's `try_commit` all succeed. Provider output, a graph bit written
   outside the fence, or Telegram API acceptance cannot create `Done`.
4. The lifecycle event is sealed with `SignedEvent::new_sealed_multi` to the actual
   recipient set when it crosses a host/trust boundary. Household membership is
   expanded to individual current encryption keys; a removed member gets no future
   wrap. Sealed sender may hide author metadata from the relay when required.
5. A WG recipient receives it through the node/FedStore inbox while offline, verifies
   cached/current sigchain and freshness, dedupes, and takes the normal trust-derived
   IC4 gate before display/automation. A typed lifecycle renderer never executes text.
6. An external Telegram/web destination receives a rendering of the already-signed
   canonical event through a durable delivery adapter. The rendering records the
   canonical event CID; it never becomes a new source of task truth.

### Durable outbox/receipt seam

Casa's retry and report-back need a durable, idempotent connector state, not a new
message ledger:

```rust
struct DeliveryIntentV1 {
    schema: "wg.delivery-intent.v1",
    event_cid: String,
    destination_id: String,       // opaque key into protected locator store
    render_profile: String,
    created_at: String,
}
struct DeliveryAttemptV1 {
    intent_cid: String,
    attempt: u32,
    state: Queued | AttemptUnknown | ApiAccepted | FailedRetryable | FailedPermanent,
    native_message_id: Option<String>,
    observed_at: String,
    error_code: Option<String>,   // bounded, no attacker text/secrets
}
```

Uniqueness is `(event_cid, destination_id)`. Network timeout is
`AttemptUnknown`; retry reuses the intent/idempotency key. Telegram `ok:true` plus a
message ID is only `ApiAccepted`, not human delivered/read. Feed projection success is
a separate state. “Delivered” or “read” is legal only when that channel supplies such
evidence. The append-only attempts are audit data; content remains in the signed event.

A generic outbox may later be accepted into WG because other channels need it, but
Casa SHALL first implement against this narrow interface. WG core must not absorb
Casa retry counts, wording, pacing, bot fallback, or notification policy.

## 7. Authoritative state versus adapter/read models

| Data | Authority | Casa treatment |
|---|---|---|
| Principal keys, active signer/encryption keys, rotation/recovery | WG-Fed sigchain + custodian | Reference by `wgid`/kid only; never copy private/root keys into Casa. |
| Channel binding/assertion and scoped action authority | Signed binding/assertion + UCAN/revocation | Adapter creates/verifies; local alias index is a cache. |
| Author trust and review depth | `federation.yaml` peer trust, strictest trust resolver, Review revoke override | No Casa trust enum/score. Roster membership cannot upgrade trust. |
| Canonical inbound/outbound message identity | Authenticated SignedEvent/inner CID | Native IDs/`srcId` are evidence/indexes only. |
| Confidential recipient access | SignedEvent per-recipient wraps (`to` is ACL) | UI/product chooses intended recipients, WG crypto enforces them. |
| Content consumption decision | WG-Review verdict record + exact digest pin | Casa acts only on accept; quarantine remains held. |
| WG tasks/dependencies/status | WG graph | Casa renders/projects and stores request refs; it cannot override status. |
| Remote placement/result/completion | WG-Exec offer, claim, grant, lease ledger, result, accept | Direct reuse; no Casa provider/lease/result records as authority. |
| Household membership, aliases, election/domain owner, voice/emoji, reminder policy | Versioned Casa household policy signed/authorized by named principals | Product-authoritative for Casa routing/presentation only; never identity/trust/capability authority. |
| Telegram/web locator, update cursor, bot fallback, transcription job | Protected connector operational state | Adapter-authoritative only for contacting/polling the external service. |
| Delivery attempt/API acceptance | Durable outbox/receipt | Operational delivery truth with precise states; not task/result truth. |
| `group-feed.jsonl`, per-human/per-agent thread files, conversation panes | Rebuildable Casa projection | Never accepted as proof of author, ACL, review, delivery, task, or completion. |
| Portable conversation context | Signed/sealed `StateSnapshot` and ADR-fed-004 load gate | Projection may cache; any agent load uses IC3 safety/model binding. |

A projection row may expose the calm legacy fields, but its protected form is versioned:

```json
{
  "schema": "casa.projection.v2",
  "eventCid": "b3:…",
  "authorWgid": "wgid:zGateway…",
  "subjectWgid": "wgid:zHuman…",
  "recipientWgids": ["wgid:zAgent…"],
  "reviewRecordCid": "b3:…",
  "direction": "inbound",
  "channel": "telegram",
  "channelMessageId": "protected-or-null",
  "inReplyTo": "b3:…",
  "deliveryState": "api-accepted",
  "text": "…",
  "display": {"sender": "Luca", "persona": "Nora"}
}
```

The six/eight-field legacy UI can be derived from this. A local attacker may corrupt
the projection, so readers verify/rebuild from canonical signed events and verdicts.
Plaintext projections require explicit 0600 files/0700 directories, encryption-at-rest
policy, bounded retention, and documented household-operator visibility.

## 8. Privacy and ACL rules

- Portable/cross-host content is sealed to individual current recipients. `to` is the
  ACL; a household name or shared secret is not.
- Canonical events minimize external locators. Raw Telegram IDs, chat IDs, bot tokens,
  cookies, nonce values, health/meal text, and device labels do not enter public heads,
  logs, errors, or unsealed refs. Low-entropy ID hashes are not advertised as secrecy.
- Endpoint/alias metadata is independent of message authorship. A fallback bot may
  deliver Nora's event without becoming Nora.
- Group-to-DM or DM-to-group routing changes require explicit recipient recomputation
  before sealing; a bot-loop guard is not an ACL.
- Revoking a household member removes future key wraps and capabilities. It cannot
  erase plaintext legitimately received earlier; that residual is surfaced.
- Static recipient keys support offline delivery but not forward secrecy. Rotation
  limits future exposure; an online ratchet is outside this adapter.
- Media is untrusted before transcription. Enforce byte/MIME/time/process limits,
  retain source/transcript derivation CIDs, and review transcript/media metadata before
  use. A generic media sandbox is a later WG seam, not Casa trust policy.

## 9. Rotation, recovery, audit, and revoke

- Human/agent/service/device signer and encryption keys rotate/revoke/recover through
  their WG-Fed sigchains. Stable `wgid` survives legitimate root recovery.
- Channel bindings and device/session capabilities have independent expiry/revocation.
  A Telegram account change does not rotate the human root; a compromised gateway
  service key does not become the human key.
- High-value household writes and capability checks require fresh sigchain/revocation
  heads and fail closed on stale/rollback sequence according to WG-Fed freshness.
- Every acted-on item links event CID → assertion/binding CIDs → Review verdict CID →
  typed operation/UCAN CID → task/result CID → lifecycle event CID → delivery intent
  and attempts. Bounded reason/error codes prevent second-order injection.
- Later-discovered poison uses `wg review revoke`: lower the local author trust,
  identify downstream consumers to rerun, and block the next item more deeply. Revoked
  capabilities kill their delegated subtree. A projection row is never the revoke key.
- Copying public identity bundles, `.casa` files, browser cookies, bot maps, or
  `auth-confirm.secret` cannot enroll a signer or issue a capability. Same-self
  continuation still requires the WG-Fed enrollment/recovery ceremony; otherwise it is
  a fork.

## 10. Migration from existing Casa files

Migration is fail-closed and reversible until cutover:

1. **Inventory and freeze authority growth.** Pin the Casa/gateway commits and obtain
   the missing `claw3d-bridge` source. Inventory `household.toml`, human/Telegram
   bindings, bot map/token refs, `.casa/auth-confirm.secret`, group feed, thread JSONL,
   lifecycle fired log, update cursors, and report-back locators. New string/secret
   authority mechanisms are frozen.
2. **Archive, hash, and protect.** Copy legacy files read-only, record a BLAKE3 manifest,
   modes, parser version, and source path. Do not publish secrets or raw locators.
3. **Mint/map identities.** Create/recover human, agent/persona, service, and device
   `wgid`s. Create `GraphRefV1` and Casa household revision 1. Mapping is explicit and
   reviewed; duplicate names never auto-merge principals.
4. **Re-enroll external bindings.** Existing numeric Telegram bindings can seed a
   *pending* candidate, but cannot be transformed silently into a subject signature.
   Run a fresh one-use ceremony and issue signed binding/capability. Rotate the local
   defense secret and remove fail-open fallback.
5. **Import history as untrusted history.** Legacy feed/thread rows become
   `casa.legacy-projection.v1` records with `legacy_source_cid`, original ordinal, and
   unknown author provenance. Do **not** mint retroactive signed events claiming Luca
   or Nora authored unsigned rows. If history is loaded into a model, wrap it in a
   signed/sealed `StateSnapshot` and run the IC3 load-safety/human gate.
6. **Build canonical ingress in shadow mode.** New real/stub channel input creates
   signed assertions/events, Review verdicts, receipt state, and v2 projections while
   the legacy UI remains read-only. Compare panes, routing, and delivery without
   granting the shadow path writes.
7. **Capability-gated dual-run.** Enable accepted typed operations for a test
   household/graph. Prove legacy unauthenticated `--sender`, missing secret, alias spoof,
   replay, and non-accept content cannot mutate graph/plan/roster.
8. **Cut over atomically.** Stop legacy writers; persist final cursor; enable canonical
   ingest/outbox; rebuild projections; verify counts by canonical event CID. Keep a
   rollback pointer that disables writes rather than re-enabling legacy authority.
9. **Retire and monitor.** Delete/expire raw nonces and obsolete secrets, retain the
   protected audit archive per policy, revoke unused service/device caps, and monitor
   quarantine/replay/outbox failures. Legacy `srcId` remains only in migration metadata.

Malformed/torn JSONL is reported by ordinal and skipped only in the projection import;
it never changes canonical state. Concurrent import/cutover uses a lock and durable
cursor. Re-running a migration step is idempotent by `(legacy_source_cid, ordinal)`.

## 11. Concrete module/API ownership

The smallest implementation surface, subject to approval, is:

| Module/seam | Ownership | Public contract |
|---|---|---|
| `identity::channel` or adapter equivalent | Generic candidate; WG maintainer decides whether core | Versioned signed `ChannelBindingV1`/`ChannelAssertionV1` verification only. No Telegram/Casa policy. |
| `casa::identity_map` | Casa adapter | Resolve aliases/native accounts to canonical refs only after verified binding; render aliases. |
| `casa::ingress` | Casa adapter over Fed/Review | Verify → receipt/replay → strictest trust → IC4/IC1 → exact-digest consume. Returns accepted typed data or withheld status, never raw non-accept body. |
| `casa::household` | Casa product | Signed/UCAN-authorized roster revisions, candidate election, aliases/voice/emoji. Cannot write peer trust or issue unscoped graph caps. |
| `casa::projection` | Casa product read model | Rebuild v2 feed/threads from accepted events/verdict/outbox; legacy import. |
| `delivery::outbox` or adapter equivalent | Generic candidate; initially narrow adapter | Durable `(event_cid,destination)` intent and precise attempt states. No notification wording/pacing. |
| `casa::task_bridge` | Casa adapter | Create WG task only from accepted digest + valid typed capability; persist request/persona/review/report-back refs. |
| `casa::lifecycle` | Casa adapter | Observe canonical local/Exec accepted transitions; emit signed lifecycle events and outbox intents. |
| WG-Exec | Existing core, unchanged | Offer/claim/grant/run/result/accept and epoch fence. |

A future public CLI must accept a signed event/assertion file or CID, not authoritative
`--sender` text. Suggested service interface:

```text
POST /wg/casa/v1/ingress     SignedEvent + ChannelAssertion (body may be sealed)
GET  /wg/casa/v1/held/:cid   metadata/verdict only; body only if authorized
POST /wg/casa/v1/action      accepted_event_cid + typed action + capability
GET  /wg/casa/v1/outbox/:id  precise delivery state
POST /wg/casa/v1/rebuild     projection version + cursor (admin capability)
```

Every mutation endpoint independently verifies signature, schema major, capability,
resource, expiry/revocation/freshness, and digest; localhost and shared secret alone
are insufficient.

## 12. Dependency-ordered implementation plan (held until approval)

These are plan stages, **not released WG tasks**:

0. **Approval and source gate.** A WG maintainer accepts this revision; record Luca's
   reply/non-reply accurately; obtain reviewable external gateway source and schemas;
   confirm ADR/compat assumptions against then-current main.
1. **Contract tests and fixtures first.** Freeze schema JSON fixtures, unknown-major
   loud failure, identity mapping, capability resources, and two-filesystem threat
   fixtures. No product route yet.
2. **Channel evidence primitive.** Implement binding/assertion verification and fresh
   nonce ceremony; reuse current numeric Telegram sender parsing; remove name authority.
   Security review is required before merge.
3. **Ingress receipt + Review wiring.** Implement crash-safe received/reviewed/consumed
   state and strictest derived trust. Prove hostile bytes never reach election/compose.
   This depends on stage 2.
4. **Casa canonical identity/household adapter.** Map humans/personas/bots/devices,
   graph reference, signed roster revisions, and typed UCAN resources. Depends on 2–3.
5. **Projection/migration tooling.** Add v2 rebuildable feed/threads and untrusted
   legacy importer. Depends on canonical event/verdict IDs from 3 and mapping from 4.
6. **Durable outbox and channel sinks.** Add precise delivery attempts, Telegram/web
   renderer, retries, and offline WG-node path. Depends on signed canonical events and
   projection IDs; stays independent of task execution.
7. **Task bridge and lifecycle.** Create tasks from accepted/capable typed operations;
   emit lifecycle only from canonical local or accepted WG-Exec state. Depends on 3–6.
8. **Migration shadow/dual-run/cutover.** Exercise current Casa files and real/stub
   gateway, then remove unauthenticated `--sender` and optional-secret downgrade.
   Depends on every prior stage.
9. **Post-cutover audit.** Key/binding rotation, revoke/rerun, projection rebuild,
   privacy scan, metrics, operator runbook, and explicit residual review.

Stages 2 and 6 are the only candidates for generic WG modules. Maintainer review may
keep either adapter-local. Stages 4–5 and Casa election/rendering policy MUST NOT move
into WG core by default.

### Compatibility/release gates

- Pin minimum tested `WG_FED_COMPAT_VERSION = 0.4.0`, envelope v1, and
  `WG_EXEC_COMPAT_VERSION = 0.1.0`; call both existing handshakes where applicable.
- Reject unknown schema majors and unsupported required feature bits. Minor additive
  fields use `serde(default)` only when omission is fail-closed, never to restore
  unauthenticated legacy behavior.
- A WG-Fed/Exec semantic change requires the owning compat bump; an adapter payload
  change bumps `casa.*.vN`/adapter major, not an unrelated WG const.
- Migration records source schema/commit, adapter schema major, WG-Fed/Exec versions,
  and cursor. Downgrade to a reader that cannot enforce binding/review/capability is
  refused.
- Release remains feature-flagged per household until migration verification and
  rollback-to-disabled (not rollback-to-insecure) are proven.

## 13. End-to-end acceptance tests

All core scenarios run credential-free with stub Telegram/web transports and the real
Fed/Review/Exec libraries. Live Telegram is an additional connector gate.

### `casa_wgfed_ingress_e2e.sh`

Two filesystem-independent homes communicate only through an untrusted HTTP WG node.
Mint human, persona, gateway, and device identities; bind a stable numeric Telegram
subject; send sealed input while the persona is offline; then prove:

- changed username/same numeric ID preserves binding; same username/different numeric
  ID and web `sender="Luca"` spoof fail before Review/election;
- body/signature/binding/capability tamper and revoked service signer fail;
- relay cannot read sealed content or forge `from`; recipient later polls after origin
  goes offline;
- four-bot/native redelivery and restart consume one event; two identical-content
  native messages become two signed event IDs;
- Unknown injection is withheld before election/compose/task, Verified benign text is
  consumable under the applied policy, digest mutation fails, and revoke deepens the
  next item;
- allowed plan edit succeeds with exact UCAN while unrelated household/global graph
  write and capability widening fail.

### `casa_projection_migration_e2e.sh`

Seed current six/eight-field feed/thread JSONL, duplicate `srcId`, torn line, spoofed
sender, and secret-shaped data. Prove read-only hash archive, untrusted legacy import,
no retroactive authorship, idempotent cursor, projection v2 rebuild, concurrency
uniqueness by event CID, tamper warning/rebuild, 0600/0700, and no token/raw nonce/
unintended locator leakage. Loading imported history must take the IC3 gate.

### `casa_lifecycle_exec_e2e.sh`

Place an accepted Casa task on another provider. Inspect two scoped UCANs and sealed
minimal slice; reject wrong task, signer, expiry, replay, stale-after-reclaim, poisoned
result, and failed IC2/integrity checks. Prove no Casa `Done` is emitted before
`run_accept`/epoch commit. Then send the signed completion to an offline WG recipient
and an external stub; distinguish `AttemptUnknown`, `ApiAccepted`, projection failure,
and absent read evidence without duplicate canonical consumption.

### `casa_rotation_recovery_acl_e2e.sh`

Rotate gateway/persona signer and encryption keys, revoke old channel binding/session,
recover a human root with stable `wgid`, and copy all public/Casa files to an attacker.
Prove the attacker cannot sign/issue caps, old signer/binding fails under freshness,
new events exclude a removed household member, both retained members decrypt, and the
removed member can still read only previously received history (documented residual).

Future smoke manifest entries list the corresponding implementation task IDs as
owners and are grow-only. Unit tests additionally cover schema round trips, resource
containment, strictest trust folding, nonce single-use, atomic receipt transitions,
and unknown-major/compat loud failure.

## 14. Maintainer acceptance checklist

Implementation release requires an explicit maintainer comment/approval covering all
of the following:

- [ ] Accept the principal/resource mapping, especially **household and graph are not
      shared-key principals**, and persona ≠ bot/provider.
- [ ] Accept WG-Fed/Review/Exec as the sole identity/trust/action/remote-result authority.
- [ ] Decide whether channel evidence and durable outbox begin adapter-local or as the
      two small generic WG seams; do not approve Casa product policy into core.
- [ ] Accept the external bootstrap exception and removal of fail-open secret/name paths.
- [ ] Accept feed/thread as rebuildable projections and the no-retroactive-authorship
      migration rule.
- [ ] Accept lifecycle completion ordering and precise delivery vocabulary.
- [ ] Confirm compatibility bump rules and the four end-to-end scenarios.
- [ ] Record Luca's actual response and links, or explicitly acknowledge that Casa-owner
      agreement/source remains unresolved.

Only after that approval may stage-1 implementation tasks be published. Approval of
this boundary does not approve any existing Casa branch wholesale, does not merge
fork PRs, and does not change the still-separate review state of upstream PR #57.

## References

- [Casa/WG-Fed overlap and threat study](reports/luca-casa-wgfed-overlap-2026-07-18.md)
- [Luca coordination post and exact bodies](reports/luca-coordination-2026-07-18.md)
- [Current Casa stream inventory](reports/inventory-luca-casa-stream-2026-07-18.md)
- [ADR-fed-001](ADR-fed-001-identity-key-model.md), [ADR-fed-002](ADR-fed-002-transport.md),
  [ADR-fed-003](ADR-fed-003-custody-delegation-recovery.md),
  [ADR-fed-004](ADR-fed-004-loadable-state-safety.md)
- [ADR-CS1](ADR-content-safety-001-review-gate.md),
  [ADR-CS2](ADR-content-safety-002-reviewer-hardening.md),
  [ADR-CS3](ADR-content-safety-003-verdict-audit-revoke.md)
- [ADR-E1](ADR-exec-e1-placement.md), [ADR-E2](ADR-exec-e2-confidentiality.md),
  [ADR-E3](ADR-exec-e3-capability-lease.md), [ADR-E4](ADR-exec-e4-verification.md)
