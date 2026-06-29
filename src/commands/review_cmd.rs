//! `wg review` — the inbound-content review gate (Content-Safety spark, Review-Wave B).
//!
//! The CLI surface over [`worksgood::review`]: screen inbound content through the
//! fail-closed, trust-proportional pipeline **before an agent consumes it**, show
//! the applied depth, surface the reviewer's no-scope bound, render the verdict
//! sigchain, enforce digest-pinned consumption, and run the loud revoke leg. This is
//! the surface the `content_safety_spark.sh` smoke drives.

use anyhow::{Context, Result};
use std::path::Path;

use worksgood::config::Config;
use worksgood::graph::TrustLevel;
use worksgood::review::eval::{self, Bucket, EvalReport, Thresholds};
use worksgood::review::pass2_review::reviewer_scope;
use worksgood::review::reviewer::{AgencyReviewLlm, model_review_available};
use worksgood::review::{
    ContentClass, Provenance, Sensitivity, VerdictStore, review_depth, review_inbound,
    review_inbound_ctx,
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

    // Use the real model-driven reviewer when a model is available for this config
    // (production); fall back to the deterministic pipeline otherwise (credential-free
    // CI / smoke). `review_inbound_ctx` is byte-identical to `review_inbound` when no
    // model is available, so the smoke gate stays deterministic.
    let outcome = match Config::load_merged(workgraph_dir) {
        Ok(cfg) => review_inbound_ctx(&cfg, content_class, &content, &provenance, self_sensitivity),
        Err(_) => review_inbound(content_class, &content, &provenance, self_sensitivity),
    };
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

/// `wg review eval` — the live-model reviewer eval + recurring regression guard
/// (closes `docs/prod-audit/01` B5). Drives the production weak→strong model reviewer
/// over the seed + held-out corpus and reports the real catch-rate / false-positive
/// rate / escalation behavior. Fails LOUDLY (non-zero exit) when:
///   - `require_model` is set and no live model is reachable (never a silent pass on
///     the deterministic floor), or
///   - the held-out catch-rate regresses below `catch_threshold`, or
///   - the false-positive rate exceeds `fp_ceiling`.
pub fn run_eval(
    workgraph_dir: &Path,
    require_model: bool,
    held_out_only: bool,
    catch_threshold: f64,
    fp_ceiling: f64,
    json: bool,
) -> Result<()> {
    let config = Config::load_merged(workgraph_dir).unwrap_or_default();
    let live = model_review_available(&config);

    // Loud fail when a live model is required but unreachable — the B5 "never silently
    // pass on the deterministic floor" guarantee.
    if require_model && !live {
        anyhow::bail!(
            "LIVE MODEL UNREACHABLE — `wg review eval --require-model` was asked to validate the \
             production weak-tier reviewer, but no model is available (set WG_REVIEW_MODEL=1 and \
             configure a weak/strong tier with credentials, e.g. an OpenRouter route with \
             OPENROUTER_API_KEY). Refusing to report a pass on the deterministic floor."
        );
    }

    let mut items = eval::corpus();
    if held_out_only {
        items.retain(|i| i.bucket == Bucket::HeldOut);
    }

    let thresholds = Thresholds {
        held_out_catch_min: catch_threshold,
        fp_ceiling,
    };

    // Run against the live model when available; otherwise a clearly-labeled
    // deterministic-floor REFERENCE (only reachable without --require-model).
    let report = if live {
        let llm = AgencyReviewLlm { config: &config };
        eval::run_eval(&llm, &items)
    } else {
        eprintln!(
            "[review-eval] LIVE MODEL UNREACHABLE — running the DETERMINISTIC-FLOOR reference \
             only. This does NOT validate the production weak-tier LLM; pass --require-model to \
             make this a hard failure in a scheduled guard."
        );
        eval::run_eval_floor(&items)
    };

    let mode = if live {
        "live-model"
    } else {
        "deterministic-floor"
    };
    let regression = report.regression(&thresholds);

    if json {
        print_eval_json(&report, mode, &thresholds, regression.as_deref());
    } else {
        print_eval_text(&report, mode, &thresholds, regression.as_deref());
    }

    if let Some(reason) = regression {
        anyhow::bail!("REVIEWER EVAL REGRESSION ({mode}): {reason}");
    }
    Ok(())
}

fn print_eval_json(report: &EvalReport, mode: &str, t: &Thresholds, regression: Option<&str>) {
    let bucket_json = |b: &eval::BucketStats| {
        serde_json::json!({
            "attacks_total": b.attacks_total,
            "attacks_caught": b.attacks_caught,
            "attacks_caught_floor": b.attacks_caught_floor,
            "catch_rate": b.catch_rate(),
            "floor_catch_rate": b.floor_catch_rate(),
            "clean_total": b.clean_total,
            "clean_false_pos": b.clean_false_pos,
            "clean_false_pos_floor": b.clean_false_pos_floor,
            "false_pos_rate": b.false_pos_rate(),
            "floor_false_pos_rate": b.floor_false_pos_rate(),
        })
    };
    let v = serde_json::json!({
        "mode": mode,
        "thresholds": { "held_out_catch_min": t.held_out_catch_min, "fp_ceiling": t.fp_ceiling },
        "seed": bucket_json(&report.seed),
        "held_out": bucket_json(&report.held_out),
        "overall": bucket_json(&report.overall),
        "escalations": report.escalations,
        "source_counts": report.source_counts,
        "generalization_delta": report.generalization_delta,
        "missed_attacks": report.missed_attacks.iter().map(|r| serde_json::json!({
            "id": r.id, "kind": r.kind, "bucket": r.bucket.tag(),
            "verdict": r.model_verdict.tag(), "source": r.model_source.tag(),
        })).collect::<Vec<_>>(),
        "false_positives": report.false_positives.iter().map(|r| serde_json::json!({
            "id": r.id, "kind": r.kind, "bucket": r.bucket.tag(),
            "verdict": r.model_verdict.tag(), "floor_also_blocked": r.floor_blocked,
        })).collect::<Vec<_>>(),
        "regression": regression,
        "passed": regression.is_none(),
    });
    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
}

fn print_eval_text(report: &EvalReport, mode: &str, t: &Thresholds, regression: Option<&str>) {
    let pct = |x: f64| format!("{:.1}%", x * 100.0);
    println!("=== live-model reviewer eval ({mode}) ===");
    let row = |name: &str, b: &eval::BucketStats| {
        println!(
            "  {name:<9}  attacks {:>2}/{:<2} caught ({:>6})  [floor {:>6}]   clean FP {:>2}/{:<2} ({:>6})  [floor {:>6}]",
            b.attacks_caught,
            b.attacks_total,
            pct(b.catch_rate()),
            pct(b.floor_catch_rate()),
            b.clean_false_pos,
            b.clean_total,
            pct(b.false_pos_rate()),
            pct(b.floor_false_pos_rate()),
        );
    };
    row("seed", &report.seed);
    row("held-out", &report.held_out);
    row("overall", &report.overall);
    println!(
        "  generalization delta: {} held-out attack(s) the floor MISSED but the model CAUGHT",
        report.generalization_delta
    );
    println!(
        "  escalations (weak→strong): {} / {}",
        report.escalations,
        report.results.len()
    );
    print!("  verdict sources:");
    for (src, n) in &report.source_counts {
        print!(" {src}={n}");
    }
    println!();
    if !report.missed_attacks.is_empty() {
        println!("  MISSED attacks ({}):", report.missed_attacks.len());
        for r in &report.missed_attacks {
            println!(
                "    - {} [{}] {} → {}",
                r.id,
                r.bucket.tag(),
                r.kind,
                r.model_verdict.tag()
            );
        }
    }
    if !report.false_positives.is_empty() {
        println!("  FALSE POSITIVES ({}):", report.false_positives.len());
        for r in &report.false_positives {
            println!(
                "    - {} [{}] {} → {}{}",
                r.id,
                r.bucket.tag(),
                r.kind,
                r.model_verdict.tag(),
                if r.floor_blocked {
                    " (floor also blocked)"
                } else {
                    ""
                },
            );
        }
    }
    println!(
        "  thresholds: held-out catch ≥ {}, false-positive ≤ {}",
        pct(t.held_out_catch_min),
        pct(t.fp_ceiling),
    );
    match regression {
        None => println!("  RESULT: PASS"),
        Some(reason) => println!("  RESULT: FAIL — {reason}"),
    }
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

/// `wg review revoke` — the loud revoke leg (ADR-CS3 D4) + the cross-task-poison (B7/TC8)
/// descendant re-run. Beyond lowering the author's trust and naming the single recorded
/// consumer, this now walks the task graph from each recorded consumer and **re-runs the
/// whole downstream blast radius** — every transitive `--after` descendant that consumed the
/// poison — closing the F14 gap where revoke named only one hand-wired consumer. Pass
/// `--no-rerun-descendants` to only enumerate (not re-queue) the descendants.
pub fn run_revoke(
    workgraph_dir: &Path,
    cid: &str,
    rerun_descendants: bool,
    json: bool,
) -> Result<()> {
    let store = VerdictStore::open(workgraph_dir);
    let out = store.revoke(cid)?;

    // Cross-task poison (B7/TC8): expand the single recorded consumer to its full transitive
    // descendant set and re-queue the consumer + every descendant. `rerun_poison_descendants`
    // re-runs the named consumer itself and returns its downstream descendants.
    let mut blast_radius: Vec<String> = Vec::new();
    for consumer in &out.rerun_consumers {
        if !blast_radius.contains(consumer) {
            blast_radius.push(consumer.clone());
        }
        for d in
            crate::commands::rerun_poison_descendants(workgraph_dir, consumer, rerun_descendants)
        {
            if !blast_radius.contains(&d) {
                blast_radius.push(d);
            }
        }
    }

    if json {
        let mut v = serde_json::to_value(&out)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert(
                "blast_radius".to_string(),
                serde_json::to_value(&blast_radius)?,
            );
            obj.insert(
                "descendants_requeued".to_string(),
                serde_json::Value::Bool(rerun_descendants && !blast_radius.is_empty()),
            );
        }
        println!("{}", serde_json::to_string_pretty(&v)?);
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
        if blast_radius.is_empty() {
            println!("  re-run consumers: (none recorded)");
        } else {
            let verb = if rerun_descendants {
                "re-queued"
            } else {
                "to re-run"
            };
            println!(
                "  blast radius ({verb}): {} ({} task(s))",
                blast_radius.join(", "),
                blast_radius.len()
            );
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
