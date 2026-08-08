use std::path::Path;

use anyhow::Result;
use worksgood::graph::Status;

/// Daily spend summary entry.
#[derive(Debug)]
pub struct DailySpend {
    pub date: String,
    pub total_cost: f64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub task_count: usize,
}

/// Run the spend command — show token usage and cost summaries.
pub fn run(dir: &Path, today_only: bool, json: bool) -> Result<()> {
    let (graph, _path) = super::load_workgraph(dir)?;

    let mut daily_spend: std::collections::BTreeMap<String, DailySpend> =
        std::collections::BTreeMap::new();
    let mut total_cost = 0.0;
    let mut total_input_tokens = 0u64;
    let mut total_output_tokens = 0u64;
    let mut tasks_with_usage = 0usize;
    let mut review_cost = 0.0;
    let mut review_input_tokens = 0u64;
    let mut review_output_tokens = 0u64;
    let mut review_attempts_with_usage = 0usize;

    // Source-worker totals and internal review-lane totals are deliberately
    // separate. A review call is not charged to the source task, and repeated
    // Pi snapshots never enter either projection.
    for task in graph.tasks() {
        for activity in &task.completion_review_activity {
            if let Some(usage) = activity.usage.as_ref() {
                review_attempts_with_usage += 1;
                review_cost += usage.cost_usd;
                review_input_tokens += usage.input_tokens;
                review_output_tokens += usage.output_tokens;
            }
        }
        for record in &task.evaluation_records {
            for attempt in &record.attempts {
                if let Some(usage) = attempt.usage.as_ref() {
                    review_attempts_with_usage += 1;
                    review_cost += usage.cost_usd;
                    review_input_tokens += usage.input_tokens;
                    review_output_tokens += usage.output_tokens;
                }
            }
        }
    }

    // Only count completed source tasks that have token usage
    for task in graph.tasks() {
        if task.status != Status::Done && task.status != Status::Failed {
            continue;
        }
        let Some(usage) = &task.token_usage else {
            continue;
        };

        tasks_with_usage += 1;
        total_cost += usage.cost_usd;
        total_input_tokens += usage.input_tokens;
        total_output_tokens += usage.output_tokens;

        // Use today's date for grouping (simple approach)
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

        let entry = daily_spend
            .entry(today.clone())
            .or_insert_with(|| DailySpend {
                date: today,
                total_cost: 0.0,
                total_input_tokens: 0,
                total_output_tokens: 0,
                task_count: 0,
            });
        entry.total_cost += usage.cost_usd;
        entry.total_input_tokens += usage.input_tokens;
        entry.total_output_tokens += usage.output_tokens;
        entry.task_count += 1;
    }

    if json {
        let days: Vec<_> = daily_spend
            .values()
            .map(|d| {
                serde_json::json!({
                    "date": d.date,
                    "total_cost": d.total_cost,
                    "total_input_tokens": d.total_input_tokens,
                    "total_output_tokens": d.total_output_tokens,
                    "task_count": d.task_count,
                })
            })
            .collect();

        let summary = if today_only {
            daily_spend
                .into_iter()
                .next_back()
                .map(|(date, d)| {
                    serde_json::json!({
                        "date": date,
                        "total_cost": d.total_cost,
                        "total_input_tokens": d.total_input_tokens,
                        "total_output_tokens": d.total_output_tokens,
                        "task_count": d.task_count,
                        "accounting_scope": "source-workers-only",
                        "completion_review_lane": {
                            "total_cost": review_cost,
                            "total_input_tokens": review_input_tokens,
                            "total_output_tokens": review_output_tokens,
                            "attempt_count": review_attempts_with_usage,
                            "accounting_scope": "internal-review-calls-only-not-task-usage"
                        }
                    })
                })
                .unwrap_or(serde_json::json!({
                    "date": "today",
                    "total_cost": 0.0,
                    "total_input_tokens": 0,
                    "total_output_tokens": 0,
                    "task_count": 0,
                    "accounting_scope": "source-workers-only",
                    "completion_review_lane": {
                        "total_cost": review_cost,
                        "total_input_tokens": review_input_tokens,
                        "total_output_tokens": review_output_tokens,
                        "attempt_count": review_attempts_with_usage,
                        "accounting_scope": "internal-review-calls-only-not-task-usage"
                    }
                }))
        } else {
            serde_json::json!({
                "total_cost": total_cost,
                "total_input_tokens": total_input_tokens,
                "total_output_tokens": total_output_tokens,
                "task_count": tasks_with_usage,
                "daily_breakdown": days,
                "accounting_scope": "source-workers-only",
                "completion_review_lane": {
                    "total_cost": review_cost,
                    "total_input_tokens": review_input_tokens,
                    "total_output_tokens": review_output_tokens,
                    "attempt_count": review_attempts_with_usage,
                    "accounting_scope": "internal-review-calls-only-not-task-usage"
                }
            })
        };
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else if today_only {
        // Show just today's spend
        if let Some((_date, spend)) = daily_spend.into_iter().next_back() {
            println!("Today's spend:");
            println!("  Total cost: ${:.4}", spend.total_cost);
            println!(
                "  Input tokens: {}",
                format_number(spend.total_input_tokens)
            );
            println!(
                "  Output tokens: {}",
                format_number(spend.total_output_tokens)
            );
            println!("  Tasks: {}", spend.task_count);
            println!(
                "  Internal review lane (separate): ${:.4}, {} tokens, {} attempts",
                review_cost,
                format_number(review_input_tokens + review_output_tokens),
                review_attempts_with_usage
            );
        } else {
            println!("No source-task token usage recorded yet today.");
            if review_attempts_with_usage > 0 {
                println!(
                    "Internal review lane (separate): ${:.4}, {} tokens, {} attempts",
                    review_cost,
                    format_number(review_input_tokens + review_output_tokens),
                    review_attempts_with_usage
                );
            }
        }
    } else {
        // Show full summary
        println!("=== Token Spend Summary ===");
        println!("Total cost: ${:.4}", total_cost);
        println!(
            "Total tokens: {} ({} in, {} out)",
            format_number(total_input_tokens + total_output_tokens),
            format_number(total_input_tokens),
            format_number(total_output_tokens)
        );
        println!("Tasks with usage: {}", tasks_with_usage);
        println!();
        println!("Internal completion-review lane (separate; not included above):");
        println!(
            "  Cost: ${:.4}; tokens: {} ({} in, {} out); attempts with usage: {}",
            review_cost,
            format_number(review_input_tokens + review_output_tokens),
            format_number(review_input_tokens),
            format_number(review_output_tokens),
            review_attempts_with_usage
        );
        println!();
        println!("Daily breakdown:");

        for (date, spend) in &daily_spend {
            println!(
                "  {}: ${:.4} ({} tasks, {} tokens)",
                date,
                spend.total_cost,
                spend.task_count,
                format_number(spend.total_input_tokens + spend.total_output_tokens)
            );
        }
    }

    Ok(())
}

/// Format a number with thousands separators.
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.insert(0, ',');
        }
        result.insert(0, c);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use worksgood::graph::{Node, Task, WorkGraph};
    use worksgood::parser::save_graph;

    fn setup_workgraph(dir: &Path, tasks: Vec<Task>) {
        let path = dir.join("graph.jsonl");
        let mut graph = WorkGraph::new();
        for task in tasks {
            graph.add_node(Node::Task(task));
        }
        save_graph(&graph, &path).unwrap();
    }

    #[test]
    fn test_format_number() {
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(1000000), "1,000,000");
        assert_eq!(format_number(42), "42");
    }

    #[test]
    fn test_spend_no_tasks() {
        let dir = TempDir::new().unwrap();
        setup_workgraph(dir.path(), vec![]);

        let result = run(dir.path(), false, false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_spend_with_usage() {
        let dir = TempDir::new().unwrap();
        let mut task = Task {
            id: "test-1".to_string(),
            title: "Test".to_string(),
            status: Status::Done,
            ..Default::default()
        };
        task.token_usage = Some(worksgood::graph::TokenUsage {
            cost_usd: 0.50,
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
        });

        setup_workgraph(dir.path(), vec![task]);
        let result = run(dir.path(), false, false);
        assert!(result.is_ok());
    }
}
