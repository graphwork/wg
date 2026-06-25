//! `wg review` — the inbound-content review gate (Content-Safety spark, Review-Wave B).
//!
//! The CLI surface over [`worksgood::review`]: screen inbound content through the
//! fail-closed, trust-proportional pipeline **before an agent consumes it**, show
//! the applied depth, surface the reviewer's no-scope bound, render the verdict
//! sigchain, enforce digest-pinned consumption, and run the loud revoke leg. This is
//! the surface the `content_safety_spark.sh` smoke drives.

use anyhow::{Context, Result};
use std::path::Path;

use worksgood::graph::TrustLevel;
use worksgood::review::pass2_review::reviewer_scope;
use worksgood::review::{
    ContentClass, Provenance, Sensitivity, VerdictStore, review_depth, review_inbound,
};

/// Parse a trust level from the CLI; an unrecognized value is `Unknown`
/// (fail-closed — we never silently grant *more* trust).
fn parse_trust(s: &str) -> TrustLevel {
    match s.trim().to_ascii_lowercase().as_str() {
        "verified" => TrustLevel::Verified,
        "provisional" => TrustLevel::Provisional,
        _ => TrustLevel::Unknown,
    }
}

/// Strictness ordering for trust (least-trusted wins): Verified < Provisional <
/// Unknown. Used to fold a revoke-lowered override against the declared trust.
fn trust_rank(t: &TrustLevel) -> u8 {
    match t {
        TrustLevel::Verified => 0,
        TrustLevel::Provisional => 1,
        TrustLevel::Unknown => 2,
    }
}

fn strictest_trust(a: TrustLevel, b: TrustLevel) -> TrustLevel {
    if trust_rank(&a) >= trust_rank(&b) {
        a
    } else {
        b
    }
}

/// `wg review check` — run one inbound item through the pipeline and record a verdict.
#[allow(clippy::too_many_arguments)]
pub fn run_check(
    workgraph_dir: &Path,
    class: &str,
    trust: &str,
    content_file: &str,
    author: Option<&str>,
    sensitivity: Option<&str>,
    consumer_task: Option<&str>,
    json: bool,
) -> Result<()> {
    let content_class = ContentClass::parse(class)
        .with_context(|| format!("unknown content class {class:?} (want IC1|IC2|IC3|IC4)"))?;
    let content = std::fs::read_to_string(content_file)
        .with_context(|| format!("reading content file {content_file}"))?;
    let declared_trust = parse_trust(trust);
    let self_sensitivity = Sensitivity::parse(sensitivity);

    let store = VerdictStore::open(workgraph_dir);

    // Fold any revoke-lowered trust override against the declared trust: the
    // strictest wins, so a revoked author's *next* item takes the deep path (D4).
    let effective_trust = match author.and_then(|a| store.trust_override(a).ok().flatten()) {
        Some(overridden) => strictest_trust(declared_trust, overridden),
        None => declared_trust,
    };

    let provenance = Provenance {
        author: author.map(|s| s.to_string()),
        trust: effective_trust.clone(),
    };

    let outcome = review_inbound(content_class, &content, &provenance, self_sensitivity);
    let record = store.record(&outcome, author, consumer_task)?;

    if json {
        let v = serde_json::json!({
            "verdict": outcome.verdict.tag(),
            "reason": outcome.reason.tag(),
            "content_class": content_class.tag(),
            "deciding_pass": outcome.deciding_pass,
            "confidence": outcome.confidence.tag(),
            "depth": {
                "label": outcome.depth.label,
                "max_pass": outcome.depth.max_pass,
                "quorum": outcome.depth.quorum,
                "default_verdict": outcome.depth.default_verdict.tag(),
                "is_light": outcome.depth.is_light(),
            },
            "effective_trust": trust_tag(&effective_trust),
            "effective_sensitivity": sensitivity_tag(outcome.effective_sensitivity),
            "sensitivity_overridden": outcome.sensitivity_overridden,
            "permits_consumption": outcome.verdict.permits_consumption(),
            "content_cid": outcome.content_cid,
            "provenance": {
                "author": author,
                "sigchain_pos": record.provenance.sigchain_pos,
            },
            "consumer_task": consumer_task,
            "cid": record.cid,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!("verdict:   {}", outcome.verdict.tag());
        println!("reason:    {}", outcome.reason.tag());
        println!("class:     {}", content_class.tag());
        println!(
            "depth:     {} (pass {})",
            outcome.depth.label, outcome.deciding_pass
        );
        println!(
            "trust:     {} (effective){}",
            trust_tag(&effective_trust),
            if outcome.sensitivity_overridden {
                "  [sensitivity overridden low→high by taint-inference]"
            } else {
                ""
            }
        );
        println!(
            "consume:   {}",
            if outcome.verdict.permits_consumption() {
                "PERMITTED"
            } else {
                "BLOCKED (held un-consumed until review releases)"
            }
        );
        println!("cid:       {}", outcome.content_cid);
    }
    Ok(())
}

/// `wg review depth` — show the applied review depth for a trust × sensitivity pair.
pub fn run_depth(trust: &str, sensitivity: Option<&str>, json: bool) -> Result<()> {
    let t = parse_trust(trust);
    // Fold Unlabeled → High before the matrix (the fail-closed caller contract).
    let s = match Sensitivity::parse(sensitivity) {
        Sensitivity::Unlabeled => Sensitivity::High,
        other => other,
    };
    let depth = review_depth(&t, s);
    if json {
        let v = serde_json::json!({
            "trust": trust_tag(&t),
            "sensitivity": sensitivity_tag(s),
            "label": depth.label,
            "max_pass": depth.max_pass,
            "quorum": depth.quorum,
            "default_verdict": depth.default_verdict.tag(),
            "is_light": depth.is_light(),
            "runs_pass2": depth.runs_pass2(),
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!("trust={} sensitivity={}", trust_tag(&t), sensitivity_tag(s));
        println!("  depth:           {}", depth.label);
        println!("  default verdict: {}", depth.default_verdict.tag());
        println!("  light path:      {}", depth.is_light());
        println!("  quorum:          {}", depth.quorum);
    }
    Ok(())
}

/// `wg review reviewer-scope` — surface the dual-LLM no-scope bound.
pub fn run_reviewer_scope(json: bool) -> Result<()> {
    let scope = reviewer_scope();
    if json {
        let v = serde_json::json!({
            "granted_scope": scope,
            "can_write_graph": false,
            "can_network": false,
            "can_exfil": false,
            "bound": "dual-LLM no-scope: an injection of the reviewer yields a wrong \
                      verdict, never a wrong action (ADR-CS2 D1)",
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!("Pass-2 reviewer granted scope: {scope:?}");
        println!("  graph-write: NO   network: NO   exfil: NO");
        println!(
            "  dual-LLM no-scope bound: a flipped reviewer yields a wrong VERDICT, \
             never a wrong ACTION."
        );
    }
    Ok(())
}

/// `wg review log` — render the recorded verdict sigchain.
pub fn run_log(workgraph_dir: &Path, json: bool) -> Result<()> {
    let store = VerdictStore::open(workgraph_dir);
    let chain = store.load_chain()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&chain)?);
    } else if chain.is_empty() {
        println!("(no verdicts recorded)");
    } else {
        for r in &chain {
            println!(
                "#{:<3} {:<10} {:<22} class={} pass={} author={} cid={}",
                r.seq,
                r.verdict.tag(),
                r.reason.tag(),
                r.content_class.tag(),
                r.deciding_pass,
                r.provenance.author.as_deref().unwrap_or("<none>"),
                short_cid(&r.provenance.content_cid),
            );
        }
    }
    Ok(())
}

/// `wg review consume` — digest-pinned consumption (MUST-2).
pub fn run_consume(workgraph_dir: &Path, content_file: &str, json: bool) -> Result<()> {
    let store = VerdictStore::open(workgraph_dir);
    let content = std::fs::read_to_string(content_file)
        .with_context(|| format!("reading content file {content_file}"))?;
    let result = store.digest_pin_consume(&content)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!(
            "consume:   {}",
            if result.permitted {
                "PERMITTED"
            } else {
                "REFUSED"
            }
        );
        println!("cid:       {}", result.cid);
        println!("detail:    {}", result.detail);
    }
    Ok(())
}

/// `wg review revoke` — the loud revoke leg (ADR-CS3 D4).
pub fn run_revoke(workgraph_dir: &Path, cid: &str, json: bool) -> Result<()> {
    let store = VerdictStore::open(workgraph_dir);
    let out = store.revoke(cid)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!(
            "REVOKE (loud) — poison digest {}",
            short_cid(&out.content_cid)
        );
        println!(
            "  traced author:    {} (sigchain pos {})",
            out.author, out.sigchain_pos
        );
        println!("  lowered trust to: {}", trust_tag(&out.lowered_trust));
        if out.rerun_consumers.is_empty() {
            println!("  re-run consumers: (none recorded)");
        } else {
            println!("  re-run consumers: {}", out.rerun_consumers.join(", "));
        }
    }
    Ok(())
}

fn trust_tag(t: &TrustLevel) -> &'static str {
    match t {
        TrustLevel::Verified => "verified",
        TrustLevel::Provisional => "provisional",
        TrustLevel::Unknown => "unknown",
    }
}

fn sensitivity_tag(s: Sensitivity) -> &'static str {
    match s {
        Sensitivity::Low => "low",
        Sensitivity::High => "high",
        Sensitivity::Unlabeled => "unlabeled",
    }
}

fn short_cid(cid: &str) -> String {
    if cid.len() > 14 {
        format!("{}…", &cid[..14])
    } else {
        cid.to_string()
    }
}
