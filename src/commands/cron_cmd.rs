//! `wg cron` — diagnostics & control surface for recurring cron tasks.
//!
//! Implements the recurring-wakeup diagnostics surface decided in
//! `docs/research/recurring-wakeup-heartbeat-gaps.md` §6 and the acceptance
//! criteria in `docs/repro-weekly-wakeup-heartbeat.md`
//! (`impl-recurring-heartbeat-diagnostics`).
//!
//! `wg cron doctor` lists every cron-enabled task with: schedule, resolved
//! weekday + UTC time-of-day (so the `cron` crate's non-standard 1=Sunday
//! mapping is *visible*), resolved next-fire, last-fire, whether the task is
//! currently due / overdue, paused / blocking state, and the missed-fire count
//! across daemon downtime. `wg cron list` is the JSON-friendly variant.

use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use std::path::Path;
use worksgood::cron::{
    CronDescription, describe_cron, format_countdown, missed_fires_before_reset, overdue_secs,
};
use worksgood::graph::{Status, Task};
use worksgood::query::is_time_ready;

use super::load_workgraph;

/// JSON-serializable row for `wg cron list --json` / `wg cron doctor --json`.
#[derive(Debug, Serialize)]
struct CronRow {
    id: String,
    title: String,
    status: String,
    /// Raw cron expression.
    cron_schedule: String,
    /// Resolved weekday(s) the expression fires on (e.g. `["Sunday"]`). `None`
    /// when the expression has no day-of-week constraint (fires every day).
    #[serde(skip_serializing_if = "Option::is_none")]
    weekdays: Option<Vec<String>>,
    /// Resolved UTC `HH:MM` time-of-day. `None` when the expression fires more
    /// than once a day.
    #[serde(skip_serializing_if = "Option::is_none")]
    time_utc: Option<String>,
    /// True when the day-of-week field is present — i.e. the non-standard
    /// 1=Sunday mapping is in play.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    has_dow_field: bool,
    /// One-line human summary (e.g. `"Sun 09:00 UTC (cron dow: 1=Sun … 7=Sat)"`).
    summary: String,
    /// RFC3339 timestamp of the next scheduled fire (with jitter).
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cron_fire: Option<String>,
    /// RFC3339 timestamp of the last fire.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_cron_fire: Option<String>,
    /// True when the task is currently due to run (`is_time_ready`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    due: bool,
    /// Seconds the task has been waiting past its scheduled fire time, when due
    /// but not yet dispatched. `None` when not overdue.
    #[serde(skip_serializing_if = "Option::is_none")]
    overdue_secs: Option<i64>,
    /// Number of scheduled fire windows missed across daemon downtime since the
    /// last run (excluding the one being caught up now). `None` when not
    /// computable.
    #[serde(skip_serializing_if = "Option::is_none")]
    missed_fires: Option<u32>,
    /// True when the task is paused (will not dispatch even when due).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    paused: bool,
    /// Human-readable current blocking state, e.g. `"paused"`, `"overdue"`,
    /// `"waiting"`, or `""` (ready / not due).
    blocking_state: String,
}

fn row_for(task: &Task, now: chrono::DateTime<Utc>) -> Option<CronRow> {
    if !task.cron_enabled {
        return None;
    }
    let raw = task.cron_schedule.clone()?;
    let desc: CronDescription = describe_cron(&raw).unwrap_or(CronDescription {
        raw: raw.clone(),
        weekdays: None,
        time_utc: None,
        has_dow_field: false,
        summary: format!("[unparseable: {}]", raw),
    });

    let due = is_time_ready(task);
    let overdue = if due { overdue_secs(task, now) } else { None };
    let missed = missed_fires_before_reset(task, now);

    let blocking_state = if task.paused {
        "paused".to_string()
    } else if due {
        match task.status {
            Status::Waiting | Status::PendingValidation => "waiting".to_string(),
            Status::Blocked => "blocked".to_string(),
            Status::Open if overdue.is_some() => "overdue".to_string(),
            _ => "due".to_string(),
        }
    } else {
        String::new()
    };

    Some(CronRow {
        id: task.id.clone(),
        title: task.title.clone(),
        status: format!("{:?}", task.status).to_lowercase(),
        cron_schedule: raw,
        weekdays: desc.weekdays.clone(),
        time_utc: desc.time_utc.clone(),
        has_dow_field: desc.has_dow_field,
        summary: desc.summary.clone(),
        next_cron_fire: task.next_cron_fire.clone(),
        last_cron_fire: task.last_cron_fire.clone(),
        due,
        overdue_secs: overdue,
        missed_fires: missed,
        paused: task.paused,
        blocking_state,
    })
}

/// Run `wg cron doctor` / `wg cron list` — same output, two names.
pub fn run(dir: &Path, json: bool) -> Result<()> {
    let (graph, _path) = load_workgraph(dir)?;
    let now = Utc::now();

    let mut rows: Vec<CronRow> = graph
        .tasks()
        .filter(|t| t.cron_enabled)
        .filter_map(|t| row_for(t, now))
        .collect();

    // Sort: due/overdue first, then by next_cron_fire (soonest first), then id.
    rows.sort_by(|a, b| {
        // due tasks float to the top
        match (a.due, b.due) {
            (true, false) => return std::cmp::Ordering::Less,
            (false, true) => return std::cmp::Ordering::Greater,
            _ => {}
        }
        let na = a.next_cron_fire.as_deref().unwrap_or("");
        let nb = b.next_cron_fire.as_deref().unwrap_or("");
        nb.cmp(na).reverse().then(a.id.cmp(&b.id))
    });

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("No cron-scheduled tasks.");
        return Ok(());
    }

    println!("Cron-scheduled tasks ({}):", rows.len());
    println!();
    for row in &rows {
        let status_tag = match (row.due, row.paused, row.blocking_state.as_str()) {
            (_, true, _) => "PAUSED",
            (true, _, "overdue") => "OVERDUE",
            (true, _, "waiting") => "WAITING",
            (true, _, "blocked") => "BLOCKED",
            (true, _, _) => "DUE",
            (false, _, _) => "scheduled",
        };
        let next_tag = match &row.next_cron_fire {
            Some(ts) => format!("next: {}", format_countdown(ts, now)),
            None => "next: unknown".to_string(),
        };
        let last_tag = match &row.last_cron_fire {
            Some(ts) => format!("last: {}", format_countdown(ts, now)),
            None => "last: never".to_string(),
        };
        let missed_tag = match row.missed_fires {
            Some(n) if n > 0 => format!(" \x1b[33m[missed: {}]\x1b[0m", n),
            _ => String::new(),
        };
        let overdue_tag = match row.overdue_secs {
            Some(s) => format!(" \x1b[31m[overdue: {}s]\x1b[0m", s),
            None => String::new(),
        };
        println!(
            "  \x1b[1m{}\x1b[0m — {}  [\x1b[36m{}\x1b[0m]  {}{}{}",
            row.id, row.title, status_tag, next_tag, missed_tag, overdue_tag
        );
        println!("    {}", row.summary);
        println!("    {}  {}", last_tag, row.status);
    }

    // Surface the non-standard dow mapping as a single grouped hint (no per-row
    // spam — the summary already names it).
    let dow_warn = rows.iter().any(|r| r.has_dow_field);
    if dow_warn {
        println!();
        println!(
            "\x1b[33mnote:\x1b[0m the `cron` crate maps day-of-week as 1=Sunday, 2=Monday, …, \
             7=Saturday (NOT standard cron's 0=Sunday, 1=Monday). Each cron summary above names \
             the actual weekday it will fire on — verify you are not scheduling the wrong day."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    use worksgood::graph::{Node, Task, WorkGraph};
    use worksgood::parser::save_graph;

    fn write_graph(dir: &Path, tasks: Vec<Task>) {
        fs::create_dir_all(dir).unwrap();
        let mut g = WorkGraph::new();
        for t in tasks {
            g.add_node(Node::Task(t));
        }
        save_graph(&g, &dir.join("graph.jsonl")).unwrap();
    }

    fn cron_task(id: &str, schedule: &str, next: Option<&str>, last: Option<&str>) -> Task {
        let mut t = Task {
            id: id.to_string(),
            title: format!("task {}", id),
            cron_enabled: true,
            cron_schedule: Some(schedule.to_string()),
            ..Default::default()
        };
        t.next_cron_fire = next.map(|s| s.to_string());
        t.last_cron_fire = last.map(|s| s.to_string());
        t
    }

    #[test]
    fn doctor_no_cron_tasks_succeeds() {
        let dir = tempdir().unwrap();
        write_graph(dir.path(), vec![]);
        let r = run(dir.path(), false);
        assert!(r.is_ok(), "{:?}", r);
    }

    #[test]
    fn doctor_json_emits_array_with_summary() {
        let dir = tempdir().unwrap();
        // dow=1 → Sunday (non-standard mapping). The summary must name Sunday.
        let future = (Utc::now() + chrono::Duration::days(7)).to_rfc3339();
        let t = cron_task("weekly", "0 0 9 * * 1", Some(&future), None);
        write_graph(dir.path(), vec![t]);
        // Capture stdout by running the function — it prints. We only assert
        // the row_for helper (which the printer uses) names Sunday.
        let now = Utc::now();
        let row = row_for(
            &cron_task("weekly", "0 0 9 * * 1", Some(&future), None),
            Utc::now(),
        )
        .expect("row");
        assert!(row.summary.contains("Sun"), "summary: {}", row.summary);
        assert!(row.has_dow_field);
        // JSON path should succeed (it just prints).
        let r = run(dir.path(), true);
        assert!(r.is_ok());
    }

    #[test]
    fn doctor_surfaces_paused_and_overdue_blocking_state() {
        let now = Utc::now();
        let past = (now - chrono::Duration::hours(1)).to_rfc3339();
        let mut t = cron_task("paused-due", "0 0 9 * * *", Some(&past), None);
        t.paused = true;
        let row = row_for(&t, now).expect("row");
        assert!(row.due, "past next_cron_fire ⇒ due");
        assert!(row.paused);
        assert_eq!(row.blocking_state, "paused", "paused wins over due");

        let mut t2 = cron_task("overdue-due", "0 0 9 * * *", Some(&past), None);
        t2.status = Status::Open;
        let row2 = row_for(&t2, now).expect("row");
        assert_eq!(row2.blocking_state, "overdue");
        assert!(row2.overdue_secs.unwrap_or(0) > 0);
    }

    #[test]
    fn doctor_missed_fires_column_populates_with_stale_last_fire() {
        let now = Utc::now();
        let stale = (now - chrono::Duration::days(5)).to_rfc3339();
        let t = cron_task("daily-stale", "0 0 9 * * *", None, Some(&stale));
        let row = row_for(&t, now).expect("row");
        // 5 days of daily windows behind now → missed >= 4 (one being caught up).
        let missed = row.missed_fires.expect("computable");
        assert!(
            missed >= 4,
            "expected >=4 missed daily windows, got {}",
            missed
        );
    }

    #[test]
    fn doctor_skips_non_cron_tasks() {
        let t = Task {
            id: "plain".to_string(),
            title: "plain".to_string(),
            cron_enabled: false,
            ..Default::default()
        };
        let now = Utc::now();
        assert!(row_for(&t, now).is_none());
    }

    #[test]
    fn doctor_invalid_cron_schedule_shows_unparseable_summary() {
        let t = cron_task("broken", "not a cron", None, None);
        let now = Utc::now();
        let row = row_for(&t, now).expect("row (cron_enabled)");
        assert!(
            row.summary.contains("unparseable"),
            "summary: {}",
            row.summary
        );
    }
}
