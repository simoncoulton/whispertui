use anyhow::{Result, bail};
use std::io::Write;
use std::process::{Command, Stdio};

pub fn copy(text: &str) -> Result<()> {
    // pbcopy honours the locale; launchd agents have none, which mangles UTF-8.
    let mut child = Command::new("pbcopy")
        .env("LC_ALL", "en_US.UTF-8")
        .stdin(Stdio::piped())
        .spawn()?;
    child.stdin.take().unwrap().write_all(text.as_bytes())?;
    if !child.wait()?.success() {
        bail!("pbcopy failed");
    }
    Ok(())
}

pub fn paste() {
    let _ = Command::new("osascript")
        .args([
            "-e",
            r#"tell application "System Events" to keystroke "v" using command down"#,
        ])
        .spawn();
}

pub fn notify(title: &str, body: &str) {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        r#"display notification "{}" with title "{}""#,
        esc(body),
        esc(title)
    );
    let _ = Command::new("osascript").args(["-e", &script]).spawn();
}

pub fn sound(name: &str) {
    let _ = Command::new("afplay")
        .arg(format!("/System/Library/Sounds/{name}.aiff"))
        .spawn();
}
