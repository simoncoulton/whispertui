use crate::audio::{self, Recorder};
use crate::config::{self, Config};
use crate::history::{self, Entry, Status as Progress};
use crate::{hotkey, models, sys};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

pub enum Cmd {
    Toggle,
    Start,
    Stop,
    Cancel,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Status {
    pub state: String, // idle | recording | transcribing
    pub secs: u64,
    pub hotkey: String,
    pub mode: String,
}

struct Shared {
    cfg: Arc<RwLock<Config>>,
    recording: Arc<AtomicBool>,
    started: Mutex<Option<Instant>>,
    jobs: AtomicUsize,
}

/// Ask a running daemon to do something. Err if none is listening.
pub fn request(cmd: &str) -> Result<String> {
    let mut s = UnixStream::connect(config::socket_path())?;
    s.set_read_timeout(Some(Duration::from_secs(2)))?;
    writeln!(s, "{cmd}")?;
    let mut out = String::new();
    BufReader::new(s).read_line(&mut out)?;
    Ok(out.trim().to_string())
}

pub fn status() -> Option<Status> {
    serde_json::from_str(&request("status").ok()?).ok()
}

/// Start `whispertui daemon run` detached from this terminal, unless one is
/// already listening. Returns true if it had to start one.
pub fn ensure_running() -> Result<bool> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    if status().is_some() {
        return Ok(false);
    }
    let log = config::open_log()?;
    let mut child = Command::new(std::env::current_exe()?)
        .args(["daemon", "run"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        // Own process group so ^C in this terminal doesn't reach it.
        .process_group(0)
        .spawn()
        .context("cannot start the daemon")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        if status().is_some() {
            return Ok(true);
        }
        if let Some(code) = child.try_wait()? {
            // Losing a start-up race to another launcher is fine.
            std::thread::sleep(Duration::from_millis(300));
            if status().is_some() {
                return Ok(true);
            }
            bail!(
                "the daemon exited ({code}); see {}",
                config::log_path().display()
            );
        }
    }
    bail!(
        "the daemon did not start within 5s; see {}",
        config::log_path().display()
    )
}

/// Ask the daemon to exit and wait for it to go. False if it wasn't running.
pub fn stop() -> Result<bool> {
    let asked = request("shutdown").is_ok();
    // A daemon from before the socket moved is still a daemon.
    let legacy = retire_legacy_daemon();
    // Already on its way out (socket gone, process still finishing)?
    let exiting = lock_held(&config::lock_path());
    if !asked && !legacy && !exiting {
        return Ok(false);
    }
    // Done when the process is really gone, not merely when the socket is.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if !lock_held(&config::lock_path()) {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    bail!("the daemon did not stop within 5s")
}

/// Take the lock that marks "a daemon is running", or say that one is. The
/// lock lives as long as the returned file does, and goes with the process.
fn acquire_lock(path: &std::path::Path) -> Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => bail!("daemon already running"),
        Err(std::fs::TryLockError::Error(e)) => Err(e).context("locking the daemon lock file"),
    }
}

/// Whether some process holds the daemon lock right now.
fn lock_held(path: &std::path::Path) -> bool {
    match std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
    {
        Ok(file) => matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
        Err(_) => false,
    }
}

/// Ask a daemon still listening on the old socket (in the config directory) to
/// quit, so the new one doesn't end up sharing the hotkey with it.
fn retire_legacy_daemon() -> bool {
    let old = config::legacy_socket_path();
    let Ok(mut s) = UnixStream::connect(&old) else {
        let _ = std::fs::remove_file(&old); // a stale socket file, nothing behind it
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    if writeln!(s, "shutdown").is_err() {
        return false;
    }
    let mut ack = String::new();
    let _ = BufReader::new(s).read_line(&mut ack);
    // The old daemon removes its socket just before it exits.
    let deadline = Instant::now() + Duration::from_secs(3);
    while old.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(300));
    true
}

pub fn run() -> Result<()> {
    std::fs::create_dir_all(config::recordings_dir())?;
    retire_legacy_daemon();
    // Held for the life of the process: it is what makes a second daemon impossible.
    let _lock = acquire_lock(&config::lock_path())?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(config::run_dir(), std::fs::Permissions::from_mode(0o700));
    }
    // Safe to clear: we hold the lock, so nothing live is behind an old socket file.
    let _ = std::fs::remove_file(config::socket_path());

    match history::recover() {
        Ok(0) => {}
        Ok(n) => eprintln!("{n} unfinished recording(s) marked failed; press t to transcribe"),
        Err(e) => eprintln!("could not recover unfinished recordings: {e:#}"),
    }
    let cfg = Config::load()?;
    if let Err(e) = hotkey::parse(&cfg.hotkey) {
        bail!("invalid hotkey '{}': {e}", cfg.hotkey);
    }
    eprintln!(
        "whispertui daemon: hotkey={} mode={:?}",
        cfg.hotkey, cfg.mode
    );
    let shared = Arc::new(Shared {
        cfg: Arc::new(RwLock::new(cfg)),
        recording: Arc::new(AtomicBool::new(false)),
        started: Mutex::new(None),
        jobs: AtomicUsize::new(0),
    });
    let (tx, rx) = mpsc::channel::<Cmd>();

    spawn_config_watcher(shared.clone());
    spawn_socket(shared.clone(), tx.clone())?;
    spawn_controller(shared.clone(), rx);

    // The event tap must run on the main thread on macOS.
    if let Err(e) = hotkey::listen(shared.cfg.clone(), tx, shared.recording.clone()) {
        eprintln!(
            "hotkey listener failed: {e}\n\
             Grant Input Monitoring to your terminal (or this binary) in\n\
             System Settings > Privacy & Security > Input Monitoring, then restart.\n\
             The daemon keeps running; `whispertui toggle` still works."
        );
        loop {
            std::thread::park();
        }
    }
    Ok(())
}

fn spawn_config_watcher(shared: Arc<Shared>) {
    std::thread::spawn(move || {
        let mtime = || {
            std::fs::metadata(config::config_path())
                .and_then(|m| m.modified())
                .ok()
        };
        let mut last = mtime();
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let now = mtime();
            if now != last {
                last = now;
                match Config::read(&config::config_path()) {
                    Ok(c) if hotkey::parse(&c.hotkey).is_ok() => {
                        eprintln!("config reloaded (hotkey={})", c.hotkey);
                        *shared.cfg.write().unwrap() = c;
                    }
                    Ok(c) => eprintln!("config ignored: invalid hotkey '{}'", c.hotkey),
                    Err(e) => eprintln!("config ignored: {e:#}"),
                }
            }
        }
    });
}

fn spawn_socket(shared: Arc<Shared>, tx: Sender<Cmd>) -> Result<()> {
    let listener = UnixListener::bind(config::socket_path()).context("binding daemon socket")?;
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let mut line = String::new();
            let mut conn = conn;
            if BufReader::new(&conn).read_line(&mut line).is_err() {
                continue;
            }
            let reply = match line.trim() {
                "toggle" => send(&tx, Cmd::Toggle),
                "start" => send(&tx, Cmd::Start),
                "stop" => send(&tx, Cmd::Stop),
                "cancel" => send(&tx, Cmd::Cancel),
                r if r.starts_with("retry ") => retry(&shared, r[6..].trim()),
                "shutdown" => {
                    let _ = std::fs::remove_file(config::socket_path());
                    std::thread::spawn(|| {
                        std::thread::sleep(Duration::from_millis(150)); // let the reply flush
                        std::process::exit(0);
                    });
                    "ok".into()
                }
                "status" => {
                    let c = shared.cfg.read().unwrap();
                    let rec = shared.recording.load(Ordering::Relaxed);
                    let st = Status {
                        state: if rec {
                            "recording"
                        } else if shared.jobs.load(Ordering::Relaxed) > 0 {
                            "transcribing"
                        } else {
                            "idle"
                        }
                        .into(),
                        secs: shared
                            .started
                            .lock()
                            .unwrap()
                            .map(|t| t.elapsed().as_secs())
                            .unwrap_or(0),
                        hotkey: c.hotkey.clone(),
                        mode: format!("{:?}", c.mode).to_lowercase(),
                    };
                    serde_json::to_string(&st).unwrap()
                }
                other => format!("error: unknown command '{other}'"),
            };
            let _ = writeln!(conn, "{reply}");
        }
    });
    Ok(())
}

fn send(tx: &Sender<Cmd>, c: Cmd) -> String {
    let _ = tx.send(c);
    "ok".into()
}

fn spawn_controller(shared: Arc<Shared>, rx: mpsc::Receiver<Cmd>) {
    std::thread::spawn(move || {
        // cpal streams are !Send, so the recorder lives on this thread.
        let mut rec: Option<Recorder> = None;
        loop {
            let max = Duration::from_secs(shared.cfg.read().unwrap().max_record_secs);
            let cmd = match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(c) => c,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let over = shared
                        .started
                        .lock()
                        .unwrap()
                        .is_some_and(|t| t.elapsed() > max);
                    if over { Cmd::Stop } else { continue }
                }
                Err(_) => return,
            };
            let cfg = shared.cfg.read().unwrap().clone();
            let cmd = match (cmd, rec.is_some()) {
                (Cmd::Toggle, true) => Cmd::Stop,
                (Cmd::Toggle, false) => Cmd::Start,
                (c, _) => c,
            };
            match cmd {
                // Fail before the user starts talking, not after.
                Cmd::Start if rec.is_none() && refusal(&cfg).is_some() => {
                    let why = refusal(&cfg).unwrap_or_default();
                    eprintln!("cannot start recording: {why}");
                    if cfg.sounds {
                        sys::sound("Basso");
                    }
                    // Always notify, whatever `notifications` says: the hotkey does
                    // nothing visible otherwise, and this needs the user to act.
                    sys::notify("whispertui: can't record", &why);
                }
                Cmd::Start if rec.is_none() => match Recorder::start(&cfg) {
                    Ok(r) => {
                        rec = Some(r);
                        *shared.started.lock().unwrap() = Some(Instant::now());
                        shared.recording.store(true, Ordering::Relaxed);
                        if cfg.sounds {
                            sys::sound("Tink");
                        }
                    }
                    Err(e) => {
                        eprintln!("cannot start recording: {e:#}");
                        if cfg.notifications {
                            sys::notify("whispertui", &format!("Cannot record: {e:#}"));
                        }
                    }
                },
                Cmd::Stop if rec.is_some() => {
                    let started = shared.started.lock().unwrap().take();
                    shared.recording.store(false, Ordering::Relaxed);
                    let samples = rec.take().unwrap().finish();
                    let secs = started.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
                    if cfg.sounds {
                        sys::sound("Pop");
                    }
                    shared.jobs.fetch_add(1, Ordering::Relaxed);
                    let sh = shared.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = process(&cfg, samples, secs) {
                            eprintln!("transcription failed: {e:#}");
                            if cfg.sounds {
                                sys::sound("Basso");
                            }
                            if cfg.notifications {
                                sys::notify("whispertui", &format!("Transcription failed: {e:#}"));
                            }
                        }
                        sh.jobs.fetch_sub(1, Ordering::Relaxed);
                    });
                }
                Cmd::Cancel if rec.is_some() => {
                    rec = None;
                    *shared.started.lock().unwrap() = None;
                    shared.recording.store(false, Ordering::Relaxed);
                    if cfg.sounds {
                        sys::sound("Basso");
                    }
                }
                _ => {}
            }
        }
    });
}

fn process(cfg: &Config, samples: Vec<f32>, secs: f32) -> Result<()> {
    if audio::is_blank(&samples, &cfg.silence) {
        eprintln!("blank audio ({secs:.1}s) suppressed");
        return Ok(());
    }
    let now = chrono::Local::now();
    let id = now.timestamp_millis().to_string();
    let wav = config::recordings_dir().join(format!("{id}.wav"));
    std::fs::create_dir_all(config::recordings_dir())?;
    audio::write_wav(&wav, &samples)?;

    // Listed straight away, so a recording that never finishes is still visible.
    let mut entry = Entry {
        id,
        created: now,
        text: String::new(),
        duration_secs: secs,
        audio: Some(wav),
        status: Progress::Transcribing,
        error: None,
    };
    history::add(entry.clone(), cfg.max_history)?;
    let text = finish(cfg, &mut entry, true)?;
    if text.is_empty() {
        return Ok(());
    }
    if cfg.copy_to_clipboard {
        sys::copy(&text)?;
        if cfg.auto_paste {
            sys::paste();
        }
    }
    Ok(())
}

/// Transcribe `entry`'s audio and record the outcome in the history. On
/// failure the audio stays and the entry becomes `Failed`. Returns the text.
fn finish(cfg: &Config, entry: &mut Entry, announce: bool) -> Result<String> {
    let wav = entry.audio.clone().context("no audio to transcribe")?;
    let text = match transcribe(cfg, &wav) {
        Ok(t) => t,
        Err(e) => {
            entry.status = Progress::Failed;
            entry.error = Some(format!("{e:#}"));
            history::update(entry.clone())?;
            return Err(e.context(format!("audio kept at {}", wav.display())));
        }
    };
    if text.is_empty() {
        let _ = std::fs::remove_file(&wav);
        history::delete(&entry.id)?;
        eprintln!("empty transcript suppressed");
        return Ok(text);
    }
    entry.text = text.clone();
    entry.status = Progress::Done;
    entry.error = None;
    if !cfg.keep_audio {
        let _ = std::fs::remove_file(&wav);
        entry.audio = None;
    }
    history::update(entry.clone())?;
    if announce && cfg.sounds {
        sys::sound("Glass");
    }
    if announce && cfg.notifications {
        sys::notify("Transcribed", &text.chars().take(120).collect::<String>());
    }
    Ok(text)
}

/// `retry <id>`: transcribe a failed recording again, in the background.
fn retry(shared: &Arc<Shared>, id: &str) -> String {
    let cfg = shared.cfg.read().unwrap().clone();
    if !models::installed(&cfg.model) {
        return format!("error: {}", missing_model(&cfg.model));
    }
    let Some(mut entry) = history::find(id) else {
        return "error: no such recording".into();
    };
    if entry.status != Progress::Failed {
        return "error: that recording isn't failed".into();
    }
    if !entry.audio.as_ref().is_some_and(|p| p.is_file()) {
        return "error: its audio file is gone".into();
    }
    entry.status = Progress::Transcribing;
    entry.error = None;
    if let Err(e) = history::update(entry.clone()) {
        return format!("error: {e:#}");
    }
    shared.jobs.fetch_add(1, Ordering::Relaxed);
    let sh = shared.clone();
    std::thread::spawn(move || {
        match finish(&cfg, &mut entry, true) {
            Ok(t) if t.is_empty() && cfg.notifications => {
                sys::notify("whispertui", "No speech found in that recording");
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("retry failed: {e:#}");
                if cfg.notifications {
                    sys::notify("whispertui", &format!("Transcription failed: {e:#}"));
                }
            }
        }
        sh.jobs.fetch_sub(1, Ordering::Relaxed);
    });
    "ok".into()
}

fn find_whisper(bin: &str) -> Result<PathBuf> {
    let p = config::expand(bin);
    if p.components().count() > 1 {
        return Ok(p);
    }
    // launchd gives agents a minimal PATH, so look in the usual brew prefixes too.
    let mut dirs: Vec<PathBuf> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    dirs.into_iter()
        .map(|d| d.join(bin))
        .find(|p| p.is_file())
        .ok_or_else(|| anyhow!("'{bin}' not found (brew install whisper-cpp, or set whisper_bin)"))
}

/// Why recording can't start (no model, no usable microphone), if it can't.
fn refusal(cfg: &Config) -> Option<String> {
    if !models::installed(&cfg.model) {
        return Some(missing_model(&cfg.model));
    }
    audio::mic_state(&cfg.input_device).problem()
}

fn missing_model(name: &str) -> String {
    if name.trim().is_empty() {
        "No model is set. Open whispertui and press M to download one.".to_string()
    } else {
        format!(
            "The model '{name}' isn't downloaded. Open whispertui and press M, or run `whispertui models download {name}`."
        )
    }
}

fn transcribe(cfg: &Config, wav: &std::path::Path) -> Result<String> {
    let bin = find_whisper(&cfg.whisper_bin)?;
    let model = models::path(&cfg.model);
    if !model.is_file() {
        bail!("{}", missing_model(&cfg.model));
    }
    let mut cmd = Command::new(bin);
    cmd.arg("-m")
        .arg(&model)
        .arg("-f")
        .arg(wav)
        .args(["-l", &cfg.language, "-nt", "-np"]);
    if cfg.threads > 0 {
        cmd.args(["-t", &cfg.threads.to_string()]);
    }
    if !cfg.prompt.is_empty() {
        cmd.args(["--prompt", &cfg.prompt]);
    }
    let out = cmd.output().context("running whisper-cli")?;
    if !out.status.success() {
        bail!(
            "whisper-cli: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(clean(&String::from_utf8_lossy(&out.stdout)))
}

/// Strip whisper's non-speech markers like [BLANK_AUDIO] and (silence).
pub fn clean(raw: &str) -> String {
    let mut out = String::new();
    let mut depth_close: Option<char> = None;
    let mut tag = String::new();
    for ch in raw.chars() {
        match depth_close {
            Some(close) => {
                tag.push(ch);
                if ch == close {
                    // Keep the tag only if it looks like real text, not a marker.
                    let inner = &tag[..tag.len() - 1];
                    let marker = inner.len() < 40
                        && (inner
                            .chars()
                            .all(|c| c.is_uppercase() || c == '_' || c == ' ')
                            || matches!(
                                inner.to_lowercase().trim(),
                                "silence"
                                    | "music"
                                    | "blank audio"
                                    | "applause"
                                    | "laughter"
                                    | "noise"
                            ));
                    if !marker {
                        out.push(if close == ']' { '[' } else { '(' });
                        out.push_str(&tag);
                    }
                    depth_close = None;
                    tag.clear();
                }
            }
            None if ch == '[' => depth_close = Some(']'),
            None if ch == '(' => depth_close = Some(')'),
            None => out.push(ch),
        }
    }
    if depth_close.is_some() {
        out.push(if depth_close == Some(']') { '[' } else { '(' });
        out.push_str(&tag);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::{acquire_lock, clean, lock_held, missing_model};

    #[test]
    fn the_lock_allows_one_daemon_and_is_released_with_it() {
        let dir = std::env::temp_dir().join(format!("whispertui-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("run").join("daemon.lock");
        assert!(
            !lock_held(&path),
            "nothing holds it yet (and the folder is created on demand)"
        );
        let first = acquire_lock(&path).unwrap();
        assert!(lock_held(&path));
        let err = acquire_lock(&path).unwrap_err().to_string();
        assert!(err.contains("already running"), "{err}");
        drop(first);
        // Released when the daemon goes. (Poll: another thread forking a child
        // can briefly hold a copy of the descriptor until that child execs.)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while lock_held(&path) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!lock_held(&path), "released when the daemon goes");
        assert!(acquire_lock(&path).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_model_message_says_what_to_do() {
        let unset = missing_model("");
        assert!(
            unset.starts_with("No model is set") && unset.contains("press M"),
            "{unset}"
        );
        assert_eq!(missing_model("  "), unset);
        let gone = missing_model("tiny.en");
        assert!(
            gone.contains("'tiny.en' isn't downloaded") && gone.contains("models download tiny.en"),
            "{gone}"
        );
    }

    #[test]
    fn strips_markers() {
        assert_eq!(clean(" [BLANK_AUDIO]\n"), "");
        assert_eq!(clean("(silence)"), "");
        assert_eq!(clean("[MUSIC] hello  there\n"), "hello there");
    }

    #[test]
    fn keeps_real_brackets() {
        assert_eq!(clean("call f(x) now"), "call f(x) now");
        assert_eq!(clean("see [the docs] ok"), "see [the docs] ok");
        assert_eq!(clean("unclosed (bracket"), "unclosed (bracket");
    }
}
