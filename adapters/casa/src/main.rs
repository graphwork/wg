//! Credential-free Casa adapter spark over WG-Fed, WG-Review, and WG-Exec.
//!
//! Authority boundary:
//! - input is an authenticated `wg msg poll --review --json` bundle;
//! - exact content must still match a recorded WG-Review accept verdict;
//! - remote actions remain `wg provider ...` operations outside this binary;
//! - this binary owns only simulated channel envelopes, household election,
//!   projection, and connector outbox/receipt policy.
//!
//! Adaptation/source map: `docs/casa-adapter-spark.md`. No `claw3d-bridge`
//! gateway source was available for review, so this target is not a production
//! Telegram/claw3d gateway.

mod model;
mod store;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use model::{
    AcceptedEvent, CHANNEL_SCHEMA, ChannelEnvelope, DeliveryAttempt, HouseholdRoster,
    IngressReceipt, MessageKind,
};
use store::{CasaStore, stub_send_exactly_once, write_json_atomic};
use worksgood::identity::envelope::payload_cid;
use worksgood::identity::transport::open_store;
use worksgood::review::verdict::VerdictStore;

#[derive(Parser)]
#[command(
    name = "casa-adapter",
    about = "Casa companion adapter spark (not a production gateway)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build a deterministic simulated Telegram/web envelope. Native ids are
    /// domain-hashed and are not written into the output.
    Envelope {
        #[arg(long, value_enum)]
        kind: KindArg,
        #[arg(long)]
        origin: String,
        #[arg(long)]
        native_chat: String,
        #[arg(long)]
        native_sender: String,
        #[arg(long)]
        native_date: String,
        #[arg(long)]
        device_label: String,
        #[arg(long)]
        local_date: String,
        #[arg(long)]
        text: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Put/get signed WG-Exec envelopes through the existing untrusted FedStore
    /// object API. This is an adapter call to WG transport, not a second transport.
    Relay {
        #[command(subcommand)]
        command: RelayCommand,
    },
    /// Consume only authenticated, review-accepted, exact-digest-pinned poll output.
    Ingest {
        #[arg(long)]
        graph: PathBuf,
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        poll: PathBuf,
        #[arg(long)]
        roster: PathBuf,
        #[arg(long)]
        destination: String,
        /// Test seam: stop after durable receive/source claim, before election.
        #[arg(long)]
        crash_after_receipt: bool,
    },
    /// Drive the simulated connector outbox. A crash after the stub accepts but
    /// before the receipt is written is recoverable by replaying this command.
    Deliver {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        sink: PathBuf,
        #[arg(long, value_enum, default_value = "api-accepted")]
        outcome: DeliveryOutcome,
        #[arg(long)]
        crash_after_send: bool,
    },
    /// Delete-safe deterministic feed rebuild from accepted-event and WG review
    /// receipts plus delivery attempts.
    Rebuild {
        #[arg(long)]
        graph: PathBuf,
        #[arg(long)]
        state: PathBuf,
    },
    Summary {
        #[arg(long)]
        state: PathBuf,
    },
}

#[derive(Subcommand)]
enum RelayCommand {
    Put {
        #[arg(long)]
        store: String,
        #[arg(long)]
        file: PathBuf,
    },
    Get {
        #[arg(long)]
        store: String,
        #[arg(long)]
        cid: String,
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum KindArg {
    Request,
    Report,
}

#[derive(Clone, Copy, ValueEnum)]
enum DeliveryOutcome {
    AttemptUnknown,
    ApiAccepted,
    FailedRetryable,
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("casa-adapter: {error:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Envelope {
            kind,
            origin,
            native_chat,
            native_sender,
            native_date,
            device_label,
            local_date,
            text,
            out,
        } => {
            let evidence = domain_cid(
                "casa-native-evidence-v1",
                &[&origin, &native_chat, &native_sender, &native_date],
            );
            let src = domain_cid("casa-src-v1", &[&origin, &evidence, &local_date, &text]);
            let env = ChannelEnvelope {
                schema: CHANNEL_SCHEMA.into(),
                message_kind: match kind {
                    KindArg::Request => MessageKind::Request,
                    KindArg::Report => MessageKind::Report,
                },
                origin,
                native_evidence_cid: evidence,
                src_id: format!("casa-src:v1:{src}"),
                device_label,
                local_date,
                text,
            };
            env.validate()?;
            write_json_atomic(&out, &env)?;
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "written": out,
                    "srcId": env.src_id,
                    "origin": env.origin,
                    "authority": false,
                }))?
            );
        }
        Command::Relay { command } => match command {
            RelayCommand::Put { store, file } => {
                let bytes = std::fs::read(&file)?;
                let cid = payload_cid(&bytes);
                open_store(&store)?.put_object(&cid, &bytes)?;
                println!("{}", serde_json::json!({"cid": cid, "bytes": bytes.len()}));
            }
            RelayCommand::Get { store, cid, out } => {
                let bytes = open_store(&store)?.get_object(&cid)?;
                if payload_cid(&bytes) != cid {
                    bail!("relay object CID mismatch");
                }
                worksgood::atomic_file::write_atomic(&out, &bytes)?;
                println!(
                    "{}",
                    serde_json::json!({"cid": cid, "out": out, "bytes": bytes.len()})
                );
            }
        },
        Command::Ingest {
            graph,
            state,
            poll,
            roster,
            destination,
            crash_after_receipt,
        } => ingest(
            &graph,
            &state,
            &poll,
            &roster,
            &destination,
            crash_after_receipt,
        )?,
        Command::Deliver {
            state,
            sink,
            outcome,
            crash_after_send,
        } => deliver(&state, &sink, outcome, crash_after_send)?,
        Command::Rebuild { graph, state } => {
            let rows = CasaStore::open(&state)?.rebuild(&graph)?;
            println!(
                "{}",
                serde_json::json!({"rebuilt": true, "rows": rows.len(), "feed": state.join("feed.jsonl")})
            );
        }
        Command::Summary { state } => println!("{}", CasaStore::open(&state)?.summary()?),
    }
    Ok(())
}

fn ingest(
    graph: &Path,
    state: &Path,
    poll_path: &Path,
    roster_path: &Path,
    destination: &str,
    crash_after_receipt: bool,
) -> Result<()> {
    let poll: serde_json::Value =
        serde_json::from_slice(&std::fs::read(poll_path)?).context("poll input is not JSON")?;
    let recipient = poll
        .get("wgid")
        .and_then(|v| v.as_str())
        .context("input is not a wg poll bundle (missing wgid)")?;
    worksgood::identity::keys::pubkey_from_wgid(recipient)
        .map_err(|_| anyhow::anyhow!("poll recipient is not a canonical wgid"))?;
    let events = poll
        .get("events")
        .and_then(|v| v.as_array())
        .context("input is not a wg poll bundle (missing events)")?;
    let roster = HouseholdRoster::load(roster_path)?;
    let store = CasaStore::open(state)?;
    let verdicts = VerdictStore::open(graph);
    let mut consumed = 0usize;
    let mut duplicates = 0usize;
    let mut outward = 0usize;

    for item in events {
        if item.get("verdict").and_then(|v| v.as_str()) != Some("VERIFIED") {
            continue; // authentication rejected before Casa
        }
        if item.get("consumable").and_then(|v| v.as_bool()) != Some(true) {
            continue; // quarantine/reject/replay body remains withheld
        }
        let event_cid = item
            .get("event_cid")
            .and_then(|v| v.as_str())
            .context("authenticated poll item lacks event_cid")?;
        let author = item
            .get("from")
            .and_then(|v| v.as_str())
            .context("poll item lacks from")?;
        worksgood::identity::keys::pubkey_from_wgid(author)
            .map_err(|_| anyhow::anyhow!("poll author is not a canonical wgid"))?;
        let body = item
            .get("body")
            .and_then(|v| v.as_str())
            .context("consumable item lacks body")?;
        let review = item
            .get("review")
            .context("consumable item lacks WG-Review result")?;
        if review.get("trust_derived").and_then(|v| v.as_bool()) != Some(true)
            || review.get("verdict").and_then(|v| v.as_str()) != Some("accept")
        {
            bail!("Casa refuses a poll item without derived-trust WG-Review acceptance");
        }
        let content_cid = review
            .get("content_cid")
            .and_then(|v| v.as_str())
            .context("review result lacks content_cid")?;
        let exact = worksgood::identity::content_cid(&serde_json::Value::String(body.into()));
        if exact != content_cid {
            bail!("digest-pinned input changed after Review");
        }
        let pin = verdicts.digest_pin_consume(body)?;
        if !pin.permitted || pin.cid != content_cid {
            bail!("WG-Review has no accept verdict for these exact bytes");
        }
        let review_record = verdicts
            .find_by_cid(content_cid)?
            .context("accepted review record disappeared")?;
        if review_record.provenance.author.as_deref() != Some(author)
            || review_record.provenance.content_cid != content_cid
        {
            bail!("poll author/body is not bound to the recorded WG-Review provenance");
        }
        if !event_cid.starts_with("b3:") || event_cid.len() != 67 {
            bail!("authenticated event_cid is malformed");
        }
        let env: ChannelEnvelope =
            serde_json::from_str(body).context("accepted body is not a Casa envelope")?;
        env.validate()?;
        let expected_src = format!(
            "casa-src:v1:{}",
            domain_cid(
                "casa-src-v1",
                &[
                    &env.origin,
                    &env.native_evidence_cid,
                    &env.local_date,
                    &env.text
                ]
            )
        );
        // Luca's pinned behavior includes stable origin/srcId dedupe; here srcId is
        // verified as product evidence after WG authentication/review and cannot grant.
        if env.src_id != expected_src {
            bail!("Casa srcId does not match the accepted channel evidence");
        }

        let winner = store.claim_source(&env.src_id, event_cid)?;
        let duplicate_of = (winner != event_cid).then_some(winner.clone());
        let mut accepted = AcceptedEvent {
            schema: "casa.accepted-event.v1".into(),
            event_cid: event_cid.into(),
            author_wgid: author.into(),
            review_record_cid: review_record.cid.clone(),
            content_cid: content_cid.into(),
            reviewed_body: body.into(),
            envelope: env,
            duplicate_of,
            owner_wgid: None,
            owner_alias: None,
        };
        let receipt = IngressReceipt {
            schema: "casa.ingress-receipt.v1".into(),
            event_cid: event_cid.into(),
            recipient_wgid: recipient.into(),
            state: "consumed".into(),
            review_record_cid: review_record.cid,
            content_cid: content_cid.into(),
        };
        // A retry after election/outbox completion reuses the enriched durable event;
        // a retry after the deliberate receipt-only crash resumes from owner=None.
        if let Some(existing) = store.load_event(event_cid)? {
            if existing.author_wgid != accepted.author_wgid
                || existing.content_cid != accepted.content_cid
                || existing.envelope.src_id != accepted.envelope.src_id
            {
                bail!("authenticated event id collided with different Casa content");
            }
            accepted = existing;
        }
        store.put_event(&accepted, &receipt)?;
        consumed += 1;
        if accepted.duplicate_of.is_some() {
            duplicates += 1;
            continue;
        }
        if crash_after_receipt {
            bail!("simulated crash after durable receipt/source claim");
        }
        if accepted.envelope.message_kind == MessageKind::Request {
            if accepted.owner_wgid.is_none() {
                let owner = roster.elect(&accepted.envelope.text)?;
                accepted.owner_wgid = Some(owner.wgid.clone());
                accepted.owner_alias = Some(owner.alias.clone());
                store.update_event(&accepted)?;
            }
            let date =
                chrono::NaiveDate::parse_from_str(&accepted.envelope.local_date, "%Y-%m-%d")?;
            let reply = format!(
                "{} owns this request for {}. I’ll report back after WG-Exec accepts the result.",
                accepted
                    .owner_alias
                    .as_deref()
                    .context("elected request lacks owner alias")?,
                date.format("%A, %b %-d, %Y")
            );
            store.ensure_reply_intent(&accepted, destination, &reply)?;
            outward += 1;
        }
    }
    let rows = store.rebuild(graph)?;
    println!(
        "{}",
        serde_json::json!({
            "consumed": consumed,
            "duplicates": duplicates,
            "outwardRepliesEnsured": outward,
            "projectionRows": rows.len(),
        })
    );
    Ok(())
}

fn deliver(
    state: &Path,
    sink: &Path,
    outcome: DeliveryOutcome,
    crash_after_send: bool,
) -> Result<()> {
    let store = CasaStore::open(state)?;
    let intents = store.list_intents()?;
    let mut processed = 0usize;
    for intent in intents {
        let attempt_no = store.next_attempt(&intent.id)?;
        let (state_name, native, error) = match outcome {
            DeliveryOutcome::AttemptUnknown => {
                ("attempt-unknown", None, Some("timeout-unknown".into()))
            }
            DeliveryOutcome::FailedRetryable => {
                ("failed-retryable", None, Some("stub-retryable".into()))
            }
            DeliveryOutcome::ApiAccepted => {
                let native = stub_send_exactly_once(sink, &intent)?;
                if crash_after_send {
                    bail!("simulated crash after channel accepted, before adapter ack");
                }
                ("api-accepted", Some(native), None)
            }
        };
        store.record_attempt(&DeliveryAttempt {
            schema: "wg.delivery-attempt.v1".into(),
            intent_cid: intent.id,
            attempt: attempt_no,
            state: state_name.into(),
            native_message_id: native,
            error_code: error,
        })?;
        processed += 1;
    }
    println!(
        "{}",
        serde_json::json!({"processed": processed, "outcome": match outcome {
            DeliveryOutcome::AttemptUnknown => "attempt-unknown",
            DeliveryOutcome::ApiAccepted => "api-accepted",
            DeliveryOutcome::FailedRetryable => "failed-retryable",
        }})
    );
    Ok(())
}

fn domain_cid(domain: &str, parts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain.as_bytes());
    for part in parts {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    format!("b3:{}", hasher.finalize().to_hex())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_id_is_stable_origin_sensitive_and_hides_native_ids() {
        let a = domain_cid("casa-src-v1", &["telegram", "chat-7", "1700", "hello"]);
        let b = domain_cid("casa-src-v1", &["telegram", "chat-7", "1700", "hello"]);
        let c = domain_cid("casa-src-v1", &["casa-web", "chat-7", "1700", "hello"]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(!a.contains("chat-7"));
    }
}
