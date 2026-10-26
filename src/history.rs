use crate::config;
use anyhow::Result;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

/// The daemon's transcription threads read-modify-write the same file.
static WRITE: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Entries written before statuses existed are finished ones.
    #[default]
    Done,
    Transcribing,
    Failed,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub created: DateTime<Local>,
    pub text: String,
    pub duration_secs: f32,
    pub audio: Option<PathBuf>,
    #[serde(default)]
    pub status: Status,
    /// Why transcription failed; only set with `Status::Failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
    let _g = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load();
    all.insert(0, entry);
    if max > 0 && all.len() > max {
        for old in all.split_off(max) {
            remove_audio(&old);
        }
    }
    save(&all)
}

/// Replace the entry with the same id (a no-op if it was deleted meanwhile).
pub fn update(entry: Entry) -> Result<()> {
    let _g = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load();
    if let Some(slot) = all.iter_mut().find(|e| e.id == entry.id) {
        *slot = entry;
        save(&all)?;
    }
    Ok(())
}

pub fn find(id: &str) -> Option<Entry> {
    load().into_iter().find(|e| e.id == id)
}

/// Run at daemon start, when nothing can be mid-transcription: entries left
/// `Transcribing` were interrupted, and audio in the recordings folder that no
/// entry points at (older versions kept it when whisper failed) becomes a
/// failed entry, so it can be transcribed again instead of sitting unseen.
pub fn recover() -> Result<usize> {
    let _g = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load();
    let mut changed = 0;
    for e in all.iter_mut().filter(|e| e.status == Status::Transcribing) {
        e.status = Status::Failed;
        e.error = Some("interrupted (the daemon stopped before it finished)".into());
        changed += 1;
    }
    if let Ok(dir) = std::fs::read_dir(config::recordings_dir()) {
        for f in dir.flatten() {
            let path = f.path();
            let known = all
                .iter()
                .any(|e| e.audio.as_deref() == Some(path.as_path()));
            let id = path.file_stem().map(|s| s.to_string_lossy().into_owned());
            if known || path.extension().is_none_or(|x| x != "wav") {
                continue;
            }
            let Some(id) = id else { continue };
            let meta = f.metadata().ok();
            let created = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .map(DateTime::<Local>::from)
                .unwrap_or_else(Local::now);
            // 16-bit mono at the whisper rate, plus a 44-byte header.
            let bytes = meta.map_or(0, |m| m.len().saturating_sub(44));
            let secs = bytes as f32 / (2.0 * crate::audio::WHISPER_RATE as f32);
            all.push(Entry {
                id,
                created,
                text: String::new(),
                duration_secs: secs,
                audio: Some(path),
                status: Status::Failed,
                error: Some("transcription never finished".into()),
            });
            changed += 1;
        }
    }
    if changed > 0 {
        all.sort_by_key(|e| std::cmp::Reverse(e.created));
        save(&all)?;
    }
    Ok(changed)
}

pub fn delete(id: &str) -> Result<()> {
    let _g = WRITE.lock().unwrap_or_else(|e| e.into_inner());
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
