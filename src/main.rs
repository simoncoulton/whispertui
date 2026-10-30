mod app;
mod audio;
mod config;
mod daemon;
mod history;
mod hotkey;
mod models;
mod sys;
mod theme;
mod tui;
mod ui;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::process::Command;

/// Tests that spawn child processes or hold the daemon lock take this, so a
/// child forked mid-test can't briefly inherit the lock's file descriptor.
#[cfg(test)]
pub(crate) static PROCESS_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Parser)]
#[command(about = "Hotkey voice-to-text with a history browser")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Browse and copy past transcriptions (default)
    Tui,
    /// Manage the background hotkey/recording service
    Daemon {
        #[command(subcommand)]
        action: Option<DaemonCmd>,
    },
    /// Start/stop recording in the running daemon
    Toggle,
    /// Show daemon status
    Status,
    /// Print the most recent transcription
    Last,
    /// List, download and select whisper models
    Models {
        #[command(subcommand)]
        action: Option<ModelCmd>,
    },
    /// Print the config file path (creating it if needed)
    Config,
    /// Start the daemon at login via launchd
    Install,
    /// Remove the launchd agent
    Uninstall,
}

#[derive(Subcommand)]
enum ModelCmd {
    /// Download a model (see `whispertui models` for names)
    Download { name: String },
    /// Make an installed model the active one
    Use { name: String },
    /// Fetch the model list from huggingface.co again
    Refresh,
}

#[derive(Subcommand)]
enum DaemonCmd {
    /// Run in the foreground (default)
    Run,
    /// Start in the background if not already running
    Start,
    /// Stop the running daemon
    Stop,
    /// Stop, then start again (applies new binary / permissions)
    Restart,
    /// Exit code 3 if not running
    Status,
}

const LABEL: &str = "com.whispertui.daemon";

fn plist_path() -> std::path::PathBuf {
    config::home()
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

fn uid() -> String {
    String::from_utf8_lossy(
        &Command::new("id")
            .arg("-u")
            .output()
            .map(|o| o.stdout)
            .unwrap_or_default(),
    )
    .trim()
    .to_string()
}

fn install() -> Result<()> {
    config::Config::load()?;
    let exe = std::env::current_exe()?.canonicalize()?;
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{exe}</string><string>daemon</string><string>run</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ProcessType</key><string>Interactive</string>
  <key>StandardErrorPath</key><string>{log}</string>
  <key>StandardOutPath</key><string>{log}</string>
</dict></plist>
"#,
        exe = exe.display(),
        log = config::log_path().display()
    );
    // launchd opens the log itself but won't create its directory.
    drop(config::open_log()?);
    let path = plist_path();
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, plist)?;
    let domain = format!("gui/{}", uid());
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{domain}/{LABEL}")])
        .output();
    let out = Command::new("launchctl")
        .args(["bootstrap", &domain])
        .arg(&path)
        .output()?;
    if !out.status.success() {
        bail!(
            "launchctl bootstrap: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    println!(
        "installed {} — daemon starts at login (log: {})",
        path.display(),
        config::log_path().display()
    );
    println!(
        "macOS will prompt for Microphone and Input Monitoring access for {}.",
        exe.display()
    );
    Ok(())
}

fn uninstall() -> Result<()> {
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("gui/{}/{LABEL}", uid())])
        .output();
    let _ = std::fs::remove_file(plist_path());
    println!("uninstalled");
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().cmd.unwrap_or(Cmd::Tui) {
        Cmd::Tui => tui::run(),
        Cmd::Daemon { action } => match action.unwrap_or(DaemonCmd::Run) {
            DaemonCmd::Run => daemon::run(),
            DaemonCmd::Start => {
                println!(
                    "{}",
                    if daemon::ensure_running()? {
                        "started"
                    } else {
                        "already running"
                    }
                );
                Ok(())
            }
            DaemonCmd::Stop => {
                println!(
                    "{}",
                    if daemon::stop()? {
                        "stopped"
                    } else {
                        "not running"
                    }
                );
                Ok(())
            }
            DaemonCmd::Restart => {
                daemon::stop()?;
                daemon::ensure_running()?;
                println!("restarted");
                Ok(())
            }
            DaemonCmd::Status => match daemon::status() {
                Some(s) => {
                    println!("running · {} · hotkey {} ({})", s.state, s.hotkey, s.mode);
                    Ok(())
                }
                None => {
                    println!("not running");
                    std::process::exit(3)
                }
            },
        },
        Cmd::Toggle => {
            daemon::request("toggle").context("daemon not running (try `whispertui install`)")?;
            Ok(())
        }
        Cmd::Status => {
            match daemon::status() {
                Some(s) => println!("{} · hotkey {} ({})", s.state, s.hotkey, s.mode),
                None => println!("daemon not running"),
            }
            Ok(())
        }
        Cmd::Last => {
            match history::load().first() {
                Some(e) => println!("{}", e.text),
                None => bail!("no recordings yet"),
            }
            Ok(())
        }
        Cmd::Models { action } => {
            match action {
                None | Some(ModelCmd::Refresh) => {
                    let refresh = matches!(action, Some(ModelCmd::Refresh));
                    let cfg = config::Config::load()?;
                    let cat = models::catalog(refresh)?;
                    for r in models::rows(&cfg, Some(&cat)) {
                        let mark = if r.active {
                            "*"
                        } else if r.installed {
                            "+"
                        } else {
                            " "
                        };
                        println!(
                            "{mark} {:<22} {:>8}  {}",
                            r.name,
                            models::size_label(r.bytes),
                            r.note
                        );
                    }
                    println!(
                        "\n* active  + downloaded    list from huggingface.co, cached {} ({})\nwhispertui models download <name> | use <name> | refresh",
                        cat.fetched_label(),
                        if cat.is_stale() {
                            "stale, run `models refresh`"
                        } else {
                            "fresh"
                        }
                    );
                }
                Some(ModelCmd::Download { name }) => {
                    let cat = models::catalog(false)?;
                    if !cat.models.iter().any(|m| m.name == name) {
                        bail!(
                            "unknown model '{name}' (see `whispertui models`; `models refresh` updates the list)"
                        );
                    }
                    let path = models::download(&name, false, None)?;
                    println!("saved {}", path.display());
                    if !models::installed(&config::Config::load()?.model) {
                        models::select(&name)?;
                        println!("now using {name}");
                    }
                }
                Some(ModelCmd::Use { name }) => {
                    models::select(&name)?;
                    println!("now using {name}");
                }
            }
            Ok(())
        }
        Cmd::Config => {
            config::Config::load()?;
            println!("{}", config::config_path().display());
            Ok(())
        }
        Cmd::Install => install(),
        Cmd::Uninstall => uninstall(),
    }
}
