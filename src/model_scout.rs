//! Re-runnable OpenRouter strong/weak tier scout — value-aware.
//!
//! The Pi profile exposes two **stable tier labels** (`strong` / `weak`) that
//! insulate the rest of WG from OpenRouter model churn (see
//! `docs/design-two-tier-pi-profile.md`). The model *behind* each label moves;
//! the label does not. This module is the forward-looking mechanism that
//! repoints those two labels when the market moves — so the slots get
//! (re)filled **without hardcoding** a specific model id anywhere.
//!
//! It is the research/selection logic behind the design's
//! `wg profile pi --scout` entry point, and is also runnable directly as the
//! top-level verb `wg model-scout`. The design's `--scout` flag delegates to
//! [`scout`].
//!
//! # What it does
//!
//! 1. Fetches OpenRouter's *current* model catalog (`GET /api/v1/models`).
//! 2. **Bootstraps from whatever tiers are currently set** — reads the active
//!    config (`strong ← agent.model`, `weak ← tiers.fast`, with role fallbacks)
//!    and uses those incumbents as the baseline to beat.
//! 3. Selects, per documented criteria (never hardcoded model ids):
//!    - **strong** = best **value** coding/work model available right now —
//!      quality proxy *minus* a capped logarithmic cost penalty, so a much more
//!      expensive frontier model only wins if its quality delta is large enough
//!      to justify it (not on raw price-as-capability alone).
//!    - **weak**   = cheapest model that is *reliable enough* for agency
//!      judgment one-shots (flip / assign / post-flip evaluation /
//!      off-the-rails detection) — reliability-at-low-cost, **not** raw
//!      capability, with a small bonus for prompt-cache (`cache_control`)
//!      economics.
//! 4. **Always says what it is doing**, emitting one line per tier in the form
//!    `strong: <old> -> <new>  because …` / `weak: <old> -> <new>  because …`,
//!    plus a **ranked shortlist** with cost/quality rationale so a human can
//!    pick balanced/default vs cheapest-good vs frontier-expensive.
//! 5. Defaults to **dry-run**: prints the exact, copy-pasteable apply command
//!    (per the two-tier CLI design's accepted syntax). `--apply` writes the
//!    tiers. Every change is one command to revert (re-run with the old value).
//!
//! # Selection criteria (documented, not hardcoded)
//!
//! The criteria are *capability/eligibility rules over OpenRouter metadata*,
//! not a fixed allow-list of model ids. They are expressed as the consts and
//! predicates in the [`criteria`] module so they can be tuned (or replaced with
//! a real coding-benchmark index — see [`criteria::STRONG_NOTE`]) without
//! touching the control flow.

use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;

use crate::config::{Config, RoleModelConfig, pi_strong_route};
use crate::executor::native::openai_client::{
    self, OpenRouterModel, fetch_openrouter_models_blocking,
};

/// Documented, tunable selection criteria. These are the *only* place model
/// preferences live — there is intentionally **no hardcoded list of model
/// ids**. Everything is derived from OpenRouter metadata (pricing, context
/// window, advertised capabilities).
pub mod criteria {
    /// Minimum context window for the **strong** tier (real work needs room).
    pub const STRONG_MIN_CONTEXT: u64 = 131_072;
    /// Cost penalty band (USD per 1M *blended* tokens). At `LO` the penalty
    /// saturates near zero (cheap models barely penalized); at `HI` the
    /// penalty saturates near 1 (very expensive models must clearly beat
    /// cheaper peers on quality to overcome it). Log-scaled so a 2× price bump
    /// is *not* a 2× penalty.
    pub const STRONG_COST_LO: f64 = 0.5;
    pub const STRONG_COST_HI: f64 = 30.0;
    /// Weight of the cost penalty in the strong utility. With the quality proxy
    /// capped at 1.0, this means a frontier model can overcome a large cost
    /// penalty only if its quality delta is genuinely large — not merely by
    /// being the most expensive "pro" tier.
    pub const STRONG_COST_WEIGHT: f64 = 0.45;
    /// A candidate must beat the incumbent's strong utility by this *relative*
    /// margin before the scout proposes a switch (anti-churn / "baseline to
    /// beat").
    pub const STRONG_SWITCH_MARGIN: f64 = 1.05;

    /// Quality-proxy score weights for the strong tier (sum to 1.0). The
    /// OpenRouter `/api/v1/models` endpoint does **not** expose a coding
    /// benchmark, so these are *feature-gate + context* proxies — see
    /// [`STRONG_NOTE`]. When a measured coding index becomes available it
    /// should override `quality_proxy`.
    pub const STRONG_W_TOOLS: f64 = 0.20;
    pub const STRONG_W_STRUCTURED: f64 = 0.20;
    pub const STRONG_W_PARALLEL_TOOLS: f64 = 0.15;
    pub const STRONG_W_REASONING: f64 = 0.10;
    pub const STRONG_W_CONTEXT: f64 = 0.35;

    /// Reliability floor for the **weak** tier: enough context to hold a task
    /// plus its judgment output for a one-shot agency call.
    pub const WEAK_MIN_CONTEXT: u64 = 131_072;
    /// A candidate must be at least this much *cheaper* than the incumbent
    /// (blended cost) before the scout proposes a switch.
    pub const WEAK_SWITCH_CHEAPER: f64 = 0.90;

    /// Blended-cost weighting (input vs output) used to rank cost. Mirrors the
    /// convention in `model_benchmarks.rs` (work is output-heavy).
    pub const COST_INPUT_WEIGHT: f64 = 0.3;
    pub const COST_OUTPUT_WEIGHT: f64 = 0.7;

    /// Honest caveat surfaced in docs/help: `/api/v1/models` does not expose a
    /// coding benchmark, so the strong tier uses a *feature-gate + context
    /// quality proxy* with a capped log cost penalty (NOT price-as-capability).
    /// When a measured coding index becomes available (e.g. Artificial
    /// Analysis `coding_index`, already modeled in `model_benchmarks.rs`), it
    /// should override `quality_proxy`. The control flow does not change —
    /// only the score function in `score_strong` does. Scores are labelled as
    /// proxies in the rendered rationale.
    pub const STRONG_NOTE: &str = "strong uses a feature-gate + context quality proxy with a capped log cost \
         penalty (no coding benchmark in the OpenRouter API); scores are proxies. Plug in a \
         measured coding index to override the quality proxy.";
}

// ── Public entry points ─────────────────────────────────────────────────

/// Run the scout verb (`wg model-scout`).
///
/// Default is dry-run: research + propose + print the copy-pasteable apply
/// command. With `apply = true`, the proposed tier changes are written.
pub fn run(
    dir: &Path,
    apply: bool,
    no_cache: bool,
    max_cost: Option<f64>,
    json: bool,
) -> Result<()> {
    let proposal = scout(dir, no_cache, max_cost)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&proposal)?);
    }

    if apply {
        let written = apply_proposal(dir, &proposal)?;
        if !json {
            render(&proposal, false);
            print_apply_footer(dir, &proposal, &written);
        }
    } else if !json {
        render(&proposal, true);
        print_dryrun_footer(&proposal);
    }

    Ok(())
}

/// Pure research + selection. No side effects on config — this is the entry
/// point the design's `wg profile pi --scout` delegates to.
pub fn scout(dir: &Path, no_cache: bool, max_cost: Option<f64>) -> Result<Proposal> {
    let _ = no_cache; // reserved for a future on-disk cache; always fetch fresh for now.
    let api_key = openai_client::resolve_openai_api_key_from_dir(dir).context(
        "OpenRouter API key required to scout (set one via `wg secret` / config, or \
         OPENROUTER_API_KEY)",
    )?;
    let base_url = std::env::var("OPENAI_BASE_URL")
        .or_else(|_| std::env::var("OPENROUTER_BASE_URL"))
        .ok();

    let models = fetch_openrouter_models_blocking(&api_key, base_url.as_deref())
        .context("Failed to fetch OpenRouter model catalog")?;
    let fetched = models.len();

    let candidates: Vec<Candidate> = models.iter().filter_map(Candidate::from_model).collect();

    let config = Config::load_or_default(dir);
    let baseline = Baseline::from_config(&config);

    let strong = select_strong(&candidates, baseline.strong_id.as_deref(), max_cost);
    let weak = select_weak(&candidates, baseline.weak_id.as_deref(), max_cost);

    // Normalize the externally-visible specs to canonical handler-first form,
    // *after* selection + change detection ran in `openrouter:` space.
    //
    // - **strong** must execute through the self-authenticating `pi` handler,
    //   never the in-process nex OpenRouter client (which would require a
    //   wg-side key). Rewrite to a `pi:openrouter/<model>` route.
    // - **weak** keeps its native route per the two-tier design (§1.2a), but in
    //   canonical handler-first form — `nex:openrouter:<model>` — never a bare
    //   deprecated `openrouter:` spec.
    //
    // `old` is preserved *verbatim* from the baseline so the displayed
    // incumbent matches the config exactly (no spurious prefix churn); `new`
    // is the canonical form for the proposed model.
    let strong = finalize_tier(strong, baseline.strong_spec.as_deref(), pi_strong_route);
    let weak = finalize_tier(weak, baseline.weak_spec.as_deref(), native_weak_route);

    Ok(Proposal {
        fetched,
        max_cost,
        baseline_strong: baseline.strong_spec,
        baseline_weak: baseline.weak_spec,
        strong,
        weak,
    })
}

/// Rewrite a selector's `TierChange` to canonical handler-first specs, keeping
/// `changed` (computed in `openrouter:` space) intact.
///
/// `old` becomes the verbatim baseline spec (so the display matches the
/// config). `new` becomes `canonicalize(openrouter:<id>)` when the tier
/// changes, or the verbatim baseline when unchanged (so an unchanged tier
/// prints `old == new`).
fn finalize_tier(
    mut change: TierChange,
    baseline_spec: Option<&str>,
    canonicalize: fn(&str) -> String,
) -> TierChange {
    let old = baseline_spec.map(|s| s.to_string());
    if change.changed {
        // `new` is in `openrouter:<id>` space from the selector.
        change.new = canonicalize(&change.new);
    } else {
        // Unchanged: echo the verbatim incumbent so old == new in the display.
        change.new = baseline_spec
            .map(|s| s.to_string())
            .unwrap_or_else(|| canonicalize(&change.new));
    }
    change.old = old;
    change
}

/// Canonical handler-first native route for the **weak/agency** tier, per the
/// two-tier design §1.2a ("weak keeps its native openrouter route"). The bare
/// `openrouter:` prefix is a deprecated provider-only spec (handler-first
/// model-spec design); the canonical native form is `nex:openrouter:<model>`,
/// which routes to the in-process native OpenRouter handler and keeps the loud
/// keyless-native `claude:haiku` fallback in `resolve_agency_dispatch`.
pub fn native_weak_route(spec: &str) -> String {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return spec.to_string();
    }
    // Already handler-first native: leave verbatim.
    if trimmed.starts_with("nex:openrouter:") {
        return trimmed.to_string();
    }
    // `pi:openrouter:<model>` (colon) or `pi:openrouter/<model>` (slash):
    // the user may have weak set to a pi-routed spec; preserve its model id
    // but emit the canonical native route.
    if let Some(rest) = trimmed
        .strip_prefix("pi:openrouter:")
        .or_else(|| trimmed.strip_prefix("pi:openrouter/"))
    {
        return format!("nex:openrouter:{rest}");
    }
    // Bare `openrouter:<model>` / `openrouter/<model>` → canonical native.
    if let Some(rest) = trimmed
        .strip_prefix("openrouter:")
        .or_else(|| trimmed.strip_prefix("openrouter/"))
    {
        return format!("nex:openrouter:{rest}");
    }
    // Bare `vendor/model` (no provider prefix) → OpenRouter native.
    if !trimmed.contains(':') && trimmed.contains('/') {
        return format!("nex:openrouter:{trimmed}");
    }
    // Non-OpenRouter (claude:, codex:, nex:local, …): leave verbatim.
    trimmed.to_string()
}

// ── Data model ──────────────────────────────────────────────────────────

/// A scout proposal for both tiers. Serializable for `--json` and for the
/// scheduled task template.
#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    /// Number of models in the OpenRouter catalog at scout time.
    pub fetched: usize,
    /// Optional blended-cost ceiling (USD/1M tok) applied to both pools.
    pub max_cost: Option<f64>,
    /// Incumbent strong spec (verbatim from config), if any.
    pub baseline_strong: Option<String>,
    /// Incumbent weak spec (verbatim from config), if any.
    pub baseline_weak: Option<String>,
    pub strong: TierChange,
    pub weak: TierChange,
}

impl Proposal {
    /// Whether either tier would change.
    pub fn any_change(&self) -> bool {
        self.strong.changed || self.weak.changed
    }
}

/// The proposed (or no-op) change for a single tier.
#[derive(Debug, Clone, Serialize)]
pub struct TierChange {
    /// `"strong"` or `"weak"`.
    pub tier: &'static str,
    /// Incumbent spec (verbatim from config, e.g. `pi:openrouter:z-ai/glm-5.2`),
    /// if any.
    pub old: Option<String>,
    /// Proposed spec in canonical handler-first form (e.g.
    /// `pi:openrouter/z-ai/glm-5.2` for strong, `nex:openrouter:deepseek/...`
    /// for weak).
    pub new: String,
    /// True when the selected model id differs from the incumbent's model id.
    pub changed: bool,
    /// Human-readable justification (the text after "because …").
    pub reason: String,
    /// Ranked shortlist for this tier (best value first), with cost/quality
    /// rationale. Empty when no eligible candidate was found.
    #[serde(default)]
    pub shortlist: Vec<ShortlistEntry>,
}

/// One row of the ranked shortlist for a tier.
#[derive(Debug, Clone, Serialize)]
pub struct ShortlistEntry {
    /// Canonical handler-first spec for this candidate.
    pub spec: String,
    /// Free-form label, e.g. `"balanced/default"`, `"cheapest-good"`,
    /// `"frontier-expensive"` for strong; `"cheapest-reliable"` for weak.
    pub label: String,
    /// The selector's utility/score for this candidate (proxy — see
    /// [`criteria::STRONG_NOTE`]).
    pub score: f64,
    /// Blended cost (USD/1M tokens).
    pub blended_cost: f64,
    /// Context window, in tokens.
    pub context: u64,
    /// One-line feature summary, e.g. `"tools+structured, parallel, reasoning"`.
    pub features: String,
    /// Rationale for this row's ranking.
    pub rationale: String,
}

/// A parsed, scorable OpenRouter model.
#[derive(Debug, Clone)]
struct Candidate {
    id: String,
    context: u64,
    /// USD per 1M input tokens.
    in_per_mtok: f64,
    /// USD per 1M output tokens.
    out_per_mtok: f64,
    has_tools: bool,
    has_structured: bool,
    has_parallel_tools: bool,
    has_reasoning: bool,
    has_cache_control: bool,
    is_thinking: bool,
    /// Text-only modality (from `architecture.modality`), if known. `None`
    /// means the API did not report modality.
    is_text_modality: Option<bool>,
}

impl Candidate {
    fn from_model(m: &OpenRouterModel) -> Option<Candidate> {
        // Drop floating "latest" aliases (`~vendor/model`) — unstable identity.
        if m.id.starts_with('~') {
            return None;
        }
        let pricing = m.pricing.as_ref()?;
        let in_per_mtok = parse_price_per_mtok(pricing.prompt.as_deref());
        let out_per_mtok = parse_price_per_mtok(pricing.completion.as_deref());
        // Skip free/placeholder/zero-priced entries (rate-limited, unreliable).
        if out_per_mtok <= 0.0 {
            return None;
        }
        let sp = &m.supported_parameters;
        let has = |p: &str| sp.iter().any(|x| x == p);
        let id_l = m.id.to_lowercase();
        let is_text_modality = m
            .architecture
            .as_ref()
            .and_then(|a| a.modality.as_deref())
            .map(|modality| {
                let mo = modality.to_lowercase();
                mo == "text" || mo == "text->text" || mo.contains("text")
            });
        Some(Candidate {
            context: m.context_length.unwrap_or(0),
            in_per_mtok,
            out_per_mtok,
            has_tools: has("tools"),
            has_structured: has("structured_outputs"),
            has_parallel_tools: has("parallel_tool_calls"),
            has_reasoning: has("reasoning") || has("reasoning_effort"),
            has_cache_control: has("cache_control"),
            is_thinking: id_l.contains("thinking") || id_l.contains("-r1") || id_l.ends_with("/r1"),
            id: m.id.clone(),
            is_text_modality,
        })
    }

    /// Blended cost (USD per 1M tokens), output-weighted.
    fn blended_cost(&self) -> f64 {
        criteria::COST_INPUT_WEIGHT * self.in_per_mtok
            + criteria::COST_OUTPUT_WEIGHT * self.out_per_mtok
    }

    /// The canonical WG spec for this model (always OpenRouter-prefixed) —
    /// used internally by the selectors; `scout()` rewrites it to a
    /// handler-first route for display/apply.
    fn spec(&self) -> String {
        format!("openrouter:{}", self.id)
    }

    /// Exclude unstable / non-text / specialty entries by id substring and
    /// (when reported) modality.
    fn is_stable_text(&self) -> bool {
        let id = self.id.to_lowercase();
        const BAD: &[&str] = &[
            ":free",
            "preview",
            "experimental",
            "-alpha",
            "-beta",
            "-image",
            "image-",
            "-audio",
            "-tts",
            "-voice",
            "-vl-", // vision-language specialty
        ];
        if BAD.iter().any(|b| id.contains(b)) {
            return false;
        }
        // If the API reported a non-text modality, exclude it.
        if let Some(is_text) = self.is_text_modality {
            if !is_text {
                return false;
            }
        }
        true
    }

    fn features_label(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.has_tools {
            parts.push("tools");
        }
        if self.has_structured {
            parts.push("structured");
        }
        if self.has_parallel_tools {
            parts.push("parallel");
        }
        if self.has_reasoning {
            parts.push("reasoning");
        }
        if self.has_cache_control {
            parts.push("cache");
        }
        if parts.is_empty() {
            "no-tool-features".to_string()
        } else {
            parts.join("+")
        }
    }
}

// ── Strong selection: best-value coding/work model ───────────────────────

fn strong_eligible(c: &Candidate, max_cost: Option<f64>) -> bool {
    c.has_tools
        && c.has_structured
        && c.context >= criteria::STRONG_MIN_CONTEXT
        && c.is_stable_text()
        && max_cost.is_none_or(|ceiling| c.blended_cost() <= ceiling)
}

/// Feature-gate + context quality proxy (0..1). This is a *proxy* — the
/// OpenRouter API exposes no coding benchmark (see [`criteria::STRONG_NOTE`]).
fn quality_proxy_strong(c: &Candidate) -> f64 {
    let ctx = norm_log(
        c.context as f64,
        criteria::STRONG_MIN_CONTEXT as f64,
        1_048_576.0,
    );
    criteria::STRONG_W_TOOLS * f64::from(c.has_tools)
        + criteria::STRONG_W_STRUCTURED * f64::from(c.has_structured)
        + criteria::STRONG_W_PARALLEL_TOOLS * f64::from(c.has_parallel_tools)
        + criteria::STRONG_W_REASONING * f64::from(c.has_reasoning)
        + criteria::STRONG_W_CONTEXT * ctx
}

/// Capped logarithmic cost penalty (0..1). Cheap models barely penalized;
/// very expensive ones near-saturate — but a 2× price bump is never a 2×
/// penalty, so a frontier model can win *only* if its quality delta is large.
fn cost_penalty_strong(c: &Candidate) -> f64 {
    norm_log(
        c.blended_cost(),
        criteria::STRONG_COST_LO,
        criteria::STRONG_COST_HI,
    )
}

/// Value-aware utility for the strong tier (higher is better):
/// `quality_proxy − COST_WEIGHT × cost_penalty`.
fn score_strong(c: &Candidate) -> f64 {
    quality_proxy_strong(c) - criteria::STRONG_COST_WEIGHT * cost_penalty_strong(c)
}

fn select_strong(
    candidates: &[Candidate],
    incumbent_id: Option<&str>,
    max_cost: Option<f64>,
) -> TierChange {
    let mut pool: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| strong_eligible(c, max_cost))
        .collect();
    // Deterministic: best utility first, then cheaper, then id.
    pool.sort_by(|a, b| {
        score_strong(b)
            .partial_cmp(&score_strong(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                a.blended_cost()
                    .partial_cmp(&b.blended_cost())
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(a.id.cmp(&b.id))
    });

    let shortlist = strong_shortlist(&pool);

    let incumbent = incumbent_id.and_then(|id| candidates.iter().find(|c| c.id == id));
    let old_spec = incumbent_id.map(|id| format!("openrouter:{id}"));

    let Some(best) = pool.first().copied() else {
        // Nothing eligible — keep incumbent untouched.
        return TierChange {
            tier: "strong",
            old: old_spec.clone(),
            new: old_spec.unwrap_or_else(|| "(unset)".to_string()),
            changed: false,
            reason: "no OpenRouter model currently clears the strong eligibility floor \
                     (tools + structured outputs + large context); keeping incumbent."
                .to_string(),
            shortlist,
        };
    };

    match incumbent {
        // Incumbent is a real, scorable OpenRouter model.
        Some(inc) => {
            let inc_score = score_strong(inc);
            let best_score = score_strong(best);
            if inc.id == best.id || best_score <= inc_score * criteria::STRONG_SWITCH_MARGIN {
                TierChange {
                    tier: "strong",
                    old: old_spec.clone(),
                    new: inc.spec(),
                    changed: false,
                    reason: format!(
                        "still the best value (utility {:.2} = quality-proxy {:.2} − cost-penalty \
                         {:.2}; {}); no eligible candidate beats it by the {:.0}% switch margin.",
                        inc_score,
                        quality_proxy_strong(inc),
                        cost_penalty_strong(inc),
                        describe_strong(inc),
                        (criteria::STRONG_SWITCH_MARGIN - 1.0) * 100.0
                    ),
                    shortlist,
                }
            } else {
                TierChange {
                    tier: "strong",
                    old: old_spec,
                    new: best.spec(),
                    changed: true,
                    reason: format!(
                        "better value (utility {:.2} = quality-proxy {:.2} − cost-penalty {:.2}) \
                         vs incumbent utility {:.2}; {}.",
                        best_score,
                        quality_proxy_strong(best),
                        cost_penalty_strong(best),
                        inc_score,
                        describe_strong(best)
                    ),
                    shortlist,
                }
            }
        }
        // Incumbent unset or not an OpenRouter model (e.g. claude:/codex:): adopt best.
        None => TierChange {
            tier: "strong",
            old: old_spec.clone(),
            new: best.spec(),
            changed: old_spec.as_deref() != Some(&best.spec()),
            reason: format!(
                "{}; {}.",
                match &old_spec {
                    Some(s) => format!("incumbent ({s}) is not an OpenRouter model to compare"),
                    None => "no incumbent strong tier set".to_string(),
                },
                describe_strong(best)
            ),
            shortlist,
        },
    }
}

/// Build a small ranked shortlist for the strong tier: best-value,
/// cheapest-good, and frontier-expensive (when distinct).
fn strong_shortlist(pool: &[&Candidate]) -> Vec<ShortlistEntry> {
    if pool.is_empty() {
        return Vec::new();
    }
    let mut entries: Vec<ShortlistEntry> = Vec::new();
    let take = pool.iter().take(3);
    for (i, c) in take.enumerate() {
        let label = match i {
            0 => "balanced/default".to_string(),
            1 => "runner-up".to_string(),
            _ => "alt".to_string(),
        };
        entries.push(strong_entry(c, &label));
    }
    // Add the cheapest-good (lowest blended cost) if not already present.
    if let Some(cheapest) = pool.iter().min_by(|a, b| {
        a.blended_cost()
            .partial_cmp(&b.blended_cost())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.id.cmp(&b.id))
    }) {
        if !entries
            .iter()
            .any(|e| e.spec == pi_strong_route(&cheapest.spec()))
        {
            entries.push(strong_entry(cheapest, "cheapest-good"));
        }
    }
    // Add the frontier-expensive (highest blended cost) if not already present.
    if let Some(frontier) = pool.iter().max_by(|a, b| {
        a.blended_cost()
            .partial_cmp(&b.blended_cost())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.id.cmp(&b.id))
    }) {
        if !entries
            .iter()
            .any(|e| e.spec == pi_strong_route(&frontier.spec()))
        {
            entries.push(strong_entry(frontier, "frontier-expensive"));
        }
    }
    // De-dup by spec, keep first occurrence order.
    let mut seen: Vec<String> = Vec::new();
    entries.retain(|e| {
        if seen.iter().any(|s| s == &e.spec) {
            false
        } else {
            seen.push(e.spec.clone());
            true
        }
    });
    entries
}

fn strong_entry(c: &Candidate, label: &str) -> ShortlistEntry {
    let score = score_strong(c);
    ShortlistEntry {
        spec: pi_strong_route(&c.spec()),
        label: label.to_string(),
        score,
        blended_cost: c.blended_cost(),
        context: c.context,
        features: c.features_label(),
        rationale: format!(
            "quality-proxy {:.2}, cost-penalty {:.2}, utility {:.2}; ${:.3}/Mtok blended, {} ctx, {}",
            quality_proxy_strong(c),
            cost_penalty_strong(c),
            score,
            c.blended_cost(),
            human_ctx(c.context),
            c.features_label()
        ),
    }
}

fn describe_strong(c: &Candidate) -> String {
    format!(
        "${:.2}/Mtok blended, {} ctx, {}",
        c.blended_cost(),
        human_ctx(c.context),
        c.features_label()
    )
}

// ── Weak selection: cheapest *reliable* model for agency one-shots ───────
//
// This tier explicitly optimizes **reliability-at-low-cost**, NOT raw
// capability: agency calls (flip / assign / post-flip eval / off-the-rails)
// are recoverable one-shots where a wrong verdict is cheap to correct, so the
// scout minimizes cost subject to a reliability floor, with a small bonus for
// prompt-cache (`cache_control`) economics.

fn weak_eligible(c: &Candidate, max_cost: Option<f64>) -> bool {
    c.has_tools // structured tool/function calls land reliably
        && c.has_structured // agency verdicts are structured outputs
        && c.context >= criteria::WEAK_MIN_CONTEXT
        && c.is_stable_text() // no free/preview/specialty routes
        && !c.is_thinking // predictable cheap cost: no runaway reasoning per verdict
        && max_cost.is_none_or(|ceiling| c.blended_cost() <= ceiling)
}

/// Weak-tier ranking score: lower blended cost wins, with a small discount for
/// prompt-cache support (cache-read economics matter for repeated one-shots)
/// and a tie-break toward larger context. Returns a "cost-adjusted score"
/// where LOWER is better (kept in the same direction as the old sort for
/// clarity).
fn weak_rank_cost(c: &Candidate) -> f64 {
    let mut cost = c.blended_cost();
    if c.has_cache_control {
        // 5% effective discount for cache support — cache-read reuse drops the
        // amortized cost of repeated agency one-shots.
        cost *= 0.95;
    }
    cost
}

fn select_weak(
    candidates: &[Candidate],
    incumbent_id: Option<&str>,
    max_cost: Option<f64>,
) -> TierChange {
    let mut pool: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| weak_eligible(c, max_cost))
        .collect();
    // Cheapest (cache-adjusted) first; tie-break larger context, then id.
    pool.sort_by(|a, b| {
        weak_rank_cost(a)
            .partial_cmp(&weak_rank_cost(b))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.context.cmp(&a.context))
            .then(a.id.cmp(&b.id))
    });

    let shortlist = weak_shortlist(&pool);

    let incumbent = incumbent_id.and_then(|id| candidates.iter().find(|c| c.id == id));
    let old_spec = incumbent_id.map(|id| format!("openrouter:{id}"));

    let Some(best) = pool.first().copied() else {
        return TierChange {
            tier: "weak",
            old: old_spec.clone(),
            new: old_spec.unwrap_or_else(|| "(unset)".to_string()),
            changed: false,
            reason: "no OpenRouter model currently clears the agency reliability floor \
                     (tools + structured outputs + adequate context, stable non-free, \
                     non-thinking); keeping incumbent."
                .to_string(),
            shortlist,
        };
    };

    match incumbent {
        Some(inc) if weak_eligible(inc, max_cost) => {
            let inc_cost = weak_rank_cost(inc);
            let best_cost = weak_rank_cost(best);
            if inc.id == best.id || best_cost >= inc_cost * criteria::WEAK_SWITCH_CHEAPER {
                TierChange {
                    tier: "weak",
                    old: old_spec.clone(),
                    new: inc.spec(),
                    changed: false,
                    reason: format!(
                        "already cheap and reliable ({}); no eligible model is >{:.0}% cheaper \
                         while clearing the same floor.",
                        describe_weak(inc),
                        (1.0 - criteria::WEAK_SWITCH_CHEAPER) * 100.0
                    ),
                    shortlist,
                }
            } else {
                TierChange {
                    tier: "weak",
                    old: old_spec,
                    new: best.spec(),
                    changed: true,
                    reason: format!(
                        "clears the agency reliability floor (tools+structured, {} ctx, stable \
                         non-free, non-thinking) at ${:.3}/Mtok blended (cache-adjusted \
                         ${:.3}) vs ${:.3} — {:.0}% cheaper for flip/assign/eval/off-the-rails \
                         one-shots.",
                        human_ctx(best.context),
                        best.blended_cost(),
                        best_cost,
                        inc_cost,
                        (1.0 - best_cost / inc_cost) * 100.0
                    ),
                    shortlist,
                }
            }
        }
        // Incumbent missing, non-OpenRouter, or below the reliability floor: adopt best.
        other => {
            let why_incumbent = match other {
                Some(inc) => format!(
                    "incumbent ({}) no longer clears the reliability floor",
                    inc.spec()
                ),
                None => match &old_spec {
                    Some(s) => format!("incumbent ({s}) is not an OpenRouter model to compare"),
                    None => "no incumbent weak tier set".to_string(),
                },
            };
            TierChange {
                tier: "weak",
                old: old_spec.clone(),
                new: best.spec(),
                changed: old_spec.as_deref() != Some(&best.spec()),
                reason: format!(
                    "{why_incumbent}; cheapest reliable is {} (tools+structured, stable \
                     non-free, non-thinking).",
                    describe_weak(best)
                ),
                shortlist,
            }
        }
    }
}

fn weak_shortlist(pool: &[&Candidate]) -> Vec<ShortlistEntry> {
    pool.iter().take(3).map(|c| weak_entry(c)).collect()
}

fn weak_entry(c: &Candidate) -> ShortlistEntry {
    ShortlistEntry {
        spec: native_weak_route(&c.spec()),
        label: "cheapest-reliable".to_string(),
        score: weak_rank_cost(c),
        blended_cost: c.blended_cost(),
        context: c.context,
        features: c.features_label(),
        rationale: format!(
            "${:.3}/Mtok blended (cache-adjusted ${:.3}), {} ctx, {}; \
             tools+structured, stable non-free, non-thinking",
            c.blended_cost(),
            weak_rank_cost(c),
            human_ctx(c.context),
            c.features_label()
        ),
    }
}

fn describe_weak(c: &Candidate) -> String {
    format!(
        "${:.3}/Mtok blended, {} ctx, {}",
        c.blended_cost(),
        human_ctx(c.context),
        c.features_label()
    )
}

// ── Baseline bootstrap (read current tiers) ─────────────────────────────

struct Baseline {
    strong_spec: Option<String>,
    weak_spec: Option<String>,
    strong_id: Option<String>,
    weak_id: Option<String>,
}

impl Baseline {
    /// Read the incumbent tiers from config, per the design read-path:
    /// `strong ← agent.model` (fallback `[models.default]`),
    /// `weak ← tiers.fast` (fallback `[models.evaluator]`).
    fn from_config(config: &Config) -> Baseline {
        let strong_spec = first_non_empty([
            config.models.default.as_ref().and_then(|r| r.model.clone()),
            non_empty(&config.agent.model),
        ]);
        let weak_spec = first_non_empty([
            config.tiers.fast.clone(),
            config
                .models
                .evaluator
                .as_ref()
                .and_then(|r| r.model.clone()),
        ]);
        Baseline {
            strong_id: strong_spec.as_deref().and_then(openrouter_id_of),
            weak_id: weak_spec.as_deref().and_then(openrouter_id_of),
            strong_spec,
            weak_spec,
        }
    }
}

/// Extract the OpenRouter model id from a WG spec, if it is an OpenRouter
/// route. Handles every handler-first form in play:
/// - `openrouter:vendor/model` (bare, deprecated)
/// - `openrouter/vendor/model` (bare slash)
/// - `pi:openrouter:vendor/model` (canonical pi route, colon)
/// - `pi:openrouter/vendor/model` (pi route, slash — produced by `pi_strong_route`)
/// - `nex:openrouter:vendor/model` (canonical native route)
/// - `nex:openrouter/vendor/model`
/// - a bare `vendor/model` (no provider prefix)
///
/// Returns `None` for non-OpenRouter providers (`claude:`, `codex:`,
/// `nex:qwen3-coder`, `local:…`), which cannot be scored against the catalog.
fn openrouter_id_of(spec: &str) -> Option<String> {
    let spec = spec.trim();
    // Handler-first forms: strip the `<handler>:openrouter[:|/]` prefix.
    for handler in ["pi", "nex"] {
        let pfx_colon = format!("{handler}:openrouter:");
        if let Some(rest) = spec.strip_prefix(&pfx_colon) {
            return Some(rest.to_string());
        }
        let pfx_slash = format!("{handler}:openrouter/");
        if let Some(rest) = spec.strip_prefix(&pfx_slash) {
            return Some(rest.to_string());
        }
    }
    // Bare provider prefix (deprecated): `openrouter:vendor/model`.
    if let Some(rest) = spec.strip_prefix("openrouter:") {
        return Some(rest.to_string());
    }
    // Bare slash form: `openrouter/vendor/model`.
    if let Some(rest) = spec.strip_prefix("openrouter/") {
        return Some(rest.to_string());
    }
    // Bare `vendor/model` with no known provider prefix → treat as OpenRouter.
    if !spec.contains(':') && spec.contains('/') {
        return Some(spec.to_string());
    }
    None
}

// ── Rendering ───────────────────────────────────────────────────────────

fn render(p: &Proposal, dry_run: bool) {
    if dry_run {
        println!("DRY RUN — no files written.");
    }
    println!(
        "Scouting OpenRouter for the two Pi tiers ({} models fetched).",
        p.fetched
    );
    println!(
        "  baseline: strong={}, weak={}",
        p.baseline_strong.as_deref().unwrap_or("(unset)"),
        p.baseline_weak.as_deref().unwrap_or("(unset)")
    );
    if let Some(ceiling) = p.max_cost {
        println!("  cost ceiling: ${ceiling:.3}/Mtok blended");
    }
    println!();
    println!("Proposed Pi profile tiers");
    print_tier_line(&p.strong);
    print_shortlist(&p.strong);
    print_tier_line(&p.weak);
    print_shortlist(&p.weak);
}

/// The "always say what we are doing" line, in the exact required form:
/// `strong: <old> -> <new>  because …`.
fn print_tier_line(c: &TierChange) {
    let old = c.old.as_deref().unwrap_or("(unset)");
    let suffix = if c.changed { "" } else { "  (unchanged)" };
    println!("  {}: {} -> {}{}", c.tier, old, c.new, suffix);
    println!("          because {}", c.reason);
}

fn print_shortlist(c: &TierChange) {
    if c.shortlist.is_empty() {
        return;
    }
    println!("          shortlist (ranked, value-aware; scores are proxies):");
    for e in &c.shortlist {
        println!(
            "            • {:<18} {:<28} {:.2}  ${:.3}/Mtok  {} ctx  {} — {}",
            e.label,
            e.spec,
            e.score,
            e.blended_cost,
            human_ctx(e.context),
            e.features,
            e.rationale
        );
    }
}

/// Build the canonical apply command, using the partial-update flag form when
/// only one tier changes (the scout's common case), per design §7.
fn apply_command(p: &Proposal) -> String {
    let mut parts = vec!["wg profile pi".to_string()];
    if p.strong.changed {
        parts.push(format!("--strong {}", p.strong.new));
    }
    if p.weak.changed {
        parts.push(format!("--weak {}", p.weak.new));
    }
    parts.join(" ")
}

fn print_dryrun_footer(p: &Proposal) {
    println!();
    if p.any_change() {
        println!("Apply with:");
        println!("  wg model-scout --apply");
        println!(
            "  # canonical equivalent (per design-two-tier): {}",
            apply_command(p)
        );
        println!();
        println!(
            "(dry run — nothing written. Re-run with --apply to write; revert by re-running \
             the command with the old value.)"
        );
    } else {
        println!("(dry run — both tiers are already optimal; nothing to apply.)");
    }
    println!("note: {}", criteria::STRONG_NOTE);
}

fn print_apply_footer(dir: &Path, p: &Proposal, written: &[&str]) {
    println!();
    if written.is_empty() {
        println!("Both tiers already optimal — nothing written.");
        return;
    }
    println!(
        "Wrote {} tier(s) to {}.",
        written.join(" + "),
        dir.join("config.toml").display()
    );
    println!("Revert with: {}", revert_command(p, written));
    println!(
        "Next spawned worker uses the new tiers; reload or restart the daemon to pick them up."
    );
}

fn revert_command(p: &Proposal, written: &[&str]) -> String {
    let mut parts = vec!["wg profile pi".to_string()];
    if written.contains(&"strong")
        && let Some(old) = &p.strong.old
    {
        parts.push(format!("--strong {old}"));
    }
    if written.contains(&"weak")
        && let Some(old) = &p.weak.old
    {
        parts.push(format!("--weak {old}"));
    }
    parts.join(" ")
}

// ── Apply (write tiers) ─────────────────────────────────────────────────

/// Write the proposed tier changes into the config for `dir`, returning which
/// tiers were written. Only changed tiers are touched (partial update).
///
/// This writes the same key-set the design's `wg profile pi` setter owns
/// (`strong` → work/default/standard/premium keys via
/// [`Config::pin_default_route_model`]; `weak` → `tiers.fast` plus the four
/// agency-role overrides). Until the canonical `wg profile pi` setter lands,
/// this self-contained writer makes `--apply` real and testable; afterwards
/// `--apply` becomes a thin call into it.
fn apply_proposal<'a>(dir: &Path, p: &'a Proposal) -> Result<Vec<&'a str>> {
    if !p.any_change() {
        return Ok(vec![]);
    }
    let mut config = Config::load_or_default(dir);
    let mut written = Vec::new();

    if p.strong.changed {
        // Defensive (idempotent): persist the strong tier as a pi: route so it
        // runs through the self-authenticating pi handler even if a caller hands
        // us a proposal that did not pass through `scout()`'s normalization.
        config.pin_default_route_model(&pi_strong_route(&p.strong.new));
        written.push(p.strong.tier);
    }
    if p.weak.changed {
        // Persist weak as the canonical native route (nex:openrouter:<model>),
        // never a bare deprecated `openrouter:` spec.
        let weak_spec = native_weak_route(&p.weak.new);
        config.tiers.fast = Some(weak_spec.clone());
        let role = RoleModelConfig {
            provider: None,
            model: Some(weak_spec.clone()),
            tier: None,
            endpoint: None,
        };
        config.models.evaluator = Some(role.clone());
        config.models.assigner = Some(role.clone());
        config.models.flip_inference = Some(role.clone());
        config.models.flip_comparison = Some(role);
        written.push(p.weak.tier);
    }

    config
        .save(dir)
        .with_context(|| format!("Failed to write tiers to {}", dir.display()))?;
    Ok(written)
}

// ── Small numeric / string helpers ──────────────────────────────────────

/// Parse an OpenRouter per-token price string (USD/token) to USD per 1M tokens.
fn parse_price_per_mtok(raw: Option<&str>) -> f64 {
    raw.and_then(|s| s.parse::<f64>().ok())
        .map(|per_tok| per_tok * 1_000_000.0)
        .unwrap_or(0.0)
}

/// Normalize `x` to 0..1 on a log scale between `lo` and `hi` (clamped).
fn norm_log(x: f64, lo: f64, hi: f64) -> f64 {
    if x <= 0.0 || hi <= lo {
        return 0.0;
    }
    ((x.ln() - lo.ln()) / (hi.ln() - lo.ln())).clamp(0.0, 1.0)
}

fn human_ctx(ctx: u64) -> String {
    if ctx >= 1_000_000 {
        format!("{:.1}M", ctx as f64 / 1_048_576.0)
    } else if ctx >= 1_000 {
        format!("{}k", ctx / 1_024)
    } else {
        ctx.to_string()
    }
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn first_non_empty<const N: usize>(opts: [Option<String>; N]) -> Option<String> {
    opts.into_iter().flatten().find(|s| !s.trim().is_empty())
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::native::openai_client::{OpenRouterArchitecture, OpenRouterPricing};

    fn model(id: &str, ctx: u64, in_tok: &str, out_tok: &str, params: &[&str]) -> OpenRouterModel {
        OpenRouterModel {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            context_length: Some(ctx),
            pricing: Some(OpenRouterPricing {
                prompt: Some(in_tok.to_string()),
                completion: Some(out_tok.to_string()),
            }),
            supported_parameters: params.iter().map(|s| s.to_string()).collect(),
            architecture: None,
            top_provider: None,
        }
    }

    fn full_params() -> Vec<&'static str> {
        vec![
            "tools",
            "structured_outputs",
            "parallel_tool_calls",
            "reasoning",
        ]
    }

    /// A catalog where an expensive frontier model has the SAME feature gates
    /// as the cheaper incumbent (so quality_proxy ties) but a much higher
    /// output price. Value-aware scoring must keep the cheaper incumbent on
    /// top — this is the core `tune-model-scout-cost-quality` requirement.
    fn catalog() -> Vec<Candidate> {
        let p = full_params();
        vec![
            // frontier-priced, 1M ctx, same features as incumbent → loses on
            // value (cost penalty dominates the tied quality proxy).
            model("openai/gpt-5.5", 1_048_576, "0.00001", "0.00003", &p),
            // strong incumbent: much cheaper coder, 1M ctx, same features →
            // wins balanced/default on value.
            model("z-ai/glm-5.2", 1_048_576, "0.0000009", "0.000003", &p),
            // cheap + reliable (tools+structured only, no parallel/reasoning) →
            // wins weak on cost; lower quality_proxy than GLM so it does NOT
            // win strong on value.
            model(
                "vendor/cheap-flash",
                1_048_576,
                "0.00000009",
                "0.00000018",
                &["tools", "structured_outputs"],
            ),
            // weak incumbent: pricier but reliable (tools+structured only).
            model(
                "deepseek/deepseek-chat",
                1_048_576,
                "0.0000002",
                "0.0000008",
                &["tools", "structured_outputs"],
            ),
            // free route — excluded everywhere
            model("vendor/something:free", 1_048_576, "0", "0", &p),
            // thinking model — excluded from weak (runaway reasoning cost)
            model(
                "vendor/cheap-thinking",
                262_144,
                "0.00000001",
                "0.00000002",
                &["tools", "structured_outputs", "reasoning"],
            ),
            // no tools — excluded everywhere
            model(
                "vendor/no-tools",
                1_048_576,
                "0.0000001",
                "0.0000002",
                &["structured_outputs"],
            ),
        ]
        .iter()
        .filter_map(Candidate::from_model)
        .collect()
    }

    #[test]
    fn parses_pricing_to_per_mtok() {
        assert!((parse_price_per_mtok(Some("0.000005")) - 5.0).abs() < 1e-9);
        assert_eq!(parse_price_per_mtok(Some("0")), 0.0);
        assert_eq!(parse_price_per_mtok(None), 0.0);
    }

    #[test]
    fn openrouter_id_extraction_handles_all_spec_forms() {
        // Canonical pi route (colon) — the form actually stored in pi.toml.
        assert_eq!(
            openrouter_id_of("pi:openrouter:z-ai/glm-5.2").as_deref(),
            Some("z-ai/glm-5.2")
        );
        // pi route slash form (produced by `pi_strong_route`).
        assert_eq!(
            openrouter_id_of("pi:openrouter/z-ai/glm-5.2").as_deref(),
            Some("z-ai/glm-5.2")
        );
        // Canonical native route (nex:openrouter:) — weak tier.
        assert_eq!(
            openrouter_id_of("nex:openrouter:deepseek/deepseek-chat").as_deref(),
            Some("deepseek/deepseek-chat")
        );
        // nex slash form.
        assert_eq!(
            openrouter_id_of("nex:openrouter/deepseek/deepseek-chat").as_deref(),
            Some("deepseek/deepseek-chat")
        );
        // Bare deprecated openrouter: form.
        assert_eq!(
            openrouter_id_of("openrouter:z-ai/glm-5.2").as_deref(),
            Some("z-ai/glm-5.2")
        );
        // Bare vendor/model.
        assert_eq!(
            openrouter_id_of("vendor/model").as_deref(),
            Some("vendor/model")
        );
        // Non-OpenRouter providers are not scorable against the catalog.
        assert_eq!(openrouter_id_of("claude:opus"), None);
        assert_eq!(openrouter_id_of("codex:gpt-5.5"), None);
        assert_eq!(openrouter_id_of("nex:qwen3-coder"), None);
        assert_eq!(openrouter_id_of("local:qwen3-coder"), None);
    }

    #[test]
    fn free_and_zero_priced_models_are_dropped() {
        let cands = catalog();
        assert!(!cands.iter().any(|c| c.id.contains(":free")));
    }

    #[test]
    fn strong_keeps_glm_on_value_when_frontier_is_more_expensive() {
        // Core tune-model-scout-cost-quality requirement: an expensive frontier
        // model with the SAME feature gates as GLM 5.2 must NOT displace it —
        // value (quality minus capped cost penalty) keeps the cheaper model on
        // top. Raw frontier price is no longer a capability signal.
        let cands = catalog();
        let change = select_strong(&cands, Some("z-ai/glm-5.2"), None);
        assert!(
            !change.changed,
            "should NOT switch to the frontier model on value; reason: {}",
            change.reason
        );
        assert_eq!(change.new, "openrouter:z-ai/glm-5.2");
        // The shortlist should rank GLM above the frontier model.
        assert_eq!(
            change.shortlist.first().unwrap().spec,
            "pi:openrouter/z-ai/glm-5.2"
        );
        assert!(change.reason.contains("value"));
    }

    #[test]
    fn strong_adopts_best_value_when_incumbent_unset() {
        let cands = catalog();
        let change = select_strong(&cands, None, None);
        assert_eq!(change.new, "openrouter:z-ai/glm-5.2");
        assert!(
            change
                .shortlist
                .first()
                .unwrap()
                .spec
                .contains("z-ai/glm-5.2")
        );
    }

    #[test]
    fn strong_shortlist_includes_cheapest_good_and_frontier_expensive() {
        let cands = catalog();
        let change = select_strong(&cands, Some("z-ai/glm-5.2"), None);
        let labels: Vec<&str> = change.shortlist.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"balanced/default"));
        assert!(labels.contains(&"cheapest-good"));
        assert!(labels.contains(&"frontier-expensive"));
    }

    #[test]
    fn strong_keeps_incumbent_when_it_is_already_the_best() {
        let cands = catalog();
        let change = select_strong(&cands, Some("z-ai/glm-5.2"), None);
        assert!(!change.changed);
        assert_eq!(change.new, "openrouter:z-ai/glm-5.2");
    }

    #[test]
    fn strong_switches_when_a_model_clearly_beats_incumbent_on_value() {
        // A model with strictly more features (parallel_tool_calls that the
        // incumbent lacks) AND cheaper should switch — value delta is real.
        let p_inc = vec!["tools", "structured_outputs", "reasoning"];
        let p_best = vec![
            "tools",
            "structured_outputs",
            "parallel_tool_calls",
            "reasoning",
        ];
        let models = vec![
            model(
                "vendor/incumbent",
                1_048_576,
                "0.000002",
                "0.000005",
                &p_inc,
            ),
            model("vendor/better", 1_048_576, "0.000001", "0.000002", &p_best),
        ];
        let cands: Vec<Candidate> = models.iter().filter_map(Candidate::from_model).collect();
        let change = select_strong(&cands, Some("vendor/incumbent"), None);
        assert!(change.changed, "should switch to the better-value model");
        assert_eq!(change.new, "openrouter:vendor/better");
    }

    #[test]
    fn weak_picks_cheapest_reliable_not_most_capable() {
        let cands = catalog();
        let change = select_weak(&cands, Some("deepseek/deepseek-chat"), None);
        assert!(
            change.changed,
            "should switch to the cheaper reliable model"
        );
        assert_eq!(change.new, "openrouter:vendor/cheap-flash");
        assert!(change.reason.contains("cheaper"));
    }

    #[test]
    fn weak_excludes_thinking_models() {
        let cands = catalog();
        // cheap-thinking is cheaper than cheap-flash but must be excluded.
        let change = select_weak(&cands, None, None);
        assert_eq!(change.new, "openrouter:vendor/cheap-flash");
        assert_ne!(change.new, "openrouter:vendor/cheap-thinking");
    }

    #[test]
    fn max_cost_ceiling_filters_strong_pool() {
        let cands = catalog();
        // Ceiling below the frontier model's blended cost → keep cheaper incumbent.
        let change = select_strong(&cands, Some("z-ai/glm-5.2"), Some(5.0));
        assert!(!change.changed);
        assert_eq!(change.new, "openrouter:z-ai/glm-5.2");
    }

    #[test]
    fn apply_command_uses_partial_form_for_single_tier() {
        let p = Proposal {
            fetched: 1,
            max_cost: None,
            baseline_strong: Some("pi:openrouter:vendor/frontier".into()),
            baseline_weak: Some("nex:openrouter:vendor/incumbent-weak".into()),
            strong: TierChange {
                tier: "strong",
                old: Some("pi:openrouter:vendor/frontier".into()),
                new: "pi:openrouter:vendor/frontier".into(),
                changed: false,
                reason: "x".into(),
                shortlist: Vec::new(),
            },
            weak: TierChange {
                tier: "weak",
                old: Some("nex:openrouter:vendor/incumbent-weak".into()),
                new: "nex:openrouter:vendor/cheap-flash".into(),
                changed: true,
                reason: "y".into(),
                shortlist: Vec::new(),
            },
        };
        assert_eq!(
            apply_command(&p),
            "wg profile pi --weak nex:openrouter:vendor/cheap-flash"
        );
    }

    #[test]
    fn apply_proposal_persists_strong_as_pi_and_weak_as_native_route() {
        // The scout's selectors run in `openrouter:` space; `scout()`
        // canonicalizes strong → `pi:openrouter/<model>` (self-authenticating
        // pi handler) and weak → `nex:openrouter:<model>` (canonical native,
        // never a bare deprecated `openrouter:` spec). `apply_proposal`
        // re-applies those canonicalizations defensively.
        let tmp = tempfile::tempdir().unwrap();
        let p = Proposal {
            fetched: 1,
            max_cost: None,
            baseline_strong: None,
            baseline_weak: None,
            strong: TierChange {
                tier: "strong",
                old: None,
                new: "pi:openrouter/z-ai/glm-5.2".into(),
                changed: true,
                reason: "x".into(),
                shortlist: Vec::new(),
            },
            weak: TierChange {
                tier: "weak",
                old: None,
                new: "nex:openrouter:deepseek/deepseek-chat".into(),
                changed: true,
                reason: "y".into(),
                shortlist: Vec::new(),
            },
        };
        apply_proposal(tmp.path(), &p).unwrap();
        let content = std::fs::read_to_string(tmp.path().join("config.toml")).unwrap();
        let cfg: Config = toml::from_str(&content).unwrap();
        // Strong routes to the pi handler (self-authenticating).
        assert_eq!(cfg.agent.model, "pi:openrouter/z-ai/glm-5.2");
        assert_eq!(
            cfg.tiers.standard.as_deref(),
            Some("pi:openrouter/z-ai/glm-5.2")
        );
        assert_eq!(
            cfg.tiers.premium.as_deref(),
            Some("pi:openrouter/z-ai/glm-5.2")
        );
        // Weak keeps its native route, in canonical handler-first form (no bare
        // `openrouter:`).
        assert_eq!(
            cfg.tiers.fast.as_deref(),
            Some("nex:openrouter:deepseek/deepseek-chat")
        );
        assert_eq!(
            cfg.models.evaluator.as_ref().unwrap().model.as_deref(),
            Some("nex:openrouter:deepseek/deepseek-chat")
        );
        assert!(content.contains("nex:openrouter:deepseek/deepseek-chat"));
        assert!(!content.contains("= \"openrouter:"));
        // The persisted strong spec routes to the pi handler.
        assert_eq!(
            crate::dispatch::handler_for_model(&cfg.agent.model),
            crate::dispatch::ExecutorKind::Pi
        );
    }

    #[test]
    fn scout_normalization_keeps_select_strong_in_openrouter_space() {
        // The pi: normalization happens in `scout()`/`finalize_tier`, NOT in
        // `select_strong`, so the selector's change detection stays in
        // `openrouter:` space. A pi-form incumbent and an openrouter-form best
        // for the SAME model must therefore compare equal (no spurious
        // "changed").
        let cands = catalog();
        let change = select_strong(&cands, Some("z-ai/glm-5.2"), None);
        assert!(!change.changed);
        assert_eq!(change.new, "openrouter:z-ai/glm-5.2");
        // …and the public-facing normalization turns it into a pi: route.
        assert_eq!(pi_strong_route(&change.new), "pi:openrouter/z-ai/glm-5.2");
    }

    #[test]
    fn finalize_tier_preserves_verbatim_old_and_canonicalizes_new() {
        // When unchanged, old == new == verbatim baseline (no spurious prefix
        // churn in the display). When changed, old is the verbatim incumbent
        // and new is canonicalized.
        let unchanged = TierChange {
            tier: "weak",
            old: Some("openrouter:deepseek/deepseek-chat".into()),
            new: "openrouter:deepseek/deepseek-chat".into(),
            changed: false,
            reason: "x".into(),
            shortlist: Vec::new(),
        };
        let out = finalize_tier(
            unchanged,
            Some("pi:openrouter:deepseek/deepseek-chat"),
            native_weak_route,
        );
        assert_eq!(
            out.old.as_deref(),
            Some("pi:openrouter:deepseek/deepseek-chat")
        );
        // Unchanged → echo verbatim baseline (NOT re-canonicalized to nex:).
        assert_eq!(out.new, "pi:openrouter:deepseek/deepseek-chat");

        let changed = TierChange {
            tier: "weak",
            old: Some("openrouter:deepseek/deepseek-chat".into()),
            new: "openrouter:vendor/cheap-flash".into(),
            changed: true,
            reason: "y".into(),
            shortlist: Vec::new(),
        };
        let out = finalize_tier(
            changed,
            Some("pi:openrouter:deepseek/deepseek-chat"),
            native_weak_route,
        );
        assert_eq!(
            out.old.as_deref(),
            Some("pi:openrouter:deepseek/deepseek-chat")
        );
        assert_eq!(out.new, "nex:openrouter:vendor/cheap-flash");
    }

    #[test]
    fn native_weak_route_emits_canonical_nex_no_bare_openrouter() {
        assert_eq!(
            native_weak_route("openrouter:deepseek/deepseek-chat"),
            "nex:openrouter:deepseek/deepseek-chat"
        );
        assert_eq!(
            native_weak_route("openrouter/deepseek/deepseek-chat"),
            "nex:openrouter:deepseek/deepseek-chat"
        );
        assert_eq!(
            native_weak_route("pi:openrouter:deepseek/deepseek-chat"),
            "nex:openrouter:deepseek/deepseek-chat"
        );
        assert_eq!(
            native_weak_route("pi:openrouter/deepseek/deepseek-chat"),
            "nex:openrouter:deepseek/deepseek-chat"
        );
        // Already canonical → idempotent.
        assert_eq!(
            native_weak_route("nex:openrouter:deepseek/deepseek-chat"),
            "nex:openrouter:deepseek/deepseek-chat"
        );
        // Bare vendor/model → canonical native.
        assert_eq!(
            native_weak_route("deepseek/deepseek-chat"),
            "nex:openrouter:deepseek/deepseek-chat"
        );
        // Non-OpenRouter → verbatim.
        assert_eq!(native_weak_route("claude:haiku"), "claude:haiku");
        assert_eq!(native_weak_route(""), "");
    }

    #[test]
    fn architecture_modality_excludes_non_text() {
        let mut m = model(
            "vendor/voice",
            1_048_576,
            "0.0000001",
            "0.0000002",
            &full_params(),
        );
        m.architecture = Some(OpenRouterArchitecture {
            modality: Some("text+image->text".to_string()),
            tokenizer: None,
        });
        // text+image->text contains "text" → still eligible (text-capable).
        let c = Candidate::from_model(&m).unwrap();
        assert!(c.is_stable_text());
        let mut m2 = m.clone();
        m2.architecture = Some(OpenRouterArchitecture {
            modality: Some("image->image".to_string()),
            tokenizer: None,
        });
        let c2 = Candidate::from_model(&m2).unwrap();
        assert!(!c2.is_stable_text(), "non-text modality must be excluded");
    }

    #[test]
    fn weak_cache_control_discounts_ranking() {
        // Two equally-cheap eligible models; the one with cache_control ranks
        // first (cache-adjusted cost is 5% lower).
        let p_cache = vec!["tools", "structured_outputs", "cache_control"];
        let p_no = vec!["tools", "structured_outputs"];
        let models = vec![
            model(
                "vendor/with-cache",
                1_048_576,
                "0.0000002",
                "0.0000008",
                &p_cache,
            ),
            model(
                "vendor/no-cache",
                1_048_576,
                "0.0000002",
                "0.0000008",
                &p_no,
            ),
        ];
        let cands: Vec<Candidate> = models.iter().filter_map(Candidate::from_model).collect();
        let change = select_weak(&cands, None, None);
        assert_eq!(change.new, "openrouter:vendor/with-cache");
    }
}
