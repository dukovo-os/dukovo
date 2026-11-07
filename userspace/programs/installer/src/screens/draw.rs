// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The screens drawn, as the approved design draws them: a box of 100 by
//! 29 at most on the slate background, its title and the bar of steps at
//! the top, the page's rows in the middle, its buttons at the bottom, and
//! the keys' hints on the steel bar along the screen's last row. The
//! colours and styles are [`look`]'s.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::app::{App, Field, Id, Kind, Look, Modal, Page, Row};
use super::look::{self, BLUE_GREY, CORAL, GREY, MINT, SAND, TAN, WHITE};
use super::rdisk::{self, FormField};
use crate::log::{self, Step};
use crate::messages::{self, Arg};

/// The box's size at most, in columns and rows: with the key bar under
/// it, the design's 100 by 30.
const WIDTH: u16 = 100;
const HEIGHT: u16 = 29;

/// Where an input's or a list's box starts, from the row's start.
const LABEL: usize = 24;

/// How wide an input's or a list's box is.
const BOX: usize = 40;

/// Draws `app` on `frame`.
pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    // The key bar takes the screen's last row, the box is centred above.
    let above = area.height.saturating_sub(1);
    let outer = Rect {
        x: area.x + area.width.saturating_sub(WIDTH) / 2,
        y: area.y + above.saturating_sub(HEIGHT) / 2,
        width: area.width.min(WIDTH),
        height: above.min(HEIGHT),
    };
    let border = Style::new().fg(BLUE_GREY);
    let title = messages::with("title", &[("version", Arg::Text(&app.version))]);
    // The title starts a cell in from the corner, as the design has it.
    let block = Block::bordered().border_style(border).title(Line::from(vec![
        Span::styled("─", border),
        Span::styled(format!(" {title} "), look::title()),
    ]));
    let inner = block.inner(outer);
    frame.render_widget(block, outer);
    if area.height > 0 {
        let bar = Rect { x: area.x, y: area.bottom() - 1, width: area.width, height: 1 };
        frame.render_widget(
            Paragraph::new(format!(" {}", hints_text(app))).style(look::key_bar()),
            bar,
        );
    }
    if inner.height < 8 || inner.width < 40 {
        return;
    }
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width - 2, height: 1 };
    frame.render_widget(Paragraph::new(step_bar(app)), line(inner.y));
    // The rule under the steps meets the box's sides.
    let rule = format!("├{}┤", "─".repeat(usize::from(outer.width.saturating_sub(2))));
    frame.render_widget(
        Paragraph::new(Span::styled(rule, border)),
        Rect { x: outer.x, y: inner.y + 1, width: outer.width, height: 1 },
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            heading(app),
            Style::new().fg(WHITE).add_modifier(Modifier::BOLD),
        )),
        line(inner.y + 2),
    );
    let content = Rect {
        x: inner.x + 2,
        y: inner.y + 4,
        width: inner.width - 3,
        height: inner.height.saturating_sub(7),
    };
    match app.page {
        Page::Installing | Page::Done | Page::Failed => draw_progress(frame, app, content),
        Page::Rdisk => {
            if let Some(shown) = &app.rdisk {
                rdisk::draw(frame, shown, &app.machine, content);
            }
        }
        _ => draw_rows(frame, app, content),
    }
    let said = app
        .problem
        .as_ref()
        .map(|p| (format!("! {p}"), Style::new().fg(CORAL).add_modifier(Modifier::BOLD)))
        .or_else(|| app.said.clone().map(|s| (s, Style::new().fg(SAND))));
    if let Some((text, style)) = said {
        let text = wrap(&text, usize::from(content.width)).into_iter().next().unwrap_or_default();
        frame.render_widget(Paragraph::new(Span::styled(text, style)), line(inner.bottom() - 2));
    }
    // The buttons end two cells in from the box's side.
    let buttons_line = Rect { width: inner.width - 3, ..line(inner.bottom() - 1) };
    frame.render_widget(Paragraph::new(buttons(app)).right_aligned(), buttons_line);
    if let Some(modal) = &app.modal {
        draw_modal(frame, app, modal, area);
    }
}

/// The name the startup screen shows, spaced out as the boot menu's.
const NAME: &str = "D U K O V O";

/// How wide the startup screen's bar is.
const STARTUP_BAR: usize = 32;

/// How wide the startup screen's text is at most, in columns, when it
/// tells why the installer cannot start.
const STARTUP_TEXT: u16 = 72;

/// Draws the startup screen, while the installer gets ready for the first
/// page: the name, a bar `done` parts of `parts` full, and what is being
/// done now, `now`.
pub fn startup(frame: &mut Frame<'_>, done: usize, parts: usize, now: &str) {
    let filled = STARTUP_BAR * done / parts.max(1);
    startup_lines(
        frame,
        vec![
            Line::from(vec![
                Span::styled("█".repeat(filled), Style::new().fg(TAN)),
                Span::styled("░".repeat(STARTUP_BAR - filled), Style::new().fg(GREY)),
            ]),
            Line::raw(""),
            Line::raw(messages::text("starting")),
            Line::from(Span::styled(now.to_string(), Style::new().fg(GREY))),
        ],
    );
}

/// Draws the startup screen when the installer cannot start: under the
/// name, that it cannot and `why`, and the key that leaves it.
pub fn cannot_start(frame: &mut Frame<'_>, why: &str) {
    let mut lines = vec![
        Line::from(Span::styled(messages::text("cannot-start"), Style::new().fg(CORAL))),
        Line::raw(""),
    ];
    let width = usize::from(frame.area().width.min(STARTUP_TEXT));
    lines.extend(wrap(why, width).into_iter().map(Line::raw));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        messages::text("cannot-start-leave"),
        Style::new().fg(GREY),
    )));
    startup_lines(frame, lines);
}

/// Draws the startup screen's name and, two rows under it, `lines`, each
/// centred, the name three rows above the middle as the design places it.
fn startup_lines(frame: &mut Frame<'_>, lines: Vec<Line<'_>>) {
    let area = frame.area();
    let mut all = vec![Line::from(Span::styled(NAME, look::title())), Line::raw(""), Line::raw("")];
    all.extend(lines);
    let height = u16::try_from(all.len()).unwrap_or(u16::MAX);
    let top = (area.height / 2).saturating_sub(3).min(area.height.saturating_sub(height));
    let at =
        Rect { x: area.x, y: area.y + top, width: area.width, height: height.min(area.height) };
    frame.render_widget(Paragraph::new(all).centered(), at);
}

/// The bar of steps: those done ticked, the one shown highlighted.
fn step_bar(app: &App) -> Line<'static> {
    let at = match app.page {
        Page::SetupFile => None,
        Page::Installing | Page::Done | Page::Failed => Some(Page::STEPS.len()),
        page => page.step(),
    };
    let mut spans = Vec::new();
    for (i, page) in Page::STEPS.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" › ", Style::new().fg(GREY)));
        }
        let name = messages::text(page.name());
        spans.push(match at {
            Some(at) if i < at => Span::styled(format!(" ✓ {name} "), Style::new().fg(MINT)),
            Some(at) if i == at => Span::styled(format!(" {name} "), look::focus()),
            _ => Span::styled(format!(" {name} "), Style::new().fg(GREY)),
        });
    }
    Line::from(spans)
}

/// The page's heading: `Step 2 of 5 · Network and name`.
fn heading(app: &App) -> String {
    let name = messages::text(app.page.name());
    match (app.page.step(), app.page) {
        (Some(i), _) => messages::with(
            "step-of",
            &[
                ("step", Arg::Number(i as u64 + 1)),
                ("steps", Arg::Number(Page::STEPS.len() as u64)),
                ("name", Arg::Text(&name)),
            ],
        ),
        (None, _) => name,
    }
}

/// The keys' hints under the box.
fn hints_text(app: &App) -> String {
    messages::text(match app.page {
        Page::Installing => "hints-installing",
        Page::Done => "hints-done",
        Page::Failed => "hints-failed",
        Page::Rdisk => "hints-rdisk",
        _ => "hints",
    })
}

/// The page's buttons: `< Back > < Next > < Help >`.
fn buttons(app: &App) -> Line<'static> {
    let focused = app.focused();
    let mut spans = Vec::new();
    for (id, label) in app.buttons() {
        spans.push(Span::raw(" "));
        let main = matches!(id, Id::Next | Id::Install | Id::Load | Id::Restart);
        let style = if focused == Some(id) {
            look::focus()
        } else if main {
            look::field().add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        spans.push(Span::styled(format!("< {label} >"), style));
    }
    Line::from(spans)
}

/// Draws the page's rows in `area`, scrolled to show the focused one.
fn draw_rows(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let rows = app.rows();
    let focused = app.focused();
    let width = usize::from(area.width);
    let choice_width = rows
        .iter()
        .filter_map(|row| match row {
            Row::Field(Field { kind: Kind::Choice(_) | Kind::Toggle(_), label, .. }) => {
                Some(label.chars().count() + 6)
            }
            _ => None,
        })
        .max()
        .unwrap_or(0)
        .min(width / 2);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut focus_line = 0;
    let mut cursor = None;
    for row in &rows {
        match row {
            Row::Blank => lines.push(Line::raw("")),
            Row::Text(text, look) => {
                for part in wrap(text, width) {
                    lines.push(Line::from(Span::styled(part, look_style(*look))));
                }
            }
            Row::Field(field) => {
                let has_focus = focused == Some(field.id) && field.enabled;
                if has_focus {
                    focus_line = lines.len();
                    if let Kind::Input { value, .. } = &field.kind {
                        // After the value, past the field's margin.
                        let typed = value.chars().count().min(BOX - 2);
                        cursor = Some((lines.len(), LABEL + 1 + typed));
                    }
                }
                lines.extend(field_lines(field, has_focus, choice_width, width));
            }
        }
    }
    let height = usize::from(area.height);
    let top = if lines.len() > height { (focus_line + 1).saturating_sub(height) } else { 0 };
    let more = lines.len() > top + height;
    let shown: Vec<Line<'static>> = lines.into_iter().skip(top).take(height).collect();
    frame.render_widget(Paragraph::new(shown), area);
    if more {
        let at = Rect { x: area.x, y: area.bottom() - 1, width: area.width, height: 1 };
        frame.render_widget(Clear, at);
        frame.render_widget(
            Paragraph::new(Span::styled(messages::text("more"), Style::new().fg(SAND)))
                .right_aligned(),
            at,
        );
    }
    if let Some((line, column)) = cursor
        && line >= top
        && line < top + height
        && app.modal.is_none()
    {
        let x = area.x + u16::try_from(column).unwrap_or(0);
        let y = area.y + u16::try_from(line - top).unwrap_or(0);
        frame.set_cursor_position(Position { x, y });
    }
}

/// A field's lines, `width` cells wide at most: the field with its note
/// beside it, or under it, indented as its value, when the two do not fit
/// on one line.
fn field_lines(
    field: &Field,
    has_focus: bool,
    choice_width: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let mut line = field_line(field, has_focus, choice_width);
    if field.note.is_empty() {
        return vec![line];
    }
    let dim = Style::new().fg(GREY);
    let beside = format!("  {}", field.note);
    if line.width() + beside.chars().count() <= width {
        line.push_span(Span::styled(beside, dim));
        return vec![line];
    }
    let indent = match field.kind {
        Kind::Input { .. } | Kind::Cycle(_) => LABEL,
        Kind::Choice(_) | Kind::Toggle(_) | Kind::Action => 4,
    };
    let under = wrap(&field.note, width.saturating_sub(indent).max(1));
    let mut lines = vec![line];
    lines.extend(
        under
            .into_iter()
            .map(|part| Line::from(Span::styled(format!("{}{part}", " ".repeat(indent)), dim))),
    );
    lines
}

/// A field's line, without its note.
fn field_line(field: &Field, has_focus: bool, choice_width: usize) -> Line<'static> {
    let plain = if field.enabled { Style::new() } else { Style::new().fg(GREY) };
    let label_style = if has_focus { look::focus() } else { plain };
    match &field.kind {
        Kind::Choice(on) | Kind::Toggle(on) => {
            let mark = match (&field.kind, on) {
                (Kind::Choice(_), true) => "(•)",
                (Kind::Choice(_), false) => "( )",
                (_, true) => "[x]",
                (_, false) => "[ ]",
            };
            let text = format!("{mark} {}", field.label);
            let pad = choice_width.saturating_sub(text.chars().count());
            let style = if *on && field.enabled && !has_focus {
                Style::new().fg(WHITE).add_modifier(Modifier::BOLD)
            } else {
                label_style
            };
            Line::from(vec![Span::styled(text, style), Span::raw(" ".repeat(pad))])
        }
        Kind::Input { value, masked, hint } => {
            let shown = if *masked { "•".repeat(value.chars().count()) } else { value.clone() };
            let (text, style) = if shown.is_empty() {
                (hint.clone(), look::field().fg(BLUE_GREY))
            } else {
                (shown, look::field())
            };
            // The end of what was typed shows, a cell in from each side.
            let text: String =
                text.chars().rev().take(BOX - 2).collect::<Vec<_>>().into_iter().rev().collect();
            let mut spans = label_spans(&field.label, label_style);
            spans.push(Span::styled(pad_to(&format!(" {text}"), BOX), style));
            Line::from(spans)
        }
        Kind::Cycle(value) => {
            let mut spans = label_spans(&field.label, label_style);
            spans.push(Span::styled(pad_to(&format!(" ‹ {value} ›"), BOX), look::field()));
            Line::from(spans)
        }
        Kind::Action => {
            let style = if has_focus {
                look::focus()
            } else if field.enabled {
                Style::new().fg(WHITE).add_modifier(Modifier::BOLD)
            } else {
                plain
            };
            Line::from(vec![Span::styled(format!("[ {} ]", field.label), style)])
        }
    }
}

/// A field's label in `style`, then the spaces to where its box starts:
/// the focus marks the label alone, as the design has it.
fn label_spans(label: &str, style: Style) -> Vec<Span<'static>> {
    let text: String = label.chars().take(LABEL - 1).collect();
    let pad = LABEL - text.chars().count();
    vec![Span::styled(text, style), Span::raw(" ".repeat(pad))]
}

/// Draws the install's progress: the bar, every step, and the log's last
/// line, or how the install ended.
fn draw_progress(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let progress = &app.progress;
    let percent = progress.percent();
    let width = usize::from(area.width).saturating_sub(8);
    let filled = width * usize::from(percent) / 100;
    let mut lines = vec![
        Line::from(vec![
            Span::styled("█".repeat(filled), Style::new().fg(TAN)),
            Span::styled("░".repeat(width - filled), Style::new().fg(GREY)),
            Span::styled(format!(" {percent:>3}%"), Style::new().fg(WHITE)),
        ]),
        Line::raw(""),
    ];
    let disk = app.answers().map_or_else(|_| String::new(), |answers| answers.target().to_string());
    let failed = matches!(progress.finished, Some(Err(_)));
    let done = matches!(progress.finished, Some(Ok(_)));
    for step in Step::ALL {
        let place = progress.started.iter().position(|s| *s == step);
        let last = place.is_some() && place == progress.started.len().checked_sub(1);
        let (mark, style) = match place {
            _ if done => ("✓", Style::new().fg(MINT)),
            Some(_) if !last => ("✓", Style::new().fg(MINT)),
            Some(_) if failed => ("!", Style::new().fg(CORAL).add_modifier(Modifier::BOLD)),
            Some(_) => ("•", Style::new().fg(SAND).add_modifier(Modifier::BOLD)),
            None => ("○", Style::new().fg(GREY)),
        };
        lines.push(Line::from(Span::styled(
            format!("{mark}  {}", step.title(&disk, &app.version)),
            style,
        )));
    }
    lines.push(Line::raw(""));
    let width = usize::from(area.width);
    match &progress.finished {
        Some(Ok(_)) => {
            for (id, look) in [("done-saved", Look::Plain), ("done-remove", Look::Bold)] {
                lines.extend(
                    wrap(&messages::text(id), width)
                        .into_iter()
                        .map(|t| Line::from(Span::styled(t, look_style(look)))),
                );
            }
        }
        Some(Err(why)) => {
            for text in wrap(why, width) {
                lines.push(Line::from(Span::styled(text, Style::new().fg(CORAL))));
            }
            let log = messages::with("failed-log", &[("log", Arg::Text(log::LIVE_PATH))]);
            lines.extend(wrap(&log, width).into_iter().map(Line::raw));
        }
        None => {
            let last: String = progress.last.chars().take(width).collect();
            lines.push(Line::from(Span::styled(last, Style::new().fg(GREY))));
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// Draws `modal` over the page.
fn draw_modal(frame: &mut Frame<'_>, app: &App, modal: &Modal, area: Rect) {
    // The partition form is a small box over the list it changes.
    let small = matches!(modal, Modal::Partition(_));
    let width = area.width.saturating_sub(4).min(if small { 76 } else { 90 });
    let height = area.height.saturating_sub(4).min(26);
    let (title, lines, hint) = match modal {
        Modal::Help(page) => {
            let id = format!("help-{}", page.name().trim_start_matches("page-"));
            let lines = messages::text(&id)
                .lines()
                .flat_map(|line| wrap(line, usize::from(width) - 4))
                .map(Line::raw)
                .collect();
            (messages::text("help-title"), lines, messages::text("hint-close"))
        }
        Modal::Log { top } => {
            let text = std::fs::read_to_string(log::LIVE_PATH).unwrap_or_default();
            let all: Vec<&str> = text.lines().collect();
            let height = usize::from(height) - 3;
            let top = (*top).min(all.len().saturating_sub(height));
            let lines = all
                .iter()
                .skip(top)
                .take(height)
                .map(|line| {
                    Line::raw(line.chars().take(usize::from(width) - 4).collect::<String>())
                })
                .collect();
            (log::LIVE_PATH.to_string(), lines, messages::text("hint-log"))
        }
        Modal::Person(form) => {
            let fields = [
                ("you-name", &form.name, false),
                ("you-user", &form.user, false),
                ("you-password", &form.password, true),
            ];
            let mut lines: Vec<Line<'static>> = vec![Line::raw("")];
            for (i, (label, value, masked)) in fields.into_iter().enumerate() {
                let field = Field {
                    id: Id::Name,
                    kind: Kind::Input { value: value.clone(), masked, hint: String::new() },
                    label: messages::text(label),
                    note: String::new(),
                    enabled: true,
                };
                lines.push(field_line(&field, form.focus == i, 0));
            }
            lines.push(Line::raw(""));
            if let Some(problem) = &form.problem {
                lines
                    .push(Line::from(Span::styled(format!("! {problem}"), Style::new().fg(CORAL))));
            }
            (messages::text("person-title"), lines, messages::text("hint-person"))
        }
        Modal::Partition(form) => {
            let mut lines: Vec<Line<'static>> = vec![Line::raw("")];
            for (i, which) in form.field_list().iter().enumerate() {
                let (id, kind) = match which {
                    FormField::Size => {
                        ("rdisk-form-size", input_kind(&form.size, "rdisk-form-size-hint"))
                    }
                    FormField::Use => ("rdisk-form-use", Kind::Cycle(form.chosen().label())),
                    FormField::Name => ("rdisk-form-name", input_kind(&form.name, "")),
                };
                let field = Field {
                    id: Id::Name,
                    kind,
                    label: messages::text(id),
                    note: String::new(),
                    enabled: true,
                };
                lines.push(field_line(&field, form.focus == i, 0));
            }
            lines.push(Line::raw(""));
            if let Some(problem) = &form.problem {
                for part in wrap(&format!("! {problem}"), usize::from(width) - 4) {
                    lines.push(Line::from(Span::styled(part, Style::new().fg(CORAL))));
                }
            }
            let title = if form.row.is_some() { "rdisk-format-title" } else { "rdisk-new-title" };
            (messages::text(title), lines, messages::text("hint-partition"))
        }
    };
    // A box as tall as what it holds, but the log's, which fills the room
    // it has.
    let fits = !matches!(modal, Modal::Log { .. });
    let height = if fits { height.min(lines.len() as u16 + 3) } else { height };
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let border = Style::new().fg(SAND);
    let block = Block::bordered()
        .border_style(border)
        .title(Line::from(vec![
            Span::styled("─", border),
            Span::styled(format!(" {title} "), look::title()),
        ]))
        .title_bottom(Span::styled(format!(" {hint} "), Style::new().fg(GREY)));
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    frame.render_widget(
        Paragraph::new(lines),
        Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner },
    );
    let _ = app;
}

/// A box to type in, holding `value`, with the hint of the message `hint`
/// while it is empty.
fn input_kind(value: &str, hint: &str) -> Kind {
    let hint = if hint.is_empty() { String::new() } else { messages::text(hint) };
    Kind::Input { value: value.to_string(), masked: false, hint }
}

/// The style of text that looks like `look`.
fn look_style(look: Look) -> Style {
    match look {
        Look::Plain => Style::new(),
        Look::Bold => Style::new().fg(WHITE).add_modifier(Modifier::BOLD),
        Look::Dim => Style::new().fg(GREY),
        Look::Good => Style::new().fg(MINT),
        Look::Warning => Style::new().fg(SAND),
        Look::Danger => Style::new().fg(CORAL).add_modifier(Modifier::BOLD),
    }
}

/// `text` padded with spaces to `width` characters, or cut to it.
fn pad_to(text: &str, width: usize) -> String {
    let mut padded: String = text.chars().take(width).collect();
    let count = padded.chars().count();
    padded.extend(std::iter::repeat_n(' ', width - count));
    padded
}

/// `text` broken into lines of `width` characters at most, between
/// words where it can be. Spaces it starts with start each of its lines,
/// so that an indented row stays indented.
#[must_use]
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let body = text.trim_start_matches(' ');
    let indent = &text[..text.len() - body.len()];
    let width = width.saturating_sub(indent.len()).max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in body.split(' ') {
        let fits = line.chars().count() + 1 + word.chars().count() <= width;
        if !line.is_empty() && !fits {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
        while line.chars().count() > width {
            let rest: String = line.chars().skip(width).collect();
            lines.push(line.chars().take(width).collect());
            line = rest;
        }
    }
    lines.push(line);
    lines.into_iter().map(|line| format!("{indent}{line}")).collect()
}
