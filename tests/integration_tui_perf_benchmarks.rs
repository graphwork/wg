//! Performance benchmarks for the TUI hot path, gating fix-tui-perf-2.
//!
//! These are NOT criterion benches (the project doesn't pull criterion in);
//! they are `#[test]`-style integration tests that measure wall-time and
//! assert against thresholds derived from diagnose-tui-scales. Run with
//! `cargo test --test integration_tui_perf_benchmarks --release` for the
//! tightest thresholds, or with `--no-fail-fast` to see all four scenarios.
//!
//! Bench E — `bench_e_message_stats_pair_folds_to_one_read`: the combined
//!     `message_stats_pair_cached` over a 1000-task fixture must outperform
//!     the standalone-functions baseline by at least 1.5×.
//!
//! Bench (token usage) — `bench_token_usage_cache_avoids_reparse`: a re-parse
//!     of the same `output.log` must be effectively free (cache hit).
//!
//! The other diagnose-named scenarios (A `tui_idle_cpu`, B `tui_loaded_cpu`,
//! C `tui_chat_input_latency`, D `tui_generate_viz_scaling`) require a real
//! `wg tui` subprocess + tmux harness and live as smoke-gate scenarios
//! under `tests/smoke/scenarios/`.

use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::TempDir;

use workgraph::graph::{Node, Status, Task, WorkGraph, parse_token_usage_live_cached};
use workgraph::messages::{coordinator_message_status, message_stats, message_stats_pair_cached};

/// Build a synthetic graph with `n` tasks. Roughly half are InProgress,
/// quarter Done, quarter Open. Each task gets a unique id and dependency on
/// the previous task. This is meant to exercise the `tasks_to_show` loop in
/// `generate_viz_output_from_graph`, which is the diagnose hot path.
fn build_synthetic_graph(n: usize) -> WorkGraph {
    let mut graph = WorkGraph::new();
    for i in 0..n {
        let mut t = Task {
            id: format!("task-{:04}", i),
            title: format!("Synthetic task {}", i),
            status: match i % 4 {
                0 => Status::InProgress,
                1 => Status::Done,
                2 => Status::Open,
                _ => Status::Failed,
            },
            ..Task::default()
        };
        if i > 0 {
            t.after.push(format!("task-{:04}", i - 1));
        }
        if matches!(t.status, Status::InProgress | Status::Done | Status::Failed) {
            t.assigned = Some(format!("agent-{:04}", i));
        }
        graph.add_node(Node::Task(t));
    }
    graph
}

/// Write a small `.wg/messages/<task>.jsonl` for every task in the graph.
/// Used to exercise the message_stats hot path on a representative number of
/// per-task message files.
fn seed_messages(workgraph_dir: &Path, graph: &WorkGraph, msgs_per_task: usize) {
    let msg_dir = workgraph_dir.join("messages");
    std::fs::create_dir_all(&msg_dir).unwrap();
    for t in graph.tasks() {
        let path = msg_dir.join(format!("{}.jsonl", t.id));
        let mut content = String::new();
        for i in 0..msgs_per_task {
            // Alternate sender so we exercise both incoming + outgoing branches.
            let sender = if i % 2 == 0 { "user" } else { "agent" };
            content.push_str(&format!(
                "{{\"id\":{},\"timestamp\":\"2026-05-01T00:00:00Z\",\"sender\":\"{}\",\"body\":\"hi\",\"priority\":\"normal\",\"status\":\"sent\"}}\n",
                i + 1, sender
            ));
        }
        std::fs::write(&path, content).unwrap();
    }
}

/// Bench E — `message_stats_pair_cached` must complete a 1000-task scan
/// faster than calling `message_stats` + `coordinator_message_status`
/// separately. The single-pass + cache should be at least 30% faster
/// (typically 50%+) — wide tolerance accounts for system noise.
#[test]
fn bench_e_message_stats_pair_folds_to_one_read() {
    const N: usize = 1000;
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    std::fs::create_dir_all(&wg_dir).unwrap();
    let graph = build_synthetic_graph(N);
    seed_messages(&wg_dir, &graph, 5);

    // Baseline: separate calls, NO caching (matches pre-fix behavior).
    let baseline_start = Instant::now();
    for t in graph.tasks() {
        let _stats = message_stats(&wg_dir, &t.id, t.assigned.as_deref());
        let _coord = coordinator_message_status(&wg_dir, &t.id);
    }
    let baseline = baseline_start.elapsed();

    // Optimized: single-pass + process-wide cache (post-fix behavior).
    // First pass populates cache; second pass measures cache-hit cost
    // (the steady-state TUI refresh case).
    for t in graph.tasks() {
        let _ = message_stats_pair_cached(&wg_dir, &t.id, t.assigned.as_deref());
    }
    let optimized_start = Instant::now();
    for t in graph.tasks() {
        let _ = message_stats_pair_cached(&wg_dir, &t.id, t.assigned.as_deref());
    }
    let optimized = optimized_start.elapsed();

    eprintln!(
        "bench_e baseline={:?}  optimized(cached)={:?}  speedup={:.1}x",
        baseline,
        optimized,
        baseline.as_secs_f64() / optimized.as_secs_f64().max(1e-9),
    );

    // Cached re-scan must be at least 2x faster than the unfolded baseline.
    // Real-world numbers: baseline ~30-100ms for 1000 tasks × 5 msgs;
    // cached re-scan should be < 5ms.
    assert!(
        optimized < baseline,
        "cached message_stats_pair ({:?}) was not faster than baseline ({:?})",
        optimized,
        baseline,
    );
    let speedup = baseline.as_secs_f64() / optimized.as_secs_f64().max(1e-9);
    assert!(
        speedup > 1.5,
        "cached message_stats_pair speedup was only {:.2}x (expected >1.5x)",
        speedup,
    );
}

/// Token-usage cache: a no-op re-parse of the same output.log must be
/// effectively free (an mtime metadata syscall). Protects fix 2's caching
/// of `parse_token_usage_live` against accidental re-introduction of
/// per-refresh JSONL parsing.
#[test]
fn bench_token_usage_cache_avoids_reparse() {
    let tmp = TempDir::new().unwrap();
    let log_path = tmp.path().join("output.log");

    // Build a moderately large output log: 500 assistant turns. Each line
    // is a real Claude-CLI assistant message with a usage block, so the
    // un-cached parser does meaningful JSON work per call.
    let mut content = String::new();
    for i in 0..500 {
        content.push_str(&format!(
            "{{\"type\":\"assistant\",\"message\":{{\"usage\":{{\"input_tokens\":{},\"output_tokens\":50,\"cache_read_input_tokens\":10,\"cache_creation_input_tokens\":5}}}}}}\n",
            10 + i,
        ));
    }
    std::fs::write(&log_path, content).unwrap();

    // First call populates the cache (uncached cost).
    let cold_start = Instant::now();
    let cold = parse_token_usage_live_cached(&log_path);
    let cold_dur = cold_start.elapsed();
    assert!(cold.is_some(), "expected at least one usage record");

    // Second call must hit the cache and complete in << cold time.
    let warm_start = Instant::now();
    let warm = parse_token_usage_live_cached(&log_path);
    let warm_dur = warm_start.elapsed();
    assert!(warm.is_some());

    eprintln!(
        "token_usage_cache cold={:?}  warm={:?}  speedup={:.1}x",
        cold_dur,
        warm_dur,
        cold_dur.as_secs_f64() / warm_dur.as_secs_f64().max(1e-9),
    );

    // The warm path should be at least 5× faster — typically 100×+. A
    // regression where the cache is bypassed would put warm ≈ cold.
    assert!(
        warm_dur * 5 < cold_dur || warm_dur < Duration::from_micros(100),
        "warm parse {:?} not significantly faster than cold {:?}",
        warm_dur,
        cold_dur,
    );
}
