//! Envelope-layer replay/dedup at the **consume edge** (audit M9, FR-M6).
//!
//! `SignedEvent::verify` authenticates *who sent* an event and that its bytes are
//! intact — it does **not** dedup. On a store-and-forward path the same signed,
//! validly-authenticated event can be **re-delivered** (the transport is
//! at-least-once), **re-polled across sessions**, or **deliberately replayed** by a
//! hostile relay that keeps a copy. Nothing above the transport stops a recipient
//! from consuming the *same* authenticated event twice — a replay (FR-M6: delivery
//! must be idempotent at the consumer).
//!
//! The defense is a small, persistent **seen-id store** the recipient consults at the
//! moment it would consume an event. The dedup key is the event's **authenticated**
//! content id (`SignedEvent.id`, which `verify` pins to `core_id()` under the sender's
//! signature — an attacker cannot mint a fresh id without re-signing, which needs the
//! sender's key), or for a sealed-sender event the content id of the *inner*
//! signed payload (the only authenticated handle a recipient has). First sight of a
//! key is recorded and the event is consumed; any later sight of the same key is a
//! **replay** and consumption is refused.
//!
//! This is intentionally separate from the inbox's own delivery-time id overwrite
//! (`FedStore::put_event` is idempotent *per inbox*): that protects one inbox from
//! duplicate storage, but it cannot stop a replay into a *fresh* inbox, a re-poll
//! after a GC, or a cross-store replay. The consume-edge store is durable and
//! recipient-scoped, so it survives all three.

use anyhow::{Context, Result};
use std::path::PathBuf;

/// A durable, recipient-scoped record of event ids already consumed (the replay
/// backstop). Backed by marker files under `dir/<recipient>/<event-id>` — content is
/// irrelevant, presence is the fact. Cheap to check, append-only, never trusted to a
/// remote (it is the *verifier's* local memory).
pub struct DedupStore {
    dir: PathBuf,
}

impl DedupStore {
    /// Bind a dedup store rooted at `dir` (e.g. `<wgdir>/identity/consumed`).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn marker_path(&self, recipient_wgid: &str, dedup_key: &str) -> PathBuf {
        self.dir
            .join(super::transport::sanitize(recipient_wgid))
            .join(super::transport::sanitize(dedup_key))
    }

    /// Whether `dedup_key` has already been consumed by `recipient_wgid`.
    pub fn seen(&self, recipient_wgid: &str, dedup_key: &str) -> bool {
        self.marker_path(recipient_wgid, dedup_key).exists()
    }

    /// Record `dedup_key` as consumed by `recipient_wgid`. Idempotent.
    pub fn record(&self, recipient_wgid: &str, dedup_key: &str) -> Result<()> {
        let path = self.marker_path(recipient_wgid, dedup_key);
        let parent = path
            .parent()
            .expect("marker path always has a recipient parent dir");
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating dedup dir {}", parent.display()))?;
        // Touch the marker (the bytes are irrelevant; presence is the fact).
        std::fs::write(&path, b"")
            .with_context(|| format!("writing dedup marker {}", path.display()))?;
        Ok(())
    }

    /// Atomically check-and-record at the consume edge: returns `true` the **first**
    /// time `dedup_key` is seen (the caller may consume the event) and `false` on any
    /// later sight (a **replay** — the caller must refuse to consume it again).
    ///
    /// Not guarded against concurrent racing pollers of the *same* recipient inbox
    /// (the spark consumes single-threaded per identity); the durable marker still
    /// makes consumption idempotent across sequential polls, which is the FR-M6 bar.
    pub fn check_and_record(&self, recipient_wgid: &str, dedup_key: &str) -> Result<bool> {
        if self.seen(recipient_wgid, dedup_key) {
            return Ok(false);
        }
        self.record(recipient_wgid, dedup_key)?;
        Ok(true)
    }
}

/// The authenticated dedup key for an event the recipient has already verified.
/// Lives here so the consume edge and any tests agree on exactly one derivation.
///
/// - A normal event: its signature-pinned `id` (`core_id`). `verify` rejects an event
///   whose `id` does not match its content, so the key cannot be forged.
/// - A sealed-sender event: the content id of the inner signed payload (`inner_cid`),
///   the only handle a recipient authenticates (the outer `id` is unsigned for a
///   sealed-sender event, so it is **not** used as the key).
pub fn dedup_key_for(event_id: &str, sealed_sender_inner_cid: Option<&str>) -> String {
    match sealed_sender_inner_cid {
        Some(cid) => cid.to_string(),
        None => event_id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "wg-dedup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p
    }

    #[test]
    fn first_sight_records_replay_refused() {
        let dir = scratch();
        let s = DedupStore::new(&dir);
        let rcpt = "wgid:zRecipient";
        let key = "b3:abc123";
        // First sight: newly recorded → consume.
        assert!(
            s.check_and_record(rcpt, key).unwrap(),
            "first sight must be fresh"
        );
        assert!(s.seen(rcpt, key));
        // Replay: already seen → refuse.
        assert!(
            !s.check_and_record(rcpt, key).unwrap(),
            "a replay of the same key must be refused"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dedup_is_recipient_scoped() {
        let dir = scratch();
        let s = DedupStore::new(&dir);
        let key = "b3:shared";
        // Two distinct recipients each consume the same key once independently.
        assert!(s.check_and_record("wgid:zAlice", key).unwrap());
        assert!(
            s.check_and_record("wgid:zBob", key).unwrap(),
            "a different recipient has not seen this key yet"
        );
        // …but neither may consume it twice.
        assert!(!s.check_and_record("wgid:zAlice", key).unwrap());
        assert!(!s.check_and_record("wgid:zBob", key).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn key_derivation_prefers_inner_cid_for_sealed_sender() {
        // A normal event keys on its (authenticated) outer id.
        assert_eq!(dedup_key_for("b3:outer", None), "b3:outer");
        // A sealed-sender event keys on the inner signed payload cid, never the
        // malleable outer id.
        assert_eq!(dedup_key_for("b3:outer", Some("b3:inner")), "b3:inner");
    }
}
