use std::path::Path;

use anyhow::Result;
use workgraph::graph::Status;

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

    // Only count completed tasks that have token usage
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
                    })
                })
                .unwrap_or(serde_json::json!({
                    "date": "today",
                    "total_cost": 0.0,
                    "total_input_tokens": 0,
                    "total_output_tokens": 0,
                    "task_count": 0,
                }))
        } else {
            serde_json::json!({
                "total_cost": total_cost,
                "total_input_tokens": total_input_tokens,
                "total_output_tokens": total_output_tokens,
                "task_count": tasks_with_usage,
                "daily_breakdown": days,
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
        } else {
            println!("No token usage recorded yet today.");
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
    use workgraph::graph::{Node, Task, WorkGraph};
    use workgraph::parser::save_graph;

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
        task.token_usage = Some(workgraph::graph::TokenUsage {
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
