//! Crash-safe Casa projection/outbox store.
//!
//! Authority is deliberately absent: the store accepts only WG-authenticated,
//! WG-Review digest-pinned input assembled by `main::ingest`. Projection rows and
//! connector receipts cannot be passed back as capabilities or signatures.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::model::{
    AcceptedEvent, DeliveryAttempt, DeliveryIntent, INTENT_SCHEMA, IngressReceipt,
    PROJECTION_SCHEMA, ProjectionDisplay, ProjectionRow,
};

pub struct CasaStore {
    root: PathBuf,
}

impl CasaStore {
    pub fn open(root: &Path) -> Result<Self> {
        create_private_dir(root)?;
        for leaf in ["events", "ingress", "source-index", "outbox", "attempts"] {
            create_private_dir(&root.join(leaf))?;
        }
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    fn event_path(&self, event_cid: &str) -> PathBuf {
        self.root
            .join("events")
            .join(format!("{}.json", safe(event_cid)))
    }

    fn receipt_path(&self, event_cid: &str) -> PathBuf {
        self.root
            .join("ingress")
            .join(format!("{}.json", safe(event_cid)))
    }

    fn source_path(&self, src_id: &str) -> PathBuf {
        self.root
            .join("source-index")
            .join(format!("{}.json", safe(src_id)))
    }

    fn intent_path(&self, intent_id: &str) -> PathBuf {
        self.root
            .join("outbox")
            .join(format!("{}.json", safe(intent_id)))
    }

    fn attempt_path(&self, intent_id: &str) -> PathBuf {
        self.root
            .join("attempts")
            .join(format!("{}.jsonl", safe(intent_id)))
    }

    /// Persist an authenticated+reviewed event before any Casa product action.
    pub fn put_event(&self, event: &AcceptedEvent, receipt: &IngressReceipt) -> Result<()> {
        write_json_idempotent(&self.event_path(&event.event_cid), event)?;
        write_json_idempotent(&self.receipt_path(&event.event_cid), receipt)?;
        Ok(())
    }

    /// Atomically elect the first authenticated+reviewed event carrying a product
    /// srcId as the projection winner. The key suppresses duplicate UI/election work;
    /// it confers no authority because this function is reachable only after WG gates.
    pub fn claim_source(&self, src_id: &str, event_cid: &str) -> Result<String> {
        let path = self.source_path(src_id);
        let bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "casa.source-index.v1",
            "srcId": src_id,
            "eventCid": event_cid,
            "authority": false,
        }))?;
        match create_new_private(&path, &bytes) {
            Ok(()) => Ok(event_cid.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let v: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
                v.get("eventCid")
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
                    .context("source-index record lacks eventCid")
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn load_event(&self, event_cid: &str) -> Result<Option<AcceptedEvent>> {
        let path = self.event_path(event_cid);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn update_event(&self, event: &AcceptedEvent) -> Result<()> {
        write_json_atomic(&self.event_path(&event.event_cid), event)
    }

    pub fn ensure_reply_intent(
        &self,
        event: &AcceptedEvent,
        destination: &str,
        reply: &str,
    ) -> Result<DeliveryIntent> {
        let core = serde_json::json!({
            "schema": INTENT_SCHEMA,
            "eventCid": event.event_cid,
            "destinationId": destination,
            "renderProfile": "casa-calm-v1",
            "text": reply,
        });
        let id = worksgood::identity::content_cid(&core);
        let intent = DeliveryIntent {
            schema: INTENT_SCHEMA.into(),
            id: id.clone(),
            event_cid: event.event_cid.clone(),
            destination_id: destination.into(),
            render_profile: "casa-calm-v1".into(),
            text: reply.into(),
        };
        write_json_idempotent(&self.intent_path(&id), &intent)?;
        Ok(intent)
    }

    pub fn list_intents(&self) -> Result<Vec<DeliveryIntent>> {
        read_json_dir(&self.root.join("outbox"))
    }

    pub fn record_attempt(&self, attempt: &DeliveryAttempt) -> Result<()> {
        let path = self.attempt_path(&attempt.intent_cid);
        let mut all = self.load_attempts(&attempt.intent_cid)?;
        // Retrying an already-recorded terminal API acceptance is idempotent.
        if all.iter().any(|a| a.state == "api-accepted") && attempt.state == "api-accepted" {
            return Ok(());
        }
        all.push(attempt.clone());
        let mut bytes = Vec::new();
        for row in all {
            serde_json::to_writer(&mut bytes, &row)?;
            bytes.push(b'\n');
        }
        write_bytes_atomic(&path, &bytes)
    }

    pub fn load_attempts(&self, intent_id: &str) -> Result<Vec<DeliveryAttempt>> {
        let path = self.attempt_path(intent_id);
        let text = match fs::read_to_string(path) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).map_err(Into::into))
            .collect()
    }

    pub fn next_attempt(&self, intent_id: &str) -> Result<u32> {
        Ok(self.load_attempts(intent_id)?.len() as u32 + 1)
    }

    pub fn rebuild(&self, graph: &Path) -> Result<Vec<ProjectionRow>> {
        let mut events: Vec<AcceptedEvent> = read_json_dir(&self.root.join("events"))?;
        events.sort_by(|a, b| a.event_cid.cmp(&b.event_cid));
        let verdicts = worksgood::review::verdict::VerdictStore::open(graph);
        let intents = self.list_intents()?;
        let mut rows = Vec::new();
        for event in events.iter().filter(|event| event.duplicate_of.is_none()) {
            // Rebuild never blindly trusts the adapter cache: exact bytes still need a
            // live WG-Review accept pin. Projection deletion is recoverable; review
            // authority is not reconstructed from Casa files.
            let pin = verdicts.digest_pin_consume(&event.reviewed_body)?;
            let review_record = verdicts.load_chain()?.into_iter().find(|record| {
                record.cid == event.review_record_cid
                    && record.provenance.content_cid == event.content_cid
                    && record.provenance.author.as_deref() == Some(event.author_wgid.as_str())
            });
            if !pin.permitted || pin.cid != event.content_cid || review_record.is_none() {
                bail!(
                    "event {} no longer matches its exact WG-Review provenance pin",
                    event.event_cid
                );
            }
            rows.push(ProjectionRow {
                schema: PROJECTION_SCHEMA.into(),
                event_cid: event.event_cid.clone(),
                author_wgid: event.author_wgid.clone(),
                recipient_wgids: event.owner_wgid.clone().into_iter().collect(),
                review_record_cid: event.review_record_cid.clone(),
                direction: "inbound".into(),
                channel: event.envelope.origin.clone(),
                src_id: event.envelope.src_id.clone(),
                in_reply_to: None,
                delivery_state: None,
                text: event.envelope.text.clone(),
                display: ProjectionDisplay {
                    sender: "family member".into(),
                    persona: event.owner_alias.clone(),
                    device: event.envelope.device_label.clone(),
                },
            });
            for intent in intents.iter().filter(|i| i.event_cid == event.event_cid) {
                let attempts = self.load_attempts(&intent.id)?;
                let state = attempts
                    .last()
                    .map(|a| a.state.clone())
                    .unwrap_or_else(|| "queued".into());
                rows.push(ProjectionRow {
                    schema: PROJECTION_SCHEMA.into(),
                    event_cid: intent.id.clone(),
                    author_wgid: event
                        .owner_wgid
                        .clone()
                        .unwrap_or_else(|| event.author_wgid.clone()),
                    recipient_wgids: Vec::new(),
                    review_record_cid: event.review_record_cid.clone(),
                    direction: "outbound".into(),
                    channel: event.envelope.origin.clone(),
                    src_id: event.envelope.src_id.clone(),
                    in_reply_to: Some(event.event_cid.clone()),
                    delivery_state: Some(state),
                    text: intent.text.clone(),
                    display: ProjectionDisplay {
                        sender: event.owner_alias.clone().unwrap_or_else(|| "Casa".into()),
                        persona: event.owner_alias.clone(),
                        device: event.envelope.device_label.clone(),
                    },
                });
            }
        }
        let mut bytes = Vec::new();
        for row in &rows {
            serde_json::to_writer(&mut bytes, row)?;
            bytes.push(b'\n');
        }
        write_bytes_atomic(&self.root.join("feed.jsonl"), &bytes)?;
        Ok(rows)
    }

    pub fn summary(&self) -> Result<serde_json::Value> {
        let events: Vec<AcceptedEvent> = read_json_dir(&self.root.join("events"))?;
        let intents = self.list_intents()?;
        let primary = events.iter().filter(|e| e.duplicate_of.is_none()).count();
        let duplicate = events.len().saturating_sub(primary);
        let owner_elections = events
            .iter()
            .filter(|e| e.duplicate_of.is_none() && e.owner_wgid.is_some())
            .count();
        let accepted_deliveries = intents
            .iter()
            .filter(|i| {
                self.load_attempts(&i.id)
                    .is_ok_and(|a| a.iter().any(|x| x.state == "api-accepted"))
            })
            .count();
        Ok(serde_json::json!({
            "events": events.len(),
            "primaryEvents": primary,
            "duplicateEvents": duplicate,
            "ownerElections": owner_elections,
            "outwardReplies": intents.len(),
            "apiAccepted": accepted_deliveries,
            "feed": self.root.join("feed.jsonl"),
        }))
    }
}

/// Simulated Telegram/web sink with idempotency on the Casa delivery-intent CID.
/// It is connector evidence only; the deterministic native id is not task truth.
pub fn stub_send_exactly_once(sink: &Path, intent: &DeliveryIntent) -> Result<String> {
    create_private_dir(sink)?;
    let path = sink.join("messages.jsonl");
    let mut rows: Vec<serde_json::Value> = match fs::read_to_string(&path) {
        Ok(text) => text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<std::result::Result<_, _>>()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    if let Some(id) = rows.iter().find_map(|row| {
        (row.get("intentCid")?.as_str()? == intent.id)
            .then(|| row.get("nativeMessageId")?.as_str().map(str::to_string))
            .flatten()
    }) {
        return Ok(id);
    }
    let native = format!(
        "stub:{}",
        &safe(&intent.id)[..16.min(safe(&intent.id).len())]
    );
    rows.push(serde_json::json!({
        "intentCid": intent.id,
        "nativeMessageId": native,
        "destinationId": intent.destination_id,
        "text": intent.text,
    }));
    let mut bytes = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut bytes, &row)?;
        bytes.push(b'\n');
    }
    write_bytes_atomic(&path, &bytes)?;
    Ok(native)
}

pub fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    write_bytes_atomic(path, &serde_json::to_vec_pretty(value)?)
}

fn write_json_idempotent(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    match create_new_private(path, &bytes) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if fs::read(path)? == bytes {
                Ok(())
            } else {
                bail!("idempotency collision at {}", path.display())
            }
        }
        Err(e) => Err(e.into()),
    }
}

fn read_json_dir<T: serde::de::DeserializeOwned>(dir: &Path) -> Result<Vec<T>> {
    let mut paths: Vec<_> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            serde_json::from_slice(&fs::read(&path)?)
                .with_context(|| format!("parsing {}", path.display()))
        })
        .collect()
}

fn create_new_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    worksgood::atomic_file::write_atomic(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChannelEnvelope, MessageKind};

    fn event(id: &str) -> AcceptedEvent {
        AcceptedEvent {
            schema: "casa.accepted-event.v1".into(),
            event_cid: id.into(),
            author_wgid: "wgid:test".into(),
            review_record_cid: "b3:review".into(),
            content_cid: "b3:content".into(),
            reviewed_body: "{\"fixture\":true}".into(),
            envelope: ChannelEnvelope {
                schema: crate::model::CHANNEL_SCHEMA.into(),
                message_kind: MessageKind::Request,
                origin: "telegram".into(),
                native_evidence_cid: "b3:evidence".into(),
                src_id: "casa-src:v1:b3:stable".into(),
                device_label: "your iPhone".into(),
                local_date: "2026-07-22".into(),
                text: "Plan dinner".into(),
            },
            duplicate_of: None,
            owner_wgid: None,
            owner_alias: None,
        }
    }

    #[test]
    fn source_claim_is_stable_and_cannot_authorize() {
        let tmp = tempfile::tempdir().unwrap();
        let store = CasaStore::open(tmp.path()).unwrap();
        assert_eq!(store.claim_source("stable", "event-a").unwrap(), "event-a");
        assert_eq!(store.claim_source("stable", "event-b").unwrap(), "event-a");
        let raw = fs::read_to_string(store.source_path("stable")).unwrap();
        assert!(raw.contains("\"authority\": false"));
        assert!(!raw.contains("capability"));
    }

    #[test]
    fn identical_event_write_is_idempotent_but_collision_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let store = CasaStore::open(tmp.path()).unwrap();
        let e = event("b3:event");
        let receipt = IngressReceipt {
            schema: "casa.ingress-receipt.v1".into(),
            event_cid: e.event_cid.clone(),
            recipient_wgid: "wgid:recipient".into(),
            state: "consumed".into(),
            review_record_cid: e.review_record_cid.clone(),
            content_cid: e.content_cid.clone(),
        };
        store.put_event(&e, &receipt).unwrap();
        store.put_event(&e, &receipt).unwrap();
        let mut changed = e;
        changed.author_wgid = "wgid:forged".into();
        assert!(store.put_event(&changed, &receipt).is_err());
    }

    #[test]
    fn stub_sink_is_exactly_once() {
        let tmp = tempfile::tempdir().unwrap();
        let intent = DeliveryIntent {
            schema: INTENT_SCHEMA.into(),
            id: "b3:intent".into(),
            event_cid: "b3:event".into(),
            destination_id: "protected:telegram-family".into(),
            render_profile: "casa-calm-v1".into(),
            text: "Nora owns this".into(),
        };
        let a = stub_send_exactly_once(tmp.path(), &intent).unwrap();
        let b = stub_send_exactly_once(tmp.path(), &intent).unwrap();
        assert_eq!(a, b);
        assert_eq!(
            fs::read_to_string(tmp.path().join("messages.jsonl"))
                .unwrap()
                .lines()
                .count(),
            1
        );
    }
}
