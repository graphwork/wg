use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};
use std::collections::HashMap;
use std::path::Path;
use worksgood::adaptive_agency::{
    ADAPTIVE_POLICY_VERSION, AdaptiveStore, AssignmentProvenanceV1, CandidateBindingV1,
    ConsumptionEffect, InfrastructureOutcome, PolicySnapshot, ReviewAttemptHandle,
    ReviewFinishInput, ReviewOutcomeV1, ReviewProduct, RouteSnapshot, SemanticOutcome,
    SourceBindingV1, SourceQualityEligibilityV1, TerminalDispositionV1, TerminalEpisodeInputV1,
    TerminalProvenanceV1, UsageV1, is_virtual_review_alias,
};
use worksgood::completion_manifest::{CompletionManifest, ContentDigest};
use worksgood::completion_review::{
    ReviewAttemptObserver, ReviewFailureClass, ReviewFinding, ReviewerKind, StoredReviewReceipt,
};
use worksgood::config::{Config, DispatchRole};
use worksgood::graph::{Status, Task};
use worksgood::identity::canonical_json;
use worksgood::parser::load_graph;
use worksgood::simple_land::ReviewVerdict;

pub(crate) struct LiveReviewObserver {
    adaptive: AdaptiveStore,
    binding: CandidateBindingV1,
    policy: PolicySnapshot,
    flip_reasoning: Option<String>,
    eval_reasoning: Option<String>,
    handles: HashMap<String, ReviewAttemptHandle>,
}

pub(crate) fn live_review_observer(dir: &Path, id: &str) -> Result<LiveReviewObserver> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?;
    let binding = candidate_binding(dir, task)?.context("adaptive candidate binding is missing")?;
    let config = Config::load_merged(dir)?;
    let flip_reasoning =
        worksgood::service::llm::resolve_agency_dispatch(&config, DispatchRole::Reviewer)
            .ok()
            .and_then(|dispatch| dispatch.reasoning.map(|value| value.to_string()));
    let eval_reasoning =
        worksgood::service::llm::resolve_agency_dispatch(&config, DispatchRole::Evaluator)
            .ok()
            .and_then(|dispatch| dispatch.reasoning.map(|value| value.to_string()));
    Ok(LiveReviewObserver {
        adaptive: AdaptiveStore::open(dir)?,
        binding,
        policy: PolicySnapshot {
            policy_id: "completion-review-v1".to_string(),
            policy_digest: digest_json(&(
                "completion-review-v1",
                config.agency.completion_review_strict,
                config.agency.gate_max_attempts.max(1),
            ))?,
            strict: config.agency.completion_review_strict,
            max_infrastructure_attempts: config.agency.gate_max_attempts.max(1),
        },
        flip_reasoning,
        eval_reasoning,
        handles: HashMap::new(),
    })
}

impl ReviewAttemptObserver for LiveReviewObserver {
    fn attempt_started(
        &mut self,
        reviewer_kind: ReviewerKind,
        exact_route: &str,
    ) -> std::result::Result<String, String> {
        let now = Utc::now();
        self.adaptive
            .review_sink()
            .settle_expired(&now.to_rfc3339())
            .map_err(|error| error.to_string())?;
        let existing = self
            .adaptive
            .reader()
            .review_attempts()
            .map_err(|error| error.to_string())?;
        let reasoning = match reviewer_kind {
            ReviewerKind::Flip => self.flip_reasoning.clone(),
            ReviewerKind::Eval => self.eval_reasoning.clone(),
        };
        let route_generation = existing
            .iter()
            .filter(|attempt| {
                attempt.binding == self.binding && attempt.reviewer_kind == reviewer_kind
            })
            .find(|attempt| {
                attempt.route.exact_route == exact_route && attempt.route.reasoning == reasoning
            })
            .map(|attempt| attempt.route.route_generation)
            .unwrap_or_else(|| {
                existing
                    .iter()
                    .filter(|attempt| {
                        attempt.binding == self.binding && attempt.reviewer_kind == reviewer_kind
                    })
                    .map(|attempt| attempt.route.route_generation)
                    .max()
                    .map_or(0, |generation| generation.saturating_add(1))
            });
        let route = RouteSnapshot::exact(
            exact_route,
            reasoning,
            "completion-review",
            env!("CARGO_PKG_VERSION"),
            route_generation,
        )
        .map_err(|error| error.to_string())?;
        let handle = self
            .adaptive
            .review_sink()
            .start(
                self.binding.clone(),
                reviewer_kind,
                ReviewProduct::Completion,
                self.policy.clone(),
                route,
                capability_manifest_digest().map_err(|error| error.to_string())?,
                now.to_rfc3339(),
                (now + Duration::seconds(900)).to_rfc3339(),
            )
            .map_err(|error| error.to_string())?;
        let token = handle.review_attempt_id.clone();
        self.handles.insert(token.clone(), handle);
        Ok(token)
    }

    fn attempt_finished(
        &mut self,
        observer_token: &str,
        stored: &StoredReviewReceipt,
    ) -> std::result::Result<(), String> {
        let handle = self
            .handles
            .remove(observer_token)
            .ok_or_else(|| "adaptive review start token is missing".to_string())?;
        self.adaptive
            .review_sink()
            .finish(
                &handle,
                ReviewFinishInput {
                    outcome: classify_outcome(
                        stored.receipt.verdict,
                        stored.receipt.failure_class,
                        &[],
                    ),
                    completed_at: Utc::now().to_rfc3339(),
                    duration_ms: stored.receipt.duration_ms.unwrap_or_default(),
                    response_digest: None,
                    findings_digest: Some(stored.receipt.findings_digest.to_string()),
                    inspected_output_digests: stored.receipt.inspected_output_digests.clone(),
                    usage: stored.receipt.usage.as_ref().map(UsageV1::from),
                    stop_reason: stored
                        .receipt
                        .failure_class
                        .map(|failure| format!("{failure:?}")),
                    provider_reported_route: stored.receipt.model_route.clone(),
                    receipt_digest: stored.receipt_object.content_digest.to_string(),
                },
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

pub(crate) fn prepare_candidate(dir: &Path, id: &str) -> Result<()> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?;
    if let Some(binding) = candidate_binding(dir, task)? {
        AdaptiveStore::open(dir)?
            .selection_sink()
            .select(binding, Utc::now().to_rfc3339())?;
    }
    Ok(())
}

pub(crate) fn sync_candidate_and_reviews(dir: &Path, id: &str) -> Result<()> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?;
    let Some(binding) = candidate_binding(dir, task)? else {
        return Ok(());
    };
    let adaptive = AdaptiveStore::open(dir)?;
    adaptive
        .selection_sink()
        .select(binding.clone(), Utc::now().to_rfc3339())?;
    let reader = adaptive.reader();
    let existing_attempts = reader.review_attempts()?;
    let already = existing_attempts
        .iter()
        .filter_map(|attempt| attempt.receipt_digest.clone())
        .collect::<std::collections::HashSet<_>>();
    let verified = worksgood::completion_review::verified_review_activities(dir, task);
    if verified.invalid_count > 0 {
        bail!(
            "adaptive candidate projection refused {} invalid review activities",
            verified.invalid_count
        );
    }
    let config = Config::load_merged(dir)?;
    for activity in verified.activities {
        if activity.binding.as_ref()
            != task
                .completion_candidate
                .as_ref()
                .and_then(|candidate| candidate.review_binding.as_ref())
            || activity.manifest_digest.to_string() != binding.manifest_digest
            || already.contains(&activity.activity_id)
        {
            continue;
        }
        let role = match activity.reviewer_kind {
            ReviewerKind::Flip => DispatchRole::Reviewer,
            ReviewerKind::Eval => DispatchRole::Evaluator,
        };
        let reasoning = worksgood::service::llm::resolve_agency_dispatch(&config, role)
            .ok()
            .and_then(|dispatch| dispatch.reasoning.map(|value| value.to_string()));
        let exact_route = activity.model_route.clone().unwrap_or_else(|| {
            format!("unknown:unknown:{:?}", activity.reviewer_kind).to_ascii_lowercase()
        });
        let route_generation = existing_attempts
            .iter()
            .filter(|attempt| {
                attempt.binding == binding && attempt.reviewer_kind == activity.reviewer_kind
            })
            .find(|attempt| {
                attempt.route.exact_route == exact_route && attempt.route.reasoning == reasoning
            })
            .map(|attempt| attempt.route.route_generation)
            .unwrap_or_else(|| {
                existing_attempts
                    .iter()
                    .filter(|attempt| {
                        attempt.binding == binding
                            && attempt.reviewer_kind == activity.reviewer_kind
                    })
                    .map(|attempt| attempt.route.route_generation)
                    .max()
                    .map_or(0, |generation| generation.saturating_add(1))
            });
        let route = RouteSnapshot::exact(
            exact_route,
            reasoning,
            "completion-review",
            env!("CARGO_PKG_VERSION"),
            route_generation,
        )?;
        let policy_digest = digest_json(&(
            "completion-review-v1",
            config.agency.completion_review_strict,
            config.agency.gate_max_attempts.max(1),
        ))?;
        let policy = PolicySnapshot {
            policy_id: "completion-review-v1".to_string(),
            policy_digest,
            strict: config.agency.completion_review_strict,
            max_infrastructure_attempts: config.agency.gate_max_attempts.max(1),
        };
        let completed_at = chrono::DateTime::parse_from_rfc3339(&activity.created_at)
            .map(|value| value.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let duration = activity.duration_ms.unwrap_or_default();
        let started_at = completed_at
            .checked_sub_signed(Duration::milliseconds(
                i64::try_from(duration).unwrap_or(i64::MAX),
            ))
            .unwrap_or(completed_at);
        let lease_expires = started_at + Duration::seconds(900);
        let handle = adaptive.review_sink().start(
            binding.clone(),
            activity.reviewer_kind,
            ReviewProduct::Completion,
            policy,
            route.clone(),
            capability_manifest_digest()?,
            started_at.to_rfc3339(),
            lease_expires.to_rfc3339(),
        )?;
        let outcome =
            classify_outcome(activity.verdict, activity.failure_class, &activity.findings);
        adaptive.review_sink().finish(
            &handle,
            ReviewFinishInput {
                outcome,
                completed_at: completed_at.to_rfc3339(),
                duration_ms: duration,
                response_digest: None,
                findings_digest: activity.findings_digest.as_ref().map(ToString::to_string),
                inspected_output_digests: binding.output_digests.clone(),
                usage: activity.usage.as_ref().map(UsageV1::from),
                stop_reason: activity.failure_class.map(|failure| format!("{failure:?}")),
                provider_reported_route: Some(route.exact_route),
                receipt_digest: activity.activity_id.clone(),
            },
        )?;
    }
    Ok(())
}

pub(crate) fn consume_current_reviews(dir: &Path, id: &str, controller_policy: &str) -> Result<()> {
    sync_candidate_and_reviews(dir, id)?;
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?;
    let Some(binding) = candidate_binding(dir, task)? else {
        return Ok(());
    };
    let adaptive = AdaptiveStore::open(dir)?;
    let attempts = adaptive.reader().review_attempts()?;
    let selected = task
        .completion_candidate
        .as_ref()
        .context("completion candidate is missing")?;
    for receipt in selected
        .flip_receipt
        .iter()
        .chain(selected.eval_receipt.iter())
    {
        let receipt_digest = receipt.content_digest.to_string();
        let Some(attempt) = attempts.iter().find(|attempt| {
            attempt.binding == binding
                && attempt.receipt_digest.as_deref() == Some(receipt_digest.as_str())
        }) else {
            bail!("adaptive review attempt is missing for receipt {receipt_digest}");
        };
        adaptive.completion_consumption_sink().consume(
            &attempt.review_attempt_id,
            &binding,
            &receipt_digest,
            controller_policy,
            binding.source.source_fence,
            ConsumptionEffect::AcceptedEvidence,
            &Utc::now().to_rfc3339(),
        )?;
    }
    Ok(())
}

pub(crate) fn project_terminal_episode(dir: &Path, id: &str) -> Result<Option<String>> {
    // Reconcile review evidence first.  It remains observation-only even if
    // this projection fails; terminal lifecycle state is never changed here.
    let _ = sync_candidate_and_reviews(dir, id);
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?;
    let disposition = match task.status {
        Status::Done => TerminalDispositionV1::Done,
        Status::Failed => TerminalDispositionV1::Failed,
        Status::Abandoned => TerminalDispositionV1::Abandoned,
        _ => return Ok(None),
    };
    let terminal = task
        .lifecycle
        .audit
        .iter()
        .rev()
        .find(|event| {
            event.generation == task.lifecycle.generation
                && matches!(
                    event.new_state,
                    Status::Done | Status::Failed | Status::Abandoned
                )
        })
        .context("terminal task has no terminal lifecycle event")?;
    let graph_identity = worksgood::worker_control::load_or_create_graph_identity(dir)?;
    let adaptive = AdaptiveStore::open(dir)?;
    let candidate = candidate_binding(dir, task)?;
    let source_attempt_id = terminal.attempt_id.clone().or_else(|| {
        task.lifecycle
            .current_attempt
            .as_ref()
            .map(|attempt| attempt.id.clone())
    });
    let source_fence = source_attempt_id.as_ref().map(|_| terminal.fence);
    let assignment_provenance = candidate
        .as_ref()
        .map(|binding| {
            AssignmentProvenanceV1::BoundReceipt(binding.source.assignment_receipt_id.clone())
        })
        .unwrap_or_else(|| {
            if source_attempt_id.is_some() {
                AssignmentProvenanceV1::UnknownLegacy(digest_bytes(id.as_bytes()))
            } else {
                AssignmentProvenanceV1::NoAttempt
            }
        });
    let operator_accepted = terminal.reason_code == "operator_acceptance";
    let terminal_provenance = if operator_accepted {
        TerminalProvenanceV1::OperatorAcceptance(
            task.completion_receipt
                .clone()
                .unwrap_or_else(|| terminal.event_id.clone()),
        )
    } else if task.status == Status::Done {
        TerminalProvenanceV1::CompletionReceipt(
            task.completion_receipt
                .clone()
                .unwrap_or_else(|| terminal.event_id.clone()),
        )
    } else {
        TerminalProvenanceV1::FailureEvent(terminal.event_id.clone())
    };
    let eligibility = if task.status == Status::Done && !operator_accepted {
        SourceQualityEligibilityV1::Eligible
    } else if operator_accepted {
        SourceQualityEligibilityV1::Ineligible {
            reason: "operator-accepted completion is not ordinary publication truth".to_string(),
        }
    } else {
        SourceQualityEligibilityV1::Ineligible {
            reason: task.failure_reason.clone().unwrap_or_else(|| {
                format!(
                    "terminal disposition {:?} is not source-proven",
                    task.status
                )
            }),
        }
    };
    let projector = adaptive.learning_projector();
    let seal = projector.seal_trajectory(
        &graph_identity,
        id,
        task.lifecycle.generation,
        &terminal.event_id,
        &terminal.committed_at,
    )?;
    let episode = projector.project(
        TerminalEpisodeInputV1 {
            graph_identity,
            task_id: id.to_string(),
            generation: task.lifecycle.generation,
            terminal_event_id: terminal.event_id.clone(),
            terminal_disposition: disposition,
            source_attempt_id,
            source_fence,
            assignment_provenance,
            terminal_provenance,
            terminal_candidate_binding: candidate,
            source_quality_eligibility: eligibility,
            created_at: terminal.committed_at.clone(),
        },
        &seal,
    )?;
    let _ = projector.performance_projection()?;
    Ok(Some(episode.episode_id))
}

pub fn run_reviews_list(
    dir: &Path,
    task: Option<&str>,
    candidate: &str,
    kind: Option<&str>,
    json: bool,
) -> Result<()> {
    let mut attempts = AdaptiveStore::open_existing(dir)
        .map(|adaptive| adaptive.reader().review_attempts())
        .transpose()?
        .unwrap_or_default();
    if let Some(task) = task {
        attempts.retain(|attempt| attempt.binding.source.task_id == task);
    }
    match candidate {
        "all" => {}
        "current" => attempts.retain(|attempt| attempt.current_candidate),
        other => bail!("--candidate must be current or all, got '{other}'"),
    }
    if let Some(kind) = kind {
        let expected = match kind {
            "flip" => ReviewerKind::Flip,
            "eval" | "evaluate" => ReviewerKind::Eval,
            other => bail!("--kind must be flip or eval, got '{other}'"),
        };
        attempts.retain(|attempt| attempt.reviewer_kind == expected);
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&attempts)?);
        return Ok(());
    }
    println!(
        "VIRTUAL REVIEW — not a graph task; no status, edge, worker slot, or lifecycle authority"
    );
    if attempts.is_empty() {
        println!("No adaptive review attempts found");
    }
    for attempt in attempts {
        let outcome = attempt
            .outcome
            .as_ref()
            .map(|outcome| format!("{outcome:?}"))
            .unwrap_or_else(|| "running".to_string());
        let candidate = if attempt.current_candidate {
            "current"
        } else {
            "superseded"
        };
        let cost = attempt
            .usage
            .as_ref()
            .and_then(|usage| usage.provider_cost)
            .map(|cost| format!("${cost:.6}"))
            .unwrap_or_else(|| "unknown".to_string());
        println!(
            "[R] {}  {}  {}  cost={} route={} reasoning={}",
            attempt.alias,
            outcome,
            candidate,
            cost,
            attempt.route.exact_route,
            attempt.route.reasoning.as_deref().unwrap_or("unset")
        );
    }
    Ok(())
}

pub fn run_reviews_show(dir: &Path, target: &str, json: bool) -> Result<()> {
    let adaptive = AdaptiveStore::open_existing(dir)
        .with_context(|| format!("adaptive review '{target}' not found"))?;
    let attempts = adaptive.reader().review_attempts()?;
    let attempt = attempts
        .into_iter()
        .find(|attempt| {
            attempt.alias == target
                || attempt.review_attempt_id == target
                || attempt.review_run_id == target
        })
        .with_context(|| format!("adaptive review '{target}' not found"))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&attempt)?);
    } else {
        println!(
            "VIRTUAL REVIEW — not a graph task; no status, edge, worker slot, or lifecycle authority"
        );
        println!("Alias: {}", attempt.alias);
        println!(
            "Attempt: {} ordinal={}",
            attempt.review_attempt_id, attempt.ordinal
        );
        println!("Run: {}", attempt.review_run_id);
        println!(
            "Source: {} generation={} attempt={} fence={} candidate={}",
            attempt.binding.source.task_id,
            attempt.binding.source.generation,
            attempt.binding.source.source_attempt_id,
            attempt.binding.source.source_fence,
            attempt.binding.candidate_sequence
        );
        println!(
            "Route: {} reasoning={} adapter={}@{} generation={}",
            attempt.route.exact_route,
            attempt.route.reasoning.as_deref().unwrap_or("unset"),
            attempt.route.adapter,
            attempt.route.adapter_version,
            attempt.route.route_generation
        );
        println!("Outcome: {:?}", attempt.outcome);
        println!(
            "Candidate: {}",
            if attempt.current_candidate {
                "current"
            } else {
                "superseded"
            }
        );
        println!("Consumed: {}", attempt.consumed);
        if let Some(usage) = attempt.usage {
            println!(
                "Provider usage: in={} out={} cache-read={} cache-write={} cost={}",
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_tokens,
                usage.cache_write_tokens,
                usage
                    .provider_cost
                    .map(|cost| format!("${cost:.6}"))
                    .unwrap_or_else(|| "unknown".to_string())
            );
        } else {
            println!("Provider usage: unavailable; cost=unknown");
        }
        println!(
            "Findings digest: {}",
            attempt.findings_digest.as_deref().unwrap_or("none")
        );
        let findings = attempt
            .findings_digest
            .as_deref()
            .map(|digest| load_bounded_findings(dir, digest))
            .unwrap_or_default();
        if findings.is_empty() {
            println!("Findings: none or unavailable");
        } else {
            println!("Findings (bounded immutable projection):");
            for finding in findings.into_iter().take(32) {
                let message = finding.message.chars().take(400).collect::<String>();
                println!("  - {}: {}", finding.code, message);
            }
        }
        println!(
            "Receipt: {}",
            attempt.receipt_digest.as_deref().unwrap_or("pending")
        );
    }
    Ok(())
}

pub fn run_learning_show(dir: &Path, target: &str, json: bool) -> Result<()> {
    let mut episodes = AdaptiveStore::open_existing(dir)
        .map(|adaptive| adaptive.reader().episodes())
        .transpose()?
        .unwrap_or_default();
    episodes.retain(|episode| episode.episode_id == target || episode.task_id == target);
    if episodes.is_empty() && !target.starts_with("b3:") {
        let _ = project_terminal_episode(dir, target);
        episodes = AdaptiveStore::open_existing(dir)
            .map(|adaptive| adaptive.reader().episodes())
            .transpose()?
            .unwrap_or_default();
        episodes.retain(|episode| episode.episode_id == target || episode.task_id == target);
    }
    let episode = episodes
        .into_iter()
        .max_by_key(|episode| episode.generation)
        .with_context(|| format!("learning episode for '{target}' not found"))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&episode)?);
    } else {
        println!("Learning episode: {}", episode.episode_id);
        println!(
            "Terminal generation observation: task={} generation={} disposition={:?}",
            episode.task_id, episode.generation, episode.terminal_disposition
        );
        println!(
            "Trajectory: candidates={} semantic-pass={} semantic-reject={} infrastructure-attempts={}",
            episode.semantic_trajectory.candidate_count,
            episode.semantic_trajectory.passes,
            episode.semantic_trajectory.rejects,
            episode.infrastructure_summary.attempts
        );
        println!("Source quality: {:?}", episode.source_quality_eligibility);
        let adaptive = AdaptiveStore::open_existing(dir).expect("episode requires adaptive store");
        let rewards = adaptive.reader().active_assignment_rewards()?;
        if let Some(reward) = rewards
            .iter()
            .find(|reward| reward.episode_id == episode.episode_id)
        {
            println!(
                "Delayed assignment reward: {:.3} receipt={} outcome={}",
                reward.reward, reward.assignment_receipt_id, reward.effective_outcome_id
            );
            let manifests = adaptive.reader().evolution_inputs()?;
            println!(
                "Evolver input: projected={} manifest={}",
                manifests
                    .iter()
                    .any(|manifest| manifest.assignment_reward_ids.contains(&reward.reward_id)),
                manifests
                    .iter()
                    .find(|manifest| manifest.assignment_reward_ids.contains(&reward.reward_id))
                    .map(|manifest| manifest.manifest_id.as_str())
                    .unwrap_or("pending")
            );
        } else {
            println!("Delayed assignment reward: pending/unscored independent outcome");
            println!("Evolver input: pending");
        }
        println!(
            "Policy: {} (one episode per terminal generation)",
            ADAPTIVE_POLICY_VERSION
        );
    }
    Ok(())
}

pub fn run_learning_backlog(dir: &Path, json: bool) -> Result<()> {
    let backlog = AdaptiveStore::open_existing(dir)
        .map(|adaptive| adaptive.reader().backlog(&Utc::now().to_rfc3339()))
        .transpose()?
        .unwrap_or_default();
    if json {
        println!("{}", serde_json::to_string_pretty(&backlog)?);
    } else {
        println!("Adaptive learning backlog:");
        println!(
            "  expired/unsettled review attempts: {}",
            backlog.expired_unsettled_attempts
        );
        println!("  invalid canonical objects: {}", backlog.invalid_objects);
        println!("  projector failures never change terminal source lifecycle");
    }
    Ok(())
}

pub fn show_virtual_if_present(dir: &Path, target: &str, json: bool) -> Result<bool> {
    if !is_virtual_review_alias(target) {
        return Ok(false);
    }
    run_reviews_show(dir, target, json)?;
    Ok(true)
}

fn load_bounded_findings(dir: &Path, digest: &str) -> Vec<ReviewFinding> {
    let Ok(digest) = ContentDigest::parse(digest.to_string()) else {
        return Vec::new();
    };
    let Ok(store) = super::completion_submit::store(dir) else {
        return Vec::new();
    };
    let Some(object_name) = digest.as_str().strip_prefix("b3:") else {
        return Vec::new();
    };
    let path = store.root().join("objects").join(object_name);
    let Ok(metadata) = std::fs::metadata(&path) else {
        return Vec::new();
    };
    if metadata.len() > worksgood::completion_task::MAX_COMPLETION_METADATA_BYTES {
        return Vec::new();
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    if ContentDigest::of_bytes(&bytes) != digest {
        return Vec::new();
    }
    serde_json::from_slice::<Vec<ReviewFinding>>(&bytes).unwrap_or_default()
}

pub(crate) fn composition_snapshot(
    dir: &Path,
    agent: &worksgood::agency::Agent,
) -> Result<worksgood::adaptive_agency::CompositionSnapshotV1> {
    let role =
        worksgood::agency::find_role_by_prefix(&dir.join("agency/cache/roles"), &agent.role_id)?;
    let composition_digest = digest_json(&(
        &agent.id,
        &agent.role_id,
        &agent.tradeoff_id,
        &role.component_ids,
        &role.outcome_id,
    ))?;
    Ok(worksgood::adaptive_agency::CompositionSnapshotV1 {
        agent_id: agent.id.clone(),
        role_id: agent.role_id.clone(),
        tradeoff_id: agent.tradeoff_id.clone(),
        component_ids: role.component_ids,
        outcome_id: role.outcome_id,
        composition_digest,
    })
}

/// Persist the attempt-bound assignment evidence before reservation. The
/// caller includes the returned receipt ID in the lifecycle reservation.
/// This lane writes only adaptive evidence and never creates an edge/task.
pub(crate) fn prepare_next_attempt_assignment(
    dir: &Path,
    task: &Task,
) -> Result<worksgood::adaptive_agency::AssignmentReceiptV1> {
    use worksgood::adaptive_agency::{
        AssignmentDecisionV1, AssignmentReceiptInputV1, AssignmentSelectorSnapshotV1,
    };
    let graph_identity = worksgood::worker_control::load_or_create_graph_identity(dir)?;
    let attempt_id = format!(
        "attempt-{}-{}",
        task.lifecycle.generation,
        task.lifecycle.attempt_sequence.saturating_add(1)
    );
    let attempt_fence = task.lifecycle.fence.saturating_add(1);
    let admission_snapshot_digest = digest_json(&(
        &graph_identity,
        &task.id,
        task.lifecycle.generation,
        task.lifecycle.revision,
        task.lifecycle.fence,
        task.lifecycle.attempt_sequence,
        task.status,
        &task.agent,
    ))?;
    let adaptive = AdaptiveStore::open(dir)?;
    let intent = adaptive.assignment_intent(&task.id)?;
    let (decision, selector, candidate_scores, selected_composition, failure) =
        if let Some(intent) = intent {
            let intent_matches = match (&intent.selected_composition, &task.agent) {
                (Some(composition), Some(agent)) => composition.agent_id == *agent,
                (None, None) => true,
                _ => false,
            };
            if intent_matches {
                (
                    intent.decision,
                    intent.selector,
                    intent.candidate_scores,
                    intent.selected_composition,
                    intent.failure,
                )
            } else {
                (
                    AssignmentDecisionV1::Uncomposed {
                        reason: "assignment intent no longer matches the admitted task".to_string(),
                    },
                    AssignmentSelectorSnapshotV1::direct(),
                    std::collections::BTreeMap::new(),
                    None,
                    None,
                )
            }
        } else if let Some(agent_id) = task.agent.as_deref() {
            let agent = worksgood::agency::find_agent_by_prefix(
                &dir.join("agency/cache/agents"),
                agent_id,
            )?;
            let composition = composition_snapshot(dir, &agent)?;
            (
                AssignmentDecisionV1::Explicit {
                    composition_digest: composition.composition_digest.clone(),
                },
                AssignmentSelectorSnapshotV1 {
                    kind: "explicit-task-intent".to_string(),
                    principal: worksgood::current_user(),
                    policy_digest: "explicit-assignment-v1".to_string(),
                    exact_route: None,
                },
                std::collections::BTreeMap::new(),
                Some(composition),
                None,
            )
        } else {
            (
                AssignmentDecisionV1::Uncomposed {
                    reason: "direct dispatch without agency composition".to_string(),
                },
                AssignmentSelectorSnapshotV1::direct(),
                std::collections::BTreeMap::new(),
                None,
                None,
            )
        };
    let history_class = crate::commands::service::assignment::history_class_for_assignment(task);
    let now = Utc::now().to_rfc3339();
    Ok(
        adaptive.record_attempt_assignment(AssignmentReceiptInputV1 {
            graph_identity,
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            attempt_id,
            attempt_fence,
            admission_snapshot_digest,
            context_partition: history_class.label().to_string(),
            decision,
            selector,
            candidate_scores,
            selected_composition,
            started_at: now.clone(),
            completed_at: now,
            failure,
        })?,
    )
}

fn candidate_binding(dir: &Path, task: &Task) -> Result<Option<CandidateBindingV1>> {
    let Some(candidate) = task.completion_candidate.as_ref() else {
        return Ok(None);
    };
    let Some(review_binding) = candidate.review_binding.as_ref() else {
        return Ok(None);
    };
    let store = super::completion_submit::store(dir)?;
    let manifest: CompletionManifest = store.read_manifest(
        &candidate.manifest,
        worksgood::completion_task::MAX_COMPLETION_METADATA_BYTES,
    )?;
    let graph_identity = worksgood::worker_control::load_or_create_graph_identity(dir)?;
    let attempt_id = review_binding
        .attempt_id
        .clone()
        .unwrap_or_else(|| "no-attempt".to_string());
    let adaptive = AdaptiveStore::open(dir)?;
    let assignment = adaptive
        .reader()
        .assignment_for_attempt(
            &graph_identity,
            &task.id,
            review_binding.generation,
            &attempt_id,
            review_binding.attempt_fence,
        )?
        .map(Ok)
        .unwrap_or_else(|| {
            adaptive.ensure_uncomposed_assignment(
                &graph_identity,
                &task.id,
                review_binding.generation,
                &attempt_id,
                review_binding.attempt_fence,
                "attempt predates adaptive assignment admission receipt",
            )
        })?;
    let output_digests = manifest
        .outputs
        .iter()
        .map(digest_json)
        .collect::<Result<Vec<_>>>()?;
    Ok(Some(
        CandidateBindingV1 {
            source: SourceBindingV1 {
                graph_identity,
                task_id: task.id.clone(),
                generation: review_binding.generation,
                source_attempt_id: attempt_id,
                source_fence: review_binding.attempt_fence,
                assignment_receipt_id: assignment.receipt_id,
            },
            candidate_sequence: review_binding.candidate_sequence,
            manifest_digest: candidate.manifest.content_digest.to_string(),
            requirements_digest: candidate.requirements.content_digest.to_string(),
            source_revision: manifest.source_revision,
            dependency_revision_digest: digest_json(&candidate.dependency_outputs)?,
            output_digests,
            validation_evidence_digest: digest_json(&manifest.validation_evidence)?,
        }
        .normalized(),
    ))
}

fn classify_outcome(
    verdict: ReviewVerdict,
    failure: Option<ReviewFailureClass>,
    findings: &[worksgood::completion_review::ReviewFinding],
) -> ReviewOutcomeV1 {
    match verdict {
        ReviewVerdict::Pass => ReviewOutcomeV1::Semantic(SemanticOutcome::Pass),
        ReviewVerdict::Reject => ReviewOutcomeV1::Semantic(SemanticOutcome::Reject),
        ReviewVerdict::Absent => ReviewOutcomeV1::Semantic(SemanticOutcome::Inconclusive),
        ReviewVerdict::IncompleteEvidence => {
            ReviewOutcomeV1::Infrastructure(InfrastructureOutcome::EvidenceUnavailable)
        }
        ReviewVerdict::Unavailable => {
            let codes = findings
                .iter()
                .map(|finding| finding.code.to_ascii_lowercase())
                .collect::<Vec<_>>();
            let outcome = if codes.iter().any(|code| code.contains("timeout")) {
                InfrastructureOutcome::Timeout
            } else if codes
                .iter()
                .any(|code| code.contains("invalid") || code.contains("malformed"))
            {
                InfrastructureOutcome::MalformedOutput
            } else if failure == Some(ReviewFailureClass::IncompleteEvidence) {
                InfrastructureOutcome::EvidenceUnavailable
            } else {
                InfrastructureOutcome::AdapterUnavailable
            };
            ReviewOutcomeV1::Infrastructure(outcome)
        }
    }
}

fn capability_manifest_digest() -> Result<String> {
    digest_json(&serde_json::json!({
        "capabilities": ["observe-immutable-review-bundle"],
        "tools": [],
        "graph_write": false,
        "lifecycle": false,
        "publication": false,
        "source_worktree": false,
    }))
}

fn digest_json<T: serde::Serialize>(value: &T) -> Result<String> {
    let bytes = canonical_json(&serde_json::to_value(value)?);
    Ok(digest_bytes(&bytes))
}

fn digest_bytes(bytes: &[u8]) -> String {
    format!("b3:{}", blake3::hash(bytes).to_hex())
}
