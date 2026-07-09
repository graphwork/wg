//! `wg provider` — the WG-Exec execution-federation surface (Exec-Wave B spark).
//!
//! The thin CLI glue over `worksgood::providers` (the library carries the crypto + the
//! invariants; this module is on-disk wiring + JSON). It reuses the WG-Fed identity
//! plane verbatim (`wg identity` mints/publishes; this module loads those identities and
//! resolves sigchains over the same `--store`) — **no second identity or crypto system
//! (NFR-4)**. The authorizer's canonical state (the [`ProviderRegistry`] and the
//! [`LeaseLedger`]) lives under `<wgdir>/exec/`; the canonical graph stays at the
//! authorizer (the single write boundary the epoch fence guards).
//!
//! The six-step flow (mirrors `docs/.../06` §4.2): `enroll` → `offer` → `claim` →
//! `grant` → `run` → `accept`, with `reclaim` / `verify` exercising the leash bounds and
//! `show` / `providers` surfacing the applied leash (ADR-E3 Consequences).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde_json::json;

use worksgood::identity::custody::{self, Ability, LeashPolicy, Scope};
use worksgood::identity::keys::Custodian;
use worksgood::identity::transport::open_store;
use worksgood::providers::bundle::{ContextSlice, DepArtifact, SealedBundle, recipient_enc_key};
use worksgood::providers::cross_task::{
    GraphPosition, classify_position, inputs_crossing_trust_boundary,
};
use worksgood::providers::lease::{Lease, LeaseLedger};
use worksgood::providers::placement::{
    PlacementVerdict, TaskRequirements, VerificationDepth, evaluate_placement, leash, pool_tier,
};
use worksgood::providers::verify::{
    Checkability, PinnedSpec, VerifyRequest, authorize_graph_write, verify_attribution,
    verify_result,
};
use worksgood::providers::worker;
use worksgood::providers::{
    CapabilityAd, Claim, IsolationClass, LeaseRenewal, PlacementOffer, PoolClass, ProviderRegistry,
    ResultEnvelope, Sensitivity, TrustLevel, WG_EXEC_COMPAT_VERSION, check_exec_compat,
    parse_trust, trust_str,
};

use super::identity_cmd::{load_local, resolve_auth_cached, signing_kid};

// ── On-disk authorizer state (under <wgdir>/exec/) ──────────────────────────────

fn exec_dir(workgraph_dir: &Path) -> PathBuf {
    workgraph_dir.join("exec")
}

fn load_registry(workgraph_dir: &Path) -> ProviderRegistry {
    // Delegate to the single canonical reader so the leash and the review gate read the
    // SAME persisted trust dial (see `worksgood::trust`).
    ProviderRegistry::load(workgraph_dir)
}

fn save_registry(workgraph_dir: &Path, reg: &ProviderRegistry) -> Result<()> {
    let dir = exec_dir(workgraph_dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::write(
        worksgood::providers::registry_path(workgraph_dir),
        serde_json::to_string_pretty(reg)?,
    )?;
    Ok(())
}

/// Read-only ledger load (audit B3). Refuses on a corrupt/partial parse rather than
/// silently resetting to empty — a reset would drop the epoch fence. Mutating paths use
/// [`LeaseLedger::open_locked`] instead, which holds an exclusive lock across the whole
/// read-modify-write.
fn load_ledger(workgraph_dir: &Path) -> Result<LeaseLedger> {
    LeaseLedger::load(workgraph_dir)
}

fn emit(json: bool, value: serde_json::Value, human: &str) {
    if json {
        println!("{}", serde_json::to_string(&value).unwrap_or_default());
    } else {
        println!("{human}");
    }
}

/// The task's **graph position** for the tier-by-graph-position floor (cross-task poison /
/// TC8 / B7). A task with downstream descendants in the authorizer's graph is
/// `Foundational` (its output feeds others ⇒ floors at Verified); one with none — or a task
/// not in the graph (a pure-exec spark task has no known descendants) — is a `Leaf`. The
/// classification uses the authorizer's *known* topology; an isolated task carries no known
/// blast radius.
fn graph_position(workgraph_dir: &Path, task_id: &str) -> GraphPosition {
    match crate::commands::load_workgraph(workgraph_dir) {
        Ok((g, _)) => classify_position(g.transitive_descendants(task_id).len()),
        Err(_) => GraphPosition::Leaf,
    }
}

/// Trust → pool class (ADR-E1 D4: one mechanism, three operating points).
fn pool_for(trust: TrustLevel) -> PoolClass {
    match trust {
        TrustLevel::Verified => PoolClass::Private,
        TrustLevel::Provisional => PoolClass::Cooperative,
        TrustLevel::Unknown => PoolClass::Cooperative,
    }
}

fn now_or(override_ts: Option<&str>) -> Result<chrono::DateTime<chrono::Utc>> {
    match override_ts {
        Some(s) => chrono::DateTime::parse_from_rfc3339(s)
            .map(|t| t.with_timezone(&chrono::Utc))
            .map_err(|e| anyhow::anyhow!("--now {s:?} is not RFC3339: {e}")),
        None => Ok(Utc::now()),
    }
}

// ── enroll ───────────────────────────────────────────────────────────────────

/// `wg provider enroll` — record a provider in the authorizer's pool at an
/// authorizer-asserted trust level + advertised capability (ADR-E1 D6). Trust is the
/// authorizer's to set; the provider never self-certifies it.
#[allow(clippy::too_many_arguments)]
pub fn run_enroll(
    workgraph_dir: &Path,
    provider_wgid: &str,
    trust: &str,
    model: &str,
    isolation: &str,
    attested: bool,
    json: bool,
) -> Result<()> {
    let trust = parse_trust(trust)?;
    let isolation = IsolationClass::parse(isolation)?;
    let mut reg = load_registry(workgraph_dir);
    reg.enroll(
        provider_wgid,
        trust,
        Some(CapabilityAd {
            model: model.to_string(),
            isolation,
            attested,
        }),
    );
    save_registry(workgraph_dir, &reg)?;
    emit(
        json,
        json!({
            "enrolled": provider_wgid,
            "trust": trust_str(trust),
            "model": model,
            "isolation": isolation.as_str(),
            "attested": attested,
        }),
        &format!(
            "enrolled {provider_wgid} at trust={} ({}, attested={attested})",
            trust_str(trust),
            isolation.as_str()
        ),
    );
    Ok(())
}

// ── offer (the fail-closed placement gate; ADR-E1 + ADR-E2 D2) ──────────────────

/// `wg provider offer` — the authorizer emits a `PlacementOffer` after the **fail-closed
/// filter+leash**. A confidential task to a non-attested provider, or an unlabeled task,
/// is **refused here** — no offer is written, so context is NEVER shipped (ADR-E2 D2/D-i,
/// the spark step-6 assertion).
#[allow(clippy::too_many_arguments)]
pub fn run_offer(
    workgraph_dir: &Path,
    as_name: &str,
    task: &str,
    model: &str,
    isolation: &str,
    sensitivity: Option<&str>,
    checkable: bool,
    provider_wgid: &str,
    out: &str,
    json: bool,
) -> Result<()> {
    let g = load_local(workgraph_dir, as_name)?;
    let g_auth = g.auth()?;
    let cust = Custodian::new(g.name());
    let signer = signing_kid(&g, &cust, &g_auth)?;

    let sens = match sensitivity {
        Some(s) => Sensitivity::parse(s),
        None => Sensitivity::Unlabeled,
    };
    let min_iso = IsolationClass::parse(isolation)?;
    let reg = load_registry(workgraph_dir);
    let provider_trust = reg.trust_of(provider_wgid);
    let provider_cap = reg.get(provider_wgid).and_then(|e| e.capability.clone());

    // Tier-by-graph-position (B7/TC8): a foundational task (one with descendants) floors at
    // the Verified (A) tier — only a leaf may be offered to a lower-trust provider.
    let position = graph_position(workgraph_dir, task);
    let req = TaskRequirements {
        task_id: task.to_string(),
        required_model: model.to_string(),
        min_isolation: min_iso,
        sensitivity: sens,
        checkable,
        position,
    };

    match evaluate_placement(
        &req,
        provider_trust,
        provider_cap.as_ref(),
        pool_for(provider_trust),
    ) {
        PlacementVerdict::Refused(r) => {
            // Fail-closed: no offer is emitted; context is never shipped to the provider.
            emit(
                json,
                json!({
                    "placed": false,
                    "refused": true,
                    "reason": r.reason,
                    "detail": r.detail,
                    "context_shipped": false,
                    "task": task,
                    "provider": provider_wgid,
                }),
                &format!("REFUSED ({}): {} — context NOT shipped", r.reason, r.detail),
            );
            Ok(())
        }
        PlacementVerdict::Eligible(decision) => {
            // Reserve the lease epoch for this placement (epoch starts at 1) AND record the
            // authorizer's signed-offer sensitivity + checkability (M17/S7) on the lease.
            // grant + accept re-derive the leash from THIS authoritative label, never a
            // hardcoded `Normal` or a provider-supplied value. The exclusive lock
            // serializes this read-modify-write against concurrent writers; the load
            // refuses on a corrupt ledger (audit B3).
            let mut guard = LeaseLedger::open_locked(workgraph_dir)?;
            let epoch = guard.ledger.place(task, provider_wgid);
            guard.ledger.record_offer_terms(task, sens, checkable);
            guard.save()?;

            let mut offer = PlacementOffer::build(
                task,
                g.wgid(),
                provider_wgid,
                model,
                min_iso,
                sens,
                decision.trust_floor,
                epoch,
                &Utc::now().to_rfc3339(),
            );
            offer.sign(&cust, &signer)?;
            std::fs::write(out, serde_json::to_string_pretty(&offer)?)
                .with_context(|| format!("writing offer to {out}"))?;

            emit(
                json,
                json!({
                    "placed": true,
                    "refused": false,
                    "task": task,
                    "provider": provider_wgid,
                    "trust_floor": trust_str(decision.trust_floor),
                    "pool_tier": pool_tier(provider_trust),
                    "graph_position": position.as_str(),
                    "sensitivity": sens.as_str(),
                    "checkable": checkable,
                    "lease_epoch": epoch,
                    "context_seal": decision.context_seal.as_str(),
                    "verification_depth": decision.verification_depth.as_str(),
                    "exec_compat": WG_EXEC_COMPAT_VERSION,
                    "offer_file": out,
                }),
                &format!(
                    "offered {task} to {provider_wgid} (tier {}, epoch {epoch}, seal={}, verify={})",
                    pool_tier(provider_trust),
                    decision.context_seal.as_str(),
                    decision.verification_depth.as_str()
                ),
            );
            Ok(())
        }
    }
}

// ── place (M5: the coordinator drives placement FROM the planner's graph task) ──

/// `wg provider place` — the **coordinator-side placement driver** (audit M5). Where
/// `offer` takes every parameter on the CLI, `place` sources them from a **task already in
/// the authorizer's graph** that carries typed remote execution metadata
/// (`remote_provider = "wgid:*"`): the placement target, the model, and the
/// checkability all come from the task. It then runs the SAME fail-closed leash+matcher and
/// emits the signed offer. This is the wiring the audit flags as missing — `Placement::Provider`
/// produced by the planner metadata, turned into the first wire envelope by the coordinator.
#[allow(clippy::too_many_arguments)]
pub fn run_place(
    workgraph_dir: &Path,
    as_name: &str,
    task_id: &str,
    sensitivity_override: Option<&str>,
    non_checkable: bool,
    out: &str,
    json: bool,
) -> Result<()> {
    let (graph, _path) = crate::commands::load_workgraph(workgraph_dir)?;
    let task = graph
        .get_task(task_id)
        .ok_or_else(|| anyhow::anyhow!("no task {task_id:?} in the authorizer's graph"))?;

    // The planner's placement decision: typed `remote_provider` metadata (the same signal
    // `dispatch::plan_spawn` turns into `Placement::Provider`). Tags are labels only.
    let provider_wgid = task
        .remote_provider
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "task {task_id:?} is not placed on a remote provider — set typed \
                 `remote_provider` metadata (for example `wg add --remote-provider <wgid>`)"
            )
        })?;
    let model = task.model.as_deref().unwrap_or("claude:opus").to_string();
    // Sensitivity comes from the typed command argument. With no explicit value,
    // placement stays fail-closed as `unlabeled`; freeform tags are ignored.
    let sensitivity = sensitivity_override.map(|s| s.to_string());
    // The deliverable is checkable unless the operator passes --non-checkable.
    // Freeform tags are labels and do not control WG-Exec verification policy.
    let checkable = !non_checkable;

    run_offer(
        workgraph_dir,
        as_name,
        task_id,
        &model,
        "container",
        sensitivity.as_deref(),
        checkable,
        &provider_wgid,
        out,
        json,
    )
}

// ── claim (provider → authorizer; a request, not an authorization) ──────────────

/// `wg provider claim` — a provider builds a signed `Claim` against an offer (ADR-E1 D2,
/// the OQ3 eligibility proof). It advertises capability + signs (identity proof); it does
/// **not** assert its own trust. The authorizer's `RunGrant`, not this, authorizes.
pub fn run_claim(
    workgraph_dir: &Path,
    as_name: &str,
    offer_file: &str,
    store_loc: &str,
    out: &str,
    json: bool,
) -> Result<()> {
    let p = load_local(workgraph_dir, as_name)?;
    let p_auth = p.auth()?;
    let cust = Custodian::new(p.name());
    let signer = signing_kid(&p, &cust, &p_auth)?;

    let offer: PlacementOffer = read_json(offer_file)?;
    check_exec_compat(&offer.exec_compat)?;
    // Authenticate the offer: it must be signed by the authorizer it names.
    let store = open_store(store_loc)?;
    let auth_auth = resolve_auth_cached(workgraph_dir, store.as_ref(), &offer.authorizer)
        .with_context(|| format!("resolving offer authorizer {}", offer.authorizer))?;
    offer
        .verify_sig(&auth_auth)
        .context("offer signature does not verify against the named authorizer")?;
    if offer.provider != p.wgid() {
        bail!(
            "offer is addressed to {} but this provider is {}",
            offer.provider,
            p.wgid()
        );
    }

    let cap = CapabilityAd {
        model: offer.required_model.clone(),
        isolation: offer.min_isolation,
        attested: false, // v1 attestation slot is empty — a spark provider is never attested.
    };
    // Echo the authorizer's signed-offer sensitivity into the signed Claim (M17). The
    // authorizer re-derives the authoritative value from its own ledger at grant and
    // cross-checks this against a downgrade attempt.
    let mut claim = Claim::build(
        &offer.task_id,
        p.wgid(),
        cap,
        offer.sensitivity,
        &Utc::now().to_rfc3339(),
    );
    claim.sign(&cust, &signer)?;
    std::fs::write(out, serde_json::to_string_pretty(&claim)?)
        .with_context(|| format!("writing claim to {out}"))?;

    emit(
        json,
        json!({
            "claimed": true,
            "task": offer.task_id,
            "provider": p.wgid(),
            "model": offer.required_model,
            "isolation": offer.min_isolation.as_str(),
            "claim_file": out,
        }),
        &format!("claimed {} as {}", offer.task_id, p.wgid()),
    );
    Ok(())
}

// ── grant (the placement decision: two scoped UCANs + sealed bundle + lease) ─────

/// `wg provider grant` — the authorizer verifies the claim, runs the leash, issues the
/// **two scoped attenuating UCANs** (act-as-agent + graph-write-task-only — never the
/// root key, never blanket graph write), seals the minimal `ContextScope` slice to the
/// provider, and emits a signed `RunGrant`. The field-scan (no root key / no blanket
/// write) is the step-1 assertion (ADR-E3 D1).
#[allow(clippy::too_many_arguments)]
pub fn run_grant(
    workgraph_dir: &Path,
    as_name: &str,
    claim_file: &str,
    task_input_file: &str,
    after: &[String],
    ucan_ttl_secs: Option<i64>,
    store_loc: &str,
    out: &str,
    json: bool,
) -> Result<()> {
    let g = load_local(workgraph_dir, as_name)?;
    let g_auth = g.auth()?;
    let cust = Custodian::new(g.name());
    let signer = signing_kid(&g, &cust, &g_auth)?;
    let now = Utc::now();

    let claim: Claim = read_json(claim_file)?;
    check_exec_compat(&claim.exec_compat)?;
    let store = open_store(store_loc)?;

    // Authenticate the claim (identity proof) against the provider's sigchain.
    let provider_auth = resolve_auth_cached(workgraph_dir, store.as_ref(), &claim.provider)
        .with_context(|| format!("resolving claimant {}", claim.provider))?;
    claim
        .verify_sig(&provider_auth)
        .context("claim signature does not verify against the claimant's sigchain")?;

    // Re-derive the AUTHORITATIVE sensitivity + checkability from the authorizer's OWN
    // ledger record (set at offer time from the signed offer), NOT a hardcoded `Normal`
    // (audit M17). This binds the fail-closed confidential/unlabeled gate at grant too, so
    // grant can independently refuse — the gate no longer relies solely on offer having run
    // first (X-1: floor before context).
    let led_ro = load_ledger(workgraph_dir)?;
    let sensitivity = led_ro
        .sensitivity_of(&claim.task_id)
        .unwrap_or(Sensitivity::Unlabeled);
    let checkable = led_ro.checkable_of(&claim.task_id).unwrap_or(true);
    // A provider must not DOWNGRADE the sensitivity in its signed claim (M17). Equal or
    // stricter is fine (it cannot loosen the authorizer's label); strictly weaker is a
    // downgrade attempt and is refused.
    if claim.sensitivity.strictness_rank() < sensitivity.strictness_rank() {
        bail!(
            "refusing to grant — the claim's sensitivity {} is WEAKER than the authorizer's \
             signed offer ({}): a provider may not downgrade sensitivity (M17)",
            claim.sensitivity.as_str(),
            sensitivity.as_str()
        );
    }

    // Re-run the fail-closed filter+leash with the authorizer's OWN trust record.
    let reg = load_registry(workgraph_dir);
    let provider_trust = reg.trust_of(&claim.provider);
    let provider_cap = reg.get(&claim.provider).and_then(|e| e.capability.clone());
    // Tier-by-graph-position (B7/TC8): re-assert the foundational floor at grant too, so a
    // foundational task can never be granted to a low-trust provider even if offer was
    // bypassed (defense in depth — the floor is re-derived from the authorizer's own graph).
    let position = graph_position(workgraph_dir, &claim.task_id);
    let req = TaskRequirements {
        task_id: claim.task_id.clone(),
        required_model: claim.capability.model.clone(),
        min_isolation: claim.capability.isolation,
        sensitivity,
        checkable,
        position,
    };
    let decision = match evaluate_placement(
        &req,
        provider_trust,
        provider_cap.as_ref(),
        pool_for(provider_trust),
    ) {
        PlacementVerdict::Eligible(d) => d,
        PlacementVerdict::Refused(r) => {
            bail!(
                "refusing to grant — placement filter says {}: {}",
                r.reason,
                r.detail
            )
        }
    };

    // Cross-task poison (B7/TC8) — **input re-verification across trust boundaries.** The
    // consumer (this task, granted to a `provider_trust` box) must NOT consume an input
    // produced by a STRICTLY LOWER-TRUST provider until that input has been independently
    // re-verified in a trusted domain. Map each remote `--after` dep to its producing
    // provider's trust from the authorizer's ledger; a dep that crossed the boundary
    // downward and is not yet integrity-verified makes us REFUSE to seal it into the context
    // (received ≠ consumed — the poison never reaches the consumer's bundle). A local input
    // (absent from the exec ledger) carries no remote producer and is not gated here.
    let led_inputs = load_ledger(workgraph_dir)?;
    let upstream_producers: Vec<(String, TrustLevel)> = after
        .iter()
        .filter_map(|spec| spec.split_once('='))
        .filter_map(|(dep, _)| {
            led_inputs
                .provider_of(dep)
                .map(|p| (dep.to_string(), reg.trust_of(p)))
        })
        .collect();
    let unverified: Vec<String> =
        inputs_crossing_trust_boundary(provider_trust, &upstream_producers)
            .into_iter()
            .filter(|dep| !led_inputs.is_input_verified(dep))
            .collect();
    if !unverified.is_empty() {
        bail!(
            "refusing to grant — cross-trust-input-unverified: task {} (on a {} provider) \
             consumes lower-trust input(s) [{}] that have not been re-verified across the \
             trust boundary. Re-verify each in a trusted domain first \
             (`wg provider verify --result <dep-result> --verifier <G> --pinned-spec <spec>`), \
             then re-grant (B7/TC8).",
            claim.task_id,
            trust_str(provider_trust),
            unverified.join(", ")
        );
    }

    let ttl = ucan_ttl_secs.unwrap_or(decision.delegation_ttl_secs);
    let task = &claim.task_id;

    // The two scoped attenuating UCANs (ADR-E3 D1), issued via WG-Fed's UCAN — no new
    // delegation system. act-as-agent is intent-bound to THIS task; graph-write is the
    // single task subtree only (never graph://*).
    let policy = LeashPolicy::from_env();
    let act_scope = Scope::new(vec![Ability::new(
        "act-as-agent",
        &format!("agent://{}/task/{task}", g.wgid()),
    )]);
    let write_scope = Scope::new(vec![Ability::new(
        "graph/write",
        &format!("graph://task/{task}"),
    )]);
    let act_as_agent_ucan = custody::issue_root(
        &cust,
        &signer,
        g.wgid(),
        &claim.provider,
        act_scope,
        Some(ttl),
        now,
        &policy,
        false,
    )?;
    let graph_write_ucan = custody::issue_root(
        &cust,
        &signer,
        g.wgid(),
        &claim.provider,
        write_scope,
        Some(ttl),
        now,
        &policy,
        false,
    )?;

    // Build + seal the minimal context slice to the provider's enrollment enc key.
    let task_input = std::fs::read_to_string(task_input_file)
        .with_context(|| format!("reading task input {task_input_file}"))?;
    let mut after_artifacts = Vec::new();
    for spec in after {
        let (dep, file) = spec
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--after expects dep_task=path, got {spec:?}"))?;
        let artifact = std::fs::read_to_string(file)
            .with_context(|| format!("reading --after artifact {file}"))?;
        after_artifacts.push(DepArtifact {
            task_id: dep.to_string(),
            artifact,
        });
    }
    let slice = ContextSlice::build(
        task,
        decision.context_scope_tier,
        &task_input,
        after_artifacts,
    );
    let (enc_kid, enc_pub) = recipient_enc_key(&provider_auth)?;
    let bundle = SealedBundle::seal(
        &slice,
        g.wgid(),
        &claim.provider,
        &cust,
        &signer,
        &[(enc_kid, enc_pub)],
        &now.to_rfc3339(),
    )?;

    // The lease (epoch reserved at offer time; reuse it). Locked read-modify-write so a
    // concurrent grant cannot lose this placement (audit B3).
    let mut guard = LeaseLedger::open_locked(workgraph_dir)?;
    let epoch = guard
        .ledger
        .current_epoch(task)
        .unwrap_or_else(|| guard.ledger.place(task, &claim.provider));
    // Stamp the leash-decided lease term + grant time so the timeout sweep (M16) has a
    // deadline anchor; a dead lease (no renewal within the term) auto-reclaims.
    guard
        .ledger
        .set_lease_terms(task, decision.lease_term_secs, &now.to_rfc3339());
    guard.save()?;
    let mut lease = Lease::build(
        task,
        g.wgid(),
        &claim.provider,
        epoch,
        decision.lease_term_secs,
        decision.lease_renew_cadence_secs,
        &now.to_rfc3339(),
    );
    lease.sign(&cust, &signer)?;

    let mut grant = worksgood::providers::RunGrant {
        v: worksgood::identity::ENVELOPE_V,
        alg: worksgood::identity::ALG_ED25519.to_string(),
        exec_compat: WG_EXEC_COMPAT_VERSION.to_string(),
        task_id: task.clone(),
        authorizer: g.wgid().to_string(),
        provider: claim.provider.clone(),
        // The authorizer names the silicon the worker must drive (echoes the claimed
        // model). The real worker (`wg provider run`) reads this when no explicit
        // `--worker-cmd` backend is set.
        model: claim.capability.model.clone(),
        act_as_agent_ucan,
        graph_write_ucan,
        bundle,
        lease,
        created_at: now.to_rfc3339(),
        sig: String::new(),
    };
    grant.sign(&cust, &signer)?;
    std::fs::write(out, serde_json::to_string_pretty(&grant)?)
        .with_context(|| format!("writing grant to {out}"))?;

    let scan = grant.field_scan();
    emit(
        json,
        json!({
            "granted": true,
            "task": task,
            "provider": claim.provider,
            "exec_compat": WG_EXEC_COMPAT_VERSION,
            "signed": true,
            "ucan_ttl_secs": ttl,
            "lease_epoch": epoch,
            "field_scan": {
                "contains_private_key_material": scan.contains_private_key_material,
                "has_blanket_graph_write": scan.has_blanket_graph_write,
                "graph_write_resource": scan.graph_write_resource,
                "act_as_agent_resource": scan.act_as_agent_resource,
            },
            "grant_file": out,
        }),
        &format!(
            "granted {task} to {} — two scoped UCANs (write={}), no root key={}",
            claim.provider, scan.graph_write_resource, !scan.contains_private_key_material
        ),
    );
    Ok(())
}

// ── run (the worker on the provider: open slice, produce a signed result) ────────

/// `wg provider run` — the worker on the provider verifies the grant + both UCANs
/// offline, opens its sealed slice (asserting it is exactly the configured tier with no
/// out-of-slice secret), **drives a REAL worker backend** over the task slice, and emits a
/// `ResultEnvelope` signed by its delegated signer carrying the **real** usage measured
/// from that run (FR-V3) — never the constant-diff / canned-usage stub (audit-exec
/// F10/F11).
///
/// The backend is resolved by [`worker::resolve_backend`]: an explicit `--worker-cmd` (or
/// `WG_EXEC_WORKER_CMD`) real subprocess, else the model handler the authorizer named in
/// the grant. `--corrupt` simulates a defecting provider by grafting a poisoned hunk
/// (backdoor + exfil + test-disable) onto the **real** output (the step-5 integrity
/// assertion). `--target-task` aims the write at a different task for the step-4(i)
/// over-scope assertion.
#[allow(clippy::too_many_arguments)]
pub fn run_worker_run(
    workgraph_dir: &Path,
    as_name: &str,
    grant_file: &str,
    store_loc: &str,
    out: &str,
    target_task: Option<&str>,
    corrupt: bool,
    scope_probe: Option<&str>,
    worker_cmd: Option<&str>,
    json: bool,
) -> Result<()> {
    let p = load_local(workgraph_dir, as_name)?;
    let p_auth = p.auth()?;
    let cust = Custodian::new(p.name());
    let signer = signing_kid(&p, &cust, &p_auth)?;
    let now = Utc::now();

    let grant: worksgood::providers::RunGrant = read_json(grant_file)?;
    check_exec_compat(&grant.exec_compat)?;
    let store = open_store(store_loc)?;
    let resolve = |w: &str| resolve_auth_cached(workgraph_dir, store.as_ref(), w);

    // Authenticate the grant (signed by the authorizer it names).
    let authorizer_auth = resolve(&grant.authorizer)
        .with_context(|| format!("resolving grant authorizer {}", grant.authorizer))?;
    grant
        .verify_sig(&authorizer_auth)
        .context("grant signature does not verify against the authorizer")?;

    // Verify the two UCANs offline (chain to G, not expired/revoked).
    custody::verify(&grant.act_as_agent_ucan, now, &[], &resolve)
        .context("act-as-agent UCAN does not verify")?;
    custody::verify(&grant.graph_write_ucan, now, &[], &resolve)
        .context("graph-write UCAN does not verify")?;

    // Open the sealed slice; only this provider's enc key can (encryption = ACL).
    grant
        .bundle
        .verify_sealer(&authorizer_auth)
        .context("bundle sealer signature does not verify")?;
    let slice = grant
        .bundle
        .open(&cust)
        .context("opening sealed context bundle")?;

    let out_of_slice_secret_found = scope_probe
        .map(|s| slice.contains(s) || grant.bundle.wire_leaks(s))
        .unwrap_or(false);
    // No standing credential beyond the two scoped UCANs may ride in the slice.
    let credential_beyond_ucans_found = {
        let s = serde_json::to_string(&slice).unwrap_or_default();
        s.contains("ed25519:") || s.contains("x25519:")
    };

    // Drive the REAL worker backend over the task slice — a real subprocess (command) or
    // the live model handler — producing a real work product + real usage.
    let config = worksgood::config::Config::load_or_default(workgraph_dir);
    let backend = worker::resolve_backend(worker_cmd, &grant.model)?;
    let mut work = worker::run_backend(
        &backend,
        &grant.task_id,
        &slice.task_input,
        &grant.model,
        &config,
        worker::WORKER_TIMEOUT_SECS,
    )?;
    let backend_kind = work.backend;
    // A defecting provider: graft the poison onto the REAL output (keeps the real usage).
    if corrupt {
        work.work_product = worker::apply_hostile_transform(&work.work_product);
    }
    let target = target_task.unwrap_or(&grant.task_id).to_string();

    let mut result = ResultEnvelope {
        v: worksgood::identity::ENVELOPE_V,
        alg: worksgood::identity::ALG_ED25519.to_string(),
        exec_compat: WG_EXEC_COMPAT_VERSION.to_string(),
        task_id: target.clone(),
        agent: grant.authorizer.clone(),
        producer: p.wgid().to_string(),
        epoch: grant.lease.epoch,
        work_product: work.work_product,
        // The provider's *claim* — believed only after the integrity re-run. Even a
        // defecting provider claims success (that lie is exactly what the trusted re-run
        // catches); attribution + the re-run, not this bit, are the integrity gate.
        claims_tests_pass: true,
        usage: work.usage.clone(),
        act_as_agent_ucan: grant.act_as_agent_ucan.clone(),
        graph_write_ucan: grant.graph_write_ucan.clone(),
        created_at: now.to_rfc3339(),
        sig: String::new(),
    };
    result.sign(&cust, &signer)?;
    std::fs::write(out, serde_json::to_string_pretty(&result)?)
        .with_context(|| format!("writing result to {out}"))?;

    emit(
        json,
        json!({
            "ran": true,
            "slice_scope_tier": slice.scope_tier,
            "slice_task_id": slice.task_id,
            "out_of_slice_secret_found": out_of_slice_secret_found,
            "credential_beyond_ucans_found": credential_beyond_ucans_found,
            "attribution_signer": p.wgid(),
            "target_task": target,
            "corrupted": corrupt,
            "backend": backend_kind,
            "model": work.model,
            "usage": {
                "input_tokens": work.usage.input_tokens,
                "output_tokens": work.usage.output_tokens,
                "cost_usd": work.usage.cost_usd,
            },
            "result_file": out,
        }),
        &format!(
            "ran {} via {} backend (slice tier={}, out-of-slice-secret={}); usage in={}/out={}; \
             signed by {}",
            target,
            backend_kind,
            slice.scope_tier,
            out_of_slice_secret_found,
            work.usage.input_tokens,
            work.usage.output_tokens,
            p.wgid()
        ),
    );
    Ok(())
}

// ── accept (the canonical write boundary: attribution + scope + verify + epoch fence) ──

/// `wg provider accept` — the authorizer's canonical-write accept path: verify
/// attribution (rejecting unsigned / wrong-signed / **expired**), authorize the write
/// under the task-scoped graph-write UCAN (rejecting a write to a **different task**),
/// screen the artifact (IC2 review), **gate on the integrity re-run when the leash demands
/// it (audit B4)**, then the **atomic epoch CAS** (rejecting a **stale** or **replayed**
/// write). On success, bridge the result's usage into the graph task's accounting (audit
/// M15). `--now` overrides the clock for the post-expiry assertion.
///
/// **B4 — accept gates verify.** The leash's `verification_depth` is now consulted *at the
/// canonical write boundary*, not in a decoupled manual `wg provider verify`. A Verified+
/// Normal result (the A/trusted tier) commits on attribution+scope as before. A low-trust
/// (B/verified-overflow) result requires a **trusted-domain re-run vs the authorizer's
/// pinned spec** (`--pinned-spec`, on a `--verifier` disjoint from the producer, defaulting
/// to the authorizer itself) to PASS before the epoch is consumed; a corrupted result is
/// rejected and the producer's trust is lowered (the audit/revoke leg). A low-trust result
/// with no pinned spec is refused (`verification-required`) — fail closed.
#[allow(clippy::too_many_arguments)]
pub fn run_accept(
    workgraph_dir: &Path,
    result_file: &str,
    store_loc: &str,
    now_override: Option<&str>,
    review: bool,
    pinned_spec_file: Option<&str>,
    verifier_wgid: Option<&str>,
    complete_task: bool,
    json: bool,
) -> Result<()> {
    let result: ResultEnvelope = read_json(result_file)?;
    check_exec_compat(&result.exec_compat)?;
    let now = now_or(now_override)?;
    let store = open_store(store_loc)?;
    let resolve = |w: &str| resolve_auth_cached(workgraph_dir, store.as_ref(), w);

    // 1. Attribution — unsigned / wrong-signed / expired ⇒ rejected (ADR-E4 D1).
    let attr = verify_attribution(&result, now, &[], &resolve);
    if !attr.ok {
        return reject(json, "attribution-failed", &attr.reason);
    }

    // 2. The write must be authorized by the task-scoped graph-write UCAN (FR-C2/V4).
    if let Err(e) = authorize_graph_write(
        &result.graph_write_ucan,
        &result.agent,
        &result.producer,
        &result.task_id,
        now,
        &[],
        &resolve,
    ) {
        return reject(json, "graph-write-scope-violation", &e.to_string());
    }

    // 2b. IC2 ingest review (ENFORCING, on by default): screen the work product through
    // the AI review pipeline BEFORE the canonical write is committed. Attribution proves
    // *who* produced the bytes, never that the bytes are *safe* — a malicious-but-correct
    // diff (a backdoor that passes its own tests) is caught here. A non-accept verdict
    // WITHHOLDS the bytes: the write is refused before it consumes the epoch (received ≠
    // consumed). `--no-review` opts out for a deliberately-unscreened accept.
    if review {
        if let Some((reason, detail)) = screen_accept_artifact(workgraph_dir, &result) {
            return reject(json, &reason, &detail);
        }
    }

    // 2c. B4 — gate on the integrity re-run when the leash demands it. Recompute the leash
    // from the producer's trust + the authorizer's OWN sensitivity record (M17) to learn
    // the verification depth. A Verified+Normal result clears on attribution+eval-gate (no
    // re-run); a low-trust result is re-run vs the pinned spec in a trusted domain BEFORE
    // the epoch is consumed.
    let reg = load_registry(workgraph_dir);
    let producer_trust = reg.trust_of(&result.producer);
    let attested = reg
        .get(&result.producer)
        .and_then(|e| e.capability.as_ref())
        .map(|c| c.attested)
        .unwrap_or(false);
    let led_ro = load_ledger(workgraph_dir)?;
    let sensitivity = led_ro
        .sensitivity_of(&result.task_id)
        .unwrap_or(Sensitivity::Unlabeled);
    let checkable = led_ro.checkable_of(&result.task_id).unwrap_or(true);
    let position = graph_position(workgraph_dir, &result.task_id);
    match leash(
        producer_trust,
        sensitivity,
        pool_for(producer_trust),
        attested,
        position,
    ) {
        // The fail-closed confidential/unlabeled gate is re-asserted at accept too.
        Err(refusal) => return reject(json, &refusal.reason, &refusal.detail),
        Ok(decision) => match decision.verification_depth {
            // A/trusted: attribution + eval-gate — no trusted-domain re-run needed.
            VerificationDepth::AttributionPlusEvalGate => {}
            // B/overflow or high-stakes: REQUIRE the trusted-domain re-run before commit.
            VerificationDepth::ReRunInTrustedDomain | VerificationDepth::Escalate => {
                if let Some((reason, detail)) = gate_on_rerun(
                    workgraph_dir,
                    &result,
                    producer_trust,
                    checkable,
                    pinned_spec_file,
                    verifier_wgid,
                    now,
                    &resolve,
                ) {
                    return reject(json, &reason, &detail);
                }
            }
        },
    }

    // 3. The atomic epoch CAS at the single canonical-write boundary (ADR-E3 D6). The
    // exclusive lock makes the compare-and-set a single serialized writer even across
    // concurrent processes; the load refuses on a corrupt ledger so a bad parse cannot
    // silently reset the fence and re-open double-commit/replay (audit B3). On a fenced
    // (rejected) commit we return WITHOUT saving — the guard drops, releasing the lock.
    let mut guard = LeaseLedger::open_locked(workgraph_dir)?;
    if let Err(fence) = guard.ledger.try_commit(&result.task_id, result.epoch) {
        return reject(json, fence_code(&fence), &fence.to_string());
    }
    guard.save()?;

    // Record the provider's liveness (an accepted write implies an accepted renewal).
    let mut reg = load_registry(workgraph_dir);
    reg.record_renewal(&result.producer, result.epoch, &now.to_rfc3339());
    save_registry(workgraph_dir, &reg)?;

    // M15 — bridge the remote result's usage into the graph task's accounting so remote
    // spend shows in `wg show` / `wg spend` / `wg stats` (best-effort; a no-op when the
    // exec task id is not a graph task, e.g. the pure-exec spark tasks). `--complete-task`
    // marks the task Done so `wg spend` (which counts Done/Failed) reflects it.
    let accounted = bridge_usage_into_graph(workgraph_dir, &result, complete_task);

    // Observability (M20): a committed result, correlated by the task id.
    worksgood::obs::record_exec_result(true);
    tracing::info!(
        task = %result.task_id,
        producer = %result.producer,
        epoch = result.epoch,
        "exec result accepted"
    );

    emit(
        json,
        json!({
            "accepted": true,
            "attributed_to": result.agent,
            "producer": result.producer,
            "task": result.task_id,
            "epoch": result.epoch,
            "pool_tier": pool_tier(producer_trust),
            "usage": {
                "input_tokens": result.usage.input_tokens,
                "output_tokens": result.usage.output_tokens,
                "cost_usd": result.usage.cost_usd,
            },
            "usage_accounted_to_graph": accounted,
            "reason": "accepted",
        }),
        &format!(
            "accepted result for {} — attributed to {} (produced by {})",
            result.task_id, result.agent, result.producer
        ),
    );
    Ok(())
}

/// B4 helper — the trusted-domain integrity re-run that `accept` requires for a low-trust
/// (B-tier) result before consuming the epoch. Returns `Some((reason, detail))` to REJECT
/// (no pinned spec, the re-run failed, or the X-5 same-domain guard fired), or `None` to
/// let the commit proceed. On a failed re-run it lowers the producer's trust (the
/// audit/revoke leg, ADR-E4 D4/D5) so its next item takes the deeper path.
#[allow(clippy::too_many_arguments)]
fn gate_on_rerun(
    workgraph_dir: &Path,
    result: &ResultEnvelope,
    producer_trust: TrustLevel,
    checkable: bool,
    pinned_spec_file: Option<&str>,
    verifier_wgid: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
    resolve: &dyn Fn(&str) -> Result<worksgood::identity::sigchain::AuthorizedKeys>,
) -> Option<(String, String)> {
    let Some(spec_path) = pinned_spec_file else {
        return Some((
            "verification-required".to_string(),
            "a low-trust (B/verified-overflow) result must be re-run vs the authorizer's \
             pinned spec before commit (the eval-gate). Pass --pinned-spec (and optionally \
             --verifier, a domain disjoint from the producer); the write is refused until \
             it does (fail-closed, audit B4)."
                .to_string(),
        ));
    };
    let spec: PinnedSpec = match read_json(spec_path) {
        Ok(s) => s,
        Err(e) => return Some(("pinned-spec-unreadable".to_string(), e.to_string())),
    };
    // The re-run runs in a TRUSTED DOMAIN that is NOT the producer (X-5). Default to the
    // authorizer/principal G (`result.agent`) — re-running authorizer-side is the
    // canonical trusted domain; `--verifier` names a disjoint trusted provider instead.
    let verifier = verifier_wgid.unwrap_or(&result.agent).to_string();
    // S7: a non-checkable deliverable cannot be eval-gated — `verify_result` escalates
    // (never accepts on "found nothing"), so a non-checkable low-trust result is refused.
    let checkability = if checkable {
        Checkability::Checkable
    } else {
        Checkability::NonCheckable
    };
    let req = VerifyRequest {
        result,
        producer: result.producer.clone(),
        verifier,
        trust: producer_trust,
        checkability,
        pinned_spec: &spec,
    };
    match verify_result(&req, now, &[], resolve) {
        // X-5: verifier == producer — a re-run on the producing box is theatre, refused.
        Err(e) => Some(("verifier-is-producer".to_string(), e.to_string())),
        Ok(v) if !v.accepted => {
            // A caught defection lowers the producer's trust (audit/revoke leg) — do NOT
            // commit (the epoch is left unconsumed).
            let mut reg = load_registry(workgraph_dir);
            reg.lower_trust(&result.producer);
            let _ = save_registry(workgraph_dir, &reg);
            Some((
                format!("integrity-{}", v.reason),
                format!(
                    "re-run on {} ({} / {}): {} — producer trust lowered, write refused",
                    v.reran_on, v.rerun_mode, v.rerun_detail, v.reason
                ),
            ))
        }
        Ok(_) => {
            // The eval-gate passed — record the integrity verification so a higher-trust
            // downstream consumer can confirm this input was re-verified across the trust
            // boundary (B7/TC8 input re-verification leg), then proceed to the epoch CAS.
            if let Ok(mut guard) = LeaseLedger::open_locked(workgraph_dir) {
                guard.ledger.mark_verified(&result.task_id);
                let _ = guard.save();
            }
            None
        }
    }
}

/// M15 — accumulate a remote result's usage into the graph task's `token_usage` so remote
/// spend reaches `wg show` / `wg spend` / `wg stats` (the same surfaces local spend uses).
/// Best-effort + gated: returns `false` (a no-op) when the exec task id is not a graph task.
/// `complete` marks the task `Done` so `wg spend` (which counts Done/Failed tasks) reflects
/// the remote run.
fn bridge_usage_into_graph(workgraph_dir: &Path, result: &ResultEnvelope, complete: bool) -> bool {
    use worksgood::graph::{Status, TokenUsage};
    let graph_path = workgraph_dir.join("graph.jsonl");
    if !graph_path.exists() {
        return false;
    }
    let mut accounted = false;
    let usage = TokenUsage {
        cost_usd: result.usage.cost_usd,
        input_tokens: result.usage.input_tokens,
        output_tokens: result.usage.output_tokens,
        cache_read_input_tokens: 0,
        cache_creation_input_tokens: 0,
    };
    let producer = result.producer.clone();
    let _ = worksgood::modify_graph(&graph_path, |g| {
        let Some(task) = g.get_task_mut(&result.task_id) else {
            return false;
        };
        match task.token_usage.as_mut() {
            Some(tu) => tu.accumulate(&usage),
            None => task.token_usage = Some(usage.clone()),
        }
        if complete && task.status != Status::Done {
            task.status = Status::Done;
            task.log.push(worksgood::graph::LogEntry {
                timestamp: chrono::Utc::now().to_rfc3339(),
                actor: None,
                user: Some(worksgood::current_user()),
                message: format!(
                    "remote exec completed by provider {producer} — usage bridged into accounting (M15)"
                ),
            });
        }
        accounted = true;
        true
    });
    accounted
}

/// IC2 accept-seam review: screen the result's work product (the artifact / diff)
/// through the AI review pipeline. The depth input here is the **provider dial** — the
/// producing box's execution trust in the WG-Exec pool ([`ProviderRegistry::trust_of`],
/// the SAME dial the placement leash reads) — *not* the author dial: for an artifact the
/// relevant question is "do I trust this box's output," which is exactly what the
/// provider trust answers (M18 keeps the two dials split — the author dial governs IC4
/// message ingest, the provider dial governs IC2 artifact ingest). The poison protection
/// (a malicious-but-correct diff / a backdoor that passes its own tests) comes from the
/// content detectors, which fire regardless of trust (strictest-wins, monotonic), so
/// even a Verified box's backdoor is rejected. A non-`accept` verdict means the bytes are
/// WITHHELD and the write must be refused; returns `Some((reason, detail))` to reject, or
/// `None` to proceed. The verdict is recorded to the verdict sigchain (audit leg)
/// regardless. Real model-driven review when a model is configured; the shared
/// deterministic decode-then-detect engine otherwise (credential-free CI / smoke).
fn screen_accept_artifact(
    workgraph_dir: &Path,
    result: &ResultEnvelope,
) -> Option<(String, String)> {
    use worksgood::review::{
        ContentClass, Provenance, Sensitivity as RevSensitivity, VerdictStore, review_inbound,
        review_inbound_ctx,
    };
    let trust = load_registry(workgraph_dir).trust_of(&result.producer);
    let provenance = Provenance {
        author: Some(result.producer.clone()),
        trust,
    };
    let outcome = match worksgood::config::Config::load_merged(workgraph_dir) {
        Ok(cfg) => review_inbound_ctx(
            &cfg,
            ContentClass::Ic2Artifact,
            &result.work_product,
            &provenance,
            RevSensitivity::Unlabeled,
        ),
        Err(_) => review_inbound(
            ContentClass::Ic2Artifact,
            &result.work_product,
            &provenance,
            RevSensitivity::Unlabeled,
        ),
    };
    // Audit leg (best-effort; a recording failure must not crash accept — the gate
    // decision still stands).
    let _ = VerdictStore::open(workgraph_dir).record(
        &outcome,
        Some(&result.producer),
        Some(&result.task_id),
    );
    if outcome.verdict.permits_consumption() {
        None
    } else {
        Some((
            format!("review-{}", outcome.verdict.tag()),
            format!(
                "IC2 artifact review returned {} ({}) — bytes withheld, write refused (received ≠ consumed)",
                outcome.verdict.tag(),
                outcome.reason.tag()
            ),
        ))
    }
}

fn reject(json: bool, reason: &str, detail: &str) -> Result<()> {
    // Observability (M20): a refused result at the accept boundary.
    worksgood::obs::record_exec_result(false);
    tracing::info!(reason, detail, "exec result rejected");
    emit(
        json,
        json!({ "accepted": false, "reason": reason, "detail": detail }),
        &format!("REJECTED ({reason}): {detail}"),
    );
    Ok(())
}

fn fence_code(f: &worksgood::providers::lease::FenceError) -> &'static str {
    use worksgood::providers::lease::FenceError::*;
    match f {
        StaleEpoch { .. } => "stale-epoch",
        AlreadyCommitted { .. } => "replay-already-committed",
        NoPlacement => "no-placement",
    }
}

// ── reclaim (bump the fencing epoch; ADR-E3 D6) ─────────────────────────────────

/// `wg provider reclaim` — reclaim a task, bumping the monotonic lease epoch. The old
/// worker's epoch is now stale; any late write/renewal it produces is fenced out.
pub fn run_reclaim(
    workgraph_dir: &Path,
    task: &str,
    new_provider: Option<&str>,
    json: bool,
) -> Result<()> {
    // Locked read-modify-write: the epoch bump is serialized so a concurrent commit
    // cannot race the reclaim (audit B3).
    let mut guard = LeaseLedger::open_locked(workgraph_dir)?;
    let new_epoch = guard
        .ledger
        .reclaim(task, new_provider.unwrap_or("wgid:reassigned"))
        .with_context(|| format!("reclaiming {task}"))?;
    guard.save()?;
    emit(
        json,
        json!({ "reclaimed": true, "task": task, "new_epoch": new_epoch }),
        &format!("reclaimed {task} → epoch {new_epoch} (old epoch now stale)"),
    );
    Ok(())
}

// ── liveness runtime: renew (provider) → accept-renewal (authorizer) → sweep (M16) ──

/// `wg provider renew` — the provider's signed lease **heartbeat** (audit M16). Reads the
/// grant, builds a `LeaseRenewal` for the lease epoch it holds, signs it with its delegated
/// signer (so a relay cannot fake "P is alive" — ADR-E3 D5), and writes it for the
/// authorizer to accept. This is the verb that was missing: `LeaseRenewal` was a
/// defined-but-unused wire type.
pub fn run_renew(
    workgraph_dir: &Path,
    as_name: &str,
    grant_file: &str,
    out: &str,
    json: bool,
) -> Result<()> {
    let p = load_local(workgraph_dir, as_name)?;
    let p_auth = p.auth()?;
    let cust = Custodian::new(p.name());
    let signer = signing_kid(&p, &cust, &p_auth)?;

    let grant: worksgood::providers::RunGrant = read_json(grant_file)?;
    check_exec_compat(&grant.exec_compat)?;
    if grant.provider != p.wgid() {
        bail!(
            "grant is for provider {} but this provider is {}",
            grant.provider,
            p.wgid()
        );
    }

    let mut renewal = LeaseRenewal::build(
        &grant.task_id,
        grant.lease.epoch,
        p.wgid(),
        &Utc::now().to_rfc3339(),
    );
    renewal.sign(&cust, &signer)?;
    std::fs::write(out, serde_json::to_string_pretty(&renewal)?)
        .with_context(|| format!("writing renewal to {out}"))?;
    emit(
        json,
        json!({
            "renewed": true,
            "task": grant.task_id,
            "epoch": grant.lease.epoch,
            "provider": p.wgid(),
            "renewal_file": out,
        }),
        &format!(
            "renewed lease for {} at epoch {} (provider {})",
            grant.task_id,
            grant.lease.epoch,
            p.wgid()
        ),
    );
    Ok(())
}

/// `wg provider accept-renewal` — the authorizer verifies a signed `LeaseRenewal` and
/// records liveness (audit M16). The signature must verify against the provider's sigchain
/// (an unsigned / forged "alive" is rejected), and the epoch must match the current lease
/// epoch (a STALE renewal after reclaim is fenced, exactly like a stale write). On success
/// it refreshes the lease's expiry deadline so the timeout sweep treats it as live.
pub fn run_accept_renewal(
    workgraph_dir: &Path,
    renewal_file: &str,
    store_loc: &str,
    now_override: Option<&str>,
    json: bool,
) -> Result<()> {
    let renewal: LeaseRenewal = read_json(renewal_file)?;
    check_exec_compat(&renewal.exec_compat)?;
    let now = now_or(now_override)?;
    let store = open_store(store_loc)?;

    // Authenticate: the renewal must be signed by the provider it names (no relay forgery).
    let provider_auth = resolve_auth_cached(workgraph_dir, store.as_ref(), &renewal.provider)
        .with_context(|| format!("resolving renewing provider {}", renewal.provider))?;
    if renewal.verify_sig(&provider_auth).is_err() {
        return reject(json, "renewal-unsigned-or-wrong-signed", &renewal.provider);
    }

    // Record liveness under the lock; a stale-epoch renewal (after reclaim) is fenced.
    let mut guard = LeaseLedger::open_locked(workgraph_dir)?;
    if let Err(fence) =
        guard
            .ledger
            .accept_renewal(&renewal.task_id, renewal.epoch, &now.to_rfc3339())
    {
        return reject(json, fence_code(&fence), &fence.to_string());
    }
    guard.save()?;

    let mut reg = load_registry(workgraph_dir);
    reg.record_renewal(&renewal.provider, renewal.epoch, &now.to_rfc3339());
    save_registry(workgraph_dir, &reg)?;

    emit(
        json,
        json!({
            "renewal_accepted": true,
            "task": renewal.task_id,
            "epoch": renewal.epoch,
            "provider": renewal.provider,
        }),
        &format!(
            "renewal accepted for {} at epoch {} (provider {} live)",
            renewal.task_id, renewal.epoch, renewal.provider
        ),
    );
    Ok(())
}

/// `wg provider sweep` — the authorizer's **auto-reclaim-on-timeout** runtime (audit M16):
/// reclaim every lease whose term elapsed with no accepted renewal. The loop a heartbeat
/// tick / coordinator runs periodically; `--now` injects the clock for a deterministic
/// test. Each expired lease's epoch is bumped, so a resurrected worker's late write is
/// fenced out (prefer-liveness: reclaiming a partitioned-but-live worker costs at most one
/// wasted re-run, never a corrupt graph — ADR-E3 D6).
pub fn run_sweep(
    workgraph_dir: &Path,
    new_provider: Option<&str>,
    now_override: Option<&str>,
    json: bool,
) -> Result<()> {
    let now = now_or(now_override)?;
    let mut guard = LeaseLedger::open_locked(workgraph_dir)?;
    let reclaimed = guard
        .ledger
        .sweep_expired(now, new_provider.unwrap_or("wgid:reassigned"));
    if !reclaimed.is_empty() {
        guard.save()?;
    }
    let rows: Vec<serde_json::Value> = reclaimed
        .iter()
        .map(|(task, epoch)| json!({ "task": task, "new_epoch": epoch }))
        .collect();
    emit(
        json,
        json!({ "swept": true, "reclaimed_count": reclaimed.len(), "reclaimed": rows }),
        &format!(
            "sweep: auto-reclaimed {} expired lease(s){}",
            reclaimed.len(),
            if reclaimed.is_empty() {
                String::new()
            } else {
                format!(
                    " — {}",
                    reclaimed
                        .iter()
                        .map(|(t, e)| format!("{t}→epoch {e}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        ),
    );
    Ok(())
}

// ── verify (the integrity leash: disjoint re-run vs a pinned spec; ADR-E4 D3) ────

/// `wg provider verify` — apply the low-trust integrity leash: attribution + a
/// **deterministic re-run in a trusted domain (never the producer) vs the authorizer's
/// pinned spec** (not the provider's shipped tests). Catches a corrupted result, flags a
/// test-poisoning attempt, and records provenance. `--verifier` MUST differ from the
/// producer (X-5) — the engine refuses otherwise.
#[allow(clippy::too_many_arguments)]
pub fn run_verify(
    workgraph_dir: &Path,
    result_file: &str,
    verifier_wgid: &str,
    pinned_spec_file: &str,
    checkability: &str,
    store_loc: &str,
    rerun_descendants: bool,
    json: bool,
) -> Result<()> {
    let result: ResultEnvelope = read_json(result_file)?;
    check_exec_compat(&result.exec_compat)?;
    let spec: PinnedSpec = read_json(pinned_spec_file)?;
    let store = open_store(store_loc)?;
    let resolve = |w: &str| resolve_auth_cached(workgraph_dir, store.as_ref(), w);
    let now = Utc::now();

    let check = match checkability.trim().to_ascii_lowercase().as_str() {
        "checkable" => Checkability::Checkable,
        "semi" | "semi-checkable" => Checkability::SemiCheckable,
        "non" | "non-checkable" => Checkability::NonCheckable,
        other => bail!("unknown checkability {other:?} (checkable|semi|non)"),
    };
    let reg = load_registry(workgraph_dir);
    let trust = reg.trust_of(&result.producer);

    let req = VerifyRequest {
        result: &result,
        producer: result.producer.clone(),
        verifier: verifier_wgid.to_string(),
        trust,
        checkability: check,
        pinned_spec: &spec,
    };

    match verify_result(&req, now, &[], &resolve) {
        Ok(verdict) => {
            let mut rerun = Vec::new();
            if verdict.accepted {
                // The integrity re-run PASSED — record it so a higher-trust downstream
                // consumer can confirm this input was re-verified across the trust boundary
                // (B7/TC8 input re-verification leg).
                if let Ok(mut guard) = LeaseLedger::open_locked(workgraph_dir) {
                    guard.ledger.mark_verified(&result.task_id);
                    let _ = guard.save();
                }
            } else {
                // REJECTED as a forgery/poison. Lower the producer's trust so its next item
                // takes the deeper path AND actually enumerate + re-run the descendants that
                // consumed this poisoned artifact (B7/TC8 — the comment was previously
                // aspirational; this now does the graph walk + re-queue).
                let mut reg = reg;
                reg.lower_trust(&result.producer);
                save_registry(workgraph_dir, &reg)?;
                rerun = crate::commands::rerun_poison_descendants(
                    workgraph_dir,
                    &result.task_id,
                    rerun_descendants,
                );
            }
            emit(
                json,
                json!({
                    "accepted": verdict.accepted,
                    "attribution_ok": verdict.attribution_ok,
                    "reran": verdict.reran,
                    "reran_on": verdict.reran_on,
                    "reran_on_is_producer": verdict.reran_on_is_producer,
                    "rerun_mode": verdict.rerun_mode,
                    "rerun_detail": verdict.rerun_detail,
                    "test_poisoning_flagged": verdict.test_poisoning_flagged,
                    "provenance_producer": verdict.provenance_producer,
                    "reason": verdict.reason,
                    "poison_descendants": rerun,
                    "descendants_requeued": !verdict.accepted && rerun_descendants,
                }),
                &format!(
                    "verify {} (reran on {}, test-poison={}): {}{}",
                    if verdict.accepted {
                        "ACCEPTED"
                    } else {
                        "REJECTED"
                    },
                    verdict.reran_on,
                    verdict.test_poisoning_flagged,
                    verdict.reason,
                    if rerun.is_empty() {
                        String::new()
                    } else {
                        format!(
                            " — descendants {}: {}",
                            if rerun_descendants {
                                "re-queued"
                            } else {
                                "to re-run"
                            },
                            rerun.join(", ")
                        )
                    },
                ),
            );
            Ok(())
        }
        Err(e) => {
            // The X-5 guard (verifier == producer) fires here.
            emit(
                json,
                json!({
                    "accepted": false,
                    "refused": true,
                    "reason": "verifier-is-producer",
                    "detail": e.to_string(),
                }),
                &format!("REFUSED: {e}"),
            );
            Ok(())
        }
    }
}

// ── show / providers (surface the applied leash; ADR-E3 Consequences) ───────────

/// `wg provider show` — surface the applied lease + (recomputed) leash for a task, so a
/// mis-set dial is visible at a glance (`wg show` parity).
pub fn run_show(
    workgraph_dir: &Path,
    task: &str,
    sensitivity: Option<&str>,
    json: bool,
) -> Result<()> {
    let led = load_ledger(workgraph_dir)?;
    let st = led
        .tasks
        .get(task)
        .ok_or_else(|| anyhow::anyhow!("no lease placement on record for {task}"))?;
    let reg = load_registry(workgraph_dir);
    let trust = reg.trust_of(&st.provider);
    let attested = reg
        .get(&st.provider)
        .and_then(|e| e.capability.as_ref())
        .map(|c| c.attested)
        .unwrap_or(false);
    // An explicit override wins; otherwise show the leash for the lease's RECORDED
    // sensitivity (M17), not a hardcoded `Normal`.
    let sens = sensitivity
        .map(Sensitivity::parse)
        .unwrap_or(st.sensitivity);
    // Recompute the leash with the task's real graph position so a displayed `trust_floor`
    // reflects the tier-by-graph-position bump (B7/TC8), not just the sensitivity floor.
    let position = graph_position(workgraph_dir, task);
    let integrity_verified = st.integrity_verified;
    let leash_v = match leash(trust, sens, pool_for(trust), attested, position) {
        Ok(d) => json!({
            "trust_floor": trust_str(d.trust_floor),
            "delegation_broad": d.delegation_broad,
            "delegation_ttl_secs": d.delegation_ttl_secs,
            "context_scope_tier": d.context_scope_tier.to_string(),
            "context_seal": d.context_seal.as_str(),
            "verification_depth": d.verification_depth.as_str(),
            "lease_term_secs": d.lease_term_secs,
            "lease_renew_cadence_secs": d.lease_renew_cadence_secs,
        }),
        Err(r) => json!({ "refused": r.reason, "detail": r.detail }),
    };
    emit(
        json,
        json!({
            "task": task,
            "provider": st.provider,
            "trust": trust_str(trust),
            "pool_tier": pool_tier(trust),
            "graph_position": position.as_str(),
            "integrity_verified": integrity_verified,
            "sensitivity": st.sensitivity.as_str(),
            "checkable": st.checkable,
            "epoch": st.epoch,
            "committed": st.committed,
            "term_secs": st.term_secs,
            "granted_at": st.granted_at,
            "last_renewal_epoch": st.last_renewal_epoch,
            "last_renewal_at": st.last_renewal_at,
            "leash": leash_v,
        }),
        &format!(
            "task {task}: provider {} trust={} epoch={} committed={}",
            st.provider,
            trust_str(trust),
            st.epoch,
            st.committed
        ),
    );
    Ok(())
}

/// `wg provider providers` (alias `list`) — the authorizer's known pool with trust +
/// observed liveness (ADR-E1 D6 / ADR-E3 D5).
pub fn run_providers(workgraph_dir: &Path, json: bool) -> Result<()> {
    let reg = load_registry(workgraph_dir);
    let led = load_ledger(workgraph_dir)?;
    let mut rows = Vec::new();
    for (wgid, e) in &reg.providers {
        // A provider is "live" if it has an accepted renewal at/after any task's current
        // epoch it holds; for display we report its highest renewal epoch.
        let live = led
            .tasks
            .values()
            .filter(|s| &s.provider == wgid)
            .any(|s| e.is_live(s.epoch));
        rows.push(json!({
            "wgid": wgid,
            "trust": trust_str(e.trust_level),
            // S7: the placement tier this trust maps to — A (trusted pool) / B
            // (verified-overflow) / refuse (a stranger, below the Normal floor).
            "pool_tier": pool_tier(e.trust_level),
            "capability": e.capability.as_ref().map(|c| json!({
                "model": c.model, "isolation": c.isolation.as_str(), "attested": c.attested,
            })),
            "last_renewal_epoch": e.last_renewal_epoch,
            "live": live,
        }));
    }
    emit(
        json,
        serde_json::Value::Array(rows.clone()),
        &format!("{} provider(s) enrolled", rows.len()),
    );
    Ok(())
}

// ── shared ──────────────────────────────────────────────────────────────────

fn read_json<T: serde::de::DeserializeOwned>(path: &str) -> Result<T> {
    let s = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
    serde_json::from_str(&s).with_context(|| format!("parsing {path}"))
}
