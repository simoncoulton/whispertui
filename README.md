# whispertui

Hotkey voice-to-text for macOS. Press a key, speak, press it again: the
transcript is on your clipboard. A TUI keeps the history. Transcription runs
locally with [whisper.cpp](https://github.com/ggml-org/whisper.cpp).

## Installation

### macOS (Homebrew)

Homebrew on macOS (Apple Silicon and Intel) is the supported way to install, with prebuilt binaries:

```sh
brew tap simoncoulton/whispertui https://github.com/simoncoulton/whispertui
brew install whispertui
```

The formula installs [whisper.cpp](https://github.com/ggml-org/whisper.cpp) (`whisper-cli`) for you.

### Arch Linux and other distributions

There is no package for Arch or its derivatives yet. Build from source with a recent
[Rust toolchain](https://rustup.rs) (1.89 or newer):

```sh
git clone https://github.com/simoncoulton/whispertui
cd whispertui
cargo build --release
./target/release/whispertui           # opens the TUI and starts the daemon
```

You also need `whisper-cli`; see [Troubleshooting](#troubleshooting) for building it on Arch.

Then press `M` in the TUI to pick and download a model (the default is
`large-v3-turbo-q5_0`, 547 MB), and run `whispertui install` to start the daemon at
login. macOS asks for Microphone and Input Monitoring access (the global hotkey
needs the second). Models are downloaded into `~/.config/whispertui/models/`;
whispertui does not use models from other apps.


## How it runs

Recording is done by a **background daemon**; the TUI is just a window onto it.
You never need the TUI open to dictate.

- `whispertui` opens the TUI and starts the daemon first if it isn't running.
- `q` closes the window and leaves the daemon running. `Q` also stops it (asks first).
- `whispertui daemon start | stop | restart | status | run`
- `whispertui install` / `uninstall` manage the launchd login item.
- `whispertui toggle` starts/stops a recording from a script or shortcut.
- `whispertui last` prints the latest transcript.
- `whispertui models [refresh]`, `models download <name>`, `models use <name>`.

## Keys

| Key | Action |
| --- | --- |
| `y` | Copy the selected transcript |
| `space` | Start / stop recording |
| `a` | Play the audio; press `a` again to stop it |
| `t` | Transcribe a failed recording again (failed and unfinished ones are listed in red; audio is always kept) |
| `d` | Delete the recording (asks first; removes its audio) |
| `/` | Search transcripts (`Esc` clears) |
| `↑↓`, `j k`, `g G`, `PgUp PgDn` | Navigate |
| `L` | Toggle the layout: side by side (30% / 70%) or stacked, list 30% above details 70% (saved) |
| `I` | Microphone: choose the input device (`r` rescans) |
| `M` | Models: download and pick one (`/` filters, `r` refreshes the list, `x` cancels the selected download) |
| `X` | Cancel every running model download |
| `T` | Theme picker with live preview (`b` toggles the transparent background) |
| `E` | Edit the config in `$EDITOR` |
| `?` | Help |
| `q`, `Ctrl-C` | Close the window; the daemon keeps running |
| `Q` | Close the window and stop the daemon (asks first) |

The footer shows only the everyday keys; everything else is in `?`.

## Microphone

The header shows which microphone will record: `mic MacBook Pro Microphone
(default)` when nothing is chosen, or the one you picked with `I`. The choice is
saved as `input_device` (a case-insensitive part of the device name; empty means
the system default).

If there is no microphone, or the one you chose isn't connected, the header says
`unavailable` and recording is blocked, the same as with no model: the hotkey
sends a notification explaining why, and `space` in the TUI shows it in the
window. A chosen microphone that is unplugged never falls back to another one
silently; connect it or pick a different one.

## Models

The list of models is not built in: it is read from the
[`ggerganov/whisper.cpp`](https://huggingface.co/ggerganov/whisper.cpp)
repository through the Hugging Face API, with sizes, and cached in
`~/.config/whispertui/models-cache.json`. The cache is refreshed in the
background when it is a week old, or any time with `r` in the models dialog or
`whispertui models refresh`. If you are offline the cached list is still used.

### Downloads

Start as many downloads as you like; they run side by side. While any is
running the header grows a second line, whether or not the models dialog is open:

```
│↓ downloading medium-q5_0 42% · large-v3-q5_0 7%   X cancel │
```

`X` cancels them all; `x` in the models dialog cancels just the selected one.
Cancelling stops the transfer and deletes the partial file. Quitting the window
cancels any downloads still running, so nothing is left running behind it.

The first model you finish downloading becomes the one in use. After that a
finished download doesn't swap your model: press `M` and `Enter` on it to use it.

## Config

`~/.config/whispertui/config.yml` is created on first run, fully commented.
The daemon and the TUI reload it when it changes. Unknown keys are errors.

Highlights: `hotkey` (`right_option`, `cmd+shift+space`, `f13`, ...), `mode`
(`toggle` or `hold`), `model`, `input_device`, `language`, `copy_to_clipboard`, `auto_paste`,
`layout`, and `silence:` for suppressing blank recordings.

Edits made by the app (theme, layout, model) change only their own line; your
comments and layout are kept. A `config.toml` from an earlier version is
converted on first run and kept as `config.toml.bak`.

Other files in `~/.config/whispertui/`: `history.json`, `recordings/`,
`models/`, `models-cache.json`, `themes/`.

## Logs and runtime files

`~/Library/Application Support/whispertui/daemon.log` (`~/.local/share/whispertui/`
on Linux) is the daemon's own log: startup errors, refused recordings, failed
transcriptions and crashes. Check it first when the daemon misbehaves. It starts
afresh when it passes 1 MiB.

The daemon's socket and lock file sit in a `run/` folder beside it:
`~/Library/Application Support/whispertui/run/daemon.sock` and `daemon.lock`
(`~/.local/share/whispertui/run/` on Linux). The lock is what guarantees only one
daemon runs; `whispertui daemon stop` waits for it to be released. A daemon from
an earlier version, still listening in `~/.config/whispertui/`, is asked to quit
when the new one starts, so the two never share the hotkey.

## Themes

Built in (the same set as torrentui): `default` (uses your terminal's palette),
`tokyo-night`, `catppuccin-frappe`, `catppuccin-mocha`, `catppuccin-macchiato`,
`catppuccin-latte`, `dracula`, `one-dark`, `gruvbox-dark`, `nord`,
`solarized-dark`, `rose-pine`, `monokai`, `github-dark`, `everforest`,
`kanagawa`. Press `T` to preview and save. `transparent_background` (default on)
lets your terminal's background show.

Every `*.yml` in `~/.config/whispertui/themes/` is a theme, loaded dynamically:
add, edit or remove a file while the TUI is open and it is picked up within a
second. On start, each built-in is written there with every colour spelled out.
A theme can start from another with `base` and override only some colours:

```yaml
# ~/.config/whispertui/themes/mine.yml
base: nord
accent: "#ff9e64"
```

Colours are `#rrggbb`, an ANSI name (`reset`, `red`, `darkgray`, ...) or 0-255.
Themes can also be defined under `themes:` in `config.yml`.

# Troubleshooting

You will need whisper-cli, and if you're running Arch you may not be able to just install it via pacman or aur. Instead run the following:

```
git clone https://github.com/ggml-org/whisper.cpp.git
cd whisper.cpp
cmake -B build
cmake --build build -j --config Release
sudo cp build/bin/whisper-* /usr/local/bin/
```
