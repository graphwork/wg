# Luca Casa overlap with WG-Fed, WG-Review, and WG-Exec

**Date:** 2026-07-18<br>
**Scope:** read-only design/source study; no implementation and no contact with Luca<br>
**Current WG baseline:** [`e7fad9bb`](https://github.com/graphwork/wg/commit/e7fad9bb0ca05dc5a012b78c0c4ddf75a3965368) (the source modules are unchanged from the inventory's [`528b8ee0`](https://github.com/graphwork/wg/commit/528b8ee0258bc48c6fd11ad9b87562610a276526) snapshot)<br>
**Casa integration baseline:** [`64f2cba8`](https://github.com/lucapinello/workgraph/commit/64f2cba83bf91e4e0203f09957e655ff635226ed)<br>
**Companion inventory:** [`inventory-luca-casa-stream-2026-07-18.md`](./inventory-luca-casa-stream-2026-07-18.md)

## Executive decision

Casa is a useful **household product and channel adapter**, but it must not become a second identity, trust, message, or execution plane.

The safe boundary is:

1. **WG-Fed is authoritative for principals, agents, services/devices, signatures, recipients, confidentiality, delegation, offline transport, key continuity, revocation, and recovery.** A `wgid:` and its sigchain prove the author; a signed event CID is the durable message identity; the recipient set is the sealed ACL; UCANs authorize actions.
2. **WG-Review is authoritative at the consumption edge.** Telegram and web text may be received and authenticated, but it cannot be elected, composed, turned into a task, or applied by a fast lane until the IC4/IC1 gate accepts the exact digest using canonically derived trust.
3. **WG-Exec is authoritative whenever Casa work leaves the local trusted process.** A Casa persona or gateway cannot grant a borrowed box work directly, and a lifecycle `Done` message cannot precede accepted `ResultEnvelope` attribution, IC2 review, integrity policy, and lease-epoch commit.
4. **Casa keeps product behavior:** household roster, persona names/emoji/voices, single-owner election, conversation panes, per-human/per-agent views, daily pacing, reminders, sign-in copy, and report-back wording. Its JSONL feed/ledger becomes a rebuildable, non-authoritative projection of accepted signed events and delivery records.
5. **Telegram IDs, handles, bot IDs, Casa `humanId`s, agent IDs, sender labels, nonces, and shared secrets are not federation identities.** They are channel locators, aliases, bootstrap evidence, or local transport credentials bound to distinct `wgid:` principals.

Today Casa mostly **bypasses** the three substrates rather than reusing them. Its local append-only files and restart logic solve real UX problems, but unsigned `sender`/`agentId` fields, an optional bearer secret, raw `--sender` web ingress, content hashes, and bot-loop suppression cannot carry identity/trust/authorization semantics. They may remain behind the adapter only if they are never consulted as authority.

## Classification vocabulary

| Mark | Meaning |
|---|---|
| **DR — direct reuse** | Use the existing WG primitive/schema unchanged. |
| **AD — adapter over WG substrate** | Casa-specific channel or view logic wraps a WG authority boundary. |
| **PP — safe product policy** | Household UX/presentation/routing policy; does not establish identity, trust, or authority. |
| **PT — incompatible parallel trust system** | Independently claims identity/trust/authorization where WG already has the authority plane. Must be removed or demoted to non-authoritative metadata. |
| **OD — obsolete duplicate** | Current WG already provides the generic capability more strongly. |
| **MG — genuine missing WG capability** | No current generic WG primitive fully covers it; specify once in WG or retain as an explicit adapter capability. |

A component can have two marks where its product behavior is sound but its current authority semantics are not.

## Source trace and lineage

### The four requested fork PRs

| Fork PR | Exact head / state | Concrete files and behavior | Finding |
|---|---|---|---|
| [#1](https://github.com/lucapinello/workgraph/pull/1) | [`43c1bc7f`](https://github.com/lucapinello/workgraph/commit/43c1bc7fbbb39958cb25f6ea2f60497576446bd8), merged 2026-07-11 | [`src/notify/casa_feed.rs`](https://github.com/lucapinello/workgraph/blob/43c1bc7fbbb39958cb25f6ea2f60497576446bd8/src/notify/casa_feed.rs#L50-L212) defines a six-field plaintext group-feed projection; `src/commands/telegram.rs` mirrors group inbound and agent replies; `casa_feed_write.sh` checks the allowlist. | **PP+AD.** Good privacy-minimized view schema; not an identity or audit ledger. |
| [#2](https://github.com/lucapinello/workgraph/pull/2) | [`78e09978`](https://github.com/lucapinello/workgraph/commit/78e099787b3ec9bbf889b793e542d552de46aca2), closed unmerged 2026-07-15 | [`src/notify/casa_ledger.rs`](https://github.com/lucapinello/workgraph/blob/78e099787b3ec9bbf889b793e542d552de46aca2/src/notify/casa_ledger.rs#L73-L400) defines eight-field per-human/per-agent JSONL, `srcId` dedupe, pending-reply replay, and thread enumeration. | **PP+AD**, with replay/concurrency flaws. It is not in the integration tip and must not be treated as shipped Casa authority. |
| [#4](https://github.com/lucapinello/workgraph/pull/4) | [`8e15c9c1`](https://github.com/lucapinello/workgraph/commit/8e15c9c195595456614b0a30e3660f1026a0fc7f), merged via `0eeebfbe` 2026-07-15 | [`src/commands/telegram.rs`](https://github.com/lucapinello/workgraph/blob/8e15c9c195595456614b0a30e3660f1026a0fc7f/src/commands/telegram.rs#L62-L133) parses `/start login_<nonce>` and POSTs `{nonce, telegram_id}` to loopback `/auth/confirm`. | **PP+AD** as a bootstrap ceremony; **PT** if the nonce/Telegram claim is treated as the enduring principal identity. |
| [#5](https://github.com/lucapinello/workgraph/pull/5) | [`a144f9ed`](https://github.com/lucapinello/workgraph/commit/a144f9edb877539a7ebadd671025f4853b6a3274), merged via `5bb89133` 2026-07-14 | `src/commands/telegram.rs` makes web inbound fall back from explicit/legacy group chat ID to the first configured bot-map chat ID. | **PP.** Correct product configuration fallback, with no trust meaning. |

PR #2 is materially important to this study but was **closed without merge**. The later integration branch contains the group-feed writer but not `casa_ledger.rs`; any claim that current Casa has the Rust 1:1 recovery ledger must be checked against the external gateway tree, not inferred from the fork tip.

### Related Casa commits

| Concern | Exact commit(s) / files | What changed |
|---|---|---|
| Feed provenance/dedupe | [`b359f6ff`](https://github.com/lucapinello/workgraph/commit/b359f6ff) in [`src/notify/casa_feed.rs`](https://github.com/lucapinello/workgraph/blob/b359f6ff/src/notify/casa_feed.rs#L68-L300) | Adds `origin` and 64-bit `srcId = DefaultHasher(chat_id, sender_id, date, text)`; reader-side non-null `srcId` dedupe. This is open fork PR #23 and absent from integration. |
| Web inbound | [`f1ef4e35`](https://github.com/lucapinello/workgraph/commit/f1ef4e35) in `src/commands/telegram.rs` | Gateway shells out to `wg telegram web-inbound --sender --message`; a display name/`humanId` is mapped to a Telegram binding, then the normal group election/composer runs. Bot authorship suppresses the mirrored duplicate. |
| Web/listener parity | [`14863493`](https://github.com/lucapinello/workgraph/commit/14863493) in [`src/commands/telegram.rs`](https://github.com/lucapinello/workgraph/blob/14863493/src/commands/telegram.rs#L2952-L3225) | Shares group election, adds clarification continuation, and lets a confirmed web sender use a direct plan-writing fast lane. |
| Listener→gateway auth | [`a03e82ea`](https://github.com/lucapinello/workgraph/commit/a03e82ea) in [`src/commands/telegram.rs`](https://github.com/lucapinello/workgraph/blob/a03e82ea/src/commands/telegram.rs#L75-L125) | Adds `x-casa-auth-secret` from env or `.casa/auth-confirm.secret`; absence deliberately falls back to an unauthenticated request for old-gateway compatibility. |
| Invite/founding | [`48b9d69b`](https://github.com/lucapinello/workgraph/commit/48b9d69b) plus `a03e82ea` | Adds `join_<nonce>`, empty-roster owner confirmation, and `/auth/found`/`/invite/redeem`; the shared bearer header protects gateway writes. |
| Lifecycle origin | [`0dfe117d`](https://github.com/lucapinello/workgraph/commit/0dfe117d) in `src/graph.rs` and [`src/notify/lifecycle.rs`](https://github.com/lucapinello/workgraph/blob/0dfe117d/src/notify/lifecycle.rs#L46-L95) | Adds `TaskOrigin {channel, chat_id, requester, persona, bot_id}`, `TASK_CREATE:`, started/done/failed rendering, and `lifecycle:<task>:<event>` dedupe. |
| Automatic report-back | [`14b873e9`](https://github.com/lucapinello/workgraph/commit/14b873e9), [`7fb94e0a`](https://github.com/lucapinello/workgraph/commit/7fb94e0a) | Coordinator/listener transitions trigger replies; report-backs bypass normal proactive caps because they are replies. |
| Delivery + pane parity | [`32bd52c4`](https://github.com/lucapinello/workgraph/commit/32bd52c4) in `src/commands/telegram.rs` | Sends through the normal reply sink, retries once, requires a Telegram API message ID, reports `UNDELIVERED`, and mirrors group report-backs to `.casa/group-feed.jsonl`. |
| Conversation substrate | `ddcde70e`, `de57f1cf`, `abc5343d`, `db431497`, `df6bdf75`, `589f63b0`, `057bb21e` | Multi-bot polling, content dedupe, election, 1:1/name-addressed compose, group routing/discussion, and voice transcription. These are generic channel/conversation candidates but presently bypass Fed/Review. |
| Household policy | `8fb6d1e3`, `b3386157`, `78739f3d`, `097360a4`, `982a86a3` | Persona roster, one-owner/domain election, fast paths, date/voice/answer shape. Product policy, not core trust. |

The commit comments reference external `claw3d-bridge/src/conversation.mjs`, `src/ledger.mjs`, `gatewayCore.mjs`, and documents `docs/15`, `docs/16`, and `docs/20`. Those files are not present in the fetched `lucapinello/workgraph` refs. This report can verify the Rust contracts and commit claims, but not independently audit the gateway reader/session code. That external half must be supplied before implementation review.

### Current generic Telegram identity work (#49/#51)

Current WG already contains the revised Luca identity lane:

- Upstream [#49](https://github.com/graphwork/wg/pull/49), merged at [`fb19f595`](https://github.com/graphwork/wg/commit/fb19f595), introduced the human onboarding binding; later fixes include `aa359ff2`.
- Upstream [#51](https://github.com/graphwork/wg/pull/51), merged at [`d78582a4`](https://github.com/graphwork/wg/commit/d78582a4), ends at [`f16d86bf`](https://github.com/graphwork/wg/commit/f16d86bf).
- [`src/notify/telegram.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/notify/telegram.rs#L426-L513) emits stable numeric Telegram `from.id` as `IncomingMessage.sender` and carries a mutable username separately.
- [`src/agency/human_binding.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/agency/human_binding.rs#L36-L105) makes a numeric binding match only the numeric ID; handle bindings use username. The map enforces one-human/one-agent locally.
- [`src/commands/service/human_dispatch.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/commands/service/human_dispatch.rs#L341-L449) requires a confirmed binding and rejects cross-human or wrong-bot replies.

This is valuable **channel authentication**, but it is not WG-Fed identity: the YAML binding is local, unsigned, not recoverable via sigchain, and maps a Telegram account to a local agency agent ID rather than to a `wgid:` principal.

## The WG authority substrates Casa must use

### WG-Fed

- [`src/identity/mod.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/identity/mod.rs#L1-L31) centralizes all signing/sealing crypto; `WG_FED_COMPAT_VERSION = 0.4.0` is at lines 124–141.
- [`src/identity/keys.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/identity/keys.rs#L104-L175) derives self-certifying `wgid:`/`did:key` identities; the [`Custodian`](https://github.com/graphwork/wg/blob/e7fad9bb/src/identity/keys.rs#L221-L376) only exposes sign/agreement operations, never private keys.
- [`src/identity/sigchain.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/identity/sigchain.rs#L1-L35) provides hash-linked key authorization, root-locked add/revoke/rotation, and layered recovery.
- [`src/identity/envelope.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/identity/envelope.rs#L291-L565) provides signed `SignedEvent`s, multi-recipient sealing where `to` is the ACL, and sealed-sender authentication. `IdentityRecord` and `StateSnapshot` are at lines 59–151.
- [`src/identity/transport.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/identity/transport.rs#L121-L179) defines an untrusted at-least-once `FedStore`; [`node.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/identity/node.rs#L1-L51) gives offline inbox storage, quotas, retention, and delete-after-ack.
- [`src/identity/freshness.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/identity/freshness.rs#L85-L228) adds signed validity windows and monotonic sequence rollback defense.
- [`src/identity/custody.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/identity/custody.rs#L289-L484) defines expiring, signed, attenuating-only UCAN capabilities; revocation and freshness follow.

The governing decisions are [ADR-fed-001](../ADR-fed-001-identity-key-model.md) D1–D6, [ADR-fed-002](../ADR-fed-002-transport.md) D1–D5, [ADR-fed-003](../ADR-fed-003-custody-delegation-recovery.md) D1–D6, and [ADR-fed-004](../ADR-fed-004-loadable-state-safety.md) D1–D6. Their text still says `Status: Proposed`, although the referenced implementation exists; that documentation-state discrepancy does not authorize a parallel implementation.

### WG-Review

- [`src/trust.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/trust.rs#L84-L123) resolves author trust from the peer registry, folds provider opinion only in the stricter direction, and fails closed to `Unknown`. A Verified compute box cannot promote itself to a Verified author.
- [`src/review/mod.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/review/mod.rs#L61-L169) defines IC1–IC4 and `accept/quarantine/reject`; [`review_inbound`](https://github.com/graphwork/wg/blob/e7fad9bb/src/review/mod.rs#L305-L450) pins the content digest and runs the trust/sensitivity-selected pipeline.
- [`src/review/depth.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/review/depth.rs#L60-L103) makes unknown/unlabeled input deep and quarantine-by-default.
- [`src/review/verdict.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/review/verdict.rs#L194-L284) enforces digest-pinned consumption and the trust-lowering/re-run revoke leg.
- [`identity_cmd::run_poll`](https://github.com/graphwork/wg/blob/e7fad9bb/src/commands/identity_cmd.rs#L1089-L1277) authenticates, restart-dedupes by a signature-pinned event key, and runs the trust-derived IC4 gate before exposing a body.

These implement [ADR-CS1](../ADR-content-safety-001-review-gate.md) D1–D5, [ADR-CS2](../ADR-content-safety-002-reviewer-hardening.md) D1–D5, and [ADR-CS3](../ADR-content-safety-003-verdict-audit-revoke.md) D1–D5. Current generic `src/commands/telegram.rs` does **not** call `review_inbound` or `resolve_author_trust`; Casa conversational and fast-lane ingress therefore has an explicit missing adapter seam.

### WG-Exec

- [`src/providers/mod.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/providers/mod.rs#L430-L641) defines signed `PlacementOffer`, `Claim`, `RunGrant`, `LeaseRenewal`, and `ResultEnvelope`. `RunGrant` carries two scoped UCANs and a sealed context bundle; `ResultEnvelope` carries producer provenance and epoch.
- [`src/providers/bundle.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/providers/bundle.rs#L83-L157) directly reuses the WG-Fed multi-recipient sealed envelope.
- [`src/providers/placement.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/providers/placement.rs#L125-L255) derives trust floor, scope/TTL, seal, lease, and verification depth from one leash.
- [`src/providers/lease.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/providers/lease.rs#L125-L376) holds the canonical monotonic lease-epoch fence and signed-renewal liveness.
- [`src/providers/verify.rs`](https://github.com/graphwork/wg/blob/e7fad9bb/src/providers/verify.rs#L47-L143) separates mandatory attribution from task-scoped graph-write authority; low-trust integrity uses a disjoint trusted-domain rerun against a pinned spec.
- [`exec_fed_cmd::run_accept`](https://github.com/graphwork/wg/blob/e7fad9bb/src/commands/exec_fed_cmd.rs#L835-L1012) now enforces attribution, graph-write UCAN, IC2 review, integrity policy, and epoch CAS before committing.

These implement [ADR-E1](../ADR-exec-e1-placement.md) D1–D6, [ADR-E2](../ADR-exec-e2-confidentiality.md) D1–D7, [ADR-E3](../ADR-exec-e3-capability-lease.md) D1–D7, and [ADR-E4](../ADR-exec-e4-verification.md) D1–D7.

## Component-by-component overlap matrix

| Casa component | Concrete Casa model/path | WG overlap | Current relation | Classification and disposition |
|---|---|---|---|---|
| Telegram account parsing | Numeric `from.id`, separate username, update/message IDs in generic listener | Current #49/#51 Telegram parser/binding | Fork contains older and divergent copies | **DR+OD:** retain current WG parser and confirmed binding; do not import old copies. |
| Human binding | `TelegramBinding {telegram_user, agent_id, confirmed…}` | WG-Fed human identity is one `IdentityRecord` type with a `wgid:`; alias proof slot exists but no Telegram binding protocol | Local YAML authenticates channel sender only | **AD+MG:** keep as channel evidence, add a signed `ChannelBinding` from external account/device to human `wgid`. Never call the local agent ID the principal. |
| `/start login_<nonce>` | PR #4 POST `{nonce, telegram_id}` | Federation enrollment/fork/same-self and custody | Bootstrap challenge is useful; no key continuity | **PP+AD:** nonce proves possession during enrollment; successful ceremony must issue/bind a key or signed binding. **PT** if it alone defines identity. |
| `join_<nonce>` / founding | `48b9d69b`, `a03e82ea` POST name + Telegram ID | UCAN delegation, root-controlled key enrollment, recovery | Gateway roster mutation is separate authority | **PP+AD/PT:** invitation UX is good. Membership/authority must be a principal-signed binding or scoped UCAN, never only a gateway row. |
| Shared gateway bearer secret | `x-casa-auth-secret`, mode-600 file/env; optional downgrade | Custodian, signed events, UCAN | Authenticates one local process, not author or action; no recipient ACL, rotation history, or offline verification | **PT** as an authority mechanism; may remain defense-in-depth **AD** only, stored via `wg secret`, mandatory/fail-closed, rotated, and never promoted to principal identity. |
| Web session/device identity | Gateway supplies `humanId`/display name; PR #25 supplies device label | `wgid`, signer/device key, alias proof | Display string is mapped to Telegram binding | **PT+MG:** current name matching is spoofable and conflates session with human. Add device/session binding and scoped capability; labels remain untrusted **PP**. |
| Web inbound | Shell-out `--sender <humanId> --message`; direct group election | SignedEvent + IC4 review + replay store | Bypasses signatures, peer trust, review, ACL, and Fed transport | **AD after redesign; PT today.** Gateway/service signs a channel assertion/event; verify and review before routing. |
| PR #5 chat target fallback | Explicit → legacy → first bot-map chat ID | Transport endpoint selection | No authority claim | **PP**, preserve. Validate all bots really share the group instead of silently choosing across mixed groups. |
| Multi-bot polling/dedupe | Per-bot long poll; in-memory/content dedupe; bot-loop guard | FedStore at-least-once inbox + signature-pinned `DedupStore` | Similar delivery problem, weaker keys | **AD+OD:** Telegram update cursor stays channel-local; canonical consumption dedupe must be signed event CID. Bot authorship is loop prevention only. |
| Group responder election | Mentions/name/collective/silence, one-owner/domain policy | Fed recipient addressing; no persona-election product policy | Orthogonal after identity/review | **PP.** Route only accepted canonical events, resolve candidates by agent `wgid`, render aliases afterward. |
| Composer / conversation turns | Bound session, persona voice, direct/group compose | `StateSnapshot` has `conv-cache-v1` slot; WG-Fed signed events | No canonical signed conversation event; inbound can become instructions immediately | **PP+AD.** Conversation model is product policy; events/state use Fed provenance and Review. |
| Group feed PR #1 | Six display fields; later eight with `srcId`/`origin`; unsigned plaintext JSONL | SignedEvent event log, CAS, sealing, verdict audit | Duplicates event log only if treated as canonical | **PP+AD:** retain as minimized read projection. **PT/OD** if sender/provenance or delivery truth is read from it. Add canonical event/verdict IDs and rebuild from source. |
| Per-agent ledger PR #2 | `{ts,human,agent,role,origin,sender,text,srcId}`; local file names | Signed events, per-recipient ACL, StateSnapshot history | Closed/unmerged; plaintext projection with local replay | **PP+AD:** useful UX/recovery projection, not authority. Seal portable threads to participants; canonical events remain source. |
| Feed `srcId` PR #23 | `DefaultHasher(chat,user,date,text)` → `tg-<64-bit>` | SignedEvent CID + recipient-local DedupStore | Hash hides verbatim IDs but is not authenticated or collision-resistant | **OD/PT:** replace canonical dedupe with signed event CID or a domain-separated BLAKE3 binding to channel evidence. May retain as legacy projection key only. |
| Ledger Telegram `message_id` | `srcId` in one human-agent file | Event CID | Only unique within the relevant Telegram chat; bot copies differ | **AD only:** preserve as `channel_message_id`, never global identity/dedupe authority. |
| `origin: "telegram"` / sender labels | String provenance and presentation | Signed `from`, `to`, kind, refs, verified sigchain | Anyone with file access can forge strings | **PT** if trusted; **PP** as display metadata derived from a verified event. |
| Task origin stamp | `TaskOrigin {channel, chat_id, requester, persona, bot_id}` | Signed event refs and `wgid` provenance | Raw mutable locators/display names are embedded in graph task | **AD+MG:** preserve report-back locator locally, but add canonical `request_event_cid`, requester `wgid`, persona `wgid`, review CID; encrypt/minimize channel locator. |
| `TASK_CREATE:` / fast lane | Model tail creates task; confirmed web sender can directly mutate plan | Review IC1/IC4; UCAN action scope; Exec placement | Bypasses review and capability checks | **PT today.** After gate, directive becomes typed intent; authorize with UCAN. Household edit grammar is **PP**. |
| Lifecycle status engine | Started/done/failed from graph state; `lifecycle:<task>:<event>` | WG local lifecycle; Exec lease/result plane for remote runs | Product notification, but can overstate remote completion | **PP+AD:** fire `Done` only after the canonical local completion gate or accepted remote `ResultEnvelope`/epoch commit. |
| Report-back transport | Telegram API message ID, retry once, `UNDELIVERED`; feed mirror | FedStore at-least-once and ack-delete; no general channel receipt/outbox | Good channel adapter, but API acceptance is not human delivery/read | **AD+MG:** retain Telegram sink, add generic durable delivery outbox/status vocabulary (`queued`, `API-accepted`, `read` only with evidence). |
| Delivery dedupe | FiredLog marked before send; re-armed on known failure | Event CID, inbox ack, lease epoch | Chooses at-most-once loss over duplicate; unknown timeout remains ambiguous | **AD:** use idempotency key `(event_cid,destination)` and durable attempt/receipt state; do not call API acceptance exactly-once delivery. |
| Voice/photo/media inbound | Downloads/transcribes then feeds normal message path (`057bb21e`, `15fd9fba`) | Review IC4/IC1; sealed events/ACL | Untrusted bytes and external service precede gate | **AD+MG:** bound media ingestion/sandboxing is generic missing work; review transcript and retain digest/derivation provenance before consumption. |
| Household roster/persona | `household.toml`, hard-coded names/emoji, bots and local agent IDs | One WG identity type; aliases; ACL recipient list | Presentation is useful; IDs are conflated | **PP+AD:** roster maps roles and aliases to distinct human/agent/service `wgid`s. Do not derive identity from name/emoji/bot. |
| Confidential conversation | Plain `.casa` JSONL and Telegram/web mirrors | Per-recipient sealing, sealed sender, ACL | No end-to-end Casa ACL; shared secret is not encryption | **DR:** use sealed events/state for portable/cross-host content. Local plaintext projection is explicit product risk with strict permissions/retention. |
| Capability to act | Confirmed binding and fast-lane allow; gateway process reachability | UCAN scope/TTL/revocation | Authentication is used as authorization | **PT:** issue/check scoped capabilities (`message/send`, `household/plan-edit`, task creation); never infer broad authority merely from a binding. |
| Remote work/result | Casa tasks and report-backs observe graph status | WG-Exec offer/grant/lease/result/accept | Casa has no provider/lease/result wire | **DR:** use WG-Exec verbatim when remote; no Casa provider plane. |
| Recovery | Local files, pending replay, gateway-minted secret | Sigchain rotate/revoke/recover/fork; StateSnapshot; node offline inbox | Message recovery exists; identity recovery does not | **DR+AD:** identities recover through WG-Fed; projections rebuild from events/snapshots. Never recover authority by copying `.casa` or a bearer file. |

## Why the Casa identifiers cannot replace WG identity/trust

### Shared secret

`a03e82ea` correctly recognized that “loopback” is not authentication on a kiosk. Its mode-600 bearer file is useful process hardening, but four properties make it unsuitable as the trust root:

1. It identifies **who knows one symmetric value**, not which human, bot, agent, device, or action authored a request.
2. Every holder can forge every other holder; there is no attributable signature, recipient ACL, attenuation, or offline verification.
3. The implementation explicitly sends without it when missing for backward compatibility ([lines 92–125](https://github.com/lucapinello/workgraph/blob/a03e82ea/src/commands/telegram.rs#L92-L125)). That is fail-open on the write path.
4. Copying `.casa/auth-confirm.secret` confers authority; WG custody deliberately makes “download ≠ impersonation.”

Migration: make the gateway/listener separate service/device identities; sign requests and attach a scoped capability. Keep a mandatory local shared secret only as defense-in-depth against browser-to-loopback request forgery, managed by `wg secret`, never as author identity.

### Nonces

A short-lived, single-use `login_`/`join_` nonce is a sound **ceremony challenge**. It proves that the Telegram account receiving the deep link and the browser session participated in the same enrollment. It does not provide durable continuity, recovery, or delegation. Bind the completed transcript `(nonce digest, numeric Telegram ID, device key, human wgid, expiry, use counter)` in a principal/service-signed artifact; erase the raw nonce; reject used/expired/wrong-subject challenges; then use keys/UCANs, not the nonce, for subsequent requests.

### Ledger/feed sender and persona IDs

`sender`, `human`, `agent`, `agentId`, `origin`, `requester`, and `persona` are strings in unsigned local files. They are excellent labels and indexes, but prove nothing. A local writer or filesystem attacker can write “Nora,” “Luca,” or `origin:"telegram"`. A display handle may change; a bot ID is a credentialed endpoint; a local agency ID is a graph record; none is the self-certifying public key.

Every authoritative record needs both stable identifiers and presentation:

```text
author_wgid       = cryptographic principal/service that signed the event
subject_wgid      = human represented by channel evidence, if different
recipient_wgids   = agent/human ACL
channel_binding   = signed proof linking numeric Telegram account or device key
channel_message_id= adapter-local replay evidence only
aliases           = Luca/Nora/Otto, @handle, emoji, device label (untrusted display)
event_cid         = canonical idempotency/provenance key
review_cid        = exact accepted digest/verdict link
```

### `srcId`

PR #23 improves restart behavior but overstates the hash. `DefaultHasher` is not a stable protocol commitment across Rust implementations/releases, the output is only 64 bits, it is unkeyed (so low-entropy chat/user/text tuples can be guessed), and two genuinely distinct identical messages from one sender in the same second collapse. Hashing IDs prevents verbatim disclosure; it does not provide confidentiality, authenticity, collision resistance, or author attribution.

Use the signed event CID as canonical dedupe. If the adapter must correlate four Bot API copies before a canonical event exists, compute a versioned, domain-separated BLAKE3 digest over normalized channel evidence and record all native update/message IDs. That digest still remains **adapter evidence**, not identity.

## Human, bot, agent, persona, device, and principal mapping

The mapping must be explicit and many-to-many where reality is many-to-many:

| Thing | Correct role | Federation mapping | Must not be conflated with |
|---|---|---|---|
| Luca the human | Principal with authority and recovery policy | Human `wgid:`; self-held/custodied root; no `agent_fields` required | `@luca`, numeric Telegram ID, `human-luca`, a browser cookie, or “owner” string |
| Luca's Telegram account | External channel account | Signed/revocable `ChannelBinding` to Luca's `wgid`; numeric `from.id` is canonical within Telegram | Luca's root identity or trust level |
| Telegram username/display name | Mutable alias/presentation | Optional alias metadata; never binding anchor when numeric ID exists | Principal ID |
| Telegram bot | Channel endpoint/service credential | Bot/listener service `wgid` and endpoint; bot token stays secret; may be delegated message-delivery scope | Persona or author of a human's words |
| Kitchen tablet/browser | Device/session | Device key or device `wgid`; short session UCAN from human/service; label is display-only | Luca, the household, or the gateway process |
| Nora/Bruno/Otto/Mira | Agent principals/personas | One agent `wgid` each with `agent_fields`; names/emoji/voice are aliases/product policy | Bot ID, model handler, execution provider, or human operator |
| `human-luca` / local agent ID | Local WG projection | Mapping target associated with human `wgid` | Federation address |
| Casa gateway/listener | Service/process identity | Service/device `wgid` with narrow channel-assertion/delivery capabilities | Human principal or every agent |
| Provider/borrowed box | Execution producer | Provider `wgid` in `ProviderRegistry`; trust is execution trust | Persona, author trust, or LLM provider/model |
| Household | Membership/ACL policy | In v1, an explicit signed roster selecting individual `wgid` recipients; multi-recipient sealing enforces access | One shared bearer secret or Telegram group chat ID |

A confirmed Telegram binding authenticates “this Bot API update came from the Telegram account locally associated with Luca.” It does **not** automatically make Luca `Verified` in the federation peer registry. Trust is a local consumer opinion. Likewise, enrolling a provider as `Verified` cannot make content it authors `Verified`; current `trust.rs` intentionally keeps those dials split and folds only in the stricter direction.

## Proposed no-second-trust adapter boundary

### 1. Generic channel evidence (small WG addition)

Define one generic, signed `ChannelBinding`/`ChannelAssertion` schema rather than a Casa identity database:

```text
ChannelBinding {
  subject_wgid, channel="telegram", external_subject="<numeric from.id>",
  service_wgid, created_at, expires?, revoked_at?, ceremony_cid, sig
}
ChannelAssertion {
  service_wgid, subject_wgid?, binding_cid?, native_ids[],
  received_at, content_cid, channel_context_digest, sig
}
```

The human/root signs or explicitly authorizes the binding; the bot/listener service signs what Telegram delivered. This preserves the honest distinction that Telegram, not the human's key, authenticated the original update. For browser traffic, a device key/session UCAN signs the assertion. The consumer resolves trust for the claimed subject and service fail-closed (strictest relevant opinion), not from a display name.

This is a **genuine missing WG capability**: `IdentityRecord.alias_proofs` is only a slot and current #49 bindings are local/unsigned. It should be implemented once, not separately in Casa.

### 2. Canonical ingress

Both Telegram and web adapters produce a `SignedEvent` (or a typed payload referenced by one):

- `from = service/device wgid`; include the subject binding/assertion in `refs` or typed body;
- `to = elected household/agent candidates` only after policy selection, or a household ingress identity before election;
- seal to the actual recipient set when content crosses a trust/host boundary;
- use event CID for restart dedupe; retain native IDs only as evidence;
- transport locally through the same verifier API or over `FedStore`; transport remains untrusted.

Do not let the gateway shell out with an authoritative `--sender` string. If the CLI remains as an adapter seam, it accepts a signed assertion/event file or CID and independently verifies it.

### 3. Review before behavior

Order is non-negotiable:

```text
receive bytes → authenticate signature/binding → replay check → derive trust
→ IC4 review exact digest → (if directive/task/fast write) IC1 + sensitivity/taint
→ only on accept: election / compose / task creation / household mutation
```

A non-accept verdict may produce a safe bounded rejection notice, but the hostile body must not reach election prompts, conversation memory, `TASK_CREATE:`, the direct fast lane, or a plan file. Record the verdict and use digest-pinned consumption.

### 4. Persona routing as product policy

After acceptance, Casa maps recipient/subject `wgid`s to `household.toml` aliases, voices, emojis, bots, and domain owners. The election result contains agent `wgid`s plus aliases, never only `"nora"`. Bot selection is an endpoint decision made after the author/agent decision.

### 5. Rebuildable conversation projections

Keep `group-feed.jsonl` and optional per-agent ledgers for the Casa UI, but generate them from canonical accepted event/delivery records. Add at least:

```text
eventCid, authorWgid, subjectWgid?, recipientWgids, reviewCid,
direction, channel, channelMessageId?, deliveryStatus
```

The UI may expose only the six calm display fields. The richer projection can be encrypted/local. The authoritative log is signed/CAS-addressed; the projection is deletable and rebuildable. Portable conversation cache uses a signed/sealed `StateSnapshot` and the ADR-fed-004 load safety pipeline.

### 6. Capability and execution

- Human/device UCANs scope household changes (`household/plan-edit` on a concrete home/plan resource), task creation, and message send; confirmation alone grants no blanket write.
- An agent receives the normal WG authority for the accepted task.
- Remote work uses `PlacementOffer → Claim → RunGrant(two UCANs + sealed slice + Lease) → ResultEnvelope` unchanged.
- Casa must not report remote `Done` until `run_accept` succeeds. A provider's self-reported test result, a graph status written outside the epoch fence, or a Telegram message is not completion.
- Product fast lanes either stay in the local trusted authorizer under an authorized typed operation or become ordinary scoped Exec work; they never bypass Review/UCAN because they are “simple.”

### 7. Delivery adapter and receipt vocabulary

Introduce a generic durable delivery outbox keyed by `(event_cid, destination)` with explicit states:

```text
queued → attempt-unknown | API-accepted(message_id) | failed-retryable
       → delivered/read only when the channel supplies that evidence
```

Casa's retry-once and pane mirror plug into this outbox. Telegram `ok:true` means API-accepted, not read. A feed append is a separate projection status. Fed inbox ack/delete means recipient poll/consume according to that protocol, not that a human saw a Telegram notification. Unknown timeout retries reuse the same idempotency key and tolerate duplicate channel delivery without duplicating canonical consumption.

This durable cross-channel receipt/outbox is a **genuine missing WG capability**; do not encode it in lifecycle prose or `FiredLog` alone.

### 8. Recovery

Human/agent/service/device keys rotate/revoke/recover through the sigchain. Gateway host loss restores public bundles and encrypted/signed state but grants no signing authority without custody enrollment. Rebuild Casa feed/threads from accepted events and delivery records. Rotate/revoke channel bindings and session UCANs separately from the stable principal `wgid`. Never restore authority merely by copying `.casa/auth-confirm.secret`, browser cookies, JSONL, bot mappings, or a persona name.

## Threat analysis

| Threat / failure | Casa behavior today | Required boundary/control | Residual |
|---|---|---|---|
| Forged display sender | Web `--sender` can match a binding by name/local agent ID | Verify signed device/service assertion + binding to subject `wgid`; aliases render only | Compromised legitimate device can still speak as its scoped subject until revoke/expiry. |
| Telegram handle takeover/change | Older paths compare handles; current #49/#51 separates ID/username | Reuse numeric `from.id`; signed binding revocation/re-enrollment | Telegram account compromise remains external-account compromise, not root compromise. |
| Browser-to-loopback CSRF/phantom device | Loopback was trusted; `a03e82ea` added bearer header | Mandatory local secret defense plus signed request/capability and strict origin/CSRF/session controls | Browser/device compromise within its scope. |
| Missing shared secret downgrade | Listener silently sends without header | Fail closed; explicit migration/version handshake; no legacy unauthenticated write | Availability loss during mismatch is preferable to phantom authority. |
| Nonce theft/replay | Raw bearer appears in deep link; intended five-minute/single-use semantics live mainly in gateway | High entropy, single use, subject/session binding, expiry, transcript CID, redact/referrer controls | Telegram and browser may expose link metadata; short TTL limits it. |
| Unsigned ledger tamper | JSONL `sender`/`agentId`/`origin` accepted by reader | Treat as projection only; regenerate from signed events/verdicts | Local attacker can still alter UI until rebuild; surface projection integrity alarms. |
| `srcId` collision/linkability | 64-bit unkeyed content hash; raw IDs absent | Event CID; domain-separated BLAKE3 evidence; avoid claims of secrecy | Message content itself can identify people; minimize/retain appropriately. |
| Concurrent check-then-append | PR #2 reads for `srcId`, then appends without a lock; two writers can duplicate | Single canonical dedup store/transaction or locked projection cursor | Channel may still deliver duplicates; consumption remains idempotent. |
| Torn/malformed JSONL | Parser skips malformed line, changing line-ordinal cursors | Atomic framed event/projection writes, checksum/CID, cursor over canonical CID | Last projection line may be lost and rebuilt. |
| Pending-reply ambiguity | “all humans after last agent” does not bind reply to request; crash after send/before agent append can resend | Explicit `in_reply_to=eventCid`, outbox attempt/receipt, deterministic reply event ID | Telegram may lack atomic send+record; duplicate UI delivery remains possible but canonical reply is one. |
| Bot-loop as dedupe/provenance | Any bot-authored mirror is dropped; authorship used as fingerprint | Signed event CID and explicit bridge marker/ref | Malicious/foreign bots are still blocked by channel policy. |
| Prompt/directive injection | Web/Telegram text enters election/composer and `TASK_CREATE:`; fast lane writes plan | Auth → replay → Review IC4/IC1 → typed capability before any behavior | Review detection is imperfect; no-scope reviewer, typed operations, audit/revoke bound misses. |
| Cross-recipient privacy leak | Group feed is plaintext; DM guard is conditional; Telegram bot/cloud sees content | Per-recipient sealed canonical event; projection ACL; DM/group invariant tests | Telegram itself is not end-to-end sealed to WG recipients. |
| Local plaintext retention | Allowlist omits IDs/tokens but stores sensitive text/name/meal/health context | Mode 600/700, encryption at rest, bounded retention, deletion/rebuild, no logs | Household operator with host access sees local projection unless hardware-backed encryption is used. |
| Relay/node compromise | Casa gateway/feed is implicitly trusted | WG node is untrusted; signatures, sealing, CIDs, freshness | Relay can delay/drop and observe routing metadata unless sealed-sender. |
| Offline recipient | Casa ledger replays locally; gateway loss loses reach/state | FedStore offline inbox + signed events; projection rebuild | Availability depends on at least one reachable store; correctness does not. |
| Stale key/revocation freeze | Casa has no key status chronology | Sigchain + fresh monotonic attestation/revocation head | Within configured freshness window, a just-revoked key may remain usable. |
| Provider exceeds task | Casa has no remote capability boundary | Two UCANs, sealed minimal slice, task-scoped graph write, lease epoch | Provider sees non-attested slice; minimization is blast-radius, not confidentiality. |
| Forged/poisoned result | Lifecycle may react to graph status/report text | Exec attribution + IC2 Review + trusted-domain pinned rerun as selected + epoch CAS | Verified providers still require evaluation/spot checks; attribution alone is not integrity. |
| Delivery overclaim | Telegram message ID called delivered; feed append may fail | Separate API-accepted, projected, consumed/read statuses; durable outbox | Telegram may not expose human-read evidence. |
| Lost/compromised gateway | Bearer/config copies can reconstitute service | Rotate/revoke service/device keys; custody-controlled re-enrollment; no root in backup | Recovery ceremony/operator availability. |
| Household membership churn | Roster/string edits alter recipients and authority | Signed membership/binding updates; seal future events to current individual ACL; revoke caps | Old recipients retain plaintext they legitimately received; revocation cannot erase memory. |

## Validation and regression test matrix

All adapter tests should run credential-free with stub Telegram/gateway transports plus the real Fed/Review/Exec libraries. Live Telegram tests are an additional gate, not a substitute.

| Area | Required scenario | Expected proof |
|---|---|---|
| Principal/channel separation | Same numeric Telegram ID with changed username; same username with different numeric ID | First continues binding; second is rejected. Neither changes human `wgid`. |
| Alias spoof | Web event claims `sender="Luca"` without a valid device/service assertion | Rejected before body/election; display-name match never authenticates. |
| Bot/persona separation | Nora content delivered through Otto bot due fallback | Event remains authored/attributed to Nora `wgid`; endpoint mismatch is loud and does not rewrite author. |
| Nonce ceremony | Valid, replayed, expired, wrong Telegram subject, wrong browser device | Only exact first transcript enrolls; subsequent attempts fail closed; raw nonce absent from logs/state. |
| Shared-secret migration | Missing/wrong/rotated local secret | Write refused, never legacy-fallback accepted; rotation succeeds without principal change. |
| Device labels | PR #25-style label contains markup/control/impersonation text | Escaped display only; no authorization or identity effect. |
| Signed ingress | Valid service assertion; flipped body; forged `from`; revoked signer | Valid authenticates; all tampering/revocation cases reject before Review. |
| ACL | Seal one household event to Nora+Bruno; try Luca/Otto/nonmember | Listed recipients open; nonmembers cannot; relay sees no body. |
| Sealed sender | Relay inspection and recipient open | Relay sees `wgid:anon`; recipient recovers/verifies actual service/subject evidence. |
| Restart replay | Deliver same native update through four bots and after process restart | One canonical event CID and one consumption; native IDs retained as evidence. |
| True duplicate | Same sender/text/timestamp twice but distinct native messages | Two canonical events; unlike PR #23 hash, neither is collapsed. |
| Projection concurrency | Two writers/rebuilder race same event | One projection row per event CID; no interleaved/torn JSON. |
| Projection tamper/rebuild | Modify sender/role in `.casa` JSONL | Integrity warning; rebuild restores values from signed accepted events. |
| Privacy | Scan feed/thread/outbox for bot tokens, raw shared secret, raw nonce, unintended chat/user IDs; check modes/retention | No secret leakage; declared locators only in protected store; 0600/0700 enforced. |
| Pending reply crash points | Crash before record, after record/before compose, after compose/before send, after API timeout, after API accept/before projection | Canonical request/reply consumed once; retries use same event/outbox key; ambiguous transport status is not called delivered. |
| IC4 review | Verified benign household message; Unknown prompt injection; forged review delimiter | Benign accepted; hostile held/rejected before election; reviewer injection has no action scope. |
| IC1/fast lane | Accepted message contains direct plan edit; hostile/unlabeled directive attempts `TASK_CREATE:` or secret access | Typed authorized edit succeeds; hostile/non-accept produces no task/file mutation. |
| Digest pin | Review bytes, mutate one byte before composer/task | Consumption refused. |
| Trust split | Service/provider is Verified compute but Unknown author; human peer Verified but provider Unknown | Author content remains deep/Unknown in first; provider cannot raise it. Stricter opinion only tightens. |
| Revoke | Accept then later revoke content CID | Author trust lowers, downstream consumers named, next message gets deeper review. |
| Offline delivery | Recipient offline during send; origin node then goes down | Recipient later polls/verifies from cached sigchain; duplicate poll does not re-consume. |
| Freshness | Replay older valid attestation/revocation head after a newer sequence | High-value action fails closed. |
| Conversation state | Signed/sealed `conv-cache-v1`, wrong model, unknown kind, injected state | Correct same-self loads under policy; wrong/unknown/poison fails or degrades per ADR-fed-004. |
| Capability | Confirmed human attempts allowed plan edit and unrelated graph/global write | Scoped action passes; widening/other resource fails; binding alone grants nothing. |
| Exec grant | Casa task remotely placed; inspect `RunGrant` | No root/private key, no `graph://*`, minimal sealed slice, correct agent/provider identities. |
| Lease/replay | Provider writes wrong task, after expiry, twice, or after reclaim | UCAN/scope/epoch fence rejects every invalid/stale/replayed write. |
| Result integrity | Wrong signer; malicious diff with passing provider tests; low-trust result without pinned spec; disjoint rerun pass | First three rejected; only attributed/reviewed/policy-verified matching epoch commits. |
| Lifecycle ordering | Provider emits result while task appears locally done before accept | No Casa `Done` report until `run_accept` commits; rejection yields safe failure/held status. |
| Delivery semantics | Telegram `ok:true`, network timeout, feed disk-full, real read receipt absent | Statuses are respectively API-accepted, attempt-unknown, projection-failed, and never falsely “read.” |
| Recovery | Copy public bundle/projection to new host without custody; rotate/recover legitimate identity; revoke old channel binding | Copy cannot sign/act; recovery preserves `wgid`; old signer/binding fails; projections rebuild. |
| Membership change | Remove one household member, send new sealed event, open old/new history | Removed member cannot open new event; can still read previously received old content (explicit residual). |

## Workstream recommendation

1. **Freeze authority expansion in Casa.** Product fixes may continue, but no new Casa sender/trust/capability/result semantics should be built on string IDs, JSONL, or the shared secret.
2. **Specify the generic adapter primitives first:** signed channel binding/assertion and durable delivery outbox/receipt. Media ingestion bounds are a third generic seam. These are the genuine missing WG capabilities found here.
3. **Rebase generic Telegram work on current #49/#51, not the integration branch.** Preserve numeric sender auth, confirmed binding, current handler routing, and the grow-only smoke surface.
4. **Wire Telegram and web ingress through Fed authentication/replay and Review before conversation policy.** Remove authoritative `--sender` and fail-open secret compatibility.
5. **Convert feed/ledger to projections.** Keep Casa's exact calm UI and startup behavior, but key rows by canonical event/review/delivery IDs and make them rebuildable.
6. **Wire task origin and report-back to canonical provenance.** Add requester/persona `wgid`s and request event CID; keep chat/bot locators protected and product-local.
7. **Use WG-Exec unchanged for borrowed work.** Report completion only after accept; never introduce Casa leases/providers/results.
8. **Add the test matrix before migration.** In particular, prove “hostile web/Telegram bytes never reach election/fast lane,” “copying Casa state does not impersonate,” and “remote result rejection cannot emit a Casa Done.”

## Final classification summary

- **Direct reuse:** `wgid`/sigchain/custody, signed/sealed `SignedEvent`, FedStore/node/freshness, UCAN, canonical trust, Review pipeline/verdict/digest pin, WG-Exec grant/lease/result/accept.
- **Adapters over WG:** Telegram Bot API parsing, browser/device session, nonce ceremony, feed/thread projections, persona/bot delivery, lifecycle/report-back, media conversion.
- **Safe product policy:** household roster presentation, voices/emoji, election/domain owner, group chat fallback, pacing, wording, reminders, clarification behavior.
- **Incompatible parallel trust today:** optional shared-secret authority, authoritative raw web `--sender`, unsigned ledger provenance, confirmation-as-broad-authorization, fast-lane writes before Review/UCAN, completion inferred outside Exec accept.
- **Obsolete duplicates:** Casa `srcId` as canonical dedupe, bot authorship as cross-process identity, ledger sender/origin as canonical audit, any Casa provider/result plane.
- **Genuine missing WG capability:** signed external-channel/device binding/assertion; durable cross-channel delivery outbox/receipt semantics; bounded media-ingest derivation. A signed household membership/recipient manifest is useful if dynamic group ACL management is required, but individual multi-recipient ACLs already supply the cryptographic enforcement.

Casa's strongest contribution is the product contract: a family can speak naturally, see one coherent conversation, receive honest progress, and survive a listener restart. WG's strongest contribution is the authority contract: the system knows who authored what, what they may do, who may read it, whether it was reviewed, which worker produced it, and how identity survives compromise. The migration should compose those contracts, not choose between them.
