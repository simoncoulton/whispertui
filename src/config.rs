use crate::theme::ThemeSpec;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_yaml_ng::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Press the hotkey to start, press again to stop.
    Toggle,
    /// Record while the hotkey is held down.
    Hold,
}

/// How the recordings list and the details pane sit together.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    /// Side by side: list 30%, details 70%.
    Vertical,
    /// Stacked: list above details.
    Horizontal,
}

impl Layout {
    pub fn toggled(self) -> Layout {
        match self {
            Layout::Vertical => Layout::Horizontal,
            Layout::Horizontal => Layout::Vertical,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Layout::Vertical => "vertical",
            Layout::Horizontal => "horizontal",
        }
    }
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Silence {
    /// Drop recordings that contain no speech instead of transcribing them.
    pub enabled: bool,
    /// Frames quieter than this (dBFS, 20ms RMS) count as silence.
    pub threshold_db: f32,
    /// Minimum total audio above the threshold for a recording to be kept.
    pub min_speech_ms: u32,
}

impl Default for Silence {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_db: -45.0,
            min_speech_ms: 300,
        }
    }
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub hotkey: String,
    pub mode: Mode,
    pub cancel_key: String,
    /// A whisper.cpp model by name, e.g. `large-v3-turbo-q5_0`.
    pub model: String,
    pub whisper_bin: String,
    pub language: String,
    pub prompt: String,
    pub threads: u32,
    pub input_device: String,
    pub copy_to_clipboard: bool,
    pub auto_paste: bool,
    pub sounds: bool,
    pub notifications: bool,
    pub keep_audio: bool,
    pub max_history: usize,
    pub max_record_secs: u64,
    pub theme: String,
    pub transparent_background: bool,
    pub layout: Layout,
    pub silence: Silence,
    pub themes: BTreeMap<String, ThemeSpec>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "right_option".into(),
            mode: Mode::Toggle,
            cancel_key: "escape".into(),
            model: "large-v3-turbo-q5_0".into(),
            whisper_bin: "whisper-cli".into(),
            language: "en".into(),
            prompt: String::new(),
            threads: 0,
            input_device: String::new(),
            copy_to_clipboard: true,
            auto_paste: false,
            sounds: true,
            notifications: true,
            keep_audio: true,
            max_history: 500,
            max_record_secs: 600,
            theme: "default".into(),
            transparent_background: true,
            layout: Layout::Vertical,
            silence: Silence::default(),
            themes: BTreeMap::new(),
        }
    }
}

const TEMPLATE: &str = r##"# whispertui configuration. The daemon reloads this file automatically.
# Unknown keys are errors.

# Hotkey that starts/stops recording. Join keys with "+".
#   modifiers: cmd, option, ctrl, shift (or left_/right_ variants), fn
#   keys:      a-z, 0-9, space, return, tab, f1-f20, ...
# Examples: right_option, cmd+shift+space, ctrl+option+r, f13
# A modifier-only hotkey fires on a clean tap (no other key pressed meanwhile).
hotkey: right_option

# toggle: press to start, press again to stop.  hold: record while held.
mode: toggle

# Pressing this key while recording discards the recording. "" disables it.
cancel_key: escape

# The whisper.cpp model, by name. Press M in the TUI (or run
# `whispertui models`) to see what is available and download one.
model: large-v3-turbo-q5_0
whisper_bin: whisper-cli   # brew install whisper-cpp
language: en               # "auto" to detect
prompt: ""                 # optional initial prompt (names, jargon)
threads: 0                 # 0 = whisper default

# Microphone name (substring match). "" = system default. Press I in the TUI to choose.
input_device: ""

# What happens when a transcription finishes.
copy_to_clipboard: true     # copy the finished transcript to the clipboard (false: history only)
auto_paste: false          # also press Cmd+V (needs Accessibility permission)
sounds: true
notifications: true

# Recordings.
keep_audio: true
max_history: 500
max_record_secs: 600

# Appearance of the TUI. Built-in themes: default, tokyo-night, catppuccin-frappe,
# catppuccin-mocha, catppuccin-macchiato, catppuccin-latte, dracula, one-dark,
# gruvbox-dark, nord, solarized-dark, rose-pine, monokai, github-dark, everforest,
# kanagawa. Every *.yml in ~/.config/whispertui/themes/ is a theme too. Press T in
# the TUI to preview and save.
theme: default
transparent_background: true   # show the terminal's own background
# vertical: list and details side by side (30% / 70%); horizontal: stacked. L toggles.
layout: vertical

# Blank audio suppression: recordings without speech are dropped silently.
silence:
  enabled: true
  threshold_db: -45.0      # raise (e.g. -35) in noisy rooms
  min_speech_ms: 300

# Custom theme: start from a built-in and override colours (#rrggbb or ANSI names).
# themes:
#   mine:
#     base: nord
#     accent: "#ff9e64"
"##;

pub fn dir() -> PathBuf {
    home().join(".config").join("whispertui")
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn config_path() -> PathBuf {
    dir().join("config.yml")
}
pub fn themes_dir() -> PathBuf {
    dir().join("themes")
}
pub fn history_path() -> PathBuf {
    dir().join("history.json")
}
pub fn recordings_dir() -> PathBuf {
    dir().join("recordings")
}
/// Runtime files of the daemon, next to its log: the socket and the lock.
pub fn run_dir_in(data: &Path) -> PathBuf {
    data.join("run")
}

pub fn run_dir() -> PathBuf {
    run_dir_in(&data_dir())
}

pub fn socket_path() -> PathBuf {
    run_dir().join("daemon.sock")
}

/// Held by the running daemon for as long as it lives.
pub fn lock_path() -> PathBuf {
    run_dir().join("daemon.lock")
}

/// Where earlier versions kept the socket, in the config directory.
pub fn legacy_socket_path() -> PathBuf {
    dir().join("daemon.sock")
}
/// Where data and logs live, apart from config: `~/Library/Application
/// Support/whispertui` on macOS, `$XDG_DATA_HOME` or `~/.local/share` elsewhere.
pub fn data_dir_in(home: &Path, xdg_data: Option<PathBuf>) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library")
            .join("Application Support")
            .join("whispertui")
    } else {
        xdg_data
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".local").join("share"))
            .join("whispertui")
    }
}

pub fn data_dir() -> PathBuf {
    data_dir_in(
        &home(),
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
    )
}

pub fn log_path() -> PathBuf {
    data_dir().join("daemon.log")
}

/// The daemon log, opened for appending. Creates its directory, starts afresh
/// when it has grown past 1 MiB, and picks up a log left in the config
/// directory by an earlier version.
pub fn open_log() -> Result<std::fs::File> {
    let path = log_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let old = dir().join("daemon.log");
    if old.is_file() && !path.exists() {
        let _ = std::fs::rename(&old, &path);
    }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1024 * 1024) {
        let _ = std::fs::remove_file(&path);
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))
}

pub fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(p),
    }
}

/// `~/...` form of a path under the home directory, for display.
pub fn tilde(p: &Path) -> String {
    match p.strip_prefix(home()) {
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => p.display().to_string(),
    }
}

pub fn write_atomic(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn parse(text: &str) -> Result<Config, serde_yaml_ng::Error> {
    // A file that is only comments is all defaults.
    if text
        .lines()
        .all(|l| l.trim().is_empty() || l.trim_start().starts_with('#'))
    {
        return Ok(Config::default());
    }
    serde_yaml_ng::from_str(text)
}

impl Config {
    /// Load the config. On first run, write the commented template, carrying
    /// over settings from a `config.toml` left by an earlier version.
    pub fn load() -> Result<Config> {
        let path = config_path();
        if !path.exists() {
            let legacy = dir().join("config.toml");
            let text = match std::fs::read_to_string(&legacy) {
                Ok(old) => {
                    let text = migrate_legacy(&old, &crate::models::dir());
                    let _ = std::fs::rename(&legacy, dir().join("config.toml.bak"));
                    eprintln!(
                        "Converted {} to {} (the old file is config.toml.bak).",
                        legacy.display(),
                        path.display()
                    );
                    text
                }
                Err(_) => TEMPLATE.to_string(),
            };
            write_atomic(&path, &text)?;
        }
        Self::read(&path)
    }

    pub fn read(path: &Path) -> Result<Config> {
        let s =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        parse(&s).with_context(|| format!("parsing {}", path.display()))
    }
}

/// A YAML scalar for `s`, quoted only if it needs it.
pub fn quote(s: &str) -> String {
    serde_yaml_ng::to_string(s)
        .map(|t| t.trim_end().to_string())
        .unwrap_or_else(|_| format!("{s:?}"))
}

/// Replace (or add) one key, keeping every other line and comment. `key` is a
/// top-level key, or `parent.child` for a key one level down. Refuses to touch
/// a file that is already broken, and refuses to write anything that would not
/// read back as the value asked for.
pub fn set_value(path: &Path, key: &str, value: &str) -> Result<()> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&text).with_context(|| format!("{} is already invalid; fix it first", path.display()))?;
    let updated = replace_key(&text, key, value);

    let want: Value =
        serde_yaml_ng::from_str(value).with_context(|| format!("bad value for {key}: {value}"))?;
    let got: Value =
        serde_yaml_ng::from_str(&updated).context("the edit would produce invalid YAML")?;
    let at = key.split('.').try_fold(&got, |v, k| v.get(k));
    if at != Some(&want) {
        bail!("refusing to write {key}: it would not read back as {value}");
    }
    parse(&updated)
        .with_context(|| format!("{key}: {value} is not valid in {}", path.display()))?;
    write_atomic(path, &updated)
}

/// ` # comment` after a value (whitespace kept), if there is one. A `#` inside
/// quotes is not a comment, so only a split that leaves a complete value counts.
fn trailing_comment(rest: &str) -> &str {
    for (i, _) in rest.match_indices(" #") {
        let value = rest[..i].trim();
        if value.is_empty() || serde_yaml_ng::from_str::<Value>(value).is_ok() {
            return &rest[rest[..i].trim_end().len()..];
        }
    }
    ""
}

fn replace_key(text: &str, key: &str, value: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let (parent, child) = match key.split_once('.') {
        Some((p, c)) => (Some(p), c),
        None => (None, key),
    };
    // Where the key lives: a whole file, or the indented block under `parent:`.
    let (start, end, indent) = match parent {
        None => (0, lines.len(), String::new()),
        Some(p) => match lines.iter().position(|l| l.starts_with(&format!("{p}:"))) {
            Some(at) => {
                let end = lines[at + 1..]
                    .iter()
                    .position(|l| {
                        !l.trim().is_empty() && !l.starts_with(' ') && !l.starts_with('#')
                    })
                    .map_or(lines.len(), |n| at + 1 + n);
                let indent = lines[at + 1..end]
                    .iter()
                    .find(|l| {
                        !l.trim().is_empty()
                            && l.starts_with(' ')
                            && !l.trim_start().starts_with('#')
                    })
                    .map(|l| " ".repeat(l.len() - l.trim_start().len()))
                    .unwrap_or_else(|| "  ".into());
                (at + 1, end, indent)
            }
            None => {
                lines.push(format!("{}:", parent.unwrap()));
                lines.push(format!("  {child}: {value}"));
                return lines.join("\n") + "\n";
            }
        },
    };
    let prefix = format!("{indent}{child}:");
    if let Some(i) = (start..end).find(|&i| lines[i].starts_with(&prefix)) {
        let comment = trailing_comment(&lines[i][prefix.len()..]).to_string();
        lines[i] = format!("{indent}{child}: {value}{comment}");
    } else if parent.is_some() {
        // After the block's last real line, not after trailing blanks or comments.
        let last = (start..end)
            .rev()
            .find(|&i| !lines[i].trim().is_empty() && !lines[i].trim_start().starts_with('#'))
            .map_or(start, |i| i + 1);
        lines.insert(last, format!("{indent}{child}: {value}"));
    } else {
        // A top-level key at column 0 is valid after any preceding block.
        lines.push(format!("{child}: {value}"));
    }
    lines.join("\n") + "\n"
}

/// Settings from the TOML config of an earlier version, applied to the YAML
/// template. `model_path` is carried over only if it points into our own
/// models directory; any other model is left behind.
fn migrate_legacy(old: &str, models_dir: &Path) -> String {
    const KEYS: &[&str] = &[
        "hotkey",
        "mode",
        "cancel_key",
        "whisper_bin",
        "language",
        "prompt",
        "threads",
        "input_device",
        "copy_to_clipboard",
        "auto_paste",
        "sounds",
        "notifications",
        "keep_audio",
        "max_history",
        "max_record_secs",
        "theme",
        "transparent_background",
    ];
    const SILENCE: &[&str] = &["enabled", "threshold_db", "min_speech_ms"];
    let defaults: Value = serde_yaml_ng::from_str(TEMPLATE).unwrap_or(Value::Null);
    let mut out = TEMPLATE.to_string();
    let mut section = String::new();
    for line in old.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(name) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = name.trim().to_string();
            continue;
        }
        let Some((k, v)) = t.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), strip_toml_comment(v.trim()));
        let (key, value) = match (section.as_str(), k) {
            ("", "model_path") => {
                let path = config_expand_str(&v);
                let name = path
                    .file_name()
                    .and_then(|f| f.to_str())
                    .and_then(|f| f.strip_prefix("ggml-")?.strip_suffix(".bin"))
                    .filter(|_| path.parent() == Some(models_dir));
                match name {
                    Some(n) => ("model".to_string(), quote(n)),
                    None => continue,
                }
            }
            ("", k) if KEYS.contains(&k) => (k.to_string(), v),
            ("silence", k) if SILENCE.contains(&k) => (format!("silence.{k}"), v),
            _ => continue,
        };
        let Ok(parsed) = serde_yaml_ng::from_str::<Value>(&value) else {
            continue;
        };
        let default = key.split('.').try_fold(&defaults, |d, p| d.get(p));
        if default != Some(&parsed) {
            out = replace_key(&out, &key, &value);
        }
    }
    // Never hand back a file that would not load.
    if parse(&out).is_ok() {
        out
    } else {
        TEMPLATE.to_string()
    }
}

fn config_expand_str(v: &str) -> PathBuf {
    expand(v.trim_matches('"'))
}

fn strip_toml_comment(v: &str) -> String {
    let (mut quoted, mut escaped) = (false, false);
    for (i, c) in v.char_indices() {
        match c {
            '\\' if quoted => escaped = !escaped,
            '"' if !escaped => quoted = !quoted,
            '#' if !quoted => return v[..i].trim_end().to_string(),
            _ => escaped = false,
        }
    }
    v.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("whispertui-config-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("config.yml")
    }

    #[test]
    fn data_lives_apart_from_config() {
        let home = Path::new("/Users/me");
        let data = data_dir_in(home, Some(PathBuf::from("/xdg/data")));
        if cfg!(target_os = "macos") {
            assert_eq!(
                data,
                Path::new("/Users/me/Library/Application Support/whispertui")
            );
        } else {
            assert_eq!(data, Path::new("/xdg/data/whispertui"));
            assert_eq!(
                data_dir_in(home, None),
                Path::new("/Users/me/.local/share/whispertui")
            );
        }
        assert!(!data.starts_with(home.join(".config")));
    }

    #[test]
    fn the_socket_and_lock_sit_together_in_the_data_dirs_run_folder() {
        let data = data_dir_in(Path::new("/Users/me"), None);
        let run = run_dir_in(&data);
        assert_eq!(run, data.join("run"));
        if cfg!(target_os = "macos") {
            assert_eq!(
                run.join("daemon.sock"),
                Path::new("/Users/me/Library/Application Support/whispertui/run/daemon.sock")
            );
        }
        // The real functions agree with each other and keep out of the config dir.
        assert_eq!(socket_path().parent(), lock_path().parent());
        assert_eq!(socket_path().file_name().unwrap(), "daemon.sock");
        assert_eq!(lock_path().file_name().unwrap(), "daemon.lock");
        assert!(socket_path().starts_with(data_dir()) && !socket_path().starts_with(dir()));
        assert_eq!(legacy_socket_path().parent().unwrap(), dir());
    }

    #[test]
    fn default_template_equals_defaults() {
        assert_eq!(parse(TEMPLATE).unwrap(), Config::default());
    }

    #[test]
    fn a_comment_only_file_is_all_defaults() {
        assert_eq!(parse("# nothing\n\n").unwrap(), Config::default());
        assert_eq!(parse("").unwrap(), Config::default());
    }

    #[test]
    fn unknown_keys_are_errors_with_a_location() {
        let err = parse("hotkey: f13\nhotkey_typo: 1\n")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("hotkey_typo") && err.contains("line 2"),
            "{err}"
        );
        assert!(parse("silence:\n  threshhold_db: 1\n").is_err());
        assert!(parse("layout: diagonal\n").is_err());
    }

    #[test]
    fn replace_key_keeps_everything_else() {
        let src =
            "# top\ntheme: default   # pick one\nlayout: vertical\nthemes:\n  a:\n    base: x\n";
        assert_eq!(
            replace_key(src, "theme", "dracula"),
            "# top\ntheme: dracula   # pick one\nlayout: vertical\nthemes:\n  a:\n    base: x\n"
        );
        // not confused by a prefix match like `themes:`
        let out = replace_key("themes:\n  a: {}\n", "theme", "x");
        assert!(out.starts_with("themes:") && out.ends_with("theme: x\n"));
        // commented-out template lines are not treated as the key
        assert_eq!(
            replace_key("# theme: foo\n", "theme", "x"),
            "# theme: foo\ntheme: x\n"
        );
    }

    #[test]
    fn trailing_comments_ignore_hashes_inside_quotes() {
        assert_eq!(trailing_comment(" \"x #y\"   # note"), "   # note");
        assert_eq!(trailing_comment(" \"x #y\""), "");
        assert_eq!(trailing_comment(" 3"), "");
    }

    #[test]
    fn nested_keys_are_edited_inside_their_block() {
        let src =
            "a: 1\nsilence:\n  enabled: true\n  threshold_db: -45.0   # loud\n\n# note\nb: 2\n";
        assert_eq!(
            replace_key(src, "silence.threshold_db", "-30.0"),
            "a: 1\nsilence:\n  enabled: true\n  threshold_db: -30.0   # loud\n\n# note\nb: 2\n"
        );
        assert_eq!(
            replace_key(src, "silence.min_speech_ms", "9"),
            "a: 1\nsilence:\n  enabled: true\n  threshold_db: -45.0   # loud\n  min_speech_ms: 9\n\n# note\nb: 2\n"
        );
        assert_eq!(
            replace_key("a: 1\n", "silence.enabled", "false"),
            "a: 1\nsilence:\n  enabled: false\n"
        );
        // A same-named key at the top level is left alone.
        assert!(
            replace_key(
                "enabled: 1\nsilence:\n  enabled: true\n",
                "silence.enabled",
                "false"
            )
            .starts_with("enabled: 1\n")
        );
    }

    #[test]
    fn set_value_keeps_comments_and_other_keys() {
        let path = tmp("set");
        write_atomic(&path, TEMPLATE).unwrap();
        set_value(&path, "theme", &quote("nord")).unwrap();
        set_value(&path, "transparent_background", "false").unwrap();
        set_value(&path, "layout", "horizontal").unwrap();
        set_value(&path, "silence.threshold_db", "-35.0").unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        let expected = TEMPLATE
            .replace("theme: default", "theme: nord")
            .replace(
                "transparent_background: true",
                "transparent_background: false",
            )
            .replace("layout: vertical\n", "layout: horizontal\n")
            .replace("threshold_db: -45.0 ", "threshold_db: -35.0 ");
        assert_eq!(out, expected);
        let cfg = Config::read(&path).unwrap();
        assert_eq!(
            (
                cfg.theme.as_str(),
                cfg.transparent_background,
                cfg.layout,
                cfg.silence.threshold_db
            ),
            ("nord", false, Layout::Horizontal, -35.0)
        );
    }

    #[test]
    fn set_value_refuses_broken_files_and_bad_values() {
        let path = tmp("bad");
        std::fs::write(&path, "hotkey: [\n").unwrap();
        let err = format!("{:#}", set_value(&path, "theme", "nord").unwrap_err());
        assert!(err.contains("already invalid"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hotkey: [\n");

        std::fs::write(&path, TEMPLATE).unwrap();
        assert!(set_value(&path, "layout", "diagonal").is_err()); // not a layout
        assert!(set_value(&path, "transparent_background", "\"yes\"").is_err()); // wrong type
        assert!(set_value(&path, "theme", "[unclosed").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TEMPLATE);
    }

    #[test]
    fn quote_only_when_needed() {
        assert_eq!(quote("nord"), "nord");
        assert_eq!(quote("true"), "'true'");
        assert_eq!(quote(""), "''");
        assert_eq!(quote("has: colon"), "'has: colon'");
    }

    #[test]
    fn migrates_the_old_toml_config() {
        let models = PathBuf::from("/h/.config/whispertui/models");
        let old = "# comment\nhotkey = \"cmd+shift+space\"   # mine\nmode = \"hold\"\nmodel_path = \"~/Library/Application Support/ru.starmel.OpenSuperWhisper/whisper-models/ggml-large-v3-turbo-q8_0.bin\"\nlanguage = \"en\"\nsounds = false\n\n[silence]\nenabled = true\nthreshold_db = -35.0\n";
        let out = migrate_legacy(old, &models);
        let cfg = parse(&out).unwrap();
        assert_eq!(cfg.hotkey, "cmd+shift+space");
        assert_eq!(
            (cfg.mode, cfg.sounds, cfg.silence.threshold_db),
            (Mode::Hold, false, -35.0)
        );
        assert_eq!(
            cfg.model,
            Config::default().model,
            "a model outside our directory is left behind"
        );
        assert!(
            out.contains("# Hotkey that starts/stops recording"),
            "template comments are kept"
        );
        assert!(!out.contains("OpenSuperWhisper"));

        // A model we manage is kept by name.
        let ours = format!("model_path = \"{}/ggml-tiny.en.bin\"\n", models.display());
        assert_eq!(
            parse(&migrate_legacy(&ours, &models)).unwrap().model,
            "tiny.en"
        );
        // An untouched old default config migrates to the plain template.
        assert_eq!(
            migrate_legacy(
                "hotkey = \"right_option\"\n[silence]\nenabled = true\n",
                &models
            ),
            TEMPLATE
        );
    }
}
