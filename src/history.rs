use crate::config;
use anyhow::Result;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub created: DateTime<Local>,
    pub text: String,
    pub duration_secs: f32,
    pub audio: Option<PathBuf>,
}

/// Newest first.
pub fn load() -> Vec<Entry> {
    std::fs::read_to_string(config::history_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(entries: &[Entry]) -> Result<()> {
    std::fs::create_dir_all(config::dir())?;
    let tmp = config::history_path().with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(entries)?)?;
    std::fs::rename(tmp, config::history_path())?;
    Ok(())
}

pub fn add(entry: Entry, max: usize) -> Result<()> {
    let mut all = load();
    all.insert(0, entry);
    if max > 0 && all.len() > max {
        for old in all.split_off(max) {
            remove_audio(&old);
        }
    }
    save(&all)
}

pub fn delete(id: &str) -> Result<()> {
    let mut all = load();
    if let Some(pos) = all.iter().position(|e| e.id == id) {
        remove_audio(&all.remove(pos));
    }
    save(&all)
}

fn remove_audio(e: &Entry) {
    if let Some(p) = &e.audio {
        let _ = std::fs::remove_file(p);
    }
}
