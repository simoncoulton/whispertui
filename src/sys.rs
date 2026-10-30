use anyhow::{Result, bail};
use std::io::Write;
use std::process::{Command, Stdio};

const MACOS: bool = cfg!(target_os = "macos");

/// Run `prog args` with `text` on stdin; Ok(false) when the program isn't installed.
fn pipe_to(prog: &str, args: &[&str], text: &str) -> Result<bool> {
    let mut child = match Command::new(prog)
        .args(args)
        .env("LC_ALL", "en_US.UTF-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    child.stdin.take().unwrap().write_all(text.as_bytes())?;
    if !child.wait()?.success() {
        bail!("{prog} failed");
    }
    Ok(true)
}

pub fn copy(text: &str) -> Result<()> {
    // pbcopy honours the locale; launchd agents have none, which mangles UTF-8.
    if MACOS {
        return pipe_to("pbcopy", &[], text).and_then(|ran| {
            if ran {
                Ok(())
            } else {
                bail!("pbcopy not found")
            }
        });
    }
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let tools: [(&str, &[&str]); 3] = [
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    for (prog, args) in tools {
        // wl-copy only works under Wayland; the X tools work under XWayland too.
        if prog == "wl-copy" && !wayland {
            continue;
        }
        if pipe_to(prog, args, text)? {
            return Ok(());
        }
    }
    bail!("no clipboard tool found (install wl-clipboard, or xclip on X11)")
}

/// Spawn the first of `candidates` that is installed.
fn spawn_first(candidates: &[(&str, Vec<String>)]) {
    for (prog, args) in candidates {
        if Command::new(prog)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
        {
            return;
        }
    }
}

pub fn paste() {
    if MACOS {
        let _ = Command::new("osascript")
            .args([
                "-e",
                r#"tell application "System Events" to keystroke "v" using command down"#,
            ])
            .spawn();
        return;
    }
    let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    spawn_first(&[
        ("wtype", s(&["-M", "ctrl", "v", "-m", "ctrl"])),
        ("ydotool", s(&["key", "29:1", "47:1", "47:0", "29:0"])),
        ("xdotool", s(&["key", "ctrl+v"])),
    ]);
}

pub fn notify(title: &str, body: &str) {
    if MACOS {
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            r#"display notification "{}" with title "{}""#,
            esc(body),
            esc(title)
        );
        let _ = Command::new("osascript").args(["-e", &script]).spawn();
        return;
    }
    spawn_first(&[(
        "notify-send",
        vec!["--app-name=whispertui".into(), title.into(), body.into()],
    )]);
}

/// `name` is a macOS system sound; Linux plays the closest freedesktop one.
pub fn sound(name: &str) {
    if MACOS {
        let _ = Command::new("afplay")
            .arg(format!("/System/Library/Sounds/{name}.aiff"))
            .spawn();
        return;
    }
    let event = match name {
        "Tink" => "message",
        "Pop" => "complete",
        _ => "dialog-warning",
    };
    let file = format!("/usr/share/sounds/freedesktop/stereo/{event}.oga");
    spawn_first(&[("pw-play", vec![file.clone()]), ("paplay", vec![file])]);
}
