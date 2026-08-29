use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: String,
    pub provider: String,
    pub source_lang: String,
    pub target_lang: String,
    pub original: String,
    pub translated: String,
}

fn history_path() -> Result<PathBuf> {
    let dir = config::app_config_dir()
        .context("could not determine a config directory for this platform")?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("history.jsonl"))
}

/// Appends one entry (JSON Lines: one JSON object per line) and trims the
/// file down to the most recent `max_entries` afterward.
pub fn append(entry: &HistoryEntry, max_entries: usize) -> Result<()> {
    let path = history_path()?;
    let mut entries = read_all(&path)?;
    entries.push(entry.clone());
    if entries.len() > max_entries {
        let drop = entries.len() - max_entries;
        entries.drain(0..drop);
    }
    write_all(&path, &entries)
}

/// Returns up to `limit` entries, most recent first.
pub fn load_recent(limit: usize) -> Result<Vec<HistoryEntry>> {
    let path = history_path()?;
    let mut entries = read_all(&path)?;
    entries.reverse();
    entries.truncate(limit);
    Ok(entries)
}

/// Returns the single entry at `index` in most-recent-first order (0 = latest).
pub fn get(index: usize) -> Result<Option<HistoryEntry>> {
    Ok(load_recent(index + 1)?.into_iter().nth(index))
}

pub fn clear() -> Result<()> {
    let path = history_path()?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

fn read_all(path: &std::path::Path) -> Result<Vec<HistoryEntry>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path)?;
    Ok(raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

fn write_all(path: &std::path::Path, entries: &[HistoryEntry]) -> Result<()> {
    let mut out = String::new();
    for entry in entries {
        out.push_str(&serde_json::to_string(entry)?);
        out.push('\n');
    }
    std::fs::write(path, out)?;
    Ok(())
}
