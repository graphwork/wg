# Casa credential-free adapter spark

**Status:** prototype/acceptance fixture, not a production gateway

**Pinned Casa integration baseline:** [`64f2cba83bf91e4e0203f09957e655ff635226ed`](https://github.com/lucapinello/workgraph/commit/64f2cba83bf91e4e0203f09957e655ff635226ed)

**WG design:** [`design-casa-wgfed-adapter.md`](design-casa-wgfed-adapter.md)

`casa-adapter` is a separate companion binary under `adapters/casa/`. It proves the
smallest product slice without moving Casa policy into WG-Fed, WG-Review, or WG-Exec.
It has no key type, trust enum, ACL implementation, capability implementation,
transport client, provider, lease, or result envelope of its own.

## Authority boundary

1. WG-Fed creates/verifies `wgid:` principals, signs/seals messages, enforces the
   recipient wrap ACL, and provides the untrusted HTTP object/inbox transport.
2. `wg msg poll --review --json` authenticates before screening. The adapter accepts
   only `VERIFIED && consumable` items with canonical `event_cid`, derived author
   trust, an `accept` verdict, and an exact body digest still accepted by
   `VerdictStore::digest_pin_consume`.
3. Casa parses the accepted body as `casa.channel-envelope.v1`, performs household
   election, creates one delivery intent, and derives `casa.projection.v2` rows.
   `srcId`, `origin`, native evidence digests, device labels, and projection files are
   product/channel evidence only. They can suppress a duplicate *after* WG gates;
   they cannot authorize an item.
4. Remote work uses unchanged `wg provider offer/claim/grant/run/accept` envelopes,
   two scoped UCANs, and the lease epoch fence. `casa-adapter relay put/get` merely
   calls WG's existing `FedStore` content-addressed HTTP object API so the two test
   hosts do not exchange files directly.
5. A signed report is another WG-Fed message. Casa may render it only after its normal
   authenticated Review gate.

The local adapter store uses 0700 directories and 0600 records. `events/` contains
accepted-event receipts with the exact Review-pinned body; `source-index/` records a
projection winner and explicitly says `authority:false`; `outbox/` and `attempts/`
carry precise connector states. `feed.jsonl` is deleted and rebuilt by re-checking the
exact WG-Review pin. A projection alone has neither a signature nor a UCAN and is not
accepted by the `ingest` API.

## Product acceptance fixtures and attribution

The prototype is a clean current-WG implementation, not a copy of Luca Pinello's
fork. These public behaviors informed deterministic fixtures:

| Fixture in this adapter | Luca evidence | Adaptation here |
|---|---|---|
| Stable `srcId` plus `origin` suppresses channel/listener re-delivery | Luca's PR #23 explains the stable content tuple and raw-ID hiding in [`casa_feed.rs` lines 190–213](https://github.com/lucapinello/workgraph/blob/b359f6ff9b68c2f38c8fc91509eda190a02c3627/src/notify/casa_feed.rs#L190-L213), and attaches `srcId`/`origin` to feed entries at [lines 215–245](https://github.com/lucapinello/workgraph/blob/b359f6ff9b68c2f38c8fc91509eda190a02c3627/src/notify/casa_feed.rs#L215-L245). | `domain_cid` uses domain-separated BLAKE3 rather than copying `DefaultHasher`; the source key is checked only after Fed authentication and Review acceptance. Canonical replay remains the signed event CID. |
| Device wording names the real channel device and reserves “kitchen tablet” for an explicitly marked tablet | Luca's PR #25 documents the label/fallback distinction in [`telegram.rs` lines 192–208](https://github.com/lucapinello/workgraph/blob/e610213a5afb6f97eb507deea05a4e014b460553/src/commands/telegram.rs#L192-L208) and renders the real label at [lines 218–241](https://github.com/lucapinello/workgraph/blob/e610213a5afb6f97eb507deea05a4e014b460553/src/commands/telegram.rs#L218-L241). | The fixture carries a bounded display-only `deviceLabel` such as `your iPhone`; it is never an author or trust root. No raw User-Agent is accepted as authority. |
| One ask has one domain owner | Luca's PR #26 states the single-owner/off-domain rule in [`ownership.rs` lines 1–36](https://github.com/lucapinello/workgraph/blob/982a86a356c9e1f7150bb23daf3b3f3102ed5851/src/notify/ownership.rs#L1-L36) and resolves one owner at [lines 512–535](https://github.com/lucapinello/workgraph/blob/982a86a356c9e1f7150bb23daf3b3f3102ed5851/src/notify/ownership.rs#L512-L535). | `HouseholdRoster::elect` refuses zero or ambiguous owners and returns the owner's `wgid`, with alias only for display. |
| Relative-day wording is grounded to the correct local date | Luca's PR #26 anchors today and resolves named weekdays at [`grounding.rs` lines 555–616](https://github.com/lucapinello/workgraph/blob/982a86a356c9e1f7150bb23daf3b3f3102ed5851/src/notify/grounding.rs#L555-L616). | The credential-free fixture supplies an ISO household local date and the outward reply renders its concrete weekday/date deterministically. |
| Restart/retry does not double-post | Luca's PR #26 keys one reply per `request_id` and refuses restart re-fire at [`telegram_conversation.rs` lines 1011–1049](https://github.com/lucapinello/workgraph/blob/982a86a356c9e1f7150bb23daf3b3f3102ed5851/src/notify/telegram_conversation.rs#L1011-L1049); its intent ledger describes restart-surviving dedupe at [`ownership.rs` lines 569–639](https://github.com/lucapinello/workgraph/blob/982a86a356c9e1f7150bb23daf3b3f3102ed5851/src/notify/ownership.rs#L569-L639). | The adapter keys an immutable delivery intent by `(event CID, protected destination)` and the stub sink by intent CID. A crash after channel acceptance but before local ack converges to one sink row on replay. |

These are product acceptance fixtures, **not** evidence that `srcId`, a device label,
a sender name, or a local ledger is federation authority.

## CLI sketch

```text
casa-adapter envelope ... --out request.json
casa-adapter ingest --graph B/.wg --state B/casa --poll poll.json \
  --roster household.json --destination protected:family-chat
casa-adapter deliver --state B/casa --sink stub-channel --crash-after-send
casa-adapter deliver --state B/casa --sink stub-channel
rm B/casa/feed.jsonl
casa-adapter rebuild --graph B/.wg --state B/casa
casa-adapter summary --state B/casa
```

`envelope` hashes protected native chat/sender evidence; it does not emit the raw
values. `relay put/get` transports only content-addressed objects through the existing
WG HTTP node.

## Explicit claw3d gateway boundary

As of this prototype, [graphwork/wg issue #58](https://github.com/graphwork/wg/issues/58)
is open with no recorded Luca reply, and no public/reviewable
`claw3d-bridge/ledger.mjs` source has been located. Consequently:

- this binary and its simulated Telegram/web sink are **not** a production claw3d,
  Telegram, login, or channel-assertion integration;
- no behavior is claimed for the external gateway beyond the public fork evidence
  linked above;
- no optional shared-secret fallback, authoritative `--sender` name, nonce authority,
  or external ledger authority was imported.

Production integration remains blocked until review has all of the following exact
evidence:

1. an immutable repository/commit permalink for `claw3d-bridge/ledger.mjs` and every
   writer/reader of its rows;
2. the HTTP/request schemas and source for login/join nonce creation, expiry,
   atomic single-use, device/Telegram subject binding, CSRF/Origin checks, and the
   listener-to-gateway authentication failure mode;
3. the source of `srcId`/origin dedupe, outbox replay, acknowledgement, retry, and
   crash-recovery logic plus tests demonstrating ordering and concurrency;
4. credential/token storage, log redaction, file modes, retention, and rotation paths;
5. an owner statement identifying which gateway revision is deployed and whether
   Luca accepts or rejects the WG-Fed/Review/Exec authority boundary in issue #58;
6. a security review proving the gateway emits a signed, scoped WG channel assertion,
   fails closed without process-hop authentication, and cannot bypass WG-Review or
   WG-Exec.

Until those artifacts exist, only the adapter/core product slice and deterministic
fixtures are reviewable and mergeable; a production claw3d gateway claim is not.
