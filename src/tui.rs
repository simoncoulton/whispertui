//! The window's event loop: polls state, feeds keys to `App`, and carries out
//! the `Action`s it returns.
use crate::app::{Action, App, Download, Mode};
use crate::config::{self, Config};
use crate::{audio, daemon, history, models, sys, theme, ui};
use anyhow::{Result, anyhow};
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime};

enum DownloadOutcome {
    Done,
    Cancelled,
    Failed(String),
}

/// One model download, on its own thread; any number can run at once.
struct DownloadJob {
    name: String,
    bytes: u64,
    cancel: Arc<AtomicBool>,
    rx: Receiver<DownloadOutcome>,
}

struct Env {
    cfg: Config,
    history_mtime: Option<SystemTime>,
    last_status: Instant,
    /// The config file and theme files as last loaded; a change reloads them live.
    files: Vec<(PathBuf, SystemTime)>,
    last_files_check: Instant,
    catalog: Option<models::Catalog>,
    catalog_rx: Option<Receiver<Result<models::Catalog, String>>>,
    downloads: Vec<DownloadJob>,
    /// The `afplay` process while a recording plays.
    player: Option<std::process::Child>,
    mic: audio::Mic,
    last_mic_check: Instant,
}

pub fn run() -> Result<()> {
    let cfg = Config::load()?;
    // A broken theme or config is an error, never a silent fallback.
    let (themes, index) = resolve_themes(&cfg)?;
    if daemon::ensure_running()? {
        eprintln!("Started the whispertui daemon.");
    }
    let mut app = App::new(themes, index, cfg.transparent_background);
    app.layout = cfg.layout;
    app.config_label = config::tilde(&config::dir());
    let mut env = Env {
        cfg,
        history_mtime: None,
        last_status: Instant::now() - Duration::from_secs(10),
        files: watched_files(),
        last_files_check: Instant::now(),
        catalog: models::cached(),
        catalog_rx: None,
        downloads: Vec::new(),
        player: None,
        mic: audio::Mic::None,
        last_mic_check: Instant::now(),
    };
    refresh_model_state(&mut app, &env);
    refresh_mic(&mut app, &mut env);
    let mut term = ratatui::init();
    let res = main_loop(&mut term, &mut app, &mut env);
    stop_playback(&mut app, &mut env);
    cancel_all_downloads(&mut env);
    ratatui::restore();
    res
}

fn resolve_themes(cfg: &Config) -> Result<(Vec<theme::Theme>, usize)> {
    let dir = config::themes_dir();
    let themes = theme::load_themes(&dir, &cfg.themes).map_err(|e| {
        anyhow!(
            "invalid themes (config {} or folder {}): {e:#}",
            config::config_path().display(),
            dir.display()
        )
    })?;
    let index = theme::position(&themes, &cfg.theme).ok_or_else(|| {
        let names: Vec<_> = themes.iter().map(|t| t.name.as_str()).collect();
        anyhow!(
            "unknown theme '{}' in {} (available: {})",
            cfg.theme,
            config::config_path().display(),
            names.join(", ")
        )
    })?;
    Ok((themes, index))
}

/// The config file and every theme file, with their modification times.
fn watched_files() -> Vec<(PathBuf, SystemTime)> {
    let mut paths = vec![config::config_path()];
    if let Ok(rd) = std::fs::read_dir(config::themes_dir()) {
        paths.extend(
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "yml" || x == "yaml")),
        );
    }
    paths.sort();
    paths
        .into_iter()
        .filter_map(|p| Some((p.clone(), std::fs::metadata(&p).ok()?.modified().ok()?)))
        .collect()
}

/// Everything derived from the config and the model list.
fn refresh_model_state(app: &mut App, env: &Env) {
    app.models = models::rows(&env.cfg, env.catalog.as_ref());
    app.model_label = env.cfg.model.clone();
    app.model_ready = models::installed(&env.cfg.model);
    app.catalog_updated = env.catalog.as_ref().map(|c| c.fetched_label());
    if let Some(i) = app.models.iter().position(|m| m.active)
        && app.mode != Mode::Models
    {
        app.model_sel = i;
    }
}

fn main_loop(term: &mut DefaultTerminal, app: &mut App, env: &mut Env) -> Result<()> {
    loop {
        poll_history(app, env);
        if env.last_status.elapsed() > Duration::from_millis(500) {
            app.status = daemon::status();
            env.last_status = Instant::now();
        }
        poll_files(app, env);
        poll_catalog(app, env);
        poll_downloads(app, env);
        poll_player(app, env);
        poll_mic(app, env);
        app.tick(Instant::now());
        term.draw(|f| ui::draw(f, app))?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let Event::Key(k) = event::read()? else {
            continue;
        };
        if k.kind != KeyEventKind::Press {
            continue;
        }
        if let Some(action) = app.handle_key(k)
            && perform(action, term, app, env)?
        {
            return Ok(());
        }
    }
}

fn poll_history(app: &mut App, env: &mut Env) {
    let m = std::fs::metadata(config::history_path())
        .and_then(|m| m.modified())
        .ok();
    if m != env.history_mtime {
        env.history_mtime = m;
        app.set_entries(history::load());
    }
}

/// Pick up edits to the config and theme files while the window is open.
fn poll_files(app: &mut App, env: &mut Env) {
    // Don't swap themes under the picker; the change is seen once it closes.
    if env.last_files_check.elapsed() < Duration::from_secs(1) || app.mode == Mode::Theme {
        return;
    }
    env.last_files_check = Instant::now();
    let now = watched_files();
    if now != env.files {
        env.files = now;
        reload_config(app, env);
    }
}

/// Re-read the config and themes. A broken file is reported and the old state stays.
fn reload_config(app: &mut App, env: &mut Env) -> bool {
    let result =
        Config::read(&config::config_path()).and_then(|cfg| resolve_themes(&cfg).map(|t| (cfg, t)));
    match result {
        Ok((cfg, (themes, index))) => {
            app.set_themes(themes, index, cfg.transparent_background);
            app.layout = cfg.layout;
            env.cfg = cfg;
            refresh_model_state(app, env);
            refresh_mic(app, env);
            true
        }
        Err(e) => {
            app.flash_err(one_line(&e));
            false
        }
    }
}

/// Work out which microphone would record, for the header and for blocking `space`.
fn refresh_mic(app: &mut App, env: &mut Env) {
    env.mic = audio::mic_state(&env.cfg.input_device);
    app.mic_label = env.mic.label();
    app.mic_ready = env.mic.is_ready();
}

/// Notice microphones being plugged in and out while the window is open.
fn poll_mic(app: &mut App, env: &mut Env) {
    if env.last_mic_check.elapsed() < Duration::from_secs(2) {
        return;
    }
    env.last_mic_check = Instant::now();
    let now = audio::mic_state(&env.cfg.input_device);
    if now != env.mic {
        env.mic = now;
        app.mic_label = env.mic.label();
        app.mic_ready = env.mic.is_ready();
    }
}

fn start_catalog_refresh(app: &mut App, env: &mut Env) {
    if env.catalog_rx.is_some() {
        return;
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(models::fetch().map_err(|e| format!("{e:#}")));
    });
    env.catalog_rx = Some(rx);
    app.catalog_refreshing = true;
}

fn poll_catalog(app: &mut App, env: &mut Env) {
    let Some(rx) = &env.catalog_rx else { return };
    let Ok(result) = rx.try_recv() else { return };
    env.catalog_rx = None;
    app.catalog_refreshing = false;
    match result {
        Ok(cat) => {
            env.catalog = Some(cat);
            refresh_model_state(app, env);
        }
        // The old list (if any) stays usable.
        Err(e) => {
            if app.mode == Mode::Models {
                app.models_error = Some(e);
            } else {
                app.flash_err(format!("Could not refresh the model list: {e}"));
            }
        }
    }
}

fn poll_downloads(app: &mut App, env: &mut Env) {
    let mut finished = Vec::new();
    for job in &env.downloads {
        match job.rx.try_recv() {
            Ok(outcome) => finished.push((job.name.clone(), outcome)),
            Err(TryRecvError::Disconnected) => finished.push((
                job.name.clone(),
                DownloadOutcome::Failed("the download stopped unexpectedly".into()),
            )),
            Err(TryRecvError::Empty) => {}
        }
    }
    env.downloads
        .retain(|j| !finished.iter().any(|(n, _)| *n == j.name));
    for (name, outcome) in finished {
        match outcome {
            DownloadOutcome::Done => {
                // The first model you download becomes the one in use; later
                // ones wait until you pick them, so a download never swaps the
                // model under you.
                if models::installed(&env.cfg.model) {
                    app.flash_ok(format!(
                        "Downloaded {name}; press M, then Enter on it to use it"
                    ));
                } else {
                    match models::select(&name) {
                        Ok(()) => {
                            reload_config(app, env);
                            app.flash_ok(format!("Downloaded and selected {name}"));
                        }
                        Err(e) => app.flash_err(one_line(&e)),
                    }
                }
            }
            DownloadOutcome::Cancelled => app.flash_ok(format!("Cancelled the download of {name}")),
            DownloadOutcome::Failed(e) => {
                app.models_error = Some(format!("Download of {name} failed: {e}"));
                app.flash_err(format!("Download of {name} failed"));
            }
        }
        refresh_model_state(app, env);
    }
    app.downloads = env
        .downloads
        .iter()
        .map(|job| {
            let done = std::fs::metadata(models::part_path(&job.name))
                .map(|f| f.len())
                .unwrap_or(0);
            Download {
                name: job.name.clone(),
                percent: (done * 100 / job.bytes.max(1)).min(99) as u8,
            }
        })
        .collect();
}

/// Stop every download and wait briefly for the `curl` processes to be killed,
/// so quitting the window never leaves them running behind it.
fn cancel_all_downloads(env: &mut Env) {
    for job in &env.downloads {
        job.cancel.store(true, Ordering::Relaxed);
    }
    for job in env.downloads.drain(..) {
        let _ = job.rx.recv_timeout(Duration::from_secs(2));
    }
}

fn start_playback(app: &mut App, env: &mut Env, id: String, path: std::path::PathBuf) {
    stop_playback(app, env);
    let child = std::process::Command::new("afplay")
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match child {
        Ok(c) => {
            env.player = Some(c);
            app.playing = Some(id);
        }
        Err(e) => app.flash_err(format!("Could not play the audio: {e}")),
    }
}

/// Kill the player, if there is one. Also used on the way out, so quitting
/// never leaves a recording playing behind the shell.
fn stop_playback(app: &mut App, env: &mut Env) {
    if let Some(mut child) = env.player.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    app.playing = None;
}

/// Notice when a recording finishes (or fails to play) by itself.
fn poll_player(app: &mut App, env: &mut Env) {
    let Some(child) = &mut env.player else { return };
    if let Ok(Some(status)) = child.try_wait() {
        env.player = None;
        app.playing = None;
        if !status.success() {
            app.flash_err("Could not play the audio (is the file still there?)");
        }
    }
}

fn one_line(e: &anyhow::Error) -> String {
    format!("{e:#}")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(2)
        .collect::<Vec<_>>()
        .join(" — ")
}

/// Returns true when the window should close.
fn perform(
    action: Action,
    term: &mut DefaultTerminal,
    app: &mut App,
    env: &mut Env,
) -> Result<bool> {
    match action {
        Action::Quit => return Ok(true),
        Action::QuitAndStop => {
            daemon::stop()?;
            return Ok(true);
        }
        Action::Copy(text) => match sys::copy(&text) {
            Ok(()) => app.flash_ok("Copied to clipboard"),
            Err(e) => app.flash_err(format!("Copy failed: {e}")),
        },
        Action::Delete(id) => match history::delete(&id) {
            Ok(()) => {
                if app.playing.as_deref() == Some(id.as_str()) {
                    stop_playback(app, env);
                }
                env.history_mtime = None;
                app.flash_ok("Recording deleted");
            }
            Err(e) => app.flash_err(format!("Delete failed: {e}")),
        },
        Action::ToggleRecord => {
            if !app.mic_ready {
                // The daemon would refuse (and notify); say so here, in the window.
                app.flash_err(env.mic.problem().unwrap_or_default());
            } else if !app.model_ready {
                // The daemon would refuse (and notify); say so here, in the window.
                app.flash_err(if env.cfg.model.trim().is_empty() {
                    "No model is set; press M to download one".to_string()
                } else {
                    format!("{} isn't downloaded; press M to download it", env.cfg.model)
                });
            } else if daemon::request("toggle").is_err() {
                app.flash_err("Daemon not running; run `whispertui daemon start`");
            }
            env.last_status = Instant::now() - Duration::from_secs(10);
        }
        Action::Play { id, path } => start_playback(app, env, id, path),
        Action::StopPlayback => {
            stop_playback(app, env);
            app.flash_ok("Stopped playback");
        }
        Action::EditConfig => edit_config(term, app, env)?,
        Action::RefreshModels => {
            refresh_model_state(app, env);
            if env.catalog.as_ref().is_none_or(|c| c.is_stale()) {
                start_catalog_refresh(app, env);
            }
        }
        Action::RefreshCatalog => start_catalog_refresh(app, env),
        Action::RefreshInputs => {
            refresh_mic(app, env);
            app.set_inputs(audio::input_rows(&env.cfg.input_device, &audio::inputs()));
        }
        Action::UseInput(device) => {
            match config::set_value(
                &config::config_path(),
                "input_device",
                &config::quote(&device),
            ) {
                Ok(()) => {
                    env.cfg.input_device = device;
                    env.files = watched_files();
                    refresh_mic(app, env);
                    app.set_inputs(audio::input_rows(&env.cfg.input_device, &audio::inputs()));
                    app.flash_ok(format!("Microphone: {}", env.mic.label()));
                }
                Err(e) => app.flash_err(one_line(&e)),
            }
        }
        Action::DownloadModel(name) => {
            if env.downloads.iter().any(|j| j.name == name) {
                return Ok(false);
            }
            let bytes = env
                .catalog
                .as_ref()
                .and_then(|c| c.models.iter().find(|m| m.name == name))
                .map_or(0, |m| m.bytes);
            let cancel = Arc::new(AtomicBool::new(false));
            let (tx, rx) = mpsc::channel();
            let (n, flag) = (name.clone(), cancel.clone());
            std::thread::spawn(move || {
                let outcome = match models::download(&n, true, Some(&flag)) {
                    Ok(_) => DownloadOutcome::Done,
                    Err(e) if e.downcast_ref::<models::Cancelled>().is_some() => {
                        DownloadOutcome::Cancelled
                    }
                    Err(e) => DownloadOutcome::Failed(format!("{e:#}")),
                };
                let _ = tx.send(outcome);
            });
            app.downloads.push(Download {
                name: name.clone(),
                percent: 0,
            });
            env.downloads.push(DownloadJob {
                name,
                bytes,
                cancel,
                rx,
            });
        }
        Action::CancelDownload(name) => {
            if let Some(job) = env.downloads.iter().find(|j| j.name == name) {
                job.cancel.store(true, Ordering::Relaxed);
            }
        }
        Action::CancelDownloads => {
            for job in &env.downloads {
                job.cancel.store(true, Ordering::Relaxed);
            }
        }
        Action::UseModel(name) => match models::select(&name) {
            Ok(()) => {
                reload_config(app, env);
                app.flash_ok(format!("Now using {name}"));
            }
            Err(e) => app.models_error = Some(one_line(&e)),
        },
        Action::ReloadThemes => {
            // Keep whatever is active, but pick up new and edited theme files.
            let keep = app.themes[app.theme_index].name.clone();
            match theme::load_themes(&config::themes_dir(), &env.cfg.themes) {
                Ok(themes) => {
                    let index = theme::position(&themes, &keep).unwrap_or(0);
                    app.set_themes(themes, index, app.transparent);
                    env.files = watched_files();
                }
                Err(e) => app.flash_err(one_line(&e)),
            }
        }
        Action::SaveTheme { name, transparent } => {
            let path = config::config_path();
            let mut result = Ok(());
            if theme::normalize(&env.cfg.theme) != theme::normalize(&name) {
                result = config::set_value(&path, "theme", &config::quote(&name));
            }
            if result.is_ok() && env.cfg.transparent_background != transparent {
                result =
                    config::set_value(&path, "transparent_background", &transparent.to_string());
            }
            match result {
                Ok(()) => {
                    env.cfg.theme = name.clone();
                    env.cfg.transparent_background = transparent;
                    env.files = watched_files();
                    app.flash_ok(format!("Theme {name} saved"));
                }
                Err(e) => app.flash_err(one_line(&e)),
            }
        }
        Action::SaveLayout(layout) => {
            match config::set_value(&config::config_path(), "layout", layout.as_str()) {
                Ok(()) => {
                    env.cfg.layout = layout;
                    env.files = watched_files();
                    app.flash_ok(format!("Layout: {}", layout.as_str()));
                }
                Err(e) => app.flash_err(one_line(&e)),
            }
        }
    }
    Ok(false)
}

fn edit_config(term: &mut DefaultTerminal, app: &mut App, env: &mut Env) -> Result<()> {
    ratatui::restore();
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
    let mut parts = editor.split_whitespace();
    let status = std::process::Command::new(parts.next().unwrap_or("vi"))
        .args(parts)
        .arg(config::config_path())
        .status();
    *term = ratatui::init();
    if let Err(e) = status {
        app.flash_err(format!("Could not run {editor}: {e}"));
        return Ok(());
    }
    // Re-read everything; a broken config is reported, and the old one stays in use.
    let ok = reload_config(app, env);
    env.files = watched_files();
    if ok {
        app.flash_ok("Config reloaded; the daemon picks it up automatically");
    }
    Ok(())
}
