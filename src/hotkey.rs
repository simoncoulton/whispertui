use crate::config::{Config, Mode};
use crate::daemon::Cmd;
use anyhow::{Result, bail};
use rdev::{EventType, Key};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};

/// A hotkey is a list of slots; each slot is satisfied by any one of its keys
/// (so `cmd` matches either Command key, `right_cmd` only the right one).
pub type Combo = Vec<Vec<Key>>;

pub fn parse(spec: &str) -> Result<Combo> {
    let mut combo = Vec::new();
    for tok in spec
        .split('+')
        .map(|t| t.trim().to_lowercase().replace(['-', ' '], "_"))
    {
        if tok.is_empty() {
            continue;
        }
        combo.push(key_slot(&tok)?);
    }
    if combo.is_empty() {
        bail!("empty hotkey");
    }
    Ok(combo)
}

fn key_slot(t: &str) -> Result<Vec<Key>> {
    use Key::*;
    Ok(match t {
        "cmd" | "command" | "meta" | "super" => vec![MetaLeft, MetaRight],
        "left_cmd" | "left_command" | "lcmd" => vec![MetaLeft],
        "right_cmd" | "right_command" | "rcmd" => vec![MetaRight],
        "option" | "opt" | "alt" => vec![Alt, AltGr],
        "left_option" | "left_alt" | "lopt" => vec![Alt],
        "right_option" | "right_alt" | "ropt" => vec![AltGr],
        "ctrl" | "control" => vec![ControlLeft, ControlRight],
        "left_ctrl" | "left_control" => vec![ControlLeft],
        "right_ctrl" | "right_control" => vec![ControlRight],
        "shift" => vec![ShiftLeft, ShiftRight],
        "left_shift" => vec![ShiftLeft],
        "right_shift" => vec![ShiftRight],
        "fn" | "function" => vec![Function],
        "space" => vec![Space],
        "return" | "enter" => vec![Return],
        "tab" => vec![Tab],
        "escape" | "esc" => vec![Escape],
        "backspace" | "delete" => vec![Backspace],
        "capslock" | "caps_lock" => vec![CapsLock],
        "backtick" | "grave" => vec![BackQuote],
        "minus" => vec![Minus],
        "equal" | "equals" => vec![Equal],
        "comma" => vec![Comma],
        "dot" | "period" => vec![Dot],
        "slash" => vec![Slash],
        "semicolon" => vec![SemiColon],
        "quote" => vec![Quote],
        "f1" => vec![F1],
        "f2" => vec![F2],
        "f3" => vec![F3],
        "f4" => vec![F4],
        "f5" => vec![F5],
        "f6" => vec![F6],
        "f7" => vec![F7],
        "f8" => vec![F8],
        "f9" => vec![F9],
        "f10" => vec![F10],
        "f11" => vec![F11],
        "f12" => vec![F12],
        // rdev has no names beyond F12; these are the macOS virtual key codes.
        "f13" => vec![Unknown(105)],
        "f14" => vec![Unknown(107)],
        "f15" => vec![Unknown(113)],
        "f16" => vec![Unknown(106)],
        "f17" => vec![Unknown(64)],
        "f18" => vec![Unknown(79)],
        "f19" => vec![Unknown(80)],
        "f20" => vec![Unknown(90)],
        s if s.len() == 1 => {
            let c = s.chars().next().unwrap();
            match c {
                'a' => vec![KeyA],
                'b' => vec![KeyB],
                'c' => vec![KeyC],
                'd' => vec![KeyD],
                'e' => vec![KeyE],
                'f' => vec![KeyF],
                'g' => vec![KeyG],
                'h' => vec![KeyH],
                'i' => vec![KeyI],
                'j' => vec![KeyJ],
                'k' => vec![KeyK],
                'l' => vec![KeyL],
                'm' => vec![KeyM],
                'n' => vec![KeyN],
                'o' => vec![KeyO],
                'p' => vec![KeyP],
                'q' => vec![KeyQ],
                'r' => vec![KeyR],
                's' => vec![KeyS],
                't' => vec![KeyT],
                'u' => vec![KeyU],
                'v' => vec![KeyV],
                'w' => vec![KeyW],
                'x' => vec![KeyX],
                'y' => vec![KeyY],
                'z' => vec![KeyZ],
                '0' => vec![Num0],
                '1' => vec![Num1],
                '2' => vec![Num2],
                '3' => vec![Num3],
                '4' => vec![Num4],
                '5' => vec![Num5],
                '6' => vec![Num6],
                '7' => vec![Num7],
                '8' => vec![Num8],
                '9' => vec![Num9],
                _ => bail!("unknown key '{t}'"),
            }
        }
        _ => bail!("unknown key '{t}'"),
    })
}

fn is_modifier(k: &Key) -> bool {
    use Key::*;
    matches!(
        k,
        MetaLeft
            | MetaRight
            | Alt
            | AltGr
            | ControlLeft
            | ControlRight
            | ShiftLeft
            | ShiftRight
            | Function
    )
}

fn modifier_only(c: &Combo) -> bool {
    c.iter().all(|slot| slot.iter().all(is_modifier))
}

fn satisfied(c: &Combo, pressed: &HashSet<Key>) -> bool {
    c.iter()
        .all(|slot| slot.iter().any(|k| pressed.contains(k)))
}

/// Blocks the calling thread (must be the main thread on macOS) delivering
/// recording commands to `tx`. Reads the live config on every event so hotkey
/// edits apply without restarting the daemon.
pub fn listen(cfg: Arc<RwLock<Config>>, tx: Sender<Cmd>, recording: Arc<AtomicBool>) -> Result<()> {
    let mut pressed: HashSet<Key> = HashSet::new();
    let mut cache = (String::new(), String::new(), Vec::new(), Vec::new());
    let (mut armed, mut tainted) = (false, false);

    let res = rdev::listen(move |ev| {
        let (key, down) = match ev.event_type {
            EventType::KeyPress(k) => (k, true),
            EventType::KeyRelease(k) => (k, false),
            _ => return,
        };
        let (hotkey, cancel, hold) = {
            let c = cfg.read().unwrap();
            (c.hotkey.clone(), c.cancel_key.clone(), c.mode == Mode::Hold)
        };
        if cache.0 != hotkey || cache.1 != cancel {
            let hk = parse(&hotkey).unwrap_or_else(|e| {
                eprintln!("bad hotkey '{hotkey}': {e}");
                Vec::new()
            });
            let cn = if cancel.is_empty() {
                Vec::new()
            } else {
                parse(&cancel).unwrap_or_default()
            };
            cache = (hotkey, cancel, hk, cn);
        }
        let combo = &cache.2;
        if combo.is_empty() {
            return;
        }
        let in_combo = combo.iter().any(|slot| slot.contains(&key));
        let send = |c: Cmd| {
            let _ = tx.send(c);
        };

        if down {
            if !pressed.insert(key) {
                return; // key repeat
            }
            if !in_combo {
                if recording.load(Ordering::Relaxed)
                    && cache.3.iter().any(|slot| slot.contains(&key))
                {
                    send(Cmd::Cancel);
                }
                if armed {
                    tainted = true;
                }
                return;
            }
            if !armed && satisfied(combo, &pressed) {
                armed = true;
                tainted = pressed
                    .iter()
                    .any(|p| !combo.iter().any(|slot| slot.contains(p)));
                if !modifier_only(combo) {
                    send(if hold { Cmd::Start } else { Cmd::Toggle });
                } else if hold && !tainted {
                    send(Cmd::Start);
                }
            }
        } else {
            pressed.remove(&key);
            if armed && in_combo && !satisfied(combo, &pressed) {
                armed = false;
                if modifier_only(combo) {
                    if hold {
                        send(if tainted { Cmd::Cancel } else { Cmd::Stop });
                    } else if !tainted {
                        send(Cmd::Toggle);
                    }
                } else if hold {
                    send(Cmd::Stop);
                }
            }
        }
    });
    if let Err(e) = res {
        bail!("{e:?}");
    }
    Ok(())
}
