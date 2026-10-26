//! whisper.cpp models. The list comes from the Hugging Face API, is cached on
//! disk, and is refreshed on demand (or when the cache is a week old).
use crate::config;
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

const REPO: &str = "ggerganov/whisper.cpp";
/// A cached list older than this is refreshed in the background.
const STALE_SECS: u64 = 7 * 24 * 3600;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Model {
    /// `large-v3-turbo-q5_0` for `ggml-large-v3-turbo-q5_0.bin`.
    pub name: String,
    pub bytes: u64,
}

impl Model {
    /// A one-line description worked out from the name.
    pub fn note(&self) -> String {
        let lang = if self.name.contains(".en") {
            "English only"
        } else {
            "Multilingual"
        };
        let quant = ["q5_0", "q5_1", "q8_0"]
            .iter()
            .find(|q| self.name.ends_with(*q));
        match quant {
            Some(q) => format!("{lang}, quantized ({q})"),
            None => format!("{lang}, full precision"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Catalog {
    /// Unix seconds.
    pub fetched_at: u64,
    pub models: Vec<Model>,
}

impl Catalog {
    pub fn is_stale(&self) -> bool {
        now().saturating_sub(self.fetched_at) > STALE_SECS
    }

    /// `2026-10-26`, for display.
    pub fn fetched_label(&self) -> String {
        chrono::DateTime::from_timestamp(self.fetched_at as i64, 0)
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d")
                    .to_string()
            })
            .unwrap_or_default()
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn dir() -> PathBuf {
    config::dir().join("models")
}

fn cache_path() -> PathBuf {
    config::dir().join("models-cache.json")
}

fn file(name: &str) -> String {
    format!("ggml-{name}.bin")
}

pub fn path(name: &str) -> PathBuf {
    dir().join(file(name))
}

pub fn installed(name: &str) -> bool {
    path(name).is_file()
}

/// Names end up in a file path and a URL; keep them to plain model names.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._".contains(c))
        && !name.starts_with('.')
}

fn url(name: &str) -> String {
    format!("https://huggingface.co/{REPO}/resolve/main/{}", file(name))
}

/// Partial file while a download is running (for progress).
pub fn part_path(name: &str) -> PathBuf {
    dir().join(format!("{}.part", file(name)))
}

// ---- the catalog ------------------------------------------------------------

#[derive(Deserialize)]
struct ApiModel {
    siblings: Vec<ApiFile>,
}

#[derive(Deserialize)]
struct ApiFile {
    rfilename: String,
    size: Option<u64>,
}

/// The `ggml-*.bin` files in the repo listing, smallest first.
pub fn parse_catalog(json: &str, fetched_at: u64) -> Result<Catalog> {
    let api: ApiModel =
        serde_json::from_str(json).context("unexpected reply from the Hugging Face API")?;
    let mut models: Vec<Model> = api
        .siblings
        .into_iter()
        .filter_map(|f| {
            let name = f.rfilename.strip_prefix("ggml-")?.strip_suffix(".bin")?;
            // Skip diarization variants and anything that isn't a plain model name.
            (valid_name(name) && !name.contains("tdrz")).then(|| Model {
                name: name.to_string(),
                bytes: f.size.unwrap_or(0),
            })
        })
        .collect();
    if models.is_empty() {
        bail!("the Hugging Face listing for {REPO} contained no ggml models");
    }
    models.sort_by(|a, b| a.bytes.cmp(&b.bytes).then_with(|| a.name.cmp(&b.name)));
    Ok(Catalog { fetched_at, models })
}

/// Ask the Hugging Face API for the model list. Blocks (a few seconds at most).
pub fn fetch() -> Result<Catalog> {
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "20",
            &format!("https://huggingface.co/api/models/{REPO}?blobs=true"),
        ])
        .stdin(Stdio::null())
        .output()
        .context("running curl")?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        bail!(
            "could not reach huggingface.co: {}",
            why.trim().lines().last().unwrap_or("request failed")
        );
    }
    let catalog = parse_catalog(&String::from_utf8_lossy(&out.stdout), now())?;
    save_cache(&catalog)?;
    Ok(catalog)
}

pub fn cached() -> Option<Catalog> {
    serde_json::from_str(&std::fs::read_to_string(cache_path()).ok()?).ok()
}

fn save_cache(c: &Catalog) -> Result<()> {
    config::write_atomic(&cache_path(), &serde_json::to_string_pretty(c)?)
}

/// The cached list, fetching it first if there is none or `refresh` is set.
pub fn catalog(refresh: bool) -> Result<Catalog> {
    match cached() {
        Some(c) if !refresh => Ok(c),
        _ => fetch(),
    }
}

// ---- downloading and selecting ----------------------------------------------

/// A download that was stopped on request (not a failure).
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Run `cmd` to completion, polling `cancel`. When it is set the process is
/// killed, `leftover` (the partial file) is removed, and `Cancelled` returned.
fn run_cancellable(
    mut cmd: Command,
    leftover: &std::path::Path,
    cancel: Option<&AtomicBool>,
) -> Result<std::process::ExitStatus> {
    let mut child = cmd.spawn().context("running curl")?;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(leftover);
            return Err(Cancelled.into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Download with curl into `<models dir>/ggml-<name>.bin`. `quiet` hides curl's
/// progress bar. Setting `cancel` stops it and discards the partial file; the
/// error is then [`Cancelled`].
pub fn download(name: &str, quiet: bool, cancel: Option<&AtomicBool>) -> Result<PathBuf> {
    if !valid_name(name) {
        bail!("invalid model name '{name}'");
    }
    std::fs::create_dir_all(dir())?;
    let part = part_path(name);
    let mut cmd = Command::new("curl");
    cmd.args(["-fL", "--retry", "3", "-o"])
        .arg(&part)
        .arg(url(name));
    if quiet {
        cmd.arg("-s").stdin(Stdio::null()).stderr(Stdio::null());
    } else {
        cmd.arg("--progress-bar");
    }
    let status = run_cancellable(cmd, &part, cancel)?;
    if !status.success() {
        let _ = std::fs::remove_file(&part);
        bail!("download failed ({status})");
    }
    std::fs::rename(&part, path(name))?;
    Ok(path(name))
}

/// Make an installed model the active one, editing only the `model` line.
pub fn select(name: &str) -> Result<()> {
    if !installed(name) {
        return Err(anyhow!("{name} is not downloaded"));
    }
    config::Config::load()?;
    config::set_value(&config::config_path(), "model", &config::quote(name))
}

pub fn size_label(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MIB;
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{mb:.0} MB")
    }
}

/// One line of the model list as the UI shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub name: String,
    pub bytes: u64,
    pub note: String,
    pub installed: bool,
    pub active: bool,
}

pub fn rows(cfg: &config::Config, catalog: Option<&Catalog>) -> Vec<Row> {
    catalog
        .map(|c| {
            c.models
                .iter()
                .map(|m| Row {
                    name: m.name.clone(),
                    bytes: m.bytes,
                    note: m.note(),
                    installed: installed(&m.name),
                    active: m.name == cfg.model,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const API: &str = r#"{"id":"ggerganov/whisper.cpp","siblings":[
        {"rfilename":".gitattributes","size":1477},
        {"rfilename":"ggml-large-v3-turbo-q5_0.bin","size":573341696},
        {"rfilename":"ggml-tiny.en.bin","size":77704715},
        {"rfilename":"ggml-small.en-tdrz.bin","size":465000000},
        {"rfilename":"ggml-tiny-encoder.mlmodelc.zip","size":1},
        {"rfilename":"ggml-base.bin"},
        {"rfilename":"models/ggml-nested.bin","size":5},
        {"rfilename":"ggml-..evil.bin","size":5}
    ]}"#;

    #[test]
    fn parses_the_api_listing() {
        let c = parse_catalog(API, 42).unwrap();
        let names: Vec<_> = c.models.iter().map(|m| m.name.as_str()).collect();
        // Only top-level ggml-*.bin files, no tdrz, no zips, no odd names; smallest first.
        assert_eq!(names, ["base", "tiny.en", "large-v3-turbo-q5_0"]);
        assert_eq!(c.fetched_at, 42);
        assert_eq!(c.models[1].bytes, 77_704_715);
        assert!(parse_catalog(r#"{"siblings":[]}"#, 0).is_err());
        assert!(parse_catalog("not json", 0).is_err());
    }

    #[test]
    fn notes_come_from_the_name() {
        let note = |n: &str| {
            Model {
                name: n.into(),
                bytes: 0,
            }
            .note()
        };
        assert_eq!(note("tiny.en"), "English only, full precision");
        assert_eq!(
            note("large-v3-turbo-q5_0"),
            "Multilingual, quantized (q5_0)"
        );
        assert_eq!(note("base.en-q8_0"), "English only, quantized (q8_0)");
        assert_eq!(note("large-v3"), "Multilingual, full precision");
    }

    #[test]
    fn staleness_and_labels() {
        let fresh = Catalog {
            fetched_at: now(),
            models: vec![],
        };
        assert!(!fresh.is_stale());
        let old = Catalog {
            fetched_at: now() - STALE_SECS - 10,
            models: vec![],
        };
        assert!(old.is_stale());
        assert_eq!(fresh.fetched_label().len(), 10);
        assert_eq!(size_label(77_704_715), "74 MB");
        assert_eq!(size_label(1_624_555_275), "1.5 GB");
    }

    #[test]
    fn names_are_validated_before_touching_paths_or_urls() {
        for ok in ["tiny.en", "large-v3-turbo-q5_0", "base_x"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "../x", "a/b", ".hidden", "a b", "a;rm"] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert!(download("../x", true, None).is_err());
    }

    #[test]
    fn cancelling_kills_the_process_and_removes_the_partial_file() {
        use std::sync::Arc;
        let dir = std::env::temp_dir().join(format!("whispertui-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("ggml-x.bin.part");
        std::fs::write(&part, b"partial").unwrap();

        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.store(true, Ordering::Relaxed);
        });
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let started = std::time::Instant::now();
        let err = run_cancellable(cmd, &part, Some(&cancel)).unwrap_err();
        stopper.join().unwrap();
        assert!(err.downcast_ref::<Cancelled>().is_some(), "{err:#}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "didn't wait for the 30s process"
        );
        assert!(!part.exists(), "partial file removed");

        // Without a cancel it just runs to the end and reports how it went.
        let ok = run_cancellable(Command::new("true"), &part, None).unwrap();
        assert!(ok.success());
        let bad =
            run_cancellable(Command::new("false"), &part, Some(&AtomicBool::new(false))).unwrap();
        assert!(!bad.success());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_round_trips() {
        let c = parse_catalog(API, 7).unwrap();
        let again: Catalog = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(c, again);
    }
}
