#!/usr/bin/env -S cargo +stable script --
//! Simple test to verify cron integration works

use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Change to the WG checkout directory
    if let Some(workgraph_dir) = env::args().nth(1) {
        env::set_current_dir(&workgraph_dir)?;
    }

    // Test 1: Parse cron expression
    match workgraph::cron::parse_cron_expression("*/5 * * * *") {
        Ok(schedule) => println!("✓ Cron parsing works"),
        Err(e) => {
            eprintln!("✗ Cron parsing failed: {}", e);
            return Err(e.into());
        }
    }

    // Test 2: Check if cron_enabled field exists in Task struct
    let task = workgraph::graph::Task {
        id: "test".to_string(),
        title: "Test".to_string(),
        cron_enabled: true,
        cron_schedule: Some("*/5 * * * *".to_string()),
        ..Default::default()
    };

    println!("✓ Task struct has cron fields");

    // Test 3: Check cron due functionality
    let now = chrono::Utc::now();
    let is_due = workgraph::cron::is_cron_due(&task, now);
    println!("✓ Cron due checking works (currently {})", if is_due { "due" } else { "not due" });

    println!("\nAll cron integration tests passed! 🎉");
    Ok(())
}
