//! Re-runnable OpenRouter strong/weak tier scout.
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
//!    - **strong** = best coding/work model available right now.
//!    - **weak**   = cheapest model that is *reliable enough* for agency
//!      judgment one-shots (flip / assign / post-flip evaluation /
//!      off-the-rails detection) — reliability-at-low-cost, **not** raw
//!      capability.
//! 4. **Always says what it is doing**, emitting one line per tier in the form
//!    `strong: <old> -> <new>  because …` / `weak: <old> -> <new>  because …`.
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
    /// Frontier price band (USD per 1M *output* tokens) used as a capability
    /// proxy for the strong tier. Output price below `LO` reads as a
    /// budget/utility model; at/above `HI` the capability signal saturates so
    /// the scout does not simply chase the single most expensive "pro" tier.
    pub const STRONG_PRICE_LO: f64 = 1.0;
    pub const STRONG_PRICE_HI: f64 = 30.0;
    /// A candidate must beat the incumbent's strong score by this *relative*
    /// margin before the scout proposes a switch (anti-churn / "baseline to
    /// beat").
    pub const STRONG_SWITCH_MARGIN: f64 = 1.05;

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

    /// Capability-proxy score weights for the strong tier.
    pub const STRONG_W_PRICE: f64 = 0.55;
    pub const STRONG_W_CONTEXT: f64 = 0.25;
    pub const STRONG_W_PARALLEL_TOOLS: f64 = 0.10;
    pub const STRONG_W_REASONING: f64 = 0.10;

    /// Honest caveat surfaced in docs/help: `/api/v1/models` does not expose a
    /// coding benchmark, so the strong tier uses a *spec + frontier-price
    /// proxy*. When a measured coding index becomes available (e.g. Artificial
    /// Analysis `coding_index`, already modeled in `model_benchmarks.rs`), it
    /// should dominate this proxy. The control flow does not change — only the
    /// score function in `score_strong` does.
    pub const STRONG_NOTE: &str = "strong uses a spec + frontier-price capability proxy (no coding benchmark in the \
         OpenRouter API); plug in a measured coding index to override.";
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

    let mut strong = select_strong(&candidates, baseline.strong_id.as_deref(), max_cost);
    // The strong tier must execute through the self-authenticating pi handler,
    // never the in-process nex OpenRouter client (which would require a wg-side
    // key). Rewrite the proposed/echoed/persisted strong spec to a `pi:` route.
    // Selection + change detection above ran in `openrouter:` space and are
    // unaffected; this only rewrites the externally-visible spec, keeping the
    // dry-run display, the copy-pasteable apply command, and the `--apply` write
    // all consistent and pi-routed.
    strong.new = pi_strong_route(&strong.new);
    strong.old = strong.old.as_deref().map(pi_strong_route);
    let weak = select_weak(&candidates, baseline.weak_id.as_deref(), max_cost);

    Ok(Proposal {
        fetched,
        max_cost,
        baseline_strong: baseline.strong_spec,
        baseline_weak: baseline.weak_spec,
        strong,
        weak,
    })
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
    /// Incumbent spec (e.g. `openrouter:z-ai/glm-5.2`), if any.
    pub old: Option<String>,
    /// Proposed spec (e.g. `openrouter:deepseek/deepseek-v4-flash`).
    pub new: String,
    /// True when `new` differs from `old`.
    pub changed: bool,
    /// Human-readable justification (the text after "because …").
    pub reason: String,
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
    is_thinking: bool,
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
        Some(Candidate {
            context: m.context_length.unwrap_or(0),
            in_per_mtok,
            out_per_mtok,
            has_tools: has("tools"),
            has_structured: has("structured_outputs"),
            has_parallel_tools: has("parallel_tool_calls"),
            has_reasoning: has("reasoning") || has("reasoning_effort"),
            is_thinking: id_l.contains("thinking") || id_l.contains("-r1") || id_l.ends_with("/r1"),
            id: m.id.clone(),
        })
    }

    /// Blended cost (USD per 1M tokens), output-weighted.
    fn blended_cost(&self) -> f64 {
        criteria::COST_INPUT_WEIGHT * self.in_per_mtok
            + criteria::COST_OUTPUT_WEIGHT * self.out_per_mtok
    }

    /// The canonical WG spec for this model (always OpenRouter-prefixed).
    fn spec(&self) -> String {
        format!("openrouter:{}", self.id)
    }

    /// Exclude unstable / non-text / specialty entries by id substring.
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
        !BAD.iter().any(|b| id.contains(b))
    }
}

// ── Strong selection: best coding/work model ────────────────────────────

fn strong_eligible(c: &Candidate, max_cost: Option<f64>) -> bool {
    c.has_tools
        && c.has_structured
        && c.context >= criteria::STRONG_MIN_CONTEXT
        && c.is_stable_text()
        && max_cost.is_none_or(|ceiling| c.blended_cost() <= ceiling)
}

/// Capability-proxy score for the strong tier (higher is better). See
/// [`criteria::STRONG_NOTE`] for the honest caveat about this being a proxy.
fn score_strong(c: &Candidate) -> f64 {
    let price = norm_log(
        c.out_per_mtok,
        criteria::STRONG_PRICE_LO,
        criteria::STRONG_PRICE_HI,
    );
    let ctx = norm_log(
        c.context as f64,
        criteria::STRONG_MIN_CONTEXT as f64,
        1_048_576.0,
    );
    criteria::STRONG_W_PRICE * price
        + criteria::STRONG_W_CONTEXT * ctx
        + criteria::STRONG_W_PARALLEL_TOOLS * f64::from(c.has_parallel_tools)
        + criteria::STRONG_W_REASONING * f64::from(c.has_reasoning)
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
    // Deterministic: best score first, then cheaper, then id.
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
                        "still at the capability frontier (score {:.2}; {}); no eligible \
                         candidate beats it by the {:.0}% switch margin.",
                        inc_score,
                        describe_strong(inc),
                        (criteria::STRONG_SWITCH_MARGIN - 1.0) * 100.0
                    ),
                }
            } else {
                TierChange {
                    tier: "strong",
                    old: old_spec,
                    new: best.spec(),
                    changed: true,
                    reason: format!(
                        "higher capability proxy (score {:.2} vs incumbent {:.2}); {}.",
                        best_score,
                        inc_score,
                        describe_strong(best)
                    ),
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
        },
    }
}

fn describe_strong(c: &Candidate) -> String {
    format!(
        "tools+structured, {} ctx, ${:.2}/Mtok out (frontier band)",
        human_ctx(c.context),
        c.out_per_mtok
    )
}

// ── Weak selection: cheapest *reliable* model for agency one-shots ───────
//
// This tier explicitly optimizes **reliability-at-low-cost**, NOT raw
// capability: agency calls (flip / assign / post-flip eval / off-the-rails)
// are recoverable one-shots where a wrong verdict is cheap to correct, so the
// scout minimizes cost subject to a reliability floor.

fn weak_eligible(c: &Candidate, max_cost: Option<f64>) -> bool {
    c.has_tools                         // structured tool/function calls land reliably
        && c.has_structured             // agency verdicts are structured outputs
        && c.context >= criteria::WEAK_MIN_CONTEXT
        && c.is_stable_text()           // no free/preview/specialty routes
        && !c.is_thinking               // predictable cheap cost: no runaway reasoning per verdict
        && max_cost.is_none_or(|ceiling| c.blended_cost() <= ceiling)
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
    // Cheapest first; tie-break larger context, then id for determinism.
    pool.sort_by(|a, b| {
        a.blended_cost()
            .partial_cmp(&b.blended_cost())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.context.cmp(&a.context))
            .then(a.id.cmp(&b.id))
    });

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
        };
    };

    match incumbent {
        Some(inc) if weak_eligible(inc, max_cost) => {
            let inc_cost = inc.blended_cost();
            let best_cost = best.blended_cost();
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
                }
            } else {
                TierChange {
                    tier: "weak",
                    old: old_spec,
                    new: best.spec(),
                    changed: true,
                    reason: format!(
                        "clears the agency reliability floor (tools+structured, {} ctx, stable \
                         non-free, non-thinking) at ${:.3}/Mtok blended vs ${:.3} — {:.0}% \
                         cheaper for flip/assign/eval/off-the-rails one-shots.",
                        human_ctx(best.context),
                        best_cost,
                        inc_cost,
                        (1.0 - best_cost / inc_cost) * 100.0
                    ),
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
            }
        }
    }
}

fn describe_weak(c: &Candidate) -> String {
    format!(
        "${:.3}/Mtok blended, {} ctx",
        c.blended_cost(),
        human_ctx(c.context)
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
/// route. Handles `openrouter:vendor/model`, `pi:openrouter/vendor/model`, and
/// a bare `vendor/model`. Returns `None` for non-OpenRouter providers
/// (`claude:`, `codex:`, `nex:`…), which cannot be scored against the catalog.
fn openrouter_id_of(spec: &str) -> Option<String> {
    let spec = spec.trim();
    if let Some(rest) = spec.strip_prefix("openrouter:") {
        return Some(rest.to_string());
    }
    if let Some(rest) = spec.strip_prefix("pi:openrouter/") {
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
    print_tier_line(&p.weak);
}

/// The "always say what we are doing" line, in the exact required form:
/// `strong: <old> -> <new>  because …`.
fn print_tier_line(c: &TierChange) {
    let old = c.old.as_deref().unwrap_or("(unset)");
    let suffix = if c.changed { "" } else { "  (unchanged)" };
    println!("  {}: {} -> {}{}", c.tier, old, c.new, suffix);
    println!("          because {}", c.reason);
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
        config.tiers.fast = Some(p.weak.new.clone());
        let role = RoleModelConfig {
            provider: None,
            model: Some(p.weak.new.clone()),
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
    use crate::executor::native::openai_client::OpenRouterPricing;

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

    fn catalog() -> Vec<Candidate> {
        let p = full_params();
        vec![
            // frontier-priced, 1M ctx → should win strong
            model("vendor/frontier", 1_048_576, "0.000005", "0.000025", &p),
            // strong incumbent: cheaper coder, 1M ctx
            model(
                "vendor/incumbent-strong",
                1_048_576,
                "0.0000009",
                "0.000003",
                &p,
            ),
            // cheap + reliable → should win weak
            model(
                "vendor/cheap-flash",
                1_048_576,
                "0.00000009",
                "0.00000018",
                &p,
            ),
            // weak incumbent: pricier but reliable
            model(
                "vendor/incumbent-weak",
                163_840,
                "0.0000002",
                "0.0000008",
                &p,
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
        assert_eq!(
            openrouter_id_of("openrouter:z-ai/glm-5.2").as_deref(),
            Some("z-ai/glm-5.2")
        );
        assert_eq!(
            openrouter_id_of("pi:openrouter/deepseek/deepseek-chat").as_deref(),
            Some("deepseek/deepseek-chat")
        );
        assert_eq!(
            openrouter_id_of("vendor/model").as_deref(),
            Some("vendor/model")
        );
        // Non-OpenRouter providers are not scorable against the catalog.
        assert_eq!(openrouter_id_of("claude:opus"), None);
        assert_eq!(openrouter_id_of("codex:gpt-5.5"), None);
    }

    #[test]
    fn free_and_zero_priced_models_are_dropped() {
        let cands = catalog();
        assert!(!cands.iter().any(|c| c.id.contains(":free")));
    }

    #[test]
    fn strong_prefers_frontier_over_cheaper_incumbent() {
        let cands = catalog();
        let change = select_strong(&cands, Some("vendor/incumbent-strong"), None);
        assert!(change.changed, "should switch to the frontier model");
        assert_eq!(change.new, "openrouter:vendor/frontier");
        assert!(change.reason.contains("capability"));
    }

    #[test]
    fn strong_keeps_incumbent_when_it_is_already_the_best() {
        let cands = catalog();
        let change = select_strong(&cands, Some("vendor/frontier"), None);
        assert!(!change.changed);
        assert_eq!(change.new, "openrouter:vendor/frontier");
    }

    #[test]
    fn weak_picks_cheapest_reliable_not_most_capable() {
        let cands = catalog();
        let change = select_weak(&cands, Some("vendor/incumbent-weak"), None);
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
        let change = select_strong(&cands, Some("vendor/incumbent-strong"), Some(5.0));
        assert!(!change.changed);
        assert_eq!(change.new, "openrouter:vendor/incumbent-strong");
    }

    #[test]
    fn apply_command_uses_partial_form_for_single_tier() {
        let p = Proposal {
            fetched: 1,
            max_cost: None,
            baseline_strong: Some("openrouter:vendor/frontier".into()),
            baseline_weak: Some("openrouter:vendor/incumbent-weak".into()),
            strong: TierChange {
                tier: "strong",
                old: Some("openrouter:vendor/frontier".into()),
                new: "openrouter:vendor/frontier".into(),
                changed: false,
                reason: "x".into(),
            },
            weak: TierChange {
                tier: "weak",
                old: Some("openrouter:vendor/incumbent-weak".into()),
                new: "openrouter:vendor/cheap-flash".into(),
                changed: true,
                reason: "y".into(),
            },
        };
        assert_eq!(
            apply_command(&p),
            "wg profile pi --weak openrouter:vendor/cheap-flash"
        );
    }

    #[test]
    fn apply_proposal_persists_strong_as_pi_route_not_nex() {
        // The scout's strong proposal is a raw `openrouter:` spec; persisting it
        // verbatim would route strong-tier work through the in-process nex
        // OpenRouter client and require a wg-side key. `apply_proposal` must
        // write a `pi:` route so strong work runs through the self-authenticating
        // pi handler instead. The weak/agency tier keeps its native route.
        let tmp = tempfile::tempdir().unwrap();
        let p = Proposal {
            fetched: 1,
            max_cost: None,
            baseline_strong: None,
            baseline_weak: None,
            strong: TierChange {
                tier: "strong",
                old: None,
                new: "openrouter:z-ai/glm-5.2".into(),
                changed: true,
                reason: "x".into(),
            },
            weak: TierChange {
                tier: "weak",
                old: None,
                new: "openrouter:deepseek/deepseek-chat".into(),
                changed: true,
                reason: "y".into(),
            },
        };
        apply_proposal(tmp.path(), &p).unwrap();
        // Read the written local config directly (no global merge).
        let content = std::fs::read_to_string(tmp.path().join("config.toml")).unwrap();
        let cfg: Config = toml::from_str(&content).unwrap();
        assert_eq!(cfg.agent.model, "pi:openrouter/z-ai/glm-5.2");
        assert_eq!(
            cfg.tiers.standard.as_deref(),
            Some("pi:openrouter/z-ai/glm-5.2")
        );
        assert_eq!(
            cfg.tiers.premium.as_deref(),
            Some("pi:openrouter/z-ai/glm-5.2")
        );
        // Weak/agency tier untouched: still the native openrouter: route.
        assert_eq!(
            cfg.tiers.fast.as_deref(),
            Some("openrouter:deepseek/deepseek-chat")
        );
        // The persisted strong spec routes to the pi handler.
        assert_eq!(
            crate::dispatch::handler_for_model(&cfg.agent.model),
            crate::dispatch::ExecutorKind::Pi
        );
    }

    #[test]
    fn scout_normalization_keeps_select_strong_in_openrouter_space() {
        // The pi: normalization happens in `scout()`, NOT in `select_strong`,
        // so the selector's change detection stays in openrouter: space. A
        // pi-form incumbent and an openrouter-form best for the SAME model must
        // therefore compare equal (no spurious "changed").
        let cands = catalog();
        let change = select_strong(&cands, Some("vendor/frontier"), None);
        assert!(!change.changed);
        assert_eq!(change.new, "openrouter:vendor/frontier");
        // …and the public-facing normalization turns it into a pi: route.
        assert_eq!(
            pi_strong_route(&change.new),
            "pi:openrouter/vendor/frontier"
        );
    }
}
