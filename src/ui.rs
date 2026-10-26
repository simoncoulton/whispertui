//! Rendering only: `draw(frame, &app)` never mutates state, and never names a
//! colour; everything comes from `app.theme`.
use crate::app::{App, Mode};
use crate::config::Layout;
use crate::models;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout as Split, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Wrap};

/// Side-by-side panes need room; narrower terminals stack them instead.
const MIN_SPLIT_WIDTH: u16 = 80;

pub fn draw(frame: &mut Frame, app: &App) {
    let full = frame.area();
    let t = &app.theme;
    frame.render_widget(Block::new().style(t.base()), full);

    // One extra header line while models are downloading.
    let header_rows = if app.downloads.is_empty() { 3 } else { 4 };
    let [header, body, footer] = Split::vertical([
        Constraint::Length(header_rows),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(full);
    draw_header(frame, app, header);

    let side_by_side = app.layout == Layout::Vertical && body.width >= MIN_SPLIT_WIDTH;
    if side_by_side {
        let [list, details] =
            Split::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)]).areas(body);
        draw_list(frame, app, list, true);
        draw_details(frame, app, details);
    } else {
        // Stacked: list 30%, details 70%. On short terminals the list keeps a
        // few rows and the details pane gives way first.
        let list_rows = (body.height * 30 / 100).max(5).min(body.height);
        let details_rows = body.height - list_rows;
        let [list, details] = Split::vertical([
            Constraint::Length(list_rows),
            Constraint::Length(details_rows),
        ])
        .areas(body);
        draw_list(frame, app, list, false);
        if details_rows > 0 {
            draw_details(frame, app, details);
        }
    }

    draw_footer(frame, app, footer);

    match app.mode {
        Mode::Help => draw_help(frame, app),
        Mode::Models => draw_models(frame, app),
        Mode::Inputs => draw_inputs(frame, app),
        Mode::Theme => draw_theme(frame, app),
        Mode::ConfirmDelete => draw_confirm_delete(frame, app),
        Mode::ConfirmStop => draw_confirm_stop(frame, app),
        Mode::Normal | Mode::Search => {}
    }
}

// ---- shared pieces ----------------------------------------------------------

/// Every pane and popup uses softly rounded corners.
fn rounded<'a>() -> Block<'a> {
    Block::bordered().border_type(BorderType::Rounded)
}

fn block<'a>(t: &Theme) -> Block<'a> {
    rounded().border_style(Style::new().fg(t.border))
}

pub fn title(t: &Theme, text: &str) -> Span<'static> {
    Span::styled(text.to_string(), t.key())
}

/// A key in the accent colour, for use inline.
pub fn key(t: &Theme, k: &str) -> Span<'static> {
    Span::styled(k.to_string(), t.key())
}

/// Key hints such as `"Enter save  Esc cancel"`: each key accent and bold, its
/// meaning muted. Items are separated by two spaces; the first word of an
/// item is the key.
pub fn hint_line(t: &Theme, hints: &str) -> Line<'static> {
    let mut spans = vec![Span::styled(" ", t.muted())];
    for (i, item) in hints.trim().split("  ").enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", t.muted()));
        }
        let (k, label) = item.split_once(' ').unwrap_or((item, ""));
        spans.push(key(t, k));
        if !label.is_empty() {
            spans.push(Span::styled(format!(" {label}"), t.muted()));
        }
    }
    spans.push(Span::styled(" ", t.muted()));
    Line::from(spans)
}

/// Clear the area and paint the page style, so modals match opaque and transparent themes.
fn popup(frame: &mut Frame, t: &Theme, area: Rect) {
    frame.render_widget(Clear, area);
    frame.render_widget(Block::new().style(t.base()), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let (w, h) = (width.min(area.width), height.min(area.height));
    let [row] = Split::vertical([Constraint::Length(h)])
        .flex(Flex::Center)
        .areas(area);
    let [rect] = Split::horizontal([Constraint::Length(w)])
        .flex(Flex::Center)
        .areas(row);
    rect
}

/// The standard modal frame: accent border, accent title, hints along the bottom.
fn modal<'a>(t: &Theme, name: &str, hints: &str) -> Block<'a> {
    rounded()
        .border_style(Style::new().fg(t.accent))
        .title(title(t, &format!(" {name} ")))
        .title_bottom(hint_line(t, hints))
        .style(t.base())
}

fn open_modal(
    frame: &mut Frame,
    t: &Theme,
    width: u16,
    height: u16,
    name: &str,
    hints: &str,
) -> Rect {
    let area = centered(frame.area(), width, height);
    popup(frame, t, area);
    let block = modal(t, name, hints);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

// ---- main screen ------------------------------------------------------------

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let mut block = block(t).title(title(t, " whispertui "));
    if !app.config_label.is_empty() {
        block = block.title_bottom(
            Line::styled(format!(" {} ", app.config_label), t.muted()).right_aligned(),
        );
    }
    let area_inner = block.inner(area);
    frame.render_widget(block, area);
    // The first line is the summary; downloads, when there are any, get the second.
    let [inner, downloads_row] =
        Split::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area_inner);

    // State is always spelled out in words; the colour only reinforces it.
    let mut left = vec![Span::styled("daemon: ", t.muted())];
    match &app.status {
        None => left.push(Span::styled("○ not running", Style::new().fg(t.warning))),
        Some(s) => {
            let (dot, color, label) = match s.state.as_str() {
                "recording" => ("●", t.info, format!("recording {}s", s.secs)),
                "transcribing" => ("◐", t.warning, "transcribing".to_string()),
                _ => ("●", t.success, "ready".to_string()),
            };
            left.push(Span::styled(
                format!("{dot} {label}"),
                Style::new().fg(color),
            ));
        }
    }
    if !app.mic_label.is_empty() {
        left.push(Span::styled("   mic ", t.muted()));
        if app.mic_ready {
            left.push(Span::raw(app.mic_label.clone()));
        } else {
            left.push(Span::styled(
                app.mic_label.clone(),
                Style::new().fg(t.warning),
            ));
        }
    }
    left.push(Span::styled("   model ", t.muted()));
    if app.model_ready {
        left.push(Span::raw(app.model_label.clone()));
    } else {
        left.push(Span::styled(
            format!("{} (not downloaded)", app.model_label),
            Style::new().fg(t.warning),
        ));
    }
    let left = Line::from(left);
    let left_width = left.width();
    frame.render_widget(Paragraph::new(left), inner);
    if !app.downloads.is_empty() {
        draw_downloads(frame, app, downloads_row);
    }
    // The count is the least important thing here: it yields when the line is long.
    let n = app.entries.len();
    let count = format!("{n} recording{}", if n == 1 { "" } else { "s" });
    if left_width + 2 + count.len() <= usize::from(inner.width) {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(count, t.muted()))).alignment(Alignment::Right),
            inner,
        );
    }
}

/// `↓ downloading tiny.en 42% · base.en 10%   X cancel`: every model on its way.
fn draw_downloads(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let mut spans = vec![Span::styled("↓ downloading ", Style::new().fg(t.warning))];
    for (i, d) in app.downloads.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", t.muted()));
        }
        spans.push(Span::styled(
            format!("{} {}%", d.name, d.percent),
            Style::new().fg(t.warning),
        ));
    }
    spans.push(Span::raw("   "));
    spans.push(key(t, "X"));
    spans.push(Span::styled(" cancel", t.muted()));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_list(frame: &mut Frame, app: &App, area: Rect, narrow: bool) {
    let t = &app.theme;
    let visible = app.visible();
    let mut name = format!(" Recordings ({}) ", visible.len());
    if !app.filter.is_empty() {
        name = format!(" Recordings ({}) /{} ", visible.len(), app.filter);
    }
    // The list is the focused pane.
    let block = rounded()
        .border_style(Style::new().fg(t.accent))
        .title(title(t, &name));
    if visible.is_empty() {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let msg = if !app.filter.is_empty() {
            Line::from(Span::styled(
                format!("No recordings match \"{}\".", app.filter),
                t.muted(),
            ))
        } else if !app.model_ready {
            Line::from(vec![
                Span::styled("No model downloaded yet. Press ", t.muted()),
                key(t, "M"),
                Span::styled(" to download one.", t.muted()),
            ])
        } else {
            let hotkey = app
                .status
                .as_ref()
                .map(|s| s.hotkey.as_str())
                .unwrap_or("the hotkey");
            Line::from(vec![
                Span::styled("No recordings yet. Press ", t.muted()),
                key(t, hotkey),
                Span::styled(" or ", t.muted()),
                key(t, "space"),
                Span::styled(" to record.", t.muted()),
            ])
        };
        frame.render_widget(Paragraph::new(msg).wrap(Wrap { trim: true }), inner);
        return;
    }
    // In the narrow side pane the time sits above the text; otherwise they share a line.
    let items: Vec<ListItem> = visible
        .iter()
        .map(|e| {
            let text = e.text.split_whitespace().collect::<Vec<_>>().join(" ");
            if narrow {
                ListItem::new(vec![
                    Line::from(Span::styled(
                        e.created.format("%a %d %b %H:%M").to_string(),
                        t.muted(),
                    )),
                    Line::raw(text),
                ])
            } else {
                ListItem::new(Line::from(vec![
                    Span::styled(e.created.format("%a %d %b %H:%M  ").to_string(), t.muted()),
                    Span::raw(text),
                ]))
            }
        })
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(t.selection())
        .highlight_symbol("▌ ");
    let mut state = app.list_state.borrow_mut();
    state.select(Some(app.selected.min(visible.len() - 1)));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_details(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let block = block(t).title(title(t, " Details "));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines = match app.selected_entry() {
        None => vec![Line::from(Span::styled("Nothing selected.", t.muted()))],
        Some(e) => vec![
            Line::from(vec![
                Span::styled("Recorded ", t.muted()),
                Span::raw(e.created.format("%Y-%m-%d %H:%M:%S").to_string()),
                Span::styled("   Length ", t.muted()),
                Span::raw(format!("{:.1}s", e.duration_secs)),
                Span::styled("   Audio ", t.muted()),
                if app.playing.as_deref() == Some(e.id.as_str()) {
                    Span::styled("playing", Style::new().fg(t.info))
                } else {
                    Span::raw(if e.audio.is_some() {
                        "kept"
                    } else {
                        "not kept"
                    })
                },
            ]),
            Line::raw(""),
            Line::raw(e.text.clone()),
        ],
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// A footer hint. Lower rank is more important; rank 0 is never dropped.
#[derive(Clone, Debug, PartialEq)]
pub struct Hint {
    pub rank: u8,
    pub key: &'static str,
    pub label: &'static str,
}

/// Hints that act on the selected recording are left out when nothing is selected.
pub fn footer_hints(app: &App) -> Vec<Hint> {
    let sel = app.selected_entry();
    let mut h = vec![Hint {
        rank: 0,
        key: "space",
        label: "record",
    }];
    if sel.is_some() {
        h.push(Hint {
            rank: 1,
            key: "y",
            label: "copy",
        });
        if app.playing.is_none() && sel.is_some_and(|e| e.audio.is_some()) {
            h.push(Hint {
                rank: 3,
                key: "a",
                label: "play",
            });
        }
        h.push(Hint {
            rank: 2,
            key: "d",
            label: "delete",
        });
    }
    if app.playing.is_some() {
        // While audio plays, stopping it is what `a` does, whatever is selected.
        h.push(Hint {
            rank: 1,
            key: "a",
            label: "stop",
        });
    }
    h.push(Hint {
        rank: 4,
        key: "/",
        label: "search",
    });
    h.push(Hint {
        rank: 0,
        key: "?",
        label: "help",
    });
    h.push(Hint {
        rank: 0,
        key: "q",
        label: "quit",
    });
    h
}

fn hints_width(hints: &[Hint]) -> usize {
    1 + hints
        .iter()
        .map(|h| h.key.chars().count() + 1 + h.label.chars().count())
        .sum::<usize>()
        + 2 * hints.len().saturating_sub(1)
}

/// Drop the highest-ranked (rightmost on ties) hints until the rest fit.
pub fn fit_hints(mut hints: Vec<Hint>, width: usize) -> Vec<Hint> {
    while hints_width(&hints) > width {
        let Some(max) = hints.iter().map(|h| h.rank).max().filter(|r| *r > 0) else {
            break;
        };
        let at = hints.iter().rposition(|h| h.rank == max).unwrap();
        hints.remove(at);
    }
    hints
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let line = if app.mode == Mode::Search {
        let mut spans = vec![
            Span::raw(" "),
            key(t, "/"),
            Span::raw(format!("{}█", app.filter)),
        ];
        if usize::from(area.width) > app.filter.chars().count() + 40 {
            spans.push(Span::raw("   "));
            spans.extend(hint_line(t, "Enter apply  Esc clear").spans);
        }
        Line::from(spans)
    } else if let Some(f) = &app.flash {
        let color = if f.error { t.error } else { t.success };
        Line::from(Span::styled(format!(" {}", f.text), Style::new().fg(color)))
    } else {
        let hints = fit_hints(footer_hints(app), usize::from(area.width));
        let text = hints
            .iter()
            .map(|h| format!("{} {}", h.key, h.label))
            .collect::<Vec<_>>()
            .join("  ");
        hint_line(t, &text)
    };
    let left_width = line.width();
    frame.render_widget(Paragraph::new(line), area);

    // The global record hotkey, right-aligned. It is the least important
    // thing here, so it only shows when the hints or message leave room for it.
    let Some(st) = &app.status else { return };
    let hotkey = Line::from(vec![
        Span::styled("hotkey ", t.muted()),
        key(t, &st.hotkey),
        Span::styled(format!(" ({}) ", st.mode), t.muted()),
    ]);
    if left_width + 2 + hotkey.width() <= usize::from(area.width) {
        frame.render_widget(Paragraph::new(hotkey).alignment(Alignment::Right), area);
    }
}

// ---- modals -----------------------------------------------------------------

const HELP: &[(&str, &[(&str, &str)])] = &[
    (
        "Recordings",
        &[
            ("y", "copy the transcript to the clipboard"),
            ("space", "start / stop recording"),
            ("a", "play the audio; again to stop it"),
            ("d", "delete (asks first)"),
            ("/", "search transcripts; Esc clears"),
        ],
    ),
    (
        "Move",
        &[
            ("↑↓ j k", "move"),
            ("g G", "first / last"),
            ("PgUp PgDn", "jump 10"),
        ],
    ),
    (
        "Screens",
        &[
            ("L", "layout: side by side (30/70) or stacked"),
            ("I", "microphone: choose the input device"),
            ("M", "models: download and pick one (x cancels one)"),
            ("X", "cancel running model downloads"),
            ("T", "theme picker (b: transparent background)"),
            ("E", "edit the config"),
            ("?", "this help"),
        ],
    ),
    (
        "Quit",
        &[
            ("q", "close the window; the daemon keeps running"),
            ("Q", "close and stop the daemon (asks first)"),
        ],
    ),
];

fn draw_help(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let mut lines: Vec<Line> = Vec::new();
    if let Some(s) = &app.status {
        lines.push(Line::from(vec![
            Span::styled("Record hotkey ", t.muted()),
            key(t, &s.hotkey),
            Span::styled(format!(" ({}); change it with ", s.mode), t.muted()),
            key(t, "E"),
        ]));
        lines.push(Line::raw(""));
    }
    for (i, (heading, rows)) in HELP.iter().enumerate() {
        if i > 0 {
            lines.push(Line::raw(""));
        }
        lines.push(Line::from(Span::styled(
            *heading,
            Style::new().fg(t.fg).add_modifier(Modifier::BOLD),
        )));
        for (k, d) in *rows {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{k:<10}"), t.key()),
                Span::styled(*d, t.muted()),
            ]));
        }
    }
    let h = lines.len() as u16 + 2;
    let inner = open_modal(frame, t, 66, h, "Help", "Any key close");
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_models(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let visible = app.visible_models();
    let error_rows = u16::from(app.models_error.is_some());
    // Header line + list + optional error line, inside the border. The height
    // follows the whole list, not the filtered one, so the dialog doesn't jump.
    let want = app.models.len() as u16 + 1 + error_rows + 2;
    let hints = if app.models_searching {
        "Enter apply  Esc clear"
    } else if !app.models_filter.is_empty() {
        "Enter download or use  / filter  r refresh  x cancel  Esc clear filter"
    } else {
        "Enter download or use  / filter  r refresh  x cancel  Esc close"
    };
    let inner = open_modal(frame, t, 100, want.max(8), "Models", hints);
    let [status, list_area, error] = Split::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(error_rows),
    ])
    .areas(inner);

    let status_line = if app.models_searching {
        Line::from(vec![
            key(t, "/"),
            Span::raw(format!("{}█", app.models_filter)),
            Span::styled(
                format!("   {} of {}", visible.len(), app.models.len()),
                t.muted(),
            ),
        ])
    } else if app.catalog_refreshing {
        Line::from(Span::styled(
            "Refreshing the list from huggingface.co…",
            Style::new().fg(t.warning),
        ))
    } else if !app.models_filter.is_empty() {
        Line::from(vec![
            Span::styled("Filter ", t.muted()),
            key(t, &format!("/{}", app.models_filter)),
            Span::styled(
                format!("   {} of {} models", visible.len(), app.models.len()),
                t.muted(),
            ),
        ])
    } else if let Some(day) = &app.catalog_updated {
        Line::from(Span::styled(
            format!("From huggingface.co/ggerganov/whisper.cpp, updated {day}"),
            t.muted(),
        ))
    } else {
        Line::from(Span::styled(
            "No model list yet. Press r to fetch it.",
            t.muted(),
        ))
    };
    frame.render_widget(Paragraph::new(status_line), status);

    if visible.is_empty() && !app.models.is_empty() {
        let msg = format!("No models match \"{}\".", app.models_filter);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(msg, t.muted()))),
            list_area,
        );
    } else {
        let items: Vec<ListItem> = visible
            .iter()
            .map(|m| {
                let downloading = app.downloads.iter().find(|d| d.name == m.name);
                let (mark, mark_style) = if m.active {
                    ("● ", Style::new().fg(t.success))
                } else if m.installed {
                    ("○ ", Style::new().fg(t.success))
                } else {
                    ("  ", t.muted())
                };
                let (state, state_style) = match downloading {
                    Some(d) => (
                        format!("downloading {}%", d.percent),
                        Style::new().fg(t.warning),
                    ),
                    None if m.active => ("active".to_string(), Style::new().fg(t.success)),
                    None if m.installed => ("downloaded".to_string(), Style::new().fg(t.success)),
                    None => (models::size_label(m.bytes), t.muted()),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(mark, mark_style),
                    Span::raw(format!("{:<21}", m.name)),
                    Span::styled(format!(" {state:>16}  "), state_style),
                    Span::styled(m.note.clone(), t.muted()),
                ]))
            })
            .collect();
        let list = List::new(items).highlight_style(t.selection());
        let mut state = app.models_state.borrow_mut();
        state.select((!visible.is_empty()).then(|| app.model_sel.min(visible.len() - 1)));
        frame.render_stateful_widget(list, list_area, &mut state);
    }

    if let Some(e) = &app.models_error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                e.clone(),
                Style::new().fg(t.error),
            ))),
            error,
        );
    }
}

fn draw_inputs(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let height = (app.inputs.len() as u16 + 4).max(6);
    let inner = open_modal(
        frame,
        t,
        76,
        height,
        "Microphone",
        "Enter select  r refresh  Esc close",
    );
    let [status, list_area] =
        Split::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);

    let status_line = if app.mic_ready {
        Line::from(vec![
            Span::styled("Recording from ", t.muted()),
            Span::raw(app.mic_label.clone()),
        ])
    } else {
        Line::from(vec![
            Span::styled("Recording is blocked: ", t.muted()),
            Span::styled(app.mic_label.clone(), Style::new().fg(t.warning)),
        ])
    };
    frame.render_widget(Paragraph::new(status_line), status);

    let items: Vec<ListItem> = app
        .inputs
        .iter()
        .map(|r| {
            let (mark, mark_style) = if r.active {
                ("● ", Style::new().fg(t.success))
            } else {
                ("  ", t.muted())
            };
            let detail_style = if r.detail == "not connected" {
                Style::new().fg(t.warning)
            } else {
                t.muted()
            };
            ListItem::new(Line::from(vec![
                Span::styled(mark, mark_style),
                Span::raw(format!("{:<34}", r.label)),
                Span::styled(r.detail.clone(), detail_style),
            ]))
        })
        .collect();
    let list = List::new(items).highlight_style(t.selection());
    let mut state = ListState::default()
        .with_selected((!app.inputs.is_empty()).then(|| app.input_sel.min(app.inputs.len() - 1)));
    frame.render_stateful_widget(list, list_area, &mut state);
}

fn draw_theme(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let cursor = app
        .theme_picker
        .as_ref()
        .map_or(app.theme_index, |p| p.cursor);
    let height = (app.themes.len() as u16 + 3).max(4);
    let inner = open_modal(
        frame,
        t,
        56,
        height,
        "Theme",
        "↑↓ preview  b background  Enter save  Esc cancel",
    );
    let [bg_row, list_area] =
        Split::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Background  ", t.muted()),
            Span::styled(
                if app.transparent {
                    "transparent (terminal's own)"
                } else {
                    "theme colour"
                },
                Style::new().fg(t.accent),
            ),
        ])),
        bg_row,
    );
    let name_w = app
        .themes
        .iter()
        .map(|th| th.name.chars().count())
        .max()
        .unwrap_or(0);
    let items: Vec<ListItem> = app
        .themes
        .iter()
        .map(|th| {
            let mut spans = vec![Span::raw(format!("{:<name_w$}  ", th.name))];
            // Each theme's own colours, so you can compare before choosing.
            spans.extend(
                th.swatch()
                    .into_iter()
                    .map(|c| Span::styled("██ ", Style::new().fg(c))),
            );
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(t.selection())
        .highlight_symbol("▶ ");
    let mut state = ListState::default().with_selected(Some(cursor));
    frame.render_stateful_widget(list, list_area, &mut state);
}

fn draw_confirm_delete(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let Some(e) = app.selected_entry() else {
        return;
    };
    let preview: String = e.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let lines = vec![
        Line::from(Span::styled(
            "Delete this recording?",
            Style::new().fg(t.warning),
        )),
        Line::raw(""),
        Line::from(vec![
            Span::styled(
                e.created.format("%Y-%m-%d %H:%M:%S  ").to_string(),
                t.muted(),
            ),
            Span::raw(preview),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            if e.audio.is_some() {
                "The transcript and its audio file are removed permanently."
            } else {
                "The transcript is removed permanently."
            },
            t.muted(),
        )),
    ];
    let inner = open_modal(
        frame,
        t,
        64,
        9,
        "Delete recording",
        "y delete  any other key cancel",
    );
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn draw_confirm_stop(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let busy = app.status.as_ref().is_some_and(|s| s.state != "idle");
    let mut lines = vec![
        Line::from(Span::styled(
            "Stop the background daemon?",
            Style::new().fg(t.warning),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "The hotkey stops working until the daemon is started again.",
            t.muted(),
        )),
    ];
    if busy {
        lines.push(Line::from(Span::styled(
            "A recording or transcription is in progress and will be lost.",
            Style::new().fg(t.warning),
        )));
    }
    let h = lines.len() as u16 + 2;
    let inner = open_modal(
        frame,
        t,
        64,
        h,
        "Stop daemon",
        "y stop  any other key cancel",
    );
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{app, ch, entry, key as kev};
    use crate::audio;
    use crate::daemon::Status;
    use crate::theme::{builtin_themes, position};
    use crossterm::event::KeyCode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;

    fn render_buf(app: &App, w: u16, h: u16) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        term.backend().buffer().clone()
    }

    fn text(buf: &Buffer) -> String {
        let a = buf.area;
        (0..a.height)
            .map(|y| {
                (0..a.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render(app: &App, w: u16, h: u16) -> String {
        text(&render_buf(app, w, h))
    }

    fn status(state: &str) -> Status {
        Status {
            state: state.into(),
            secs: 4,
            hotkey: "right_option".into(),
            mode: "toggle".into(),
        }
    }

    /// Foreground colour of the first cell showing `needle` on the screen.
    fn fg_of(buf: &Buffer, needle: &str) -> Option<Color> {
        let a = buf.area;
        for y in 0..a.height {
            let line: String = (0..a.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            if let Some(byte) = line.find(needle) {
                let col = line[..byte].chars().count() as u16;
                return Some(buf[(col, y)].fg);
            }
        }
        None
    }

    fn nord_app() -> App {
        let mut a = app();
        let i = position(&builtin_themes(), "nord").unwrap();
        a.set_themes(builtin_themes(), i, false);
        a.status = Some(status("idle"));
        a
    }

    fn model_rows() -> Vec<models::Row> {
        let row = |name: &str, mb: u64, installed, active| models::Row {
            name: name.into(),
            bytes: mb << 20,
            note: "Multilingual, full precision".into(),
            installed,
            active,
        };
        vec![
            row("tiny.en", 74, false, false),
            row("base.en", 141, true, true),
        ]
    }

    #[test]
    fn header_is_a_titled_box_with_the_daemon_summary() {
        let mut a = app();
        a.status = Some(status("idle"));
        a.model_label = "large-v3-turbo-q5_0".into();
        a.config_label = "~/.config/whispertui".into();
        let buf = render_buf(&a, 100, 30);
        let s = text(&buf);
        let top: Vec<&str> = s.lines().take(3).collect();
        assert!(
            top[0].contains("whispertui") && top[0].starts_with('╭'),
            "{top:?}"
        );
        assert!(
            top[1].contains("daemon: ● ready")
                && !top[1].contains("hotkey")
                && top[1].contains("model large-v3-turbo-q5_0"),
            "{top:?}"
        );
        assert!(top[1].contains("3 recordings"), "{top:?}");
        assert!(
            top[2].contains("~/.config/whispertui") && top[2].starts_with('╰'),
            "{top:?}"
        );
        // The dot and state keep their status colour; the label is muted.
        assert_eq!(fg_of(&buf, "● ready"), Some(a.theme.success));
        assert_eq!(fg_of(&buf, "daemon:"), Some(a.theme.muted));
    }

    #[test]
    fn daemon_states_are_words_with_their_colours() {
        let mut a = nord_app();
        for (state, word, color) in [
            ("recording", "● recording 4s", a.theme.info),
            ("transcribing", "◐ transcribing", a.theme.warning),
        ] {
            a.status = Some(status(state));
            let buf = render_buf(&a, 100, 20);
            assert!(text(&buf).contains(&format!("daemon: {word}")), "{state}");
            assert_eq!(fg_of(&buf, word), Some(color), "{state}");
        }
        a.status = None;
        let buf = render_buf(&a, 100, 20);
        assert!(text(&buf).contains("daemon: ○ not running"));
        assert_eq!(fg_of(&buf, "○ not running"), Some(a.theme.warning));
    }

    #[test]
    fn a_missing_model_is_called_out() {
        let mut a = nord_app();
        a.model_label = "large-v3-turbo-q5_0".into();
        a.model_ready = false;
        let buf = render_buf(&a, 110, 24);
        assert!(text(&buf).contains("large-v3-turbo-q5_0 (not downloaded)"));
        assert_eq!(
            fg_of(&buf, "large-v3-turbo-q5_0 (not"),
            Some(a.theme.warning)
        );
        a.set_entries(vec![]);
        let s = render(&a, 110, 24);
        assert!(
            s.contains("No model downloaded yet") && s.contains("to download one"),
            "{s}"
        );
    }

    #[test]
    fn panes_have_rounded_corners() {
        let mut a = nord_app();
        a.handle_key(ch('M'));
        let s = render(&a, 110, 30);
        assert!(
            !s.contains('┌') && !s.contains('└') && !s.contains('┐') && !s.contains('┘'),
            "square corners found:\n{s}"
        );
        for c in ['╭', '╮', '╰', '╯'] {
            assert!(s.contains(c), "{c}");
        }
    }

    #[test]
    fn vertical_layout_puts_the_list_left_at_30_percent() {
        let a = nord_app();
        let s = render(&a, 100, 24);
        let row = s.lines().nth(3).unwrap(); // first body row: both panes' top borders
        let list_edge = row.chars().position(|c| c == '╮').unwrap();
        assert!(
            (29..=31).contains(&(list_edge + 1)),
            "list pane is ~30 columns wide, got {}:\n{s}",
            list_edge + 1
        );
        assert!(
            row.contains("Recordings") && row.contains("Details"),
            "{row}"
        );
        // Time above text in the narrow list; the transcript is in the details pane.
        assert!(s.contains("third thing"));
    }

    #[test]
    fn horizontal_layout_stacks_the_panes() {
        let mut a = nord_app();
        a.handle_key(ch('L'));
        let s = render(&a, 100, 30);
        let rows: Vec<&str> = s.lines().collect();
        let list_row = rows.iter().position(|r| r.contains("Recordings")).unwrap();
        let details_row = rows.iter().position(|r| r.contains("Details")).unwrap();
        assert!(details_row > list_row + 3, "details below the list:\n{s}");
        assert!(!rows[list_row].contains("Details"));
        // Time and text share a line when there's width.
        assert!(
            rows.iter().any(|r| r.contains(" third thing"))
                && rows.iter().any(|r| r.contains("19:") || r.contains(":"))
        );
    }

    #[test]
    fn stacked_layout_gives_the_list_30_percent_of_the_height() {
        let mut a = nord_app();
        a.handle_key(ch('L'));
        for (h, body) in [(24u16, 20u16), (40, 36), (60, 56)] {
            let s = render(&a, 100, h);
            let rows: Vec<&str> = s.lines().collect();
            let list_top = rows.iter().position(|r| r.contains("Recordings")).unwrap();
            let details_top = rows.iter().position(|r| r.contains("Details")).unwrap();
            assert_eq!(list_top, 3, "body starts under the 3-row header");
            let list_rows = details_top - list_top;
            let want = (body * 30 / 100).max(5);
            assert_eq!(
                list_rows as u16, want,
                "height {h}: list is 30% of the {body}-row body\n{s}"
            );
            assert_eq!(
                details_top + (body - want) as usize,
                rows.len() - 1,
                "details take the rest, above the footer"
            );
        }
        // Short terminals: the list keeps a few rows, the details pane gives way.
        let s = render(&a, 100, 10);
        assert!(s.contains("third thing"), "{s}");
    }

    #[test]
    fn narrow_terminals_stack_even_in_vertical_layout() {
        let a = nord_app();
        let s = render(&a, 60, 30);
        let rows: Vec<&str> = s.lines().collect();
        let list_row = rows.iter().position(|r| r.contains("Recordings")).unwrap();
        let details_row = rows.iter().position(|r| r.contains("Details")).unwrap();
        assert!(details_row > list_row, "{s}");
        assert!(!rows[list_row].contains("Details"));
    }

    #[test]
    fn main_screen_shows_the_essentials() {
        let mut a = app();
        a.status = Some(status("idle"));
        let s = render(&a, 100, 30);
        for want in [
            "Recordings (3)",
            "Details",
            "third thing",
            "space record",
            "y copy",
            "d delete",
            "/ search",
            "? help",
            "q quit",
        ] {
            assert!(s.contains(want), "missing {want:?}:\n{s}");
        }
        // Rare keys live in help only.
        for absent in ["edit config", "models", "theme", "layout"] {
            assert!(!s.contains(absent), "footer should not mention {absent:?}");
        }
    }

    #[test]
    fn empty_states() {
        let mut a = App::new(builtin_themes(), 0, true);
        a.status = Some(status("idle"));
        let s = render(&a, 100, 24);
        assert!(
            s.contains("No recordings yet") && s.contains("Nothing selected"),
            "{s}"
        );
        assert!(
            !s.contains("y copy") && !s.contains("d delete") && s.contains("space record"),
            "{s}"
        );
        a.set_entries(vec![entry("1", "hello")]);
        a.filter = "zzz".into();
        assert!(render(&a, 100, 24).contains("No recordings match \"zzz\""));
    }

    #[test]
    fn hints_drop_by_rank_but_never_the_essentials() {
        let a = app();
        let all = footer_hints(&a);
        assert_eq!(fit_hints(all.clone(), 200), all);
        let narrow = fit_hints(all.clone(), hints_width(&all) - 1);
        assert!(
            !narrow.iter().any(|h| h.key == "/"),
            "search (rank 4) goes first"
        );
        let tiny = fit_hints(all, 1);
        let keys: Vec<_> = tiny.iter().map(|h| h.key).collect();
        assert_eq!(
            keys,
            ["space", "?", "q"],
            "record, help and quit always stay"
        );
    }

    #[test]
    fn footer_message_replaces_hints_and_uses_status_colours() {
        let mut a = nord_app();
        a.flash_ok("Copied to clipboard");
        let buf = render_buf(&a, 100, 20);
        assert!(text(&buf).contains("Copied to clipboard") && !text(&buf).contains("q quit"));
        assert_eq!(fg_of(&buf, "Copied"), Some(a.theme.success));
        a.flash_err("Copy failed");
        assert_eq!(
            fg_of(&render_buf(&a, 100, 20), "Copy failed"),
            Some(a.theme.error)
        );
    }

    #[test]
    fn footer_shows_the_record_hotkey_and_drops_it_first_when_short_of_room() {
        let a = nord_app();
        let buf = render_buf(&a, 100, 20);
        let footer = text(&buf).lines().last().unwrap().to_string();
        assert!(
            footer.trim_end().ends_with("hotkey right_option (toggle)"),
            "{footer:?}"
        );
        assert!(footer.contains("space record") && footer.contains("q quit"));
        // Like every key hint: the key is accent and bold, the words muted.
        let last = buf.area.height - 1;
        let fg_in_footer = |needle: &str| {
            let row: String = (0..buf.area.width)
                .map(|x| buf[(x, last)].symbol().to_string())
                .collect();
            let col = row[..row.find(needle).unwrap()].chars().count() as u16;
            buf[(col, last)].fg
        };
        assert_eq!(fg_in_footer("right_option"), a.theme.accent);
        assert_eq!(fg_in_footer("hotkey "), a.theme.muted);

        // Too narrow for both: the hotkey goes, the essential hints stay.
        let footer = render(&a, 62, 20).lines().last().unwrap().to_string();
        assert!(
            !footer.contains("hotkey")
                && footer.contains("space record")
                && footer.contains("? help")
                && footer.contains("q quit"),
            "{footer:?}"
        );

        // A status message keeps it when there is room.
        let mut b = nord_app();
        b.flash_ok("Copied to clipboard");
        let footer = render(&b, 100, 20).lines().last().unwrap().to_string();
        assert!(
            footer.contains("Copied to clipboard") && footer.contains("hotkey right_option"),
            "{footer:?}"
        );

        // It follows the config: hold mode, another key.
        let mut c = nord_app();
        c.status = Some(Status {
            state: "idle".into(),
            secs: 0,
            hotkey: "cmd+shift+space".into(),
            mode: "hold".into(),
        });
        assert!(
            render(&c, 100, 20)
                .lines()
                .last()
                .unwrap()
                .contains("hotkey cmd+shift+space (hold)")
        );

        // No daemon, no hotkey to show.
        c.status = None;
        assert!(
            !render(&c, 100, 20)
                .lines()
                .last()
                .unwrap()
                .contains("hotkey")
        );
    }

    #[test]
    fn footer_offers_stop_while_audio_plays_and_details_say_so() {
        let mut a = nord_app();
        let footer = |a: &App| render(a, 100, 24).lines().last().unwrap().to_string();
        assert!(footer(&a).contains("a play") && !footer(&a).contains("a stop"));

        a.playing = Some("3".into());
        let buf = render_buf(&a, 100, 24);
        let f = text(&buf).lines().last().unwrap().to_string();
        assert!(f.contains("a stop") && !f.contains("a play"), "{f:?}");
        assert!(
            text(&buf).contains("Audio playing"),
            "details show it on the playing recording"
        );
        assert_eq!(fg_of(&buf, "playing"), Some(a.theme.info));

        // Another recording selected: stop is still offered, and only the playing one says "playing".
        a.handle_key(ch('j'));
        let s = render(&a, 100, 24);
        assert!(
            s.lines().last().unwrap().contains("a stop") && !s.contains("Audio playing"),
            "{s}"
        );

        // Nothing selected, still playing: stopping must stay reachable.
        let mut empty = App::new(builtin_themes(), 0, true);
        empty.playing = Some("gone".into());
        assert!(
            render(&empty, 100, 24)
                .lines()
                .last()
                .unwrap()
                .contains("a stop")
        );
    }

    #[test]
    fn header_shows_the_microphone_or_that_it_is_unavailable() {
        let mut a = nord_app();
        a.model_label = "large-v3-turbo-q5_0".into();
        a.mic_label = "MacBook Pro Microphone (default)".into();
        let buf = render_buf(&a, 120, 24);
        let row = text(&buf).lines().nth(1).unwrap().to_string();
        assert!(
            row.contains("mic MacBook Pro Microphone (default)")
                && row.contains("model large-v3-turbo-q5_0"),
            "{row:?}"
        );
        assert!(
            row.find("mic ").unwrap() < row.find("model ").unwrap(),
            "mic comes before model"
        );
        assert_eq!(fg_of(&buf, "mic "), Some(a.theme.muted));
        assert_eq!(
            fg_of(&buf, "MacBook Pro Microphone (default)"),
            Some(a.theme.fg)
        );

        // No microphone: "unavailable", in the warning colour, like a missing model.
        a.mic_label = "unavailable".into();
        a.mic_ready = false;
        let buf = render_buf(&a, 120, 24);
        assert!(
            text(&buf)
                .lines()
                .nth(1)
                .unwrap()
                .contains("mic unavailable")
        );
        assert_eq!(fg_of(&buf, "unavailable"), Some(a.theme.warning));

        // A chosen microphone that is unplugged.
        a.mic_label = "AirPods (unavailable)".into();
        let buf = render_buf(&a, 120, 24);
        assert!(
            text(&buf)
                .lines()
                .nth(1)
                .unwrap()
                .contains("mic AirPods (unavailable)")
        );
        assert_eq!(fg_of(&buf, "AirPods (unavailable)"), Some(a.theme.warning));
    }

    #[test]
    fn a_long_header_drops_the_count_rather_than_overlapping_it() {
        let mut a = nord_app();
        a.mic_label = "Some Very Long Studio Microphone Name (default)".into();
        a.model_label = "large-v3-turbo-q5_0".into();
        a.model_ready = false;
        let row = render(&a, 120, 24).lines().nth(1).unwrap().to_string();
        assert!(
            row.contains("model large-v3-turbo-q5_0 (not downloaded)"),
            "{row:?}"
        );
        assert!(!row.contains("recordings"), "no overlap: {row:?}");
        // With room, the count is back.
        assert!(
            render(&a, 200, 24)
                .lines()
                .nth(1)
                .unwrap()
                .contains("3 recordings")
        );
    }

    #[test]
    fn microphone_dialog_lists_devices_and_marks_the_active_one() {
        let mut a = nord_app();
        a.mic_label = "USB Mic".into();
        a.handle_key(ch('I'));
        let row = |value: &str, label: &str, detail: &str, active| audio::InputRow {
            value: value.into(),
            label: label.into(),
            detail: detail.into(),
            active,
        };
        a.set_inputs(vec![
            row("", "System default", "MacBook Pro Microphone", false),
            row(
                "MacBook Pro Microphone",
                "MacBook Pro Microphone",
                "system default",
                false,
            ),
            row("USB Mic", "USB Mic", "", true),
            row("AirPods", "AirPods", "not connected", false),
        ]);
        let buf = render_buf(&a, 110, 30);
        let s = text(&buf);
        for want in [
            "Microphone",
            "Recording from USB Mic",
            "● USB Mic",
            "System default",
            "system default",
            "not connected",
            "r refresh",
        ] {
            assert!(s.contains(want), "missing {want:?}:\n{s}");
        }
        assert_eq!(fg_of(&buf, "● USB"), Some(a.theme.success));
        assert_eq!(fg_of(&buf, "not connected"), Some(a.theme.warning));

        // Blocked: the dialog says why, in the warning colour.
        a.mic_ready = false;
        a.mic_label = "AirPods (unavailable)".into();
        let buf = render_buf(&a, 110, 30);
        assert!(text(&buf).contains("Recording is blocked: AirPods (unavailable)"));
        assert_eq!(fg_of(&buf, "AirPods (unavailable)"), Some(a.theme.warning));

        // Nothing at all.
        a.set_inputs(vec![row("", "System default", "no input device", true)]);
        assert!(render(&a, 110, 30).contains("no input device"));
    }

    #[test]
    fn footer_keys_use_the_accent_colour() {
        let a = nord_app();
        let buf = render_buf(&a, 100, 20);
        assert_eq!(fg_of(&buf, "space"), Some(a.theme.accent));
        assert_eq!(fg_of(&buf, " record"), Some(a.theme.muted));
        assert_eq!(fg_of(&buf, "q quit"), Some(a.theme.accent));
    }

    #[test]
    fn modal_hints_use_the_accent_colour() {
        let mut a = nord_app();
        a.models = model_rows();
        for (open, hint, fragment) in [
            ('T', "↑↓", "Theme"),
            ('M', "Enter", "Models"),
            ('I', "Enter", "Microphone"),
            ('d', "y delete  any", "Delete recording"),
            ('Q', "y stop  any", "Stop daemon"),
            ('?', "Any", "Help"),
        ] {
            a.mode = Mode::Normal;
            a.handle_key(ch(open));
            let buf = render_buf(&a, 120, 40);
            let s = text(&buf);
            assert!(s.contains(fragment) && s.contains(hint), "{fragment}:\n{s}");
            assert_eq!(fg_of(&buf, hint), Some(a.theme.accent), "{fragment} hint");
            assert_eq!(
                fg_of(&buf, fragment),
                Some(a.theme.accent),
                "{fragment} title"
            );
        }
    }

    #[test]
    fn delete_dialog_is_explicit() {
        let mut a = nord_app();
        a.handle_key(ch('d'));
        let s = render(&a, 100, 30);
        assert!(
            s.contains("third thing")
                && s.contains("audio file are removed permanently")
                && s.contains("any other key cancel"),
            "{s}"
        );
        a.entries[0].audio = None;
        assert!(render(&a, 100, 30).contains("The transcript is removed permanently"));
    }

    #[test]
    fn stop_dialog_warns_when_busy() {
        let mut a = nord_app();
        a.handle_key(ch('Q'));
        assert!(!render(&a, 100, 30).contains("will be lost"));
        a.status = Some(status("recording"));
        assert!(render(&a, 100, 30).contains("will be lost"));
    }

    #[test]
    fn help_lists_keys_and_the_hotkey() {
        let mut a = nord_app();
        a.handle_key(ch('?'));
        let s = render(&a, 100, 40);
        for want in [
            "copy the transcript",
            "edit the config",
            "theme picker",
            "models",
            "layout",
            "right_option",
            "stop the daemon",
        ] {
            assert!(s.contains(want), "help missing {want:?}:\n{s}");
        }
    }

    #[test]
    fn models_dialog_shows_state_progress_refresh_and_errors_in_the_dialog() {
        let mut a = nord_app();
        a.models = model_rows();
        a.handle_key(ch('M'));
        let s = render(&a, 120, 30);
        assert!(
            s.contains("74 MB")
                && s.contains("active")
                && s.contains("Multilingual")
                && s.contains("r refresh"),
            "{s}"
        );
        assert!(s.contains("No model list yet"), "{s}");
        a.catalog_updated = Some("2026-10-26".into());
        assert!(render(&a, 120, 30).contains("updated 2026-10-26"));
        a.catalog_refreshing = true;
        let buf = render_buf(&a, 120, 30);
        assert!(text(&buf).contains("Refreshing the list from huggingface.co"));
        assert_eq!(fg_of(&buf, "Refreshing"), Some(a.theme.warning));
        a.catalog_refreshing = false;
        a.downloads.push(crate::app::Download {
            name: "tiny.en".into(),
            percent: 42,
        });
        assert!(render(&a, 120, 30).contains("downloading 42%"));
        a.handle_key(kev(KeyCode::Up));
        a.handle_key(kev(KeyCode::Enter)); // already downloading
        let buf = render_buf(&a, 120, 30);
        assert!(text(&buf).contains("tiny.en is already downloading (x cancels it)"));
        assert_eq!(fg_of(&buf, "tiny.en is already"), Some(a.theme.error));
        assert!(text(&buf).contains("x cancel"), "the dialog offers it");
    }

    #[test]
    fn models_dialog_filters_and_says_so() {
        let mut a = nord_app();
        a.models = vec![
            models::Row {
                name: "tiny.en".into(),
                bytes: 74 << 20,
                note: "English only, full precision".into(),
                installed: false,
                active: false,
            },
            models::Row {
                name: "base-q5_1".into(),
                bytes: 57 << 20,
                note: "Multilingual, quantized (q5_1)".into(),
                installed: false,
                active: false,
            },
            models::Row {
                name: "large-v3-turbo-q5_0".into(),
                bytes: 547 << 20,
                note: "Multilingual, quantized (q5_0)".into(),
                installed: true,
                active: true,
            },
        ];
        a.handle_key(ch('M'));
        let s = render(&a, 120, 30);
        assert!(s.contains("/ filter") && s.contains("tiny.en"), "{s}");

        // Typing: the prompt, a live count, and only matches in the list.
        a.handle_key(ch('/'));
        for c in "q5".chars() {
            a.handle_key(ch(c));
        }
        let buf = render_buf(&a, 120, 30);
        let s = text(&buf);
        assert!(s.contains("/q5█") && s.contains("2 of 3"), "{s}");
        assert!(
            s.contains("base-q5_1") && s.contains("large-v3-turbo-q5_0") && !s.contains("tiny.en"),
            "{s}"
        );
        assert!(s.contains("Enter apply") && s.contains("Esc clear"), "{s}");
        assert_eq!(fg_of(&buf, "/q5"), Some(a.theme.accent));

        // Applied: the filter stays visible, and Esc says it clears it.
        a.handle_key(kev(KeyCode::Enter));
        let s = render(&a, 120, 30);
        assert!(
            s.contains("Filter /q5")
                && s.contains("2 of 3 models")
                && s.contains("Esc clear filter")
                && !s.contains("tiny.en"),
            "{s}"
        );

        // No matches: say so in the dialog.
        a.handle_key(ch('/'));
        a.handle_key(ch('z'));
        let s = render(&a, 120, 30);
        assert!(
            s.contains("No models match \"q5z\"") && s.contains("0 of 3"),
            "{s}"
        );

        // The dialog keeps its height, so it doesn't jump as you type.
        let height = |s: &str| {
            s.lines()
                .filter(|l| l.contains('│') || l.contains('╭') || l.contains('╰'))
                .count()
        };
        a.handle_key(kev(KeyCode::Esc));
        let filtered = render(&a, 120, 30);
        a.handle_key(kev(KeyCode::Esc));
        assert_eq!(height(&render(&a, 120, 30)), height(&filtered));
    }

    #[test]
    fn the_header_lists_every_download_with_or_without_the_dialog() {
        let mut a = nord_app();
        a.set_entries(vec![entry("1", "hello")]);
        let rows = |a: &App| -> Vec<String> {
            render(a, 110, 24)
                .lines()
                .take(5)
                .map(String::from)
                .collect()
        };

        // Nothing downloading: the plain three-row header, the list right under it.
        assert!(rows(&a)[3].contains("Recordings") && !rows(&a)[2].contains("downloading"));

        a.downloads.push(crate::app::Download {
            name: "tiny.en".into(),
            percent: 42,
        });
        a.downloads.push(crate::app::Download {
            name: "large-v3-turbo-q5_0".into(),
            percent: 7,
        });
        let buf = render_buf(&a, 110, 24);
        let top: Vec<String> = text(&buf).lines().take(5).map(String::from).collect();
        assert!(
            top[2].contains("↓ downloading tiny.en 42% · large-v3-turbo-q5_0 7%"),
            "{top:?}"
        );
        assert!(top[2].contains("X cancel"), "{top:?}");
        assert!(
            top[3].starts_with('╰') && top[4].contains("Recordings"),
            "the header grew by one line: {top:?}"
        );
        // Same palette rules as everything else: progress is warning, the key is accent.
        assert_eq!(fg_of(&buf, "tiny.en 42%"), Some(a.theme.warning));
        assert_eq!(fg_of(&buf, "X cancel"), Some(a.theme.accent));
        // The summary line is unaffected.
        assert!(top[1].contains("daemon: ● ready"));

        // With the dialog closed (as here) and open, the header still shows them.
        a.models = model_rows();
        a.handle_key(ch('M'));
        assert!(
            render(&a, 110, 30)
                .lines()
                .nth(2)
                .unwrap()
                .contains("tiny.en 42%")
        );
        a.handle_key(kev(KeyCode::Esc));

        // Cancelled: the extra line goes away again.
        a.downloads.clear();
        assert!(rows(&a)[3].contains("Recordings"));
    }

    #[test]
    fn long_model_lists_scroll_with_the_selection() {
        let mut a = nord_app();
        a.models = (0..40)
            .map(|i| models::Row {
                name: format!("model-{i:02}"),
                bytes: 1 << 20,
                note: String::new(),
                installed: false,
                active: false,
            })
            .collect();
        a.handle_key(ch('M'));
        assert!(render(&a, 120, 20).contains("model-00"));
        for _ in 0..39 {
            a.handle_key(ch('j'));
        }
        let s = render(&a, 120, 20);
        assert!(s.contains("model-39") && !s.contains("model-00"), "{s}");
    }

    #[test]
    fn theme_picker_shows_swatches_and_previews_the_whole_screen() {
        let mut a = nord_app();
        let before = render_buf(&a, 100, 30)[(0, 0)].bg;
        a.handle_key(ch('T'));
        a.handle_key(ch('j'));
        let buf = render_buf(&a, 100, 30);
        let s = text(&buf);
        assert!(
            s.contains("Background  theme colour")
                && s.contains("██")
                && s.contains("catppuccin-latte")
                && s.contains("kanagawa"),
            "{s}"
        );
        assert_ne!(
            buf[(0, 0)].bg,
            before,
            "page colour follows the highlighted theme"
        );
        a.handle_key(kev(KeyCode::Esc));
        assert_eq!(render_buf(&a, 100, 30)[(0, 0)].bg, before);
        // The swatches use each theme's own colours.
        a.handle_key(ch('T'));
        let buf = render_buf(&a, 100, 30);
        let first = a.themes[0].swatch()[0];
        assert!(a.themes.iter().skip(1).any(|t| t.swatch()[0] != first));
        assert!(fg_of(&buf, "██").is_some());
    }

    #[test]
    fn popups_keep_the_right_background_opaque_and_transparent() {
        for transparent in [false, true] {
            let mut a = nord_app();
            a.models = model_rows();
            a.set_transparent(transparent);
            let want = if transparent {
                Color::Reset
            } else {
                a.theme.bg
            };
            for open in ['T', 'M', 'I', '?', 'd', 'Q'] {
                a.mode = Mode::Normal;
                a.handle_key(ch(open));
                let buf = render_buf(&a, 120, 40);
                let (w, h) = (buf.area.width, buf.area.height);
                assert_eq!(buf[(0, h - 2)].bg, want, "page, transparent={transparent}");
                assert_eq!(
                    buf[(w - 2, h / 2)].bg,
                    want,
                    "page right edge, transparent={transparent}"
                );
                // The popup interior (its right-hand padding, away from any text).
                assert_eq!(
                    buf[(w / 2 + 30, h / 2)].bg,
                    want,
                    "modal {open}, transparent={transparent}"
                );
            }
        }
        let mut a = nord_app();
        a.set_transparent(false);
        assert_ne!(a.theme.bg, Color::Reset);
    }

    #[test]
    fn nothing_panics_at_any_size() {
        let mut a = nord_app();
        a.model_label = "model".into();
        a.model_ready = false;
        a.config_label = "~/.config/whispertui".into();
        a.models = model_rows();
        a.flash_err("oops");
        for layout in [Layout::Vertical, Layout::Horizontal] {
            a.layout = layout;
            for open in [
                None,
                Some('T'),
                Some('M'),
                Some('I'),
                Some('?'),
                Some('d'),
                Some('Q'),
                Some('/'),
            ] {
                a.mode = Mode::Normal;
                if let Some(c) = open {
                    a.handle_key(ch(c));
                }
                for (w, h) in [
                    (0, 0),
                    (1, 1),
                    (2, 1),
                    (5, 3),
                    (10, 4),
                    (20, 6),
                    (40, 9),
                    (79, 12),
                    (80, 12),
                    (200, 60),
                ] {
                    render(&a, w, h);
                }
            }
        }
        let empty = App::new(builtin_themes(), 0, true);
        for (w, h) in [(0, 0), (3, 3), (30, 8), (100, 4)] {
            render(&empty, w, h);
        }
    }

    #[test]
    fn long_lists_scroll_with_the_selection() {
        let mut a = nord_app();
        a.set_entries(
            (0..100)
                .map(|i| entry(&i.to_string(), &format!("recording number {i}")))
                .collect(),
        );
        for layout in [Layout::Vertical, Layout::Horizontal] {
            a.layout = layout;
            a.handle_key(ch('G'));
            assert!(
                render(&a, 100, 24).contains("recording number 99"),
                "{layout:?}"
            );
            a.handle_key(ch('g'));
            assert!(
                render(&a, 100, 24).contains("recording number 0"),
                "{layout:?}"
            );
        }
    }

    #[test]
    fn search_prompt_shows_the_query() {
        let mut a = nord_app();
        a.handle_key(ch('/'));
        a.handle_key(ch('t'));
        let s = render(&a, 100, 20);
        assert!(s.contains("/t█"), "{s}");
    }

    #[test]
    fn hint_line_styles_keys_and_labels() {
        let all = builtin_themes();
        let t = &all[position(&all, "nord").unwrap()];
        let line = hint_line(t, "Enter save  Esc cancel");
        let styled: Vec<_> = line
            .spans
            .iter()
            .filter(|s| s.style.fg == Some(t.accent))
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(styled, ["Enter", "Esc"]);
        assert!(
            line.spans
                .iter()
                .any(|s| s.content == " save" && s.style.fg == Some(t.muted))
        );
    }
}
