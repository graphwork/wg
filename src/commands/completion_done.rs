use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::Serialize;
use std::path::Path;
use std::process::Command;
use worksgood::completion_manifest::{OutputRef, ReviewResolver};
use worksgood::completion_task::{
    load_exact_review_pair, load_review_evidence, load_submission_bytes,
};
use worksgood::graph::{
    CompletionContract, CompletionDisposition, LogEntry, Status, TokenUsage, parse_token_usage,
    parse_wg_tokens,
};
use worksgood::identity::canonical_json;
use worksgood::lifecycle::{
    ActorKind, FenceExpectation, LifecycleActor, TransitionKind, TransitionRequest,
    apply_transition,
};
use worksgood::parser::{load_graph, modify_graph};
use worksgood::service::registry::AgentRegistry;

use super::completion_submit::{collect_dependency_outputs, store};

#[derive(Clone, Default)]
pub(crate) struct SourceAccounting {
    pub(crate) usage: Option<TokenUsage>,
    pub(crate) executor: Option<String>,
    pub(crate) model: Option<String>,
}

pub(crate) fn source_accounting(dir: &Path, task: &worksgood::graph::Task) -> SourceAccounting {
    let persisted = SourceAccounting {
        usage: task.token_usage.clone(),
        executor: task.actual_executor.clone(),
        model: task.actual_model.clone(),
    };
    let Some(agent_id) = task.assigned.as_deref() else {
        return persisted;
    };
    let Ok(registry) = AgentRegistry::load(dir) else {
        return persisted;
    };
    let Some(agent) = registry
        .get_agent(agent_id)
        .filter(|agent| agent.task_id == task.id)
    else {
        return persisted;
    };
    let output = Path::new(&agent.output_file);
    let output = if output.is_absolute() {
        output.to_path_buf()
    } else {
        dir.parent().unwrap_or(dir).join(output)
    };
    SourceAccounting {
        usage: parse_token_usage(&output)
            .or_else(|| parse_wg_tokens(&output))
            .or(persisted.usage),
        executor: Some(agent.executor.clone()).or(persisted.executor),
        model: agent.model.clone().or(persisted.model),
    }
}

#[derive(Serialize)]
struct OperatorAcceptanceReceipt {
    receipt_version: u32,
    task_id: String,
    generation_before_accept: u64,
    status_before_accept: String,
    reason: String,
    operator: String,
    git_head: Option<String>,
    accepted_at: String,
}

#[derive(Serialize)]
struct CompletionReceipt {
    receipt_version: u32,
    task_id: String,
    generation: u64,
    manifest_digest: String,
    requirements_digest: String,
    flip_receipt_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    eval_receipt_digest: Option<String>,
    review_policy: String,
    contract: String,
    publication: String,
    completed_at: String,
}

/// Derive Done from exact immutable review plus current publication truth.
pub fn run(dir: &Path, id: &str, integration_ref: &str) -> Result<()> {
    let graph_path = dir.join("graph.jsonl");
    let graph = load_graph(&graph_path)?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?;
    require_completion_actor(task, id)?;
    let accounting = source_accounting(dir, task);
    let completion_store = store(dir)?;
    let (submission, manifest, requirements, summary) =
        load_submission_bytes(&completion_store, task)?;
    let dependencies = collect_dependency_outputs(&completion_store, &graph, task)?;
    let selected_dependencies = &task
        .completion_candidate
        .as_ref()
        .context("missing completion candidate")?
        .dependency_outputs;
    if &dependencies != selected_dependencies {
        bail!("dependency outputs changed after review");
    }
    let project_root = dir
        .parent()
        .context("workgraph directory has no project root")?;
    let resolver = ReviewResolver::new(&completion_store);
    let resolved = if task.completion_contract == CompletionContract::Land {
        resolver.repository(project_root).resolve_submission(
            &submission.manifest_ref,
            &requirements,
            &summary,
            &dependencies,
        )
    } else {
        resolver.resolve_submission(
            &submission.manifest_ref,
            &requirements,
            &summary,
            &dependencies,
        )
    }
    .map_err(|error| anyhow::anyhow!("completion evidence no longer resolves: {error}"))?;
    let config = worksgood::config::Config::load_merged(dir)?;
    let review_policy = if config.agency.completion_review_strict {
        load_exact_review_pair(&completion_store, &submission, &manifest, &resolved)?;
        "strict"
    } else {
        let evidence = load_review_evidence(&completion_store, &submission, &manifest, &resolved)?;
        if evidence.flip.verdict != worksgood::simple_land::ReviewVerdict::Pass
            || evidence.eval.as_ref().is_some_and(|receipt| {
                receipt.verdict != worksgood::simple_land::ReviewVerdict::Pass
            })
        {
            eprintln!(
                "Advisory model review did not pass; deterministic completion continues. Inspect `wg show {id}` for findings."
            );
        }
        "advisory"
    };
    let publication = verify_publication(
        project_root,
        task.completion_contract,
        &manifest.outputs,
        integration_ref,
    )?;
    let manifest_digest = manifest.digest().map_err(anyhow::Error::msg)?;
    if task.status == Status::Done {
        println!(
            "Done '{}': exact reviewed manifest {} and publication remain verified",
            id, manifest_digest
        );
        return Ok(());
    }
    let candidate = task
        .completion_candidate
        .as_ref()
        .context("missing completion candidate")?;
    let flip_digest = candidate
        .flip_receipt
        .as_ref()
        .context("missing FLIP receipt")?
        .content_digest
        .to_string();
    let eval_digest = candidate
        .eval_receipt
        .as_ref()
        .map(|receipt| receipt.content_digest.to_string());
    let completed_at = Utc::now().to_rfc3339();
    let receipt = CompletionReceipt {
        receipt_version: 1,
        task_id: id.to_string(),
        generation: task.lifecycle.generation,
        manifest_digest: manifest_digest.to_string(),
        requirements_digest: manifest.requirements_digest.to_string(),
        flip_receipt_digest: flip_digest,
        eval_receipt_digest: eval_digest,
        review_policy: review_policy.to_string(),
        contract: task.completion_contract.to_string(),
        publication,
        completed_at: completed_at.clone(),
    };
    let receipt_bytes = canonical_json(&serde_json::to_value(receipt)?);
    let receipt_ref = completion_store.put_bytes(
        &receipt_bytes,
        "application/vnd.worksgood.completion-receipt+json",
    )?;
    commit_done(
        &graph_path,
        id,
        task.lifecycle.generation,
        &manifest_digest,
        task.completion_contract,
        &receipt_ref.content_digest.to_string(),
        &completed_at,
        &accounting,
    )?;
    println!(
        "Done '{}': {} review evidence bound manifest {} and publication is verified",
        id, review_policy, manifest_digest
    );
    Ok(())
}

/// Explicit human recovery for a task whose ordinary completion controller is
/// unavailable or internally inconsistent. This is deliberately operator-only,
/// reasoned, immutable, and lifecycle-attributed: it is an escape from a broken
/// controller, never a silent mutation or a worker-accessible bypass.
pub fn operator_accept(dir: &Path, id: &str, reason: &str) -> Result<()> {
    if std::env::var_os("WG_AGENT_ID").is_some() {
        bail!("operator acceptance is refused inside a worker process");
    }
    let reason = reason.trim();
    if reason.is_empty() {
        bail!("--operator-accept requires a non-empty --reason");
    }

    let graph_path = dir.join("graph.jsonl");
    let graph = load_graph(&graph_path)?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?;
    if task.status == Status::Done {
        println!("Task '{id}' is already Done");
        return Ok(());
    }
    if task.status == Status::Abandoned {
        bail!("abandoned task '{id}' cannot be operator-accepted");
    }

    let operator = worksgood::current_user();
    let accepted_at = Utc::now().to_rfc3339();
    let git_head = dir.parent().and_then(|root| {
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    });
    let receipt = OperatorAcceptanceReceipt {
        receipt_version: 1,
        task_id: id.to_string(),
        generation_before_accept: task.lifecycle.generation,
        status_before_accept: format!("{:?}", task.status),
        reason: reason.to_string(),
        operator: operator.clone(),
        git_head,
        accepted_at: accepted_at.clone(),
    };
    let receipt_bytes = canonical_json(&serde_json::to_value(receipt)?);
    let receipt_ref = store(dir)?.put_bytes(
        &receipt_bytes,
        "application/vnd.worksgood.operator-acceptance+json",
    )?;
    let receipt_digest = receipt_ref.content_digest.to_string();

    let mut refusal = None;
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            refusal = Some("task disappeared during operator acceptance".to_string());
            return false;
        };
        if task.status == Status::Done {
            return false;
        }
        if task.status == Status::Abandoned {
            refusal = Some("abandoned task cannot be operator-accepted".to_string());
            return false;
        }

        let actor = LifecycleActor::operator(operator.clone());
        if matches!(task.status, Status::PendingEval | Status::PendingValidation) {
            let request = TransitionRequest::new(
                TransitionKind::AcceptanceSatisfied {
                    acceptance_ref: receipt_digest.clone(),
                },
                actor.clone(),
                "operator_acceptance",
                format!(
                    "operator-accept:{id}:{}:{receipt_digest}",
                    task.lifecycle.generation
                ),
            )
            .with_evidence(receipt_digest.clone());
            if let Err(error) = apply_transition(task, request) {
                refusal = Some(error.to_string());
                return false;
            }
        } else {
            if task.status != Status::InProgress || task.lifecycle.current_attempt.is_none() {
                if task.status != Status::Open {
                    let request = TransitionRequest::new(
                        TransitionKind::GenerationCreated,
                        actor.clone(),
                        "operator_recovery_generation",
                        format!(
                            "operator-accept-generation:{id}:{}:{receipt_digest}",
                            task.lifecycle.generation
                        ),
                    )
                    .with_evidence(receipt_digest.clone());
                    if let Err(error) = apply_transition(task, request) {
                        refusal = Some(error.to_string());
                        return false;
                    }
                }
                let request = TransitionRequest::new(
                    TransitionKind::AttemptReserved {
                        owner_id: Some(operator.clone()),
                    },
                    actor.clone(),
                    "operator_recovery_attempt",
                    format!(
                        "operator-accept-reserve:{id}:{}:{receipt_digest}",
                        task.lifecycle.generation
                    ),
                )
                .with_evidence(receipt_digest.clone());
                if let Err(error) = apply_transition(task, request) {
                    refusal = Some(error.to_string());
                    return false;
                }
            }
            let request = TransitionRequest::new(
                TransitionKind::AttemptSucceeded {
                    acceptance_ref: Some(receipt_digest.clone()),
                    manual_review: false,
                },
                actor,
                "operator_acceptance",
                format!(
                    "operator-accept-succeed:{id}:{}:{receipt_digest}",
                    task.lifecycle.generation
                ),
            )
            .expecting(FenceExpectation::current(task))
            .with_evidence(receipt_digest.clone());
            if let Err(error) = apply_transition(task, request) {
                refusal = Some(error.to_string());
                return false;
            }
        }

        task.completion_disposition = Some(match task.completion_contract {
            CompletionContract::Land => CompletionDisposition::Landed,
            CompletionContract::Report => CompletionDisposition::Reported,
            CompletionContract::Explore => CompletionDisposition::Explored,
            CompletionContract::Deliver => CompletionDisposition::Delivered,
        });
        task.completion_receipt = Some(receipt_digest.clone());
        task.completed_at = Some(accepted_at.clone());
        task.last_interaction_at = Some(accepted_at.clone());
        task.assigned = None;
        task.failure_reason = None;
        task.failure_class = None;
        task.failure_signal = None;
        task.log.push(LogEntry {
            timestamp: accepted_at.clone(),
            actor: Some(format!("operator:{operator}")),
            user: Some(operator.clone()),
            message: format!(
                "Operator accepted task with immutable receipt {receipt_digest}: {reason}"
            ),
        });
        true
    })?;
    if let Some(refusal) = refusal {
        bail!(refusal);
    }

    if let Ok(mut registry) = AgentRegistry::load_locked(dir) {
        if let Some(agent) = registry.get_agent_by_task_mut(id) {
            agent.status = worksgood::service::registry::AgentStatus::Done;
            agent.completed_at = Some(accepted_at.clone());
        }
        let _ = registry.save_ref();
    }
    let config = worksgood::config::Config::load_or_default(dir);
    let _ = worksgood::provenance::record(
        dir,
        "operator-accept",
        Some(id),
        Some(&operator),
        serde_json::json!({
            "reason": reason,
            "receipt": receipt_digest,
        }),
        config.log.rotation_threshold,
    );
    super::notify_graph_changed(dir);
    println!("Operator accepted '{id}' with immutable receipt {receipt_digest}: {reason}");
    Ok(())
}

fn require_completion_actor(task: &worksgood::graph::Task, id: &str) -> Result<()> {
    if let Ok(bound_task) = std::env::var("WG_TASK_ID")
        && !bound_task.is_empty()
        && bound_task != id
    {
        bail!("worker is bound to task '{bound_task}', not '{id}'");
    }
    if task.status == Status::Abandoned {
        bail!("abandoned task '{id}' cannot become Done");
    }
    if task.status == Status::Open && task.assigned.is_none() {
        bail!("unowned open task '{id}' cannot become Done");
    }
    if let Ok(agent) = std::env::var("WG_AGENT_ID")
        && !agent.is_empty()
        && task.status != Status::Done
        && task.assigned.as_deref() != Some(agent.as_str())
    {
        bail!("worker '{agent}' does not own task '{id}'");
    }
    Ok(())
}

fn verify_publication(
    project_root: &Path,
    contract: CompletionContract,
    outputs: &[OutputRef],
    integration_ref: &str,
) -> Result<String> {
    match contract {
        CompletionContract::Land => {
            let mut commits = outputs.iter().filter_map(|output| match output {
                OutputRef::Git(git) => Some(git.commit_oid.as_str()),
                _ => None,
            });
            let commit = commits.next().context("Land manifest has no Git output")?;
            if commits.next().is_some() {
                bail!("Land manifest has multiple Git outputs");
            }
            let status = Command::new("git")
                .args(["merge-base", "--is-ancestor", commit, integration_ref])
                .current_dir(project_root)
                .status()?;
            if !status.success() {
                bail!(
                    "publication missing: reviewed commit {} is not reachable from {}",
                    commit,
                    integration_ref
                );
            }
            Ok(format!("git:{integration_ref}:{commit}"))
        }
        CompletionContract::Report => Ok(format!(
            "artifacts:{}",
            outputs
                .iter()
                .map(output_identity)
                .collect::<Vec<_>>()
                .join(",")
        )),
        CompletionContract::Explore => Ok(format!(
            "exploration:{}",
            outputs
                .iter()
                .map(output_identity)
                .collect::<Vec<_>>()
                .join(",")
        )),
        CompletionContract::Deliver => {
            bail!("historical deliver tasks cannot use publication-derived Done")
        }
    }
}

fn output_identity(output: &OutputRef) -> String {
    match output {
        OutputRef::Git(git) => git.commit_oid.clone(),
        OutputRef::Artifact(artifact) => artifact.content_digest.to_string(),
        OutputRef::External(external) => external.after_digest.to_string(),
    }
}

fn commit_done(
    graph_path: &Path,
    id: &str,
    generation: u64,
    manifest_digest: &worksgood::completion_manifest::ContentDigest,
    contract: CompletionContract,
    receipt_digest: &str,
    completed_at: &str,
    accounting: &SourceAccounting,
) -> Result<()> {
    let mut refusal = None;
    modify_graph(graph_path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            refusal = Some("task disappeared before Done projection".to_string());
            return false;
        };
        if task.lifecycle.generation != generation
            || task
                .completion_candidate
                .as_ref()
                .map(|candidate| &candidate.manifest.content_digest)
                != Some(manifest_digest)
        {
            refusal = Some("candidate or generation changed before Done projection".to_string());
            return false;
        }
        let disposition = match contract {
            CompletionContract::Land => CompletionDisposition::Landed,
            CompletionContract::Report => CompletionDisposition::Reported,
            CompletionContract::Explore => CompletionDisposition::Explored,
            CompletionContract::Deliver => {
                refusal = Some("historical deliver cannot become Done through new protocol".into());
                return false;
            }
        };
        let mut request = TransitionRequest::new(
            TransitionKind::AttemptSucceeded {
                acceptance_ref: Some(receipt_digest.to_string()),
                manual_review: false,
            },
            LifecycleActor {
                kind: ActorKind::Finalizer,
                id: "completion-v3".to_string(),
            },
            "reviewed_publication_committed",
            format!("completion-v3:{id}:{generation}:{receipt_digest}"),
        )
        .with_evidence(receipt_digest.to_string());
        if task.lifecycle.current_attempt.is_some() {
            request.expected = FenceExpectation::current(task);
        }
        if let Err(error) = apply_transition(task, request) {
            refusal = Some(error.to_string());
            return false;
        }
        task.completion_disposition = Some(disposition);
        task.completion_receipt = Some(receipt_digest.to_string());
        if accounting.usage.is_some() {
            task.token_usage.clone_from(&accounting.usage);
        }
        if accounting.executor.is_some() {
            task.actual_executor.clone_from(&accounting.executor);
        }
        if accounting.model.is_some() {
            task.actual_model.clone_from(&accounting.model);
        }
        task.completed_at = Some(completed_at.to_string());
        task.last_interaction_at = Some(completed_at.to_string());
        task.assigned = None;
        task.failure_reason = None;
        task.failure_class = None;
        task.failure_signal = None;
        task.wait_condition = None;
        task.log.push(LogEntry {
            timestamp: completed_at.to_string(),
            actor: Some("completion-done".to_string()),
            user: None,
            message: format!(
                "Done derived from exact reviewed manifest {manifest_digest} and contract publication"
            ),
        });
        true
    })?;
    if let Some(refusal) = refusal {
        bail!(refusal);
    }
    Ok(())
}
