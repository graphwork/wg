# WG Federation — Operator Runbook (M21)

One page for running a WG federation deploy: **deploy · monitor · backup · key-rotation**,
plus the **dual-main / `wg done`** footguns federation operators hit. Covers the three
planes — WG-Fed (`src/identity/`), WG-Exec (`src/providers/`), WG-Review (`src/review/`) —
as hardened by the exec-harden / fed-harden / ops-and-tests work.

> The node is the only network-facing surface and is therefore the **security boundary**.
> A `FileStore` directory (`file://` / a path) is permissive by design — never expose it
> directly; put a node in front. See `audit-fed.md` / `audit-testops.md` for the threat model.

---

## 1. Deploy

The relay is the store-and-forward inbox node. It holds and forwards **self-verifying**
bytes; it can neither forge nor read sealed content.

```sh
wg fed-node serve --addr 0.0.0.0:8443 --store /var/lib/wg/fed-node
# --store omitted → <workgraph_dir>/fed-node
wg fed-node store-path        # print the default store dir (scriptable)
```

Run it under a supervisor (systemd/runit). It prints `wg-fed node inbox listening on …`
once bound; block-restart on exit. **Terminate TLS at a reverse proxy** (the node speaks
plain HTTP/1.1; there is no built-in TLS).

**Abuse-resistance limits** are env-overridable (no rebuild) — tighten for a hostile
network:

| Env var | Default | Guards |
|---|---|---|
| `WG_FED_NODE_MAX_BODY` | 8 MiB | per-request body cap (B2 OOM) |
| `WG_FED_NODE_MAX_CONN` | 256 | concurrent connections (flood → `503`) |
| `WG_FED_NODE_READ_TIMEOUT_MS` / `…_WRITE_TIMEOUT_MS` | 30000 | slow-loris |
| `WG_FED_NODE_INBOX_MAX_EVENTS` / `…_INBOX_MAX_BYTES` | 1024 / 64 MiB | per-inbox flood |
| `WG_FED_NODE_RETENTION_SECS` / `…_GC_INTERVAL_SECS` | 7 d / 300 s | inbox GC |

Write-auth is **always on**: `PUT /heads` and `/attestations` must be signed by a key the
wgid's sigchain authorizes; `PUT /objects` enforces `cid == hash(bytes)` on write **and**
read. The exec leash dial is env-tightenable too: `WG_FED_LEASH_MAX_TTL_SECS`,
`WG_FED_LEASH_SCOPE` (clamp only — never widens the birth default).

---

## 2. Monitor

Three read endpoints under `/wgfed/v1/`:

```sh
curl -s http://NODE/wgfed/v1/health      # → ok
curl -s http://NODE/wgfed/v1/version     # → WG_FED_COMPAT_VERSION (S-7 handshake)
curl -s http://NODE/wgfed/v1/metrics     # → Prometheus text (M20)
```

Point Prometheus at `/wgfed/v1/metrics`. Key families (all counters):

| Metric | Watch for |
|---|---|
| `wg_node_requests_total`, `wg_node_responses_total{class}` | traffic; a spike in `4xx`/`5xx` |
| `wg_fed_freshness_failures_total` | **stale / withheld-revoke** — alert if it climbs |
| `wg_exec_refusals_total` | placements the fail-closed leash refused |
| `wg_exec_results_accepted_total` / `…_rejected_total` | accept-boundary integrity rejects |
| `wg_review_verdicts_total{disposition}` | `quarantine`/`reject` rate = inbound hostility |

**Logs/tracing:** every plane emits `tracing` events bridged to the existing `env_logger`.
Set `RUST_LOG`:
- `RUST_LOG=info` — node access log (one correlated line per request: `corr`, method, path,
  status), plus exec accept/reject.
- `RUST_LOG=debug` — per-decision detail (review verdicts by `cid`, placement refusals,
  freshness failures). Each line carries a correlation id (`corr=…`) or a natural id (task
  id / content `cid` / wgid) so a single item is traceable across review → placement →
  accept and across the two-host wire.

Suggested alerts: `rate(wg_fed_freshness_failures_total[5m]) > 0`,
`rate(wg_node_responses_total{class="5xx"}[5m]) > 0`, node `/health` not `ok`.

---

## 3. Backup

All state is plain files written **atomically** (temp + fsync + rename) and lock-guarded,
so a hot copy (or filesystem snapshot) is consistent. Back up:

- **Node store** `--store` dir — `objects/`, `heads/`, `inbox/`, `attestations/`. (Inbox is
  transient store-and-forward; GC trims it. Objects/heads/attestations are durable.)
- **Keystore** — `wg secret` backend (OS keyring / `~/.wg/keystore`, mode `0600`). **This is
  the crown jewel** — root seeds live here. Losing it without a recovery key/guardian set
  means the identity cannot be continued (only forked). Back it up encrypted, off-host.
- **Exec lease ledger** `<workgraph_dir>/exec/leases.json` — the epoch fence's integrity
  backstop. A corrupt/partial ledger is **refused, never silently reset** (B3), so restore
  from backup rather than deleting it.
- **Verdict chain** `<workgraph_dir>/review/verdicts.jsonl` — the hash-linked audit/revoke
  log (append is lock-serialized, M23). Append-only; never edit by hand.

Restore = put the files back and restart. Self-verification re-validates everything on read.

---

## 4. Key rotation & recovery

The `wgid:` address is the **genesis** root and never changes; the **active** signing root
rotates underneath it (`WG_FED_COMPAT_VERSION` ≥ 0.2.0 peers verify rotated chains).

```sh
wg identity rotate  <name> --store <L>          # succession: current root signs in the next
wg identity revoke  <name> --kid <KID> --store <L>   # durably revoke a key
wg identity recover <name> --store <L>          # offline recovery key / guardian quorum
wg identity publish <name> --store <L>          # re-publish record + sigchain + a fresh attestation
```

**Compromised signer:** `revoke` the signer kid → `rotate` in a fresh signer → `publish` so
peers fetch the new head, then **`attest`/`publish`** a fresh freshness attestation
(high-value Δ ≤ 15 min) so a withheld-revocation is caught by the freshness gate. Verify
peers see it: `wg identity check-fresh <wgid> --store <L> --class high-value`.

**Lost/compromised root:** if a recovery key or M-of-N guardian set was registered at mint
(`wg identity new … --recovery|--node-less|--guardian`), `recover` installs a new root the
recoverer holds. With **no** recovery control registered, the identity can only be **forked**
to a new `wgid` (download = fork by design) — there is no other path. Register recovery at
mint time.

**UCAN / capability hygiene:** prefer **short TTLs** + `wg identity revoke-cap` over
long-lived grants; a short TTL + revocation makes a stolen signer near-worthless after
expiry. Revocations are freshness-gated, so publish a fresh revocation head after revoking.

---

## 5. dual-main & `wg done` — federation-operator footguns

These are operationally real and bite anyone running a node **from this repo tree**:

- **`wg done`'s origin push fails by design.** Local WG `main` (internal/federation) and
  GitHub `origin/main` (public, lags) diverge; `wg done` squash-merges to local `main` and
  its push to `origin` is *expected* to fail. Land contributor PRs via a `--no-ff` merge on a
  temp worktree based on `origin/main`, not by forcing `wg done`'s push.
- **Squash-merge drops authorship.** `wg done` squashes with a fixed message, dropping the
  commit author + `Co-authored-by`. Credit external contributors via the GitHub PR record
  (comment + close-as-landed), not a trailer.
- **Manual `wg` inside the repo hits the global daemon.** A `wg` command run from inside the
  WG checkout talks to the *global* daemon and shared graph. For an isolated test/op, run
  from `/tmp` with an explicit `--dir`, or pin a freshly-built local `wg` on `PATH`.
- **Smoke gate clobber.** Concurrent agents/installs can replace the global `wg` binary
  mid-run. Pin a freshly-built local `wg` on `PATH` before `wg done` / smoke runs.

---

## Quick triage

| Symptom | First check |
|---|---|
| Peers reject our messages after upgrade | `/version` mismatch — `WG_FED_COMPAT_VERSION` (S-7) |
| `freshness_failures` climbing | a peer's attestation is stale / a revoke is withheld — re-`publish` + `check-fresh` |
| Node `503`s | at `WG_FED_NODE_MAX_CONN` — raise it or front with more nodes |
| Node `413`s | body over `WG_FED_NODE_MAX_BODY` — expected for oversize; raise if legitimate |
| `exec_results_rejected_total` rising | attribution/integrity rejects at accept — inspect `RUST_LOG=info` reject lines |
| Lease ledger won't load | corrupt/partial (B3 refuses) — restore `exec/leases.json` from backup, don't delete |
