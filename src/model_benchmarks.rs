//! Model benchmark registry with fitness scoring.
//!
//! Stores benchmark data, pricing, and computed fitness scores fetched from
//! the OpenRouter API. Lives in `.workgraph/model_benchmarks.json` as a
//! machine-managed sidecar to the static `models.yaml` catalog.
//!
//! See `docs/plans/model-registry-and-update-trace.md` for the full design.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The benchmark registry file name.
pub const BENCHMARKS_FILE: &str = "model_benchmarks.json";

// ── Schema types ────────────────────────────────────────────────────────

/// Top-level benchmark registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkRegistry {
    /// Schema version (currently 1).
    pub version: u32,
    /// ISO 8601 timestamp of when data was last fetched.
    pub fetched_at: String,
    /// Data sources used.
    pub source: RegistrySource,
    /// Per-model benchmark data, keyed by OpenRouter model ID.
    pub models: BTreeMap<String, ModelBenchmark>,
}

/// Data source URLs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrySource {
    pub openrouter_api: String,
}

/// Benchmark + fitness data for a single model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelBenchmark {
    /// OpenRouter model ID (e.g. "anthropic/claude-opus-4-6").
    pub id: String,
    /// Human-readable name.
    pub name: String,

    /// Pricing per million tokens (USD).
    pub pricing: BenchmarkPricing,

    /// Architecture info.
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
    #[serde(default)]
    pub supports_tools: bool,

    /// Benchmarks (mostly null until AA integration).
    #[serde(default)]
    pub benchmarks: Benchmarks,

    /// Popularity signals from OpenRouter.
    #[serde(default)]
    pub popularity: Popularity,

    /// Computed fitness.
    #[serde(default)]
    pub fitness: Fitness,

    /// Tier classification (frontier / mid / budget).
    pub tier: String,

    /// When pricing was last updated.
    pub pricing_updated_at: String,
}

/// Per-million-token pricing in USD.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_per_mtok: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_per_mtok: Option<f64>,
}

/// Benchmark scores (nullable — populated by Artificial Analysis or similar).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Benchmarks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intelligence_index: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coding_index: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub math_index: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agentic: Option<f64>,
}

/// Popularity signals.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Popularity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_count: Option<u32>,
}

/// Computed fitness score and components.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Fitness {
    /// Composite score (0–100), null if no benchmarks available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(default)]
    pub components: FitnessComponents,
}

/// Individual fitness components.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FitnessComponents {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reliability: Option<f64>,
}

// ── Loading / Saving ────────────────────────────────────────────────────

impl BenchmarkRegistry {
    /// Load the benchmark registry from `.workgraph/model_benchmarks.json`.
    /// Returns `None` if the file doesn't exist.
    pub fn load(workgraph_dir: &Path) -> Result<Option<Self>> {
        let path = workgraph_dir.join(BENCHMARKS_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let registry: Self = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path.display()))?;
        Ok(Some(registry))
    }

    /// Save the benchmark registry to `.workgraph/model_benchmarks.json`.
    pub fn save(&self, workgraph_dir: &Path) -> Result<()> {
        let path = workgraph_dir.join(BENCHMARKS_FILE);
        let content = serde_json::to_string_pretty(self)
            .context("Failed to serialize benchmark registry")?;
        std::fs::write(&path, content)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    }

    /// Models sorted by fitness score descending (unscored models last).
    pub fn ranked(&self) -> Vec<&ModelBenchmark> {
        let mut models: Vec<&ModelBenchmark> = self.models.values().collect();
        models.sort_by(|a, b| {
            let sa = a.fitness.score.unwrap_or(f64::NEG_INFINITY);
            let sb = b.fitness.score.unwrap_or(f64::NEG_INFINITY);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        models
    }

    /// Models filtered by tier, sorted by fitness.
    pub fn ranked_by_tier(&self, tier: &str) -> Vec<&ModelBenchmark> {
        let mut models: Vec<&ModelBenchmark> = self
            .models
            .values()
            .filter(|m| m.tier == tier)
            .collect();
        models.sort_by(|a, b| {
            let sa = a.fitness.score.unwrap_or(f64::NEG_INFINITY);
            let sb = b.fitness.score.unwrap_or(f64::NEG_INFINITY);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        models
    }
}

// ── Fitness Scoring ─────────────────────────────────────────────────────

/// Compute fitness scores for all models in the registry.
///
/// Follows the formula from the design doc:
///   fitness = quality * 0.70 + value * 0.20 + reliability * 0.10
pub fn compute_fitness_scores(registry: &mut BenchmarkRegistry) {
    // First pass: compute quality scores and collect cost factors for median.
    let mut cost_factors: Vec<f64> = Vec::new();
    let mut quality_scores: BTreeMap<String, Option<f64>> = BTreeMap::new();

    for (id, model) in &registry.models {
        let quality = compute_quality(&model.benchmarks);
        quality_scores.insert(id.clone(), quality);

        let cost = model.pricing.input_per_mtok * 0.3 + model.pricing.output_per_mtok * 0.7;
        if cost > 0.0 {
            cost_factors.push(cost);
        }
    }

    let median_cost = median(&cost_factors).unwrap_or(1.0);

    // Second pass: compute full fitness.
    for (id, model) in registry.models.iter_mut() {
        let quality = quality_scores.get(id).copied().flatten();

        // Value: quality / cost_factor, normalized to 0–100.
        let raw_cost =
            model.pricing.input_per_mtok * 0.3 + model.pricing.output_per_mtok * 0.7;
        let cost_factor = if median_cost > 0.0 && raw_cost > 0.0 {
            raw_cost / median_cost
        } else {
            1.0
        };
        let value = quality.map(|q| (q / cost_factor).min(100.0));

        // Reliability: provider_count signal + base availability.
        let provider_signal = model
            .popularity
            .provider_count
            .map(|pc| (pc as f64 / 5.0).min(1.0) * 50.0)
            .unwrap_or(0.0);
        // Without request_count data from OpenRouter, we use a simplified reliability.
        let reliability = provider_signal;

        // Composite.
        let score = quality.map(|q| {
            let v = value.unwrap_or(0.0);
            q * 0.70 + v * 0.20 + reliability * 0.10
        });

        model.fitness = Fitness {
            score,
            components: FitnessComponents {
                quality,
                value,
                reliability: Some(reliability),
            },
        };

        // Only reclassify tier when we have benchmark data; otherwise
        // keep the pricing-based tier from build_from_openrouter.
        if model.fitness.score.is_some() {
            model.tier = classify_tier(&model.benchmarks, model.fitness.score);
        }
    }
}

/// Compute the quality component from benchmark scores.
///
/// quality = coding_index * 0.50 + intelligence_index * 0.30 + agentic * 0.20
fn compute_quality(benchmarks: &Benchmarks) -> Option<f64> {
    let coding = benchmarks.coding_index.or_else(|| {
        benchmarks.intelligence_index.map(|ii| ii * 0.9)
    });
    let intelligence = benchmarks.intelligence_index.or_else(|| {
        benchmarks.coding_index.map(|ci| (ci * 1.1).min(100.0))
    });

    match (coding, intelligence, benchmarks.agentic) {
        (Some(c), Some(i), Some(a)) => Some(c * 0.50 + i * 0.30 + a * 0.20),
        (Some(c), Some(i), None) => {
            // Redistribute agentic weight: 55% coding, 45% intelligence.
            Some(c * 0.55 + i * 0.45)
        }
        (None, None, _) => None,
        (Some(c), None, Some(a)) => Some(c * 0.70 + a * 0.30),
        (None, Some(i), Some(a)) => Some(i * 0.60 + a * 0.40),
        (Some(c), None, None) => Some(c),
        (None, Some(i), None) => Some(i),
    }
}

/// Classify a model into a tier based on benchmarks and fitness.
fn classify_tier(benchmarks: &Benchmarks, fitness_score: Option<f64>) -> String {
    let coding = benchmarks.coding_index.unwrap_or(0.0);
    let intelligence = benchmarks.intelligence_index.unwrap_or(0.0);
    let fitness = fitness_score.unwrap_or(0.0);

    if fitness >= 65.0 || (coding >= 48.0 && intelligence >= 50.0) {
        "frontier".to_string()
    } else if fitness >= 40.0 || coding >= 35.0 {
        "mid".to_string()
    } else {
        "budget".to_string()
    }
}

/// Compute the median of a slice.
fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        Some((sorted[mid - 1] + sorted[mid]) / 2.0)
    } else {
        Some(sorted[mid])
    }
}

// ── Build from OpenRouter data ──────────────────────────────────────────

use crate::executor::native::openai_client::OpenRouterModel;

/// Build a `BenchmarkRegistry` from OpenRouter API model data.
///
/// This populates pricing, architecture, and tool support fields.
/// Benchmark scores remain null until enriched by Artificial Analysis data.
pub fn build_from_openrouter(models: &[OpenRouterModel]) -> BenchmarkRegistry {
    let now = chrono::Utc::now().to_rfc3339();
    let mut entries = BTreeMap::new();

    for model in models {
        let pricing = parse_or_pricing(model);

        // Skip models with no pricing data (typically deprecated or placeholder entries).
        if pricing.input_per_mtok <= 0.0 && pricing.output_per_mtok <= 0.0 {
            continue;
        }

        let context_window = model.context_length;
        let max_output_tokens = model
            .top_provider
            .as_ref()
            .and_then(|tp| tp.max_completion_tokens);
        let supports_tools = model.supported_parameters.iter().any(|p| p == "tools");

        let entry = ModelBenchmark {
            id: model.id.clone(),
            name: model.name.clone(),
            pricing,
            context_window,
            max_output_tokens,
            supports_tools,
            benchmarks: Benchmarks::default(),
            popularity: Popularity {
                provider_count: None,
            },
            fitness: Fitness::default(),
            tier: "budget".to_string(), // Will be reclassified after scoring.
            pricing_updated_at: now.clone(),
        };

        entries.insert(model.id.clone(), entry);
    }

    let mut registry = BenchmarkRegistry {
        version: 1,
        fetched_at: now,
        source: RegistrySource {
            openrouter_api: "https://openrouter.ai/api/v1/models".to_string(),
        },
        models: entries,
    };

    // Classify tiers based on pricing heuristics (no benchmark data yet).
    classify_tiers_from_pricing(&mut registry);

    registry
}

/// Classify tiers heuristically from pricing when no benchmark data is available.
///
/// Uses cost as a proxy for capability: expensive models tend to be frontier.
fn classify_tiers_from_pricing(registry: &mut BenchmarkRegistry) {
    // Compute median output cost.
    let costs: Vec<f64> = registry
        .models
        .values()
        .map(|m| m.pricing.output_per_mtok)
        .filter(|c| *c > 0.0)
        .collect();
    let median_out = median(&costs).unwrap_or(1.0);

    for model in registry.models.values_mut() {
        let out = model.pricing.output_per_mtok;
        model.tier = if out >= median_out * 3.0 {
            "frontier".to_string()
        } else if out >= median_out * 0.8 {
            "mid".to_string()
        } else {
            "budget".to_string()
        };
    }
}

/// Parse OpenRouter pricing strings to per-million-token USD values.
fn parse_or_pricing(model: &OpenRouterModel) -> BenchmarkPricing {
    let pricing = match &model.pricing {
        Some(p) => p,
        None => {
            return BenchmarkPricing {
                input_per_mtok: 0.0,
                output_per_mtok: 0.0,
                cache_read_per_mtok: None,
                cache_write_per_mtok: None,
            }
        }
    };

    let input = pricing
        .prompt
        .as_deref()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|per_tok| per_tok * 1_000_000.0)
        .unwrap_or(0.0);

    let output = pricing
        .completion
        .as_deref()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|per_tok| per_tok * 1_000_000.0)
        .unwrap_or(0.0);

    BenchmarkPricing {
        input_per_mtok: input,
        output_per_mtok: output,
        cache_read_per_mtok: None,
        cache_write_per_mtok: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_quality_all_present() {
        let b = Benchmarks {
            coding_index: Some(50.0),
            intelligence_index: Some(50.0),
            agentic: Some(50.0),
            math_index: None,
        };
        let q = compute_quality(&b).unwrap();
        // 50*0.5 + 50*0.3 + 50*0.2 = 25 + 15 + 10 = 50
        assert!((q - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_compute_quality_no_agentic() {
        let b = Benchmarks {
            coding_index: Some(60.0),
            intelligence_index: Some(40.0),
            agentic: None,
            math_index: None,
        };
        let q = compute_quality(&b).unwrap();
        // 60*0.55 + 40*0.45 = 33 + 18 = 51
        assert!((q - 51.0).abs() < 0.01);
    }

    #[test]
    fn test_compute_quality_missing_coding() {
        let b = Benchmarks {
            coding_index: None,
            intelligence_index: Some(50.0),
            agentic: Some(60.0),
            math_index: None,
        };
        let q = compute_quality(&b).unwrap();
        // coding proxied from intelligence: 50*0.9 = 45
        // 45*0.5 + 50*0.3 + 60*0.2 = 22.5 + 15 + 12 = 49.5
        assert!((q - 49.5).abs() < 0.01);
    }

    #[test]
    fn test_compute_quality_all_missing() {
        let b = Benchmarks::default();
        assert!(compute_quality(&b).is_none());
    }

    #[test]
    fn test_classify_tier_frontier() {
        let b = Benchmarks {
            coding_index: Some(50.0),
            intelligence_index: Some(52.0),
            ..Default::default()
        };
        assert_eq!(classify_tier(&b, Some(70.0)), "frontier");
    }

    #[test]
    fn test_classify_tier_mid() {
        let b = Benchmarks {
            coding_index: Some(36.0),
            ..Default::default()
        };
        assert_eq!(classify_tier(&b, Some(42.0)), "mid");
    }

    #[test]
    fn test_classify_tier_budget() {
        let b = Benchmarks::default();
        assert_eq!(classify_tier(&b, Some(20.0)), "budget");
    }

    #[test]
    fn test_median() {
        assert_eq!(median(&[1.0, 2.0, 3.0]), Some(2.0));
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), Some(2.5));
        assert!(median(&[]).is_none());
    }

    #[test]
    fn test_fitness_scoring_round_trip() {
        let mut registry = BenchmarkRegistry {
            version: 1,
            fetched_at: "2026-04-01T00:00:00Z".to_string(),
            source: RegistrySource {
                openrouter_api: "https://openrouter.ai/api/v1/models".to_string(),
            },
            models: BTreeMap::new(),
        };

        registry.models.insert(
            "test/model-a".to_string(),
            ModelBenchmark {
                id: "test/model-a".to_string(),
                name: "Model A".to_string(),
                pricing: BenchmarkPricing {
                    input_per_mtok: 3.0,
                    output_per_mtok: 15.0,
                    cache_read_per_mtok: None,
                    cache_write_per_mtok: None,
                },
                context_window: Some(200_000),
                max_output_tokens: Some(32_000),
                supports_tools: true,
                benchmarks: Benchmarks {
                    coding_index: Some(50.0),
                    intelligence_index: Some(50.0),
                    agentic: Some(60.0),
                    math_index: None,
                },
                popularity: Popularity {
                    provider_count: Some(3),
                },
                fitness: Fitness::default(),
                tier: "mid".to_string(),
                pricing_updated_at: "2026-04-01T00:00:00Z".to_string(),
            },
        );

        compute_fitness_scores(&mut registry);

        let model = registry.models.get("test/model-a").unwrap();
        assert!(model.fitness.score.is_some());
        assert!(model.fitness.score.unwrap() > 0.0);
        assert!(model.fitness.components.quality.is_some());
        assert!(model.fitness.components.value.is_some());
        assert!(model.fitness.components.reliability.is_some());
    }

    #[test]
    fn test_registry_save_load() {
        let dir = tempfile::TempDir::new().unwrap();
        let registry = BenchmarkRegistry {
            version: 1,
            fetched_at: "2026-04-01T00:00:00Z".to_string(),
            source: RegistrySource {
                openrouter_api: "https://openrouter.ai/api/v1/models".to_string(),
            },
            models: BTreeMap::new(),
        };
        registry.save(dir.path()).unwrap();
        let loaded = BenchmarkRegistry::load(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.models.len(), 0);
    }

    #[test]
    fn test_registry_load_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = BenchmarkRegistry::load(dir.path()).unwrap();
        assert!(result.is_none());
    }
}
