//! Execution history persistence for Lecs tasks.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A single history entry recording a task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Task family name.
    pub family: String,
    /// ISO 8601 timestamp when the task was started.
    pub started_at: String,
    /// Duration in seconds.
    pub duration_secs: u64,
    /// Exit status description.
    pub exit_status: String,
    /// Number of containers in the task.
    pub container_count: usize,
}

/// Load history entries from a JSON file.
///
/// Returns an empty vec if the file doesn't exist.
pub fn load(path: &Path) -> Result<Vec<HistoryEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read history file: {}", path.display()))?;
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse history file: {}", path.display()))
}

/// Append a history entry to the JSON file.
///
/// Creates the parent directory and file if they don't exist.
#[allow(dead_code)]
pub fn append(path: &Path, entry: &HistoryEntry) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }
    let mut entries = load(path).unwrap_or_default();
    entries.push(entry.clone());
    let json = serde_json::to_string_pretty(&entries).context("Failed to serialize history")?;
    let mut file = std::fs::File::create(path)
        .with_context(|| format!("Failed to write history file: {}", path.display()))?;
    file.write_all(json.as_bytes())
        .with_context(|| format!("Failed to write history file: {}", path.display()))?;
    Ok(())
}

/// Clear all history by removing the file.
pub fn clear(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_file(path)
            .with_context(|| format!("Failed to remove history file: {}", path.display()))?;
    }
    Ok(())
}

/// Return the default history file path: `$HOME/.lecs/history.json`.
pub fn default_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".lecs").join("history.json")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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
    fn load_nonexistent_file_returns_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");
        let entries = load(&path).expect("should succeed");
        assert!(entries.is_empty());
    }

    #[test]
    fn load_empty_file_returns_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("history.json");
        std::fs::write(&path, "").unwrap();
        let entries = load(&path).expect("should succeed");
        assert!(entries.is_empty());
    }

    #[test]
    fn append_and_load() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("history.json");

        let entry = sample_entry();
        append(&path, &entry).expect("should succeed");

        let entries = load(&path).expect("should succeed");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].family, "my-app");
        assert_eq!(entries[0].duration_secs, 120);
    }

    #[test]
    fn append_multiple_entries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("history.json");

        append(&path, &sample_entry()).expect("first append");

        let mut second = sample_entry();
        second.family = "other-app".to_string();
        append(&path, &second).expect("second append");

        let entries = load(&path).expect("should succeed");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].family, "other-app");
    }

    #[test]
    fn clear_removes_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("history.json");

        append(&path, &sample_entry()).expect("append");
        assert!(path.exists());

        clear(&path).expect("clear");
        assert!(!path.exists());
    }

    #[test]
    fn clear_nonexistent_file_succeeds() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");
        clear(&path).expect("should succeed");
    }

    #[test]
    fn append_creates_parent_directories() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sub").join("dir").join("history.json");

        append(&path, &sample_entry()).expect("should create dirs");
        assert!(path.exists());
    }

    #[test]
    fn default_path_contains_lecs() {
        let path = default_path();
        let path_str = path.to_string_lossy();
        assert!(path_str.contains(".lecs"));
        assert!(path_str.contains("history.json"));
    }
}
