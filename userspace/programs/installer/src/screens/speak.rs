// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the screens tell the screen reader (ADR 0422). The screens draw
//! a whole page again for each key and show the focus by its colours,
//! which a reader that follows the screen's changes cannot tell apart; so
//! they name what matters themselves, with consoled's `ESC ] 7700 ; text
//! ST` (ADR 0325), and what they draw is not read meanwhile:
//!
//! - a page as it comes: its name and step, its text, and what has the
//!   focus;
//! - each field as the focus reaches it: its label, its kind, its place
//!   among the choices beside it, its state or what was typed, and its
//!   note;
//! - what a key changes in the focused field: the character typed or
//!   taken away, the list's new value, the choice made;
//! - a problem or a message as it shows, each step of the install as it
//!   starts, and how the install ended.
//!
//! What a password field holds is never said: a character typed into it
//! is "star", one taken away "deleted". consoled passes the words on only
//! while the screen reader speaks, and other terminals ignore the
//! sequence; the screens send none to the serial console.
//!
//! [`Heard::of`] takes what the screens show, and [`speech`] gives the
//! words that tell what changed since the reader last heard.

use super::app::{App, Field, Id, Kind, Modal, Page, Row};
use super::draw;
use super::rdisk::{self, FormField};
use crate::log;
use crate::messages::{self, Arg};

/// What the screen reader has been told of the screens, to tell what
/// changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heard {
    /// Which screen shows: a page, a box over it, or the rdisk screen's
    /// disk. A new one is told whole.
    screen: String,
    /// What is said as the screen comes: its name and its text.
    intro: Vec<String>,
    /// What has the focus, and what is said of it.
    focus: String,
    focus_words: String,
    /// The focused field's value.
    value: Value,
    /// The problem shown, if one is.
    problem: Option<String>,
    /// What the screens say above the buttons, if anything.
    said: Option<String>,
    /// The install's steps started, as they are called.
    steps: Vec<String>,
}

/// A focused field's value, to tell what a key changed in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Value {
    /// None: a button, or something to press.
    #[default]
    None,
    /// Text typed in; never said if `masked`, as a password.
    Typed { text: String, masked: bool },
    /// A choice's, a box's or a list's state, as it is said.
    State(String),
}

impl Heard {
    /// What `app`'s screens show now.
    #[must_use]
    pub fn of(app: &App) -> Heard {
        let mut heard =
            Heard { problem: app.problem.clone(), said: app.said.clone(), ..Heard::default() };
        match &app.modal {
            Some(modal) => heard.modal(app, modal),
            None if app.page == Page::Rdisk => heard.rdisk(app),
            None => heard.page(app),
        }
        heard
    }

    /// A page: its name, its text, and the field or button with the
    /// focus; on the progress pages, the steps started and the end.
    fn page(&mut self, app: &App) {
        self.screen = format!("{:?}", app.page);
        if app.page == Page::Welcome {
            self.intro.push(messages::with("title", &[("version", Arg::Text(&app.version))]));
        }
        self.intro.push(page_name(app));
        let rows = app.rows();
        self.intro.extend(rows.iter().filter_map(|row| match row {
            Row::Text(text, _) => Some(text.clone()),
            _ => None,
        }));
        match app.page {
            Page::Installing | Page::Done | Page::Failed => self.progress(app),
            // The keys, once: every page but these takes the same.
            Page::Welcome => self.intro.push(draw::hints_text(app)),
            _ => {}
        }
        let Some(id) = app.focused() else { return };
        self.focus = format!("{id:?}");
        let field = rows.iter().find_map(|row| match row {
            Row::Field(field) if field.id == id => Some(field),
            _ => None,
        });
        match field {
            Some(field) => {
                self.focus_words = field_words(field, place_among_choices(&rows, id));
                self.value = value_of(&field.kind);
            }
            None => {
                let label = app.buttons().into_iter().find(|(button, _)| *button == id);
                let label = label.map_or_default(|(_, label)| label);
                self.focus_words = format!("{label}, {}", messages::text("speak-button"));
            }
        }
    }

    /// The install's progress: each step started, then its end, which the
    /// Done and Failed pages say as they come.
    fn progress(&mut self, app: &App) {
        let disk = app.answers().map_or_default(|answers| answers.target().to_string());
        self.steps =
            app.progress.started.iter().map(|step| step.title(&disk, &app.version)).collect();
        match &app.progress.finished {
            Some(Ok(_)) => self.intro.extend([app.say("done-saved"), app.say("done-remove")]),
            Some(Err(why)) => self.intro.extend([
                why.clone(),
                messages::with("failed-log", &[("log", Arg::Text(log::LIVE_PATH))]),
            ]),
            None => {}
        }
        self.intro.push(draw::hints_text(app));
    }

    /// The rdisk screen: the disk shown, and the partition chosen.
    fn rdisk(&mut self, app: &App) {
        let Some(shown) = &app.rdisk else { return };
        let Some(disk) = app.machine.disks.get(shown.disk) else { return };
        self.screen = format!("rdisk {}", disk.name);
        let model = if disk.model.is_empty() { String::new() } else { format!(", {}", disk.model) };
        self.intro.extend([
            page_name(app),
            format!("{}, {}{model}", disk.name, super::app::size(disk.bytes)),
            draw::hints_text(app),
        ]);
        let slots = &shown.edits[shown.disk].slots;
        self.focus = format!("row {}", shown.row);
        self.focus_words = match slots.get(shown.row) {
            Some(slot) => rdisk::slot_words(slot, disk),
            None => messages::text("rdisk-empty"),
        };
        // An edit changes what the row says.
        self.value = Value::State(self.focus_words.clone());
    }

    /// A box over the page: the help, the log, or a form.
    fn modal(&mut self, app: &App, modal: &Modal) {
        let close = messages::text("hint-close");
        match modal {
            Modal::Help(page) => {
                self.screen = "help".into();
                let id = format!("help-{}", page.name().trim_start_matches("page-"));
                self.intro.extend([messages::text("help-title"), app.say(&id), close]);
            }
            Modal::Log { .. } => {
                self.screen = "log".into();
                self.intro.extend([log::LIVE_PATH.to_string(), messages::text("hint-log")]);
            }
            Modal::Person(form) => {
                self.screen = "person".into();
                self.intro.extend([messages::text("person-title"), messages::text("hint-person")]);
                let fields = [
                    ("you-name", &form.name, false),
                    ("you-user", &form.user, false),
                    ("you-password", &form.password, true),
                ];
                if let Some((label, value, masked)) = fields.get(form.focus) {
                    let kind = Kind::Input {
                        value: (*value).clone(),
                        masked: *masked,
                        hint: String::new(),
                    };
                    self.form_field(form.focus, messages::text(label), kind);
                }
                self.problem.clone_from(&form.problem);
            }
            Modal::Partition(form) => {
                self.screen = "partition".into();
                let title =
                    if form.row.is_some() { "rdisk-format-title" } else { "rdisk-new-title" };
                self.intro.extend([messages::text(title), messages::text("hint-partition")]);
                let (label, kind) = match form.field() {
                    FormField::Size => (
                        "rdisk-form-size",
                        Kind::Input {
                            value: form.size.clone(),
                            masked: false,
                            hint: messages::text("rdisk-form-size-hint"),
                        },
                    ),
                    FormField::Use => ("rdisk-form-use", Kind::Cycle(form.chosen().label())),
                    FormField::Name => (
                        "rdisk-form-name",
                        Kind::Input {
                            value: form.name.clone(),
                            masked: false,
                            hint: String::new(),
                        },
                    ),
                };
                self.form_field(form.focus, messages::text(label), kind);
                self.problem.clone_from(&form.problem);
            }
        }
    }

    /// A form's field `at`, called `label`, of `kind`, as having the focus.
    fn form_field(&mut self, at: usize, label: String, kind: Kind) {
        let field = Field { id: Id::Name, kind, label, note: String::new(), enabled: true };
        self.focus = format!("field {at}");
        self.focus_words = field_words(&field, None);
        self.value = value_of(&field.kind);
    }
}

/// The words that tell the screen reader what changed from `before`,
/// what it last heard, to `now`; none if nothing it is told of did.
#[must_use]
pub fn speech(before: Option<&Heard>, now: &Heard) -> Option<String> {
    let mut words: Vec<String> = Vec::new();
    match before.filter(|before| before.screen == now.screen) {
        // A new screen: all of it.
        None => {
            words.extend(now.intro.iter().cloned());
            words.extend(now.problem.iter().cloned());
            words.extend(now.said.iter().cloned());
            words.push(now.focus_words.clone());
        }
        Some(before) => {
            words.extend(now.steps.iter().skip(before.steps.len()).cloned());
            if now.problem != before.problem {
                words.extend(now.problem.iter().cloned());
            }
            if now.said != before.said {
                words.extend(now.said.iter().cloned());
            }
            if now.focus != before.focus {
                words.push(now.focus_words.clone());
            } else if let Some(changed) = changed(&before.value, &now.value) {
                words.push(changed);
            }
        }
    }
    let words: Vec<String> =
        words.iter().map(|text| spoken(text)).filter(|text| !text.is_empty()).collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// The sequence that has consoled's screen reader say `words` at once
/// (ADR 0325): `ESC ] 7700 ; words ST`. A control character in the words
/// would end the sequence early, and is sent as a space; a semicolon
/// would split them, and is sent as a comma.
#[must_use]
pub fn osc(words: &str) -> String {
    let words: String = words
        .chars()
        .map(|c| match c {
            ';' => ',',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    format!("\x1b]7700;{words}\x1b\\")
}

/// The page's name and step: `Network and name, step 2 of 6`.
fn page_name(app: &App) -> String {
    let name = messages::text(app.page.name());
    match app.page.step() {
        Some(i) => messages::with(
            "speak-step",
            &[
                ("name", Arg::Text(&name)),
                ("step", Arg::Number(i as u64 + 1)),
                ("steps", Arg::Number(Page::STEPS.len() as u64)),
            ],
        ),
        None => name,
    }
}

/// What is said of `field` as it takes the focus: its label, its kind,
/// its `place` among the choices beside it if it is one, its state or
/// what was typed, and its note.
fn field_words(field: &Field, place: Option<(usize, usize)>) -> String {
    let mut words = vec![field.label.trim_end_matches('…').to_string()];
    match &field.kind {
        Kind::Choice(_) => {
            words.push(messages::text("speak-choice"));
            if let Some((place, count)) = place {
                words.push(messages::with(
                    "speak-of",
                    &[("place", Arg::Number(place as u64)), ("count", Arg::Number(count as u64))],
                ));
            }
            words.push(state(&field.kind));
        }
        Kind::Toggle(_) => words.extend([messages::text("speak-check-box"), state(&field.kind)]),
        Kind::Input { value, masked, hint } => {
            words.push(messages::text(if *masked { "speak-password" } else { "speak-edit" }));
            if value.is_empty() {
                words.push(messages::text("speak-blank"));
                words.push(hint.clone());
            } else if *masked {
                let count = Arg::Number(value.chars().count() as u64);
                words.push(messages::with("speak-hidden", &[("count", count)]));
            } else {
                words.push(value.clone());
            }
        }
        Kind::Cycle(value) => words.extend([messages::text("speak-list"), value.clone()]),
        Kind::Action => words.push(messages::text("speak-button")),
    }
    words.retain(|word| !word.is_empty());
    let mut said = sentence(&words.join(", "));
    if !field.note.is_empty() {
        said.push(' ');
        said.push_str(&sentence(&field.note));
    }
    said
}

/// The value of a field of `kind`, to tell what a key changed in it.
fn value_of(kind: &Kind) -> Value {
    match kind {
        Kind::Input { value, masked, .. } => Value::Typed { text: value.clone(), masked: *masked },
        Kind::Choice(_) | Kind::Toggle(_) => Value::State(state(kind)),
        Kind::Cycle(value) => Value::State(value.clone()),
        Kind::Action => Value::None,
    }
}

/// A choice's or a box's state, as it is said.
fn state(kind: &Kind) -> String {
    messages::text(match kind {
        Kind::Choice(true) => "speak-chosen",
        Kind::Choice(false) => "speak-not-chosen",
        Kind::Toggle(true) => "speak-checked",
        _ => "speak-not-checked",
    })
}

/// What a key changed in the focused field, from `before` to `now`: the
/// characters typed, or taken away, the field then blank if it is; or
/// the new state.
fn changed(before: &Value, now: &Value) -> Option<String> {
    if before == now {
        return None;
    }
    match (before, now) {
        (Value::Typed { text: was, .. }, Value::Typed { text, masked }) => {
            let blank = messages::text("speak-blank");
            let said = if let Some(typed) = text.strip_prefix(was.as_str()) {
                if *masked { messages::text("speak-star") } else { character(typed) }
            } else if let Some(gone) = was.strip_prefix(text.as_str()) {
                let gone = if *masked { messages::text("speak-deleted") } else { character(gone) };
                if text.is_empty() { format!("{gone}, {blank}") } else { gone }
            } else if text.is_empty() {
                blank
            } else if *masked {
                messages::with(
                    "speak-hidden",
                    &[("count", Arg::Number(text.chars().count() as u64))],
                )
            } else {
                text.clone()
            };
            Some(said)
        }
        (_, Value::State(state)) => Some(state.clone()),
        _ => None,
    }
}

/// Characters typed or taken away, as they are said: a space by its name.
fn character(text: &str) -> String {
    if text == " " { messages::text("speak-space") } else { text.to_string() }
}

/// The place of the choice `id` among the choices beside it on the page,
/// and how many there are, if there are two or more.
fn place_among_choices(rows: &[Row], id: Id) -> Option<(usize, usize)> {
    let mut group = Vec::new();
    for row in rows.iter().map(Some).chain([None]) {
        if let Some(Row::Field(Field { id: choice, kind: Kind::Choice(_), .. })) = row {
            group.push(*choice);
            continue;
        }
        if let Some(at) = group.iter().position(|choice| *choice == id) {
            return (group.len() > 1).then_some((at + 1, group.len()));
        }
        group.clear();
    }
    None
}

/// `text` as a sentence: ending in a stop, unless it ends in one already.
fn sentence(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() || text.ends_with(['.', '!', '?', ':']) {
        text.to_string()
    } else {
        format!("{text}.")
    }
}

/// `text` as it is spoken: the screens' dots between keys as pauses, the
/// ellipsis left out, Esc by its name, and a stop at its end.
fn spoken(text: &str) -> String {
    let text = text.replace(" · ", ", ").replace('…', "");
    let words: Vec<String> = text
        .split(' ')
        .map(|word| match word.trim_end_matches([',', '.', ';', ':']) {
            "Esc" => word.replacen("Esc", "Escape", 1),
            _ => word.to_string(),
        })
        .collect();
    sentence(&words.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sequence_cannot_end_or_split_early() {
        assert_eq!(osc("Disk; vda\x1b"), "\x1b]7700;Disk, vda \x1b\\");
    }

    #[test]
    fn a_new_screen_is_told_whole_and_then_only_what_changes() {
        let mut now = Heard {
            screen: "Welcome".into(),
            intro: vec!["Welcome, step 1 of 6".into()],
            focus: "Language".into(),
            focus_words: "Language, list, English.".into(),
            value: Value::State("English".into()),
            ..Heard::default()
        };
        assert_eq!(
            speech(None, &now).as_deref(),
            Some("Welcome, step 1 of 6. Language, list, English.")
        );
        assert_eq!(speech(Some(&now), &now), None);
        let before = now.clone();
        now.focus = "Name".into();
        now.focus_words = "Your name, edit, blank.".into();
        now.value = Value::Typed { text: String::new(), masked: false };
        assert_eq!(speech(Some(&before), &now).as_deref(), Some("Your name, edit, blank."));
        let before = now.clone();
        now.value = Value::Typed { text: "A".into(), masked: false };
        now.problem = Some("The name is too long".into());
        assert_eq!(speech(Some(&before), &now).as_deref(), Some("The name is too long. A."));
    }

    #[test]
    fn a_password_is_never_said() {
        let typed = |text: &str| Value::Typed { text: text.into(), masked: true };
        assert_eq!(changed(&typed("ab"), &typed("abc")).as_deref(), Some("star"));
        assert_eq!(changed(&typed("ab"), &typed("a")).as_deref(), Some("deleted"));
        assert_eq!(changed(&typed("a"), &typed("")).as_deref(), Some("deleted, blank"));
        let field = Field {
            id: Id::Password,
            kind: Kind::Input { value: "secret".into(), masked: true, hint: String::new() },
            label: "Password".into(),
            note: String::new(),
            enabled: true,
        };
        let words = field_words(&field, None);
        assert!(!words.contains("secret"), "{words}");
        assert_eq!(words, "Password, password, 6 characters.");
    }

    #[test]
    fn what_was_typed_or_taken_away_is_said() {
        let typed = |text: &str| Value::Typed { text: text.into(), masked: false };
        assert_eq!(changed(&typed("ab"), &typed("ab ")).as_deref(), Some("space"));
        assert_eq!(changed(&typed("ab"), &typed("a")).as_deref(), Some("b"));
        assert_eq!(changed(&typed("b"), &typed("")).as_deref(), Some("b, blank"));
        let state = |text: &str| Value::State(text.into());
        assert_eq!(changed(&state("Normal"), &state("Larger")).as_deref(), Some("Larger"));
    }

    #[test]
    fn a_choice_is_said_with_its_place_among_those_beside_it() {
        let choice = |id: Id, on: bool| {
            Row::Field(Field {
                id,
                kind: Kind::Choice(on),
                label: format!("{id:?}"),
                note: String::new(),
                enabled: true,
            })
        };
        let rows = vec![
            Row::Text("How?".into(), super::super::app::Look::Bold),
            choice(Id::StepByStep, true),
            choice(Id::FromFile, false),
            Row::Blank,
            choice(Id::Dhcp, true),
        ];
        assert_eq!(place_among_choices(&rows, Id::FromFile), Some((2, 2)));
        assert_eq!(place_among_choices(&rows, Id::Dhcp), None);
        let Row::Field(field) = &rows[2] else { unreachable!() };
        assert_eq!(field_words(field, Some((2, 2))), "FromFile, choice, 2 of 2, not selected.");
    }

    #[test]
    fn the_screens_marks_are_said_as_words() {
        assert_eq!(spoken("Tab move · Esc back · F1 help"), "Tab move, Escape back, F1 help.");
        assert_eq!(spoken("Add a person…"), "Add a person.");
    }
}
