//! UI state and all key handling. Pure: no I/O, no rendering. A key becomes
//! an optional [`Action`] which `tui.rs` carries out.
use crate::audio::InputRow;
use crate::config::Layout;
use crate::daemon::Status;
use crate::history::{Entry, Status as Progress};
use crate::models::Row;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;
use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Search,
    ConfirmDelete,
    ConfirmStop,
    Models,
    Inputs,
    Theme,
    Help,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Quit,
    QuitAndStop,
    Copy(String),
    Delete(String),
    /// `t`: transcribe a failed recording again.
    Retry(String),
    ToggleRecord,
    Play {
        id: String,
        path: PathBuf,
    },
    StopPlayback,
    EditConfig,
    /// The models dialog opened: load the list (and refresh it if it is old).
    RefreshModels,
    /// `r` in the models dialog: fetch the list again.
    RefreshCatalog,
    DownloadModel(String),
    /// Stop one model's download (`x` in the models dialog).
    CancelDownload(String),
    /// Stop every download (`X`).
    CancelDownloads,
    UseModel(String),
    /// The theme picker opened: re-read the theme files so new ones show up.
    ReloadThemes,
    SaveTheme {
        name: String,
        transparent: bool,
    },
    SaveLayout(Layout),
    /// The microphone picker opened, or `r` in it: scan the devices again.
    RefreshInputs,
    /// Choose a microphone; empty means the system default.
    UseInput(String),
}

/// A transient status message; green, or red for errors.
pub struct Flash {
    pub text: String,
    pub error: bool,
    until: Instant,
}

pub struct ThemePicker {
    pub cursor: usize,
    original: usize,
    original_transparent: bool,
}

pub struct Download {
    pub name: String,
    pub percent: u8,
}

pub struct App {
    pub entries: Vec<Entry>,
    pub filter: String,
    /// Index into [`App::visible`].
    pub selected: usize,
    pub mode: Mode,
    pub status: Option<Status>,
    pub flash: Option<Flash>,
    /// The model named in the config, and whether it is downloaded.
    pub model_label: String,
    pub model_ready: bool,
    /// The microphone that will record, and whether recording can start.
    pub mic_label: String,
    pub mic_ready: bool,
    pub inputs: Vec<InputRow>,
    pub input_sel: usize,
    /// Header, bottom border: where the config lives.
    pub config_label: String,
    pub layout: Layout,

    pub themes: Vec<Theme>,
    pub theme_index: usize,
    pub transparent: bool,
    /// The active theme with transparency applied; what the UI paints with.
    pub theme: Theme,
    pub theme_picker: Option<ThemePicker>,

    pub models: Vec<Row>,
    pub model_sel: usize,
    pub models_error: Option<String>,
    /// Text filter for the models dialog; `models_searching` while it is being typed.
    pub models_filter: String,
    pub models_searching: bool,
    /// Model downloads in progress; any number can run at once.
    pub downloads: Vec<Download>,
    /// The recording whose audio is playing, kept in step by `tui.rs`.
    pub playing: Option<String>,
    /// When the model list was fetched, and whether a fetch is running.
    pub catalog_updated: Option<String>,
    pub catalog_refreshing: bool,
    pub models_state: RefCell<ListState>,

    /// Scroll position of the list. View state only; rendering keeps it in range.
    pub list_state: RefCell<ListState>,
}

impl App {
    pub fn new(themes: Vec<Theme>, theme_index: usize, transparent: bool) -> App {
        let theme = themes[theme_index].clone().transparent(transparent);
        App {
            entries: Vec::new(),
            filter: String::new(),
            selected: 0,
            mode: Mode::Normal,
            status: None,
            flash: None,
            model_label: String::new(),
            model_ready: true,
            mic_label: String::new(),
            mic_ready: true,
            inputs: Vec::new(),
            input_sel: 0,
            config_label: String::new(),
            layout: Layout::Vertical,
            themes,
            theme_index,
            transparent,
            theme,
            theme_picker: None,
            models: Vec::new(),
            model_sel: 0,
            models_error: None,
            models_filter: String::new(),
            models_searching: false,
            downloads: Vec::new(),
            playing: None,
            catalog_updated: None,
            catalog_refreshing: false,
            models_state: RefCell::new(ListState::default()),
            list_state: RefCell::new(ListState::default()),
        }
    }

    pub fn visible(&self) -> Vec<&Entry> {
        let f = self.filter.to_lowercase();
        self.entries
            .iter()
            .filter(|e| f.is_empty() || e.text.to_lowercase().contains(&f))
            .collect()
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.visible().get(self.selected).copied()
    }

    /// Replace the history, keeping the same recording selected if it is still there.
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        let keep = self.selected_entry().map(|e| e.id.clone());
        self.entries = entries;
        let vis = self.visible();
        self.selected = keep
            .and_then(|id| vis.iter().position(|e| e.id == id))
            .unwrap_or(self.selected);
        self.clamp_selection();
    }

    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.visible().len().saturating_sub(1));
    }

    fn step(&mut self, delta: isize) {
        let last = self.visible().len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub fn flash_ok(&mut self, text: impl Into<String>) {
        self.set_flash(text.into(), false);
    }

    pub fn flash_err(&mut self, text: impl Into<String>) {
        self.set_flash(text.into(), true);
    }

    fn set_flash(&mut self, text: String, error: bool) {
        let secs = if error { 6 } else { 3 };
        self.flash = Some(Flash {
            text,
            error,
            until: Instant::now() + Duration::from_secs(secs),
        });
    }

    pub fn tick(&mut self, now: Instant) {
        if self.flash.as_ref().is_some_and(|f| now >= f.until) {
            self.flash = None;
        }
    }

    pub fn set_transparent(&mut self, on: bool) {
        self.transparent = on;
        self.theme = self.themes[self.theme_index].clone().transparent(on);
    }

    fn preview_theme(&mut self, index: usize) {
        self.theme_index = index;
        self.set_transparent(self.transparent);
    }

    /// Swap in reloaded themes with `index` active. An open picker follows,
    /// so reloading the list under it doesn't leave its cursor pointing elsewhere.
    pub fn set_themes(&mut self, themes: Vec<Theme>, index: usize, transparent: bool) {
        self.themes = themes;
        self.theme_index = index.min(self.themes.len().saturating_sub(1));
        if let Some(p) = &mut self.theme_picker {
            p.cursor = self.theme_index;
            p.original = self.theme_index;
            p.original_transparent = transparent;
        }
        self.set_transparent(transparent);
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_theme_picker(false);
            return Some(Action::Quit);
        }
        match self.mode {
            Mode::Normal => self.key_normal(key),
            Mode::Search => self.key_search(key),
            Mode::ConfirmDelete => self.key_confirm_delete(key),
            Mode::ConfirmStop => self.key_confirm_stop(key),
            Mode::Models => self.key_models(key),
            Mode::Inputs => self.key_inputs(key),
            Mode::Theme => self.key_theme(key),
            Mode::Help => {
                // Any key closes help.
                self.mode = Mode::Normal;
                None
            }
        }
    }

    fn key_normal(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Char('q') => return Some(Action::Quit),
            KeyCode::Char('Q') => self.mode = Mode::ConfirmStop,
            KeyCode::Down | KeyCode::Char('j') => self.step(1),
            KeyCode::Up | KeyCode::Char('k') => self.step(-1),
            KeyCode::PageDown => self.step(10),
            KeyCode::PageUp => self.step(-10),
            KeyCode::Home | KeyCode::Char('g') => self.selected = 0,
            KeyCode::End | KeyCode::Char('G') => {
                self.selected = self.visible().len().saturating_sub(1)
            }
            KeyCode::Char('y') => {
                return self.selected_entry().map(|e| Action::Copy(e.text.clone()));
            }
            // `a` plays the selected recording; while something plays it stops it.
            KeyCode::Char('a') => {
                if self.playing.is_some() {
                    return Some(Action::StopPlayback);
                }
                return self.selected_entry().and_then(|e| {
                    Some(Action::Play {
                        id: e.id.clone(),
                        path: e.audio.clone()?,
                    })
                });
            }
            KeyCode::Char('t') => {
                return self
                    .selected_entry()
                    .filter(|e| e.status == Progress::Failed)
                    .map(|e| Action::Retry(e.id.clone()));
            }
            KeyCode::Char('d') if self.selected_entry().is_some() => {
                self.mode = Mode::ConfirmDelete
            }
            KeyCode::Char(' ') => return Some(Action::ToggleRecord),
            KeyCode::Char('/') => self.mode = Mode::Search,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('E') => return Some(Action::EditConfig),
            KeyCode::Char('X') if !self.downloads.is_empty() => {
                return Some(Action::CancelDownloads);
            }
            KeyCode::Char('L') => {
                self.layout = self.layout.toggled();
                return Some(Action::SaveLayout(self.layout));
            }
            KeyCode::Char('M') => {
                self.models_error = None;
                self.models_filter.clear();
                self.models_searching = false;
                self.model_sel = self.models.iter().position(|m| m.active).unwrap_or(0);
                self.mode = Mode::Models;
                return Some(Action::RefreshModels);
            }
            KeyCode::Char('I') => {
                self.mode = Mode::Inputs;
                return Some(Action::RefreshInputs);
            }
            KeyCode::Char('T') => {
                self.theme_picker = Some(ThemePicker {
                    cursor: self.theme_index,
                    original: self.theme_index,
                    original_transparent: self.transparent,
                });
                self.mode = Mode::Theme;
                return Some(Action::ReloadThemes);
            }
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.selected = 0;
            }
            _ => {}
        }
        None
    }

    /// The search box is a text field: plain characters are text, never commands.
    fn key_search(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Esc => {
                self.filter.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => self.mode = Mode::Normal,
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Down => self.step(1),
            KeyCode::Up => self.step(-1),
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.filter.push(c);
            }
            _ => return None,
        }
        if !matches!(key.code, KeyCode::Down | KeyCode::Up) {
            self.selected = 0;
        }
        None
    }

    /// `y` confirms; anything else cancels.
    fn key_confirm_delete(&mut self, key: KeyEvent) -> Option<Action> {
        self.mode = Mode::Normal;
        match (key.code, self.selected_entry()) {
            (KeyCode::Char('y'), Some(e)) => Some(Action::Delete(e.id.clone())),
            _ => None,
        }
    }

    fn key_confirm_stop(&mut self, key: KeyEvent) -> Option<Action> {
        self.mode = Mode::Normal;
        (key.code == KeyCode::Char('y')).then_some(Action::QuitAndStop)
    }

    /// Replace the microphone list (after a scan), pointing at the one in use.
    pub fn set_inputs(&mut self, rows: Vec<InputRow>) {
        self.input_sel = rows.iter().position(|r| r.active).unwrap_or(0);
        self.inputs = rows;
    }

    fn key_inputs(&mut self, key: KeyEvent) -> Option<Action> {
        let last = self.inputs.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            KeyCode::Down | KeyCode::Char('j') => self.input_sel = (self.input_sel + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => {
                self.input_sel = self.input_sel.min(last).saturating_sub(1)
            }
            KeyCode::Char('r') => return Some(Action::RefreshInputs),
            KeyCode::Enter => {
                return self
                    .inputs
                    .get(self.input_sel.min(last))
                    .map(|r| Action::UseInput(r.value.clone()));
            }
            _ => {}
        }
        None
    }

    pub fn is_downloading(&self, name: &str) -> bool {
        self.downloads.iter().any(|d| d.name == name)
    }

    /// Models matching the filter (name or description), in list order.
    pub fn visible_models(&self) -> Vec<&Row> {
        let f = self.models_filter.to_lowercase();
        self.models
            .iter()
            .filter(|m| {
                f.is_empty()
                    || m.name.to_lowercase().contains(&f)
                    || m.note.to_lowercase().contains(&f)
            })
            .collect()
    }

    fn key_models(&mut self, key: KeyEvent) -> Option<Action> {
        if self.models_searching {
            return self.key_models_search(key);
        }
        let last = self.visible_models().len().saturating_sub(1);
        match key.code {
            // Esc first backs out of a filter, then closes.
            KeyCode::Esc if !self.models_filter.is_empty() => {
                self.models_filter.clear();
                self.model_sel = 0;
            }
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            KeyCode::Char('/') => self.models_searching = true,
            KeyCode::Down | KeyCode::Char('j') => self.model_sel = (self.model_sel + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => {
                self.model_sel = self.model_sel.min(last).saturating_sub(1)
            }
            KeyCode::Char('r') => {
                self.models_error = None;
                if self.catalog_refreshing {
                    self.models_error = Some("The list is already being refreshed.".into());
                    return None;
                }
                return Some(Action::RefreshCatalog);
            }
            KeyCode::Enter => {
                let (name, installed) = {
                    let visible = self.visible_models();
                    let row = visible.get(self.model_sel.min(last))?;
                    (row.name.clone(), row.installed)
                };
                self.models_error = None;
                if installed {
                    return Some(Action::UseModel(name));
                }
                if self.is_downloading(&name) {
                    self.models_error =
                        Some(format!("{name} is already downloading (x cancels it)."));
                    return None;
                }
                return Some(Action::DownloadModel(name));
            }
            KeyCode::Char('x') => {
                let name = self
                    .visible_models()
                    .get(self.model_sel.min(last))
                    .map(|r| r.name.clone())?;
                self.models_error = None;
                if self.is_downloading(&name) {
                    return Some(Action::CancelDownload(name));
                }
                self.models_error = Some(format!("{name} isn't downloading."));
            }
            _ => {}
        }
        None
    }

    /// The models filter box is a text field: plain characters are text, never commands.
    fn key_models_search(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Esc => {
                self.models_filter.clear();
                self.models_searching = false;
            }
            KeyCode::Enter => self.models_searching = false,
            KeyCode::Backspace => {
                self.models_filter.pop();
            }
            KeyCode::Down => {
                let last = self.visible_models().len().saturating_sub(1);
                self.model_sel = (self.model_sel + 1).min(last);
                return None;
            }
            KeyCode::Up => {
                self.model_sel = self.model_sel.saturating_sub(1);
                return None;
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.models_filter.push(c);
            }
            _ => return None,
        }
        if !matches!(key.code, KeyCode::Enter) {
            self.model_sel = 0;
        }
        None
    }

    fn key_theme(&mut self, key: KeyEvent) -> Option<Action> {
        let last = self.themes.len().saturating_sub(1);
        let cursor = self.theme_picker.as_ref()?.cursor;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.close_theme_picker(false),
            KeyCode::Down | KeyCode::Char('j') => self.move_theme_cursor((cursor + 1).min(last)),
            KeyCode::Up | KeyCode::Char('k') => self.move_theme_cursor(cursor.saturating_sub(1)),
            KeyCode::Char('b') => self.set_transparent(!self.transparent),
            KeyCode::Enter => {
                let action = Action::SaveTheme {
                    name: self.themes[self.theme_index].name.clone(),
                    transparent: self.transparent,
                };
                self.close_theme_picker(true);
                return Some(action);
            }
            _ => {}
        }
        None
    }

    fn move_theme_cursor(&mut self, to: usize) {
        if let Some(p) = &mut self.theme_picker {
            p.cursor = to;
        }
        self.preview_theme(to);
    }

    /// Close the picker; without `keep`, restore what was active when it opened.
    fn close_theme_picker(&mut self, keep: bool) {
        if let Some(p) = self.theme_picker.take()
            && !keep
        {
            self.theme_index = p.original;
            self.set_transparent(p.original_transparent);
        }
        if self.mode == Mode::Theme {
            self.mode = Mode::Normal;
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::theme::builtin_themes;
    use chrono::Local;
    use crossterm::event::{KeyEventKind, KeyEventState};

    pub fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    pub fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }

    pub fn entry(id: &str, text: &str) -> Entry {
        Entry {
            id: id.into(),
            created: Local::now(),
            text: text.into(),
            duration_secs: 2.5,
            audio: Some(PathBuf::from(format!("/tmp/{id}.wav"))),
            status: Progress::Done,
            error: None,
        }
    }

    pub fn failed(id: &str) -> Entry {
        Entry {
            status: Progress::Failed,
            error: Some("whisper-cli: boom".into()),
            ..entry(id, "")
        }
    }

    pub fn app() -> App {
        let mut a = App::new(builtin_themes(), 0, true);
        a.set_entries(vec![
            entry("3", "third thing"),
            entry("2", "second thing"),
            entry("1", "first thing"),
        ]);
        a
    }

    #[test]
    fn q_quits_but_esc_does_not() {
        let mut a = app();
        assert_eq!(a.handle_key(key(KeyCode::Esc)), None);
        assert_eq!(a.handle_key(ch('q')), Some(Action::Quit));
        let mut ctrl_c = ch('c');
        ctrl_c.modifiers = KeyModifiers::CONTROL;
        assert_eq!(a.handle_key(ctrl_c), Some(Action::Quit));
    }

    #[test]
    fn t_transcribes_only_failed_recordings() {
        let mut a = app();
        assert_eq!(a.handle_key(ch('t')), None);
        a.set_entries(vec![failed("9"), entry("1", "ok")]);
        assert_eq!(a.handle_key(ch('t')), Some(Action::Retry("9".into())));
        a.handle_key(key(KeyCode::Down));
        assert_eq!(a.handle_key(ch('t')), None);
    }

    #[test]
    fn y_copies_and_enter_does_not() {
        let mut a = app();
        assert_eq!(a.handle_key(key(KeyCode::Enter)), None);
        assert_eq!(
            a.handle_key(ch('y')),
            Some(Action::Copy("third thing".into()))
        );
        a.handle_key(ch('j'));
        assert_eq!(
            a.handle_key(ch('y')),
            Some(Action::Copy("second thing".into()))
        );
        let mut empty = App::new(builtin_themes(), 0, true);
        assert_eq!(empty.handle_key(ch('y')), None);
    }

    #[test]
    fn navigation_stays_in_range() {
        let mut a = app();
        a.handle_key(ch('k'));
        assert_eq!(a.selected, 0);
        a.handle_key(ch('G'));
        assert_eq!(a.selected, 2);
        a.handle_key(key(KeyCode::PageDown));
        assert_eq!(a.selected, 2);
        a.handle_key(ch('g'));
        a.handle_key(key(KeyCode::PageUp));
        assert_eq!(a.selected, 0);
        let mut empty = App::new(builtin_themes(), 0, true);
        for c in ['j', 'k', 'G', 'g'] {
            empty.handle_key(ch(c));
        }
        assert_eq!(empty.selected, 0);
    }

    #[test]
    fn selection_follows_the_recording_when_history_changes() {
        let mut a = app();
        a.handle_key(ch('j'));
        assert_eq!(a.selected_entry().unwrap().id, "2");
        let mut newer = vec![entry("4", "fourth thing")];
        newer.extend(a.entries.clone());
        a.set_entries(newer);
        assert_eq!(a.selected_entry().unwrap().id, "2");
        a.set_entries(vec![entry("9", "only")]);
        assert_eq!(a.selected_entry().unwrap().id, "9");
        a.set_entries(vec![]);
        assert_eq!((a.selected, a.selected_entry().is_none()), (0, true));
    }

    #[test]
    fn delete_asks_first_and_only_y_confirms() {
        let mut a = app();
        assert_eq!(a.handle_key(ch('d')), None);
        assert_eq!(a.mode, Mode::ConfirmDelete);
        assert_eq!(a.handle_key(ch('y')), Some(Action::Delete("3".into())));
        assert_eq!(a.mode, Mode::Normal);
        for cancel in [
            key(KeyCode::Enter),
            key(KeyCode::Esc),
            ch('n'),
            ch('d'),
            ch('q'),
        ] {
            a.handle_key(ch('d'));
            assert_eq!(a.handle_key(cancel), None);
            assert_eq!(a.mode, Mode::Normal);
        }
        // Nothing selected: nothing to delete.
        let mut empty = App::new(builtin_themes(), 0, true);
        empty.handle_key(ch('d'));
        assert_eq!(empty.mode, Mode::Normal);
    }

    #[test]
    fn stop_daemon_asks_first_and_only_y_confirms() {
        let mut a = app();
        assert_eq!(a.handle_key(ch('Q')), None);
        assert_eq!(a.mode, Mode::ConfirmStop);
        assert_eq!(a.handle_key(key(KeyCode::Enter)), None);
        assert_eq!(a.mode, Mode::Normal);
        a.handle_key(ch('Q'));
        assert_eq!(a.handle_key(ch('y')), Some(Action::QuitAndStop));
    }

    #[test]
    fn search_text_field_eats_letters() {
        let mut a = app();
        a.handle_key(ch('/'));
        for c in "qyQdEMT? ".chars() {
            assert_eq!(a.handle_key(ch(c)), None, "{c}");
        }
        assert_eq!(a.filter, "qyQdEMT? ");
        assert_eq!(a.mode, Mode::Search);
        a.handle_key(key(KeyCode::Backspace));
        assert_eq!(a.filter, "qyQdEMT?");
    }

    #[test]
    fn search_filters_and_esc_backs_out() {
        let mut a = app();
        a.handle_key(ch('/'));
        for c in "SEC".chars() {
            a.handle_key(ch(c));
        }
        assert_eq!(a.visible().len(), 1);
        a.handle_key(key(KeyCode::Enter));
        assert_eq!((a.mode, a.visible().len()), (Mode::Normal, 1));
        assert_eq!(
            a.handle_key(ch('y')),
            Some(Action::Copy("second thing".into()))
        );
        assert_eq!(a.handle_key(key(KeyCode::Esc)), None); // clears the filter, does not quit
        assert_eq!((a.filter.as_str(), a.visible().len()), ("", 3));
        a.handle_key(ch('/'));
        a.handle_key(ch('x'));
        a.handle_key(key(KeyCode::Esc));
        assert_eq!((a.mode, a.filter.as_str()), (Mode::Normal, ""));
    }

    #[test]
    fn a_plays_and_a_again_stops() {
        let mut a = app();
        assert!(matches!(a.handle_key(ch('a')), Some(Action::Play { .. })));
        a.playing = Some("3".into());
        assert_eq!(a.handle_key(ch('a')), Some(Action::StopPlayback));
        // Stopping works whatever is selected, even a recording with no audio.
        a.handle_key(ch('j'));
        a.entries[1].audio = None;
        assert_eq!(a.handle_key(ch('a')), Some(Action::StopPlayback));
        // And not while typing in the search box.
        a.handle_key(ch('/'));
        assert_eq!(a.handle_key(ch('a')), None);
        assert_eq!(a.filter, "a");
    }

    #[test]
    fn simple_actions() {
        let mut a = app();
        assert_eq!(a.handle_key(ch(' ')), Some(Action::ToggleRecord));
        assert_eq!(a.handle_key(ch('E')), Some(Action::EditConfig));
        assert_eq!(
            a.handle_key(ch('a')),
            Some(Action::Play {
                id: "3".into(),
                path: PathBuf::from("/tmp/3.wav")
            })
        );
        // Lower-case e and m are not bound: separate screens use capitals.
        assert_eq!(a.handle_key(ch('e')), None);
        assert_eq!((a.handle_key(ch('m')), a.mode), (None, Mode::Normal));
        a.entries[0].audio = None;
        assert_eq!(a.handle_key(ch('a')), None);
    }

    #[test]
    fn help_opens_and_any_key_closes_it() {
        let mut a = app();
        a.handle_key(ch('?'));
        assert_eq!(a.mode, Mode::Help);
        assert_eq!(a.handle_key(ch('q')), None);
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn theme_picker_previews_restores_and_saves() {
        let mut a = app();
        let original = a.theme.clone();
        a.handle_key(ch('T'));
        assert_eq!(a.mode, Mode::Theme);
        a.handle_key(ch('j'));
        assert_eq!(a.theme_index, 1);
        assert_ne!(a.theme, original, "live preview");
        a.handle_key(ch('b'));
        assert!(!a.transparent);
        assert_ne!(a.theme.bg, ratatui::style::Color::Reset);
        // Esc puts everything back.
        assert_eq!(a.handle_key(key(KeyCode::Esc)), None);
        assert_eq!(
            (a.theme_index, a.transparent, a.mode),
            (0, true, Mode::Normal)
        );
        assert_eq!(a.theme, original);

        a.handle_key(ch('T'));
        a.handle_key(ch('j'));
        a.handle_key(ch('b'));
        let saved = a.handle_key(key(KeyCode::Enter));
        assert_eq!(
            saved,
            Some(Action::SaveTheme {
                name: a.themes[1].name.clone(),
                transparent: false
            })
        );
        assert_eq!(
            (a.theme_index, a.transparent, a.mode),
            (1, false, Mode::Normal)
        );
        assert!(a.theme_picker.is_none());
    }

    #[test]
    fn theme_picker_cursor_stays_in_range_and_ctrl_c_restores() {
        let mut a = app();
        a.handle_key(ch('T'));
        for _ in 0..20 {
            a.handle_key(ch('j'));
        }
        assert_eq!(a.theme_index, a.themes.len() - 1);
        for _ in 0..20 {
            a.handle_key(ch('k'));
        }
        assert_eq!(a.theme_index, 0);
        a.handle_key(ch('j'));
        let mut ctrl_c = ch('c');
        ctrl_c.modifiers = KeyModifiers::CONTROL;
        assert_eq!(a.handle_key(ctrl_c), Some(Action::Quit));
        assert_eq!(a.theme_index, 0);
    }

    fn rows() -> Vec<Row> {
        let row = |name: &str, installed, active| Row {
            name: name.into(),
            bytes: 74 << 20,
            note: String::new(),
            installed,
            active,
        };
        vec![row("tiny.en", false, false), row("base.en", true, true)]
    }

    #[test]
    fn models_screen_downloads_or_selects() {
        let mut a = app();
        a.models = rows();
        assert_eq!(a.handle_key(ch('M')), Some(Action::RefreshModels));
        assert_eq!(
            (a.mode, a.model_sel),
            (Mode::Models, 1),
            "starts on the active model"
        );
        assert_eq!(
            a.handle_key(key(KeyCode::Enter)),
            Some(Action::UseModel("base.en".into()))
        );
        a.handle_key(ch('k'));
        assert_eq!(
            a.handle_key(key(KeyCode::Enter)),
            Some(Action::DownloadModel("tiny.en".into()))
        );
        a.downloads.push(Download {
            name: "tiny.en".into(),
            percent: 3,
        });
        // That model is already on its way: Enter doesn't start it twice.
        assert_eq!(a.handle_key(key(KeyCode::Enter)), None);
        assert!(
            a.models_error
                .as_deref()
                .unwrap()
                .contains("already downloading")
        );
        a.handle_key(key(KeyCode::Esc));
        assert_eq!(a.mode, Mode::Normal);
        // Selection can't run off the end.
        a.handle_key(ch('M'));
        for _ in 0..5 {
            a.handle_key(ch('j'));
        }
        assert_eq!(a.model_sel, 1);
    }

    fn dl(name: &str, percent: u8) -> Download {
        Download {
            name: name.into(),
            percent,
        }
    }

    #[test]
    fn downloads_run_side_by_side_and_x_cancels_the_selected_one() {
        let mut a = app();
        a.models = filterable_rows();
        a.handle_key(ch('M'));
        a.handle_key(ch('g'));
        // Two models downloading at once: starting a second one is allowed.
        a.model_sel = 0;
        a.downloads.push(dl("tiny.en", 10));
        a.model_sel = 1;
        assert_eq!(
            a.handle_key(key(KeyCode::Enter)),
            Some(Action::DownloadModel("base-q5_1".into()))
        );
        a.downloads.push(dl("base-q5_1", 4));
        // `x` cancels the one under the cursor, and only that one.
        assert_eq!(
            a.handle_key(ch('x')),
            Some(Action::CancelDownload("base-q5_1".into()))
        );
        a.model_sel = 0;
        assert_eq!(
            a.handle_key(ch('x')),
            Some(Action::CancelDownload("tiny.en".into()))
        );
        // Nothing to cancel on a model that isn't downloading.
        a.model_sel = 3;
        assert_eq!(a.handle_key(ch('x')), None);
        assert_eq!(
            a.models_error.as_deref(),
            Some("large-v3 isn't downloading.")
        );
        // In the filter box, x is just a letter.
        a.handle_key(ch('/'));
        assert_eq!(a.handle_key(ch('x')), None);
        assert_eq!(a.models_filter, "x");
        assert!(a.models_searching);
    }

    #[test]
    fn capital_x_cancels_every_download_from_the_main_screen() {
        let mut a = app();
        assert_eq!(a.handle_key(ch('X')), None, "nothing to cancel");
        a.downloads.push(dl("tiny.en", 10));
        a.downloads.push(dl("base.en", 20));
        assert_eq!(a.handle_key(ch('X')), Some(Action::CancelDownloads));
        // Not a command inside the search box.
        a.handle_key(ch('/'));
        assert_eq!(a.handle_key(ch('X')), None);
        assert_eq!(a.filter, "X");
        assert!(a.is_downloading("base.en") && !a.is_downloading("nope"));
    }

    #[test]
    fn layout_toggles_and_is_saved() {
        let mut a = app();
        assert_eq!(a.layout, Layout::Vertical);
        assert_eq!(
            a.handle_key(ch('L')),
            Some(Action::SaveLayout(Layout::Horizontal))
        );
        assert_eq!(a.layout, Layout::Horizontal);
        assert_eq!(
            a.handle_key(ch('L')),
            Some(Action::SaveLayout(Layout::Vertical))
        );
        assert_eq!(a.layout, Layout::Vertical);
        // Not a text-field key: typing L into the search box searches for "l".
        a.handle_key(ch('/'));
        assert_eq!(a.handle_key(ch('L')), None);
        assert_eq!(a.layout, Layout::Vertical);
        assert_eq!(a.filter, "L");
    }

    #[test]
    fn models_dialog_can_refresh_the_list_once_at_a_time() {
        let mut a = app();
        a.handle_key(ch('M'));
        assert_eq!(a.handle_key(ch('r')), Some(Action::RefreshCatalog));
        a.catalog_refreshing = true;
        assert_eq!(a.handle_key(ch('r')), None);
        assert!(
            a.models_error
                .as_deref()
                .unwrap()
                .contains("already being refreshed")
        );
        // `r` does nothing outside the dialog.
        a.handle_key(key(KeyCode::Esc));
        assert_eq!(a.handle_key(ch('r')), None);
        // Enter on an empty list is harmless.
        a.models.clear();
        a.handle_key(ch('M'));
        assert_eq!(a.handle_key(key(KeyCode::Enter)), None);
    }

    #[test]
    fn opening_the_picker_asks_for_a_theme_reload_and_the_picker_follows_it() {
        let mut a = app();
        assert_eq!(a.handle_key(ch('T')), Some(Action::ReloadThemes));
        a.handle_key(ch('j'));
        a.handle_key(ch('j'));
        // New theme files turn up: the list grows, with the current theme moved.
        let mut themes = builtin_themes();
        themes.insert(
            0,
            Theme {
                name: "aaa".into(),
                ..themes[0].clone()
            },
        );
        a.set_themes(themes, 3, true);
        assert_eq!(a.theme_picker.as_ref().unwrap().cursor, 3);
        assert_eq!(a.theme_index, 3);
        // Esc restores to where the picker now thinks it started.
        a.handle_key(ch('j'));
        a.handle_key(key(KeyCode::Esc));
        assert_eq!(a.theme_index, 3);
    }

    fn filterable_rows() -> Vec<Row> {
        let row = |name: &str, note: &str, installed, active| Row {
            name: name.into(),
            bytes: 1 << 20,
            note: note.into(),
            installed,
            active,
        };
        vec![
            row("tiny.en", "English only, full precision", false, false),
            row("base-q5_1", "Multilingual, quantized (q5_1)", false, false),
            row(
                "large-v3-turbo-q5_0",
                "Multilingual, quantized (q5_0)",
                true,
                true,
            ),
            row("large-v3", "Multilingual, full precision", false, false),
        ]
    }

    #[test]
    fn models_filter_matches_name_or_description_and_selection_follows_it() {
        let mut a = app();
        a.models = filterable_rows();
        a.handle_key(ch('M'));
        assert_eq!(a.visible_models().len(), 4);
        assert_eq!(a.handle_key(ch('/')), None);
        assert!(a.models_searching);
        for c in "Q5".chars() {
            a.handle_key(ch(c));
        }
        let names: Vec<_> = a.visible_models().iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            ["base-q5_1", "large-v3-turbo-q5_0"],
            "case-insensitive, by name"
        );
        a.models_filter = "english".into();
        assert_eq!(
            a.visible_models()
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>(),
            ["tiny.en"],
            "by description"
        );
        a.models_filter = "q5".into();
        a.handle_key(key(KeyCode::Enter)); // apply
        assert!(!a.models_searching);
        // Enter acts on the filtered row, not the unfiltered one at that index.
        assert_eq!(
            a.handle_key(key(KeyCode::Enter)),
            Some(Action::DownloadModel("base-q5_1".into()))
        );
        a.handle_key(ch('j'));
        assert_eq!(
            a.handle_key(key(KeyCode::Enter)),
            Some(Action::UseModel("large-v3-turbo-q5_0".into()))
        );
        a.handle_key(ch('j'));
        assert_eq!(a.model_sel, 1, "can't run past the filtered list");
    }

    #[test]
    fn models_filter_box_eats_letters() {
        let mut a = app();
        a.models = filterable_rows();
        a.handle_key(ch('M'));
        a.handle_key(ch('/'));
        for c in "qrjkM/ ".chars() {
            assert_eq!(a.handle_key(ch(c)), None, "{c}");
        }
        assert_eq!(a.filter, "", "the main search is untouched");
        assert_eq!(a.models_filter, "qrjkM/ ");
        assert_eq!((a.mode, a.models_searching), (Mode::Models, true));
        a.handle_key(key(KeyCode::Backspace));
        assert_eq!(a.models_filter, "qrjkM/");
    }

    #[test]
    fn models_esc_backs_out_one_step_at_a_time() {
        let mut a = app();
        a.models = filterable_rows();
        a.handle_key(ch('M'));
        a.handle_key(ch('/'));
        a.handle_key(ch('x'));
        a.handle_key(key(KeyCode::Esc)); // out of the box, clearing it
        assert_eq!(
            (a.models_filter.as_str(), a.models_searching, a.mode),
            ("", false, Mode::Models)
        );

        a.handle_key(ch('/'));
        a.handle_key(ch('q'));
        a.handle_key(key(KeyCode::Enter)); // keep the filter
        assert_eq!((a.models_filter.as_str(), a.mode), ("q", Mode::Models));
        a.handle_key(key(KeyCode::Esc)); // clears the filter first
        assert_eq!((a.models_filter.as_str(), a.mode), ("", Mode::Models));
        a.handle_key(key(KeyCode::Esc)); // then closes
        assert_eq!(a.mode, Mode::Normal);

        // Reopening starts unfiltered.
        a.handle_key(ch('M'));
        assert_eq!(a.models_filter, "");
    }

    #[test]
    fn enter_with_nothing_matching_does_nothing() {
        let mut a = app();
        a.models = filterable_rows();
        a.handle_key(ch('M'));
        a.handle_key(ch('/'));
        for c in "zzz".chars() {
            a.handle_key(ch(c));
        }
        a.handle_key(key(KeyCode::Enter));
        assert!(a.visible_models().is_empty());
        assert_eq!(a.handle_key(key(KeyCode::Enter)), None);
        assert_eq!(a.handle_key(ch('j')), None);
    }

    fn input_rows() -> Vec<InputRow> {
        let row = |value: &str, label: &str, active| InputRow {
            value: value.into(),
            label: label.into(),
            detail: String::new(),
            active,
        };
        vec![
            row("", "System default", true),
            row("MacBook Pro Microphone", "MacBook Pro Microphone", false),
            row("USB Mic", "USB Mic", false),
        ]
    }

    #[test]
    fn microphone_picker_selects_and_rescans() {
        let mut a = app();
        assert_eq!(a.handle_key(ch('I')), Some(Action::RefreshInputs));
        assert_eq!(a.mode, Mode::Inputs);
        a.set_inputs(input_rows());
        assert_eq!(a.input_sel, 0, "starts on the one in use");
        assert_eq!(
            a.handle_key(key(KeyCode::Enter)),
            Some(Action::UseInput(String::new())),
            "empty = system default"
        );
        a.handle_key(ch('j'));
        a.handle_key(ch('j'));
        a.handle_key(ch('j'));
        assert_eq!(a.input_sel, 2, "can't run past the end");
        assert_eq!(
            a.handle_key(key(KeyCode::Enter)),
            Some(Action::UseInput("USB Mic".into()))
        );
        assert_eq!(a.handle_key(ch('r')), Some(Action::RefreshInputs));
        a.handle_key(key(KeyCode::Esc));
        assert_eq!(a.mode, Mode::Normal);
        // Lower-case i is not bound; separate screens use capitals.
        assert_eq!((a.handle_key(ch('i')), a.mode), (None, Mode::Normal));
        // An empty list is harmless.
        a.handle_key(ch('I'));
        a.set_inputs(vec![]);
        assert_eq!(a.handle_key(key(KeyCode::Enter)), None);
    }

    #[test]
    fn a_rescan_points_at_the_active_row_even_when_the_list_changed() {
        let mut a = app();
        a.handle_key(ch('I'));
        let mut rows = input_rows();
        rows[0].active = false;
        rows[2].active = true;
        a.set_inputs(rows);
        assert_eq!(a.input_sel, 2);
        a.set_inputs(input_rows()[..2].to_vec());
        assert_eq!(a.input_sel, 0);
    }

    #[test]
    fn flash_expires() {
        let mut a = app();
        a.flash_ok("copied");
        a.tick(Instant::now());
        assert!(a.flash.is_some());
        a.tick(Instant::now() + Duration::from_secs(10));
        assert!(a.flash.is_none());
    }
}
