use std::fmt::Write;
use std::path::Path;

use anyhow::Result;

use super::HistoryArgs;
use super::format::col_width;
use crate::history::{self, HistoryEntry};

/// Execute the `history` subcommand.
#[allow(clippy::print_stdout)]
pub fn execute(args: &HistoryArgs) -> Result<()> {
    let path = history::default_path();
    execute_with_path(args, &path)
}

/// History command logic with configurable path (for testing).
#[allow(clippy::print_stdout)]
pub fn execute_with_path(args: &HistoryArgs, path: &Path) -> Result<()> {
    if args.clear {
        history::clear(path)?;
        println!("History cleared.");
        return Ok(());
    }

    let entries = history::load(path)?;
    if entries.is_empty() {
        println!("No history entries found.");
        return Ok(());
    }

    println!("{}", format_history_table(&entries));
    Ok(())
}

/// Format history entries as a table.
pub fn format_history_table(entries: &[HistoryEntry]) -> String {
    let headers = ["FAMILY", "STARTED", "DURATION", "STATUS", "CONTAINERS"];

    let duration_values: Vec<String> = entries
        .iter()
        .map(|e| format_duration(e.duration_secs))
        .collect();

    let containers_values: Vec<String> = entries
        .iter()
        .map(|e| e.container_count.to_string())
        .collect();

    let family_w = col_width(entries.iter().map(|e| e.family.len()), headers[0].len());
    let started_w = col_width(entries.iter().map(|e| e.started_at.len()), headers[1].len());
    let duration_w = col_width(duration_values.iter().map(String::len), headers[2].len());
    let status_w = col_width(
        entries.iter().map(|e| e.exit_status.len()),
        headers[3].len(),
    );

    let mut output = String::new();

    let _ = writeln!(
        output,
        "{:<family_w$}  {:<started_w$}  {:<duration_w$}  {:<status_w$}  {}",
        headers[0], headers[1], headers[2], headers[3], headers[4],
    );

    for (i, entry) in entries.iter().enumerate() {
        let _ = writeln!(
            output,
            "{:<family_w$}  {:<started_w$}  {:<duration_w$}  {:<status_w$}  {}",
            entry.family,
            entry.started_at,
            duration_values[i],
            entry.exit_status,
            containers_values[i],
        );
    }

    if output.ends_with('\n') {
        output.pop();
    }

    output
}

/// Format duration in seconds as a human-readable string.
fn format_duration(secs: u64) -> String {
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let secs = secs % 60;

    if hours > 0 {
        format!("{hours}h{mins}m{secs}s")
    } else if mins > 0 {
        format!("{mins}m{secs}s")
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn sample_entry() -> HistoryEntry {
        HistoryEntry {
            family: "my-app".to_string(),
            started_at: "2025-01-15T10:00:00Z".to_string(),
            duration_secs: 120,
            exit_status: "success".to_string(),
            container_count: 3,
        }
    }

    #[test]
    fn format_history_table_single_entry() {
        let entries = vec![sample_entry()];
        let table = format_history_table(&entries);
        assert!(table.contains("FAMILY"));
        assert!(table.contains("STARTED"));
        assert!(table.contains("DURATION"));
        assert!(table.contains("STATUS"));
        assert!(table.contains("CONTAINERS"));
        assert!(table.contains("my-app"));
        assert!(table.contains("2m0s"));
        assert!(table.contains("success"));
        assert!(table.contains('3'));
    }

    #[test]
    fn format_history_table_multiple_entries() {
        let entries = vec![
            sample_entry(),
            HistoryEntry {
                family: "other-app".to_string(),
                started_at: "2025-01-15T11:00:00Z".to_string(),
                duration_secs: 30,
                exit_status: "error".to_string(),
                container_count: 1,
            },
        ];
        let table = format_history_table(&entries);
        assert_eq!(table.lines().count(), 3); // header + 2 rows
    }

    #[test]
    fn format_duration_values() {
        assert_eq!(format_duration(30), "30s");
        assert_eq!(format_duration(90), "1m30s");
        assert_eq!(format_duration(3661), "1h1m1s");
    }

    #[test]
    fn execute_no_history() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("history.json");
        let args = HistoryArgs { clear: false };
        execute_with_path(&args, &path).expect("should succeed");
    }

    #[test]
    fn execute_clear() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("history.json");
        history::append(&path, &sample_entry()).expect("append");

        let args = HistoryArgs { clear: true };
        execute_with_path(&args, &path).expect("should succeed");
        assert!(!path.exists());
    }

    #[test]
    fn execute_with_entries() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("history.json");
        history::append(&path, &sample_entry()).expect("append");

        let args = HistoryArgs { clear: false };
        execute_with_path(&args, &path).expect("should succeed");
    }
}
