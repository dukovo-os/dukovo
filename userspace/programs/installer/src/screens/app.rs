// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The screens' state and what each key does. [`App`] holds every answer
//! as it is typed; each page lists its [`Row`]s, some of them fields that
//! take the focus, and [`App::key`] acts on the focused one. Nothing is
//! written to a disk until the review's Install, which hands the answers
//! to the install engine in another thread ([`super::run`]).
//!
//! Each page's answers are checked as the answers file's are
//! ([`Answers::check`]) when Next leaves it, so the screens and an
//! answers file can never take different things.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::answers::{self, Account, Answers, Disk, Fixed, Layout, Network};
use crate::log::{Event, Step};
use crate::machine::{Holds, Machine};
use crate::messages::{self, Arg};

/// A page of the installer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// Language, keyboard, and step by step or from a setup file.
    Welcome,
    /// The setup files found, to install from one.
    SetupFile,
    /// Network cards, the computer's name and its time zone.
    Network,
    /// The disk and its layout.
    Disk,
    /// The administrator and the other accounts.
    You,
    /// Everything the install will do, and the disk's name typed to
    /// confirm it.
    Review,
    /// The install running.
    Installing,
    /// The install finished.
    Done,
    /// The install failed.
    Failed,
}

impl Page {
    /// The pages the bar at the top names, in order.
    pub const STEPS: [Page; 5] =
        [Page::Welcome, Page::Network, Page::Disk, Page::You, Page::Review];

    /// The id of its name among the messages.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Page::Welcome => "page-welcome",
            Page::SetupFile => "page-setup-file",
            Page::Network => "page-network",
            Page::Disk => "page-disk",
            Page::You => "page-you",
            Page::Review => "page-review",
            Page::Installing => "page-installing",
            Page::Done => "page-done",
            Page::Failed => "page-failed",
        }
    }

    /// Its place among [`Page::STEPS`], if it is one.
    #[must_use]
    pub fn step(self) -> Option<usize> {
        Page::STEPS.iter().position(|page| *page == self)
    }
}

/// What can take the focus: each page's fields, then its buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id {
    /// Welcome: the language.
    Language,
    /// Welcome: the keyboard layout.
    Keyboard,
    /// Welcome: a box to try the layout in.
    TryKeys,
    /// Welcome: install step by step.
    StepByStep,
    /// Welcome: install from a setup file.
    FromFile,
    /// Setup file: the setup file found, by its place in the list.
    SetupFile(usize),
    /// Network: a card, by its place in the machine's list.
    Card(usize),
    /// Network: the card asks DHCP.
    Dhcp,
    /// Network: the card has a fixed address.
    Fixed,
    /// Network: the fixed address with its prefix.
    Address,
    /// Network: the fixed address's router.
    Gateway,
    /// Network: the fixed address's DNS servers.
    Dns,
    /// Network: the computer's name.
    Hostname,
    /// Network: the time zone.
    Zone,
    /// Disk: a disk, by its place in the machine's list.
    Disk(usize),
    /// Disk: the guided layout.
    Guided,
    /// Disk: the swap's size in GiB.
    Swap,
    /// You: the administrator's full name.
    Name,
    /// You: the administrator's user name.
    User,
    /// You: the administrator's password.
    Password,
    /// You: the password again.
    Repeat,
    /// You: keep the passwords' hashes in the saved answers.
    KeepHashes,
    /// You: another person, by their place in the list.
    Person(usize),
    /// You: add another person.
    AddPerson,
    /// Review: the target disk's name, typed to confirm.
    Confirm,
    /// Leave the installer for a shell.
    Live,
    /// The page before.
    Back,
    /// The page after.
    Next,
    /// Install from the setup file chosen.
    Load,
    /// Start the install.
    Install,
    /// The page's help.
    Help,
    /// Restart the computer.
    Restart,
    /// The install log.
    ShowLog,
    /// Save the answers as a setup file on the stick.
    SaveSetup,
}

/// How a row of text looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// As text is.
    Plain,
    /// Bold, as a question.
    Bold,
    /// Grey, as a note.
    Dim,
    /// Green, as what is left alone.
    Good,
    /// Yellow, as a caution.
    Warning,
    /// Red, as what is erased.
    Danger,
}

/// A field: something on a page that takes the focus.
#[derive(Clone, Debug)]
pub struct Field {
    /// Which field.
    pub id: Id,
    /// What kind, with its value.
    pub kind: Kind,
    /// What it is called.
    pub label: String,
    /// What is said beside it, dimmed.
    pub note: String,
    /// A field that is not enabled is shown dimmed and skipped.
    pub enabled: bool,
}

/// What kind of field.
#[derive(Clone, Debug)]
pub enum Kind {
    /// One of a group: `(•)` when chosen.
    Choice(bool),
    /// On or off: `[x]`.
    Toggle(bool),
    /// Text typed in.
    Input {
        /// What was typed.
        value: String,
        /// Shown as dots, as a password.
        masked: bool,
        /// Shown, dimmed, while nothing is typed.
        hint: String,
    },
    /// One of a list, changed with Left and Right: `‹ value ›`.
    Cycle(String),
    /// Something to press, among the rows: `[ Add a person… ]`.
    Action,
}

/// A row of a page.
#[derive(Clone, Debug)]
pub enum Row {
    /// Nothing.
    Blank,
    /// Text, wrapped to the page's width.
    Text(String, Look),
    /// A field.
    Field(Field),
}

/// A box shown over the page.
#[derive(Clone, Debug)]
pub enum Modal {
    /// The page's help.
    Help(Page),
    /// The install log.
    Log {
        /// The first line shown; past the end shows the end.
        top: usize,
    },
    /// Another person's account being added.
    Person(PersonForm),
}

/// The form of another person's account.
#[derive(Clone, Debug, Default)]
pub struct PersonForm {
    /// Their full name.
    pub name: String,
    /// Their user name.
    pub user: String,
    /// Their password.
    pub password: String,
    /// Whether the user name was typed, so that the name no longer
    /// makes it.
    pub user_typed: bool,
    /// The field with the focus: 0 to 2.
    pub focus: usize,
    /// Why the person cannot be added, if they cannot.
    pub problem: Option<String>,
}

/// Another person, whose account the install makes.
#[derive(Clone, Debug)]
pub struct Person {
    /// Their full name.
    pub name: String,
    /// Their user name.
    pub user: String,
    /// Their password, until the install hashes it.
    pub password: String,
}

/// What a key asked the screens to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing more than the screens show.
    Stay,
    /// Start the install with these answers; the passwords of the
    /// accounts that have none hashed yet go with them.
    Install,
    /// End, leaving the console to a shell.
    Quit,
    /// Restart the computer.
    Restart,
    /// Save the answers as a setup file on the stick.
    SaveSetup,
}

/// A setup file found on the stick.
#[derive(Clone, Debug)]
pub struct SetupFile {
    /// Where it was found, as shown.
    pub place: String,
    /// What it holds, or why it cannot be read.
    pub answers: Result<Answers, Vec<String>>,
}

/// How the install is going.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    /// The steps started, in order; the last runs now.
    pub started: Vec<Step>,
    /// Bytes copied, of the release's.
    pub copied: (u64, u64),
    /// The log's last line.
    pub last: String,
    /// The install's end: `Ok` with its last line, or why it failed.
    pub finished: Option<Result<String, String>>,
}

impl Progress {
    /// How far the install is, from 0 to 100.
    #[must_use]
    pub fn percent(&self) -> u16 {
        if matches!(self.finished, Some(Ok(_))) {
            return 100;
        }
        let Some((current, done)) = self.started.split_last() else { return 0 };
        let mut total: u64 = done.iter().map(|step| u64::from(step.share())).sum();
        let (copied, of) = self.copied;
        if *current == Step::Copy && of > 0 {
            total += u64::from(Step::Copy.share()) * copied.min(of) / of;
        }
        u16::try_from(total.min(99)).unwrap_or(99)
    }
}

/// The screens' state.
#[derive(Debug)]
pub struct App {
    /// What the computer has.
    pub machine: Machine,
    /// The release's version.
    pub version: String,
    /// The release's size in bytes.
    pub release_bytes: u64,
    /// The page shown.
    pub page: Page,
    /// The focused field, by its place among the page's fields and
    /// buttons.
    pub focus: usize,
    /// What is wrong with the page's answers, shown above its buttons.
    pub problem: Option<String>,
    /// What was done, shown above the buttons, as a saved setup file.
    pub said: Option<String>,
    /// The box shown over the page, if one is.
    pub modal: Option<Modal>,
    /// The keyboard layout, by its place in the machine's list.
    pub keyboard: usize,
    /// What was typed to try the layout.
    pub try_keys: String,
    /// Whether the install is from a setup file.
    pub from_file: bool,
    /// The setup files found on the stick.
    pub setup_files: Vec<SetupFile>,
    /// The setup file chosen, by its place in the list.
    pub setup_file: usize,
    /// The network card set up, by its place in the machine's list.
    pub card: usize,
    /// Whether it has a fixed address, not DHCP.
    pub fixed: bool,
    /// The fixed address with its prefix.
    pub address: String,
    /// The fixed address's router.
    pub gateway: String,
    /// The fixed address's DNS servers, separated by commas or spaces.
    pub dns: String,
    /// The computer's name.
    pub hostname: String,
    /// The time zone, by its place in the machine's list.
    pub zone: usize,
    /// The target disk, by its place in the machine's list.
    pub disk: Option<usize>,
    /// The swap's size in GiB, as typed.
    pub swap: String,
    /// The administrator's full name.
    pub name: String,
    /// The administrator's user name.
    pub user: String,
    /// Whether the user name was typed, so that the name no longer
    /// makes it.
    pub user_typed: bool,
    /// The administrator's password, until the install hashes it.
    pub password: String,
    /// The password again.
    pub repeat: String,
    /// Whether the saved answers keep the passwords' hashes.
    pub keep_hashes: bool,
    /// The other people.
    pub people: Vec<Person>,
    /// The target disk's name, typed to confirm the install.
    pub confirm: String,
    /// The answers of the setup file chosen, if the install is from one.
    pub loaded: Option<Answers>,
    /// How the install is going.
    pub progress: Progress,
    /// The letters typed in a row on a list field, to find a name by its
    /// start ([`Seek`]).
    seeking: Option<Seek>,
}

/// Letters typed in a row on a list field, as the time zone's: the field
/// shows the first name that starts with them all, as a list box does, so
/// that `america/den` finds America/Denver among some 300 zones.
#[derive(Debug)]
struct Seek {
    /// The field typed on.
    id: Id,
    /// What was typed, in lower case.
    typed: String,
    /// When the last letter came.
    at: Instant,
}

/// How long a pause starts the letters typed on a list field again.
const SEEK_PAUSE: Duration = Duration::from_secs(1);

/// The guided layout's partitions before the root, in bytes: the BIOS
/// boot partition and the EFI system partition.
const BEFORE_ROOT: u64 = (1 + 512) << 20;

/// What the root needs beyond the release, as the engine counts it.
const ROOM: u64 = 1 << 30;

/// The swap the installer suggests at most, in GiB.
const SWAP_SUGGESTED_MAX: u64 = 8;

impl App {
    /// The screens' state for `machine`, to install the release
    /// `version` of `release_bytes`.
    #[must_use]
    pub fn new(machine: Machine, version: &str, release_bytes: u64) -> App {
        let keyboard =
            machine.layouts.iter().position(|(name, _)| name == keymap::DEFAULT).unwrap_or(0);
        let zone = machine.zones.iter().position(|zone| zone == "UTC").unwrap_or(0);
        let disk = machine.disks.iter().position(|disk| !disk.stick);
        // As much swap as memory, rounded up, but no more than 8 GiB.
        let swap = machine.memory.div_ceil(1 << 30).clamp(1, SWAP_SUGGESTED_MAX);
        App {
            machine,
            version: version.to_string(),
            release_bytes,
            page: Page::Welcome,
            focus: 0,
            problem: None,
            said: None,
            modal: None,
            keyboard,
            try_keys: String::new(),
            from_file: false,
            setup_files: Vec::new(),
            setup_file: 0,
            card: 0,
            fixed: false,
            address: String::new(),
            gateway: String::new(),
            dns: String::new(),
            hostname: answers::DEFAULT_HOSTNAME.to_string(),
            zone,
            disk,
            swap: swap.to_string(),
            name: String::new(),
            user: String::new(),
            user_typed: false,
            password: String::new(),
            repeat: String::new(),
            keep_hashes: false,
            people: Vec::new(),
            confirm: String::new(),
            loaded: None,
            progress: Progress::default(),
            seeking: None,
        }
    }

    /// The page's rows.
    #[must_use]
    pub fn rows(&self) -> Vec<Row> {
        match self.page {
            Page::Welcome => self.welcome_rows(),
            Page::SetupFile => self.setup_file_rows(),
            Page::Network => self.network_rows(),
            Page::Disk => self.disk_rows(),
            Page::You => self.you_rows(),
            Page::Review => self.review_rows(),
            Page::Installing | Page::Done | Page::Failed => Vec::new(),
        }
    }

    /// The page's buttons, by id and label.
    #[must_use]
    pub fn buttons(&self) -> Vec<(Id, String)> {
        let ids: &[Id] = match self.page {
            Page::Welcome => &[Id::Live, Id::Next, Id::Help],
            Page::SetupFile => &[Id::Back, Id::Load, Id::Help],
            Page::Review => &[Id::Back, Id::Install, Id::Help],
            Page::Installing => &[],
            Page::Done => &[Id::Restart, Id::ShowLog, Id::SaveSetup],
            Page::Failed => &[Id::ShowLog, Id::Live],
            Page::Network | Page::Disk | Page::You => &[Id::Back, Id::Next, Id::Help],
        };
        ids.iter().map(|id| (*id, messages::text(button_name(*id)))).collect()
    }

    /// The ids of what takes the focus, in order: the enabled fields,
    /// then the buttons.
    #[must_use]
    pub fn focusable(&self) -> Vec<Id> {
        let fields = self.rows().into_iter().filter_map(|row| match row {
            Row::Field(field) if field.enabled => Some(field.id),
            _ => None,
        });
        fields.chain(self.buttons().into_iter().map(|(id, _)| id)).collect()
    }

    /// The id of the focused field or button.
    #[must_use]
    pub fn focused(&self) -> Option<Id> {
        self.focusable().get(self.focus).copied()
    }

    /// Moves the focus to `id`.
    fn focus_on(&mut self, id: Id) {
        if let Some(at) = self.focusable().iter().position(|f| *f == id) {
            self.focus = at;
        }
    }

    /// Shows `page`, its first field focused, or the default button.
    fn show(&mut self, page: Page) {
        self.page = page;
        self.focus = 0;
        self.problem = None;
        self.said = None;
    }

    fn welcome_rows(&self) -> Vec<Row> {
        let (_, layout) = &self.machine.layouts[self.keyboard];
        vec![
            text(messages::with("welcome-intro", &[("version", Arg::Text(&self.version))])),
            Row::Blank,
            field(
                Id::Language,
                Kind::Cycle(messages::text("language-english")),
                "welcome-language",
                &messages::text("welcome-language-note"),
            ),
            field(Id::Keyboard, Kind::Cycle(layout.clone()), "welcome-keyboard", ""),
            input(
                Id::TryKeys,
                "welcome-try",
                &self.try_keys,
                false,
                &messages::text("welcome-try-hint"),
            ),
            Row::Blank,
            Row::Text(messages::text("welcome-how"), Look::Bold),
            field(
                Id::StepByStep,
                Kind::Choice(!self.from_file),
                "welcome-steps",
                &messages::text("welcome-steps-note"),
            ),
            field(
                Id::FromFile,
                Kind::Choice(self.from_file),
                "welcome-file",
                &messages::text("welcome-file-note"),
            ),
        ]
    }

    fn setup_file_rows(&self) -> Vec<Row> {
        let mut rows = vec![text(messages::text("setup-intro")), Row::Blank];
        if self.setup_files.is_empty() {
            rows.push(Row::Text(messages::text("setup-none"), Look::Dim));
        }
        for (i, found) in self.setup_files.iter().enumerate() {
            let note = match &found.answers {
                Ok(_) => messages::text("setup-ready"),
                Err(problems) => problems.first().cloned().unwrap_or_default(),
            };
            rows.push(Row::Field(Field {
                id: Id::SetupFile(i),
                kind: Kind::Choice(self.setup_file == i),
                label: found.place.clone(),
                note,
                enabled: true,
            }));
        }
        rows
    }

    fn network_rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Text(messages::text("network-cards"), Look::Plain)];
        if self.machine.cards.is_empty() {
            rows.push(Row::Text(messages::text("network-no-cards"), Look::Dim));
        }
        for (i, card) in self.machine.cards.iter().enumerate() {
            let note = if card.carrier {
                match card.speed {
                    Some(speed) => messages::with(
                        "network-cable-speed",
                        &[("speed", Arg::Number(u64::from(speed)))],
                    ),
                    None => messages::text("network-cable"),
                }
            } else {
                messages::text("network-no-cable")
            };
            rows.push(Row::Field(Field {
                id: Id::Card(i),
                kind: Kind::Choice(self.card == i),
                label: format!("{}  {}", card.name, card.model),
                note,
                enabled: true,
            }));
        }
        if !self.machine.cards.is_empty() {
            let got = self.machine.cards.get(self.card).and_then(|card| card.addresses.first());
            let dhcp_note = got.map_or_else(
                || messages::text("network-dhcp-note"),
                |address| messages::with("network-dhcp-got", &[("address", Arg::Text(address))]),
            );
            rows.extend([
                Row::Blank,
                field(Id::Dhcp, Kind::Choice(!self.fixed), "network-dhcp", &dhcp_note),
                field(
                    Id::Fixed,
                    Kind::Choice(self.fixed),
                    "network-fixed",
                    &messages::text("network-fixed-note"),
                ),
            ]);
            if self.fixed {
                rows.extend([
                    input(Id::Address, "network-address", &self.address, false, "192.168.1.10/24"),
                    input(Id::Gateway, "network-gateway", &self.gateway, false, "192.168.1.1"),
                    input(Id::Dns, "network-dns", &self.dns, false, "192.168.1.1"),
                ]);
            }
        }
        rows.extend([
            Row::Blank,
            input(Id::Hostname, "network-hostname", &self.hostname, false, ""),
            field(
                Id::Zone,
                Kind::Cycle(self.machine.zones[self.zone].clone()),
                "network-zone",
                &messages::text("network-zone-note"),
            ),
            Row::Text(messages::text("network-offline"), Look::Dim),
        ]);
        rows
    }

    fn disk_rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Text(messages::text("disk-which"), Look::Plain)];
        for (i, disk) in self.machine.disks.iter().enumerate() {
            let note = if disk.stick {
                messages::text("disk-stick")
            } else if disk.bytes < self.needed() {
                messages::with("disk-too-small", &[("size", Arg::Text(&size(self.needed())))])
            } else {
                holds(disk.holds(), disk.partitions.len())
            };
            rows.push(Row::Field(Field {
                id: Id::Disk(i),
                kind: Kind::Choice(self.disk == Some(i)),
                label: disk_label(disk),
                note,
                enabled: !disk.stick && disk.bytes >= self.needed(),
            }));
        }
        rows.extend([
            Row::Blank,
            Row::Text(messages::text("disk-how"), Look::Bold),
            field(
                Id::Guided,
                Kind::Choice(true),
                "disk-guided",
                &messages::text("disk-guided-note"),
            ),
            input(Id::Swap, "disk-swap", &self.swap, false, "0"),
            Row::Text(messages::text("disk-swap-note"), Look::Dim),
        ]);
        if let Some(disk) = self.disk.and_then(|i| self.machine.disks.get(i)) {
            let name = Arg::Text(&disk.name);
            let warning = match disk.holds() {
                Holds::Nothing => None,
                Holds::Windows => Some(messages::with("disk-erase-windows", &[("disk", name)])),
                Holds::Dukovo | Holds::Linux | Holds::Other => {
                    Some(messages::with("disk-erase", &[("disk", name)]))
                }
            };
            if let Some(warning) = warning {
                rows.extend([Row::Blank, Row::Text(warning, Look::Danger)]);
            }
            let others: Vec<String> = self
                .machine
                .disks
                .iter()
                .filter(|other| other.name != disk.name && !other.stick)
                .map(|other| other.name.clone())
                .collect();
            if !others.is_empty() {
                let others = others.join(", ");
                rows.push(Row::Text(
                    messages::with("disk-untouched", &[("disks", Arg::Text(&others))]),
                    Look::Dim,
                ));
            }
        }
        rows
    }

    fn you_rows(&self) -> Vec<Row> {
        let mut rows = vec![
            text(messages::text("you-intro")),
            Row::Blank,
            input(Id::Name, "you-name", &self.name, false, ""),
            input(Id::User, "you-user", &self.user, false, ""),
            input(Id::Password, "you-password", &self.password, true, ""),
            input(Id::Repeat, "you-repeat", &self.repeat, true, ""),
        ];
        if !self.password.is_empty() && self.password.chars().count() < 8 {
            rows.push(Row::Text(messages::text("you-short"), Look::Warning));
        }
        rows.push(field(
            Id::KeepHashes,
            Kind::Toggle(self.keep_hashes),
            "you-keep",
            &messages::text("you-keep-note"),
        ));
        rows.extend([Row::Blank, Row::Text(messages::text("you-others"), Look::Bold)]);
        if self.people.is_empty() {
            rows.push(Row::Text(messages::text("you-none-yet"), Look::Dim));
        }
        for (i, person) in self.people.iter().enumerate() {
            rows.push(Row::Field(Field {
                id: Id::Person(i),
                kind: Kind::Action,
                label: format!("{} ({})", person.user, person.name),
                note: messages::text("you-person-note"),
                enabled: true,
            }));
        }
        rows.push(field(Id::AddPerson, Kind::Action, "you-add", ""));
        rows
    }

    fn review_rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Text(messages::text("review-intro"), Look::Warning), Row::Blank];
        if let Ok(answers) = self.answers() {
            rows.extend(self.review_lines(&answers));
            rows.extend([
                Row::Blank,
                input(
                    Id::Confirm,
                    &messages::with("review-confirm", &[("disk", Arg::Text(answers.target()))]),
                    &self.confirm,
                    false,
                    "",
                ),
            ]);
        }
        rows
    }

    /// What the install of `answers` will do, a row a line.
    fn review_lines(&self, answers: &Answers) -> Vec<Row> {
        let target = answers.target();
        let disk = self.machine.disks.iter().find(|disk| disk.name == target);
        let mut rows = Vec::new();
        let label = |id: &str, rest: String| format!("{:<10}{rest}", messages::text(id));
        let erase = match disk.map(crate::machine::DiskInfo::holds) {
            Some(Holds::Windows) => messages::text("review-erase-windows"),
            Some(Holds::Nothing) | None => String::new(),
            Some(_) => messages::text("review-erase-all"),
        };
        let model = disk.map_or("", |disk| disk.model.as_str());
        rows.push(Row::Text(
            label("review-erase", format!("{target}  {model}  {erase}")),
            Look::Danger,
        ));
        rows.push(Row::Text(
            label("review-create", messages::with("review-on", &[("disk", Arg::Text(target))])),
            Look::Plain,
        ));
        let swap = u64::from(answers.disk.swap_gib) << 30;
        let root = disk.map_or(0, |disk| disk.bytes.saturating_sub(BEFORE_ROOT + swap));
        let mut parts = vec![
            ("review-bios-boot", 1 << 20, "-".to_string()),
            ("review-efi", 512 << 20, messages::text("review-efi-note")),
        ];
        if swap > 0 {
            parts.push(("review-swap", swap, messages::text("review-swap-note")));
        }
        parts.push(("review-root", root, messages::text("review-root-note")));
        for (i, (name, bytes, what)) in parts.into_iter().enumerate() {
            rows.push(Row::Text(
                format!(
                    "{:12}{}  {:<16}{:>9}   {what}",
                    "",
                    i + 1,
                    messages::text(name),
                    size(bytes)
                ),
                Look::Plain,
            ));
        }
        rows.push(Row::Text(
            format!("{:12}{}", "", messages::text("review-subvolumes")),
            Look::Dim,
        ));
        rows.push(Row::Text(
            label(
                "review-install",
                messages::with("review-release", &[("version", Arg::Text(&self.version))]),
            ),
            Look::Plain,
        ));
        rows.push(Row::Text(format!("{:10}{}", "", messages::text("review-grub")), Look::Plain));
        let mut settings = vec![answers.hostname().to_string()];
        for (card, network) in &answers.network {
            settings.push(match network {
                Network::Dhcp(_) => messages::with("review-dhcp", &[("card", Arg::Text(card))]),
                Network::Fixed(fixed) => format!("{card} {}", fixed.address),
            });
        }
        settings.push(answers.time_zone().to_string());
        settings
            .push(messages::with("review-keyboard", &[("layout", Arg::Text(&answers.keyboard))]));
        rows.push(Row::Text(label("review-settings", settings.join(" · ")), Look::Plain));
        let mut people = vec![messages::with(
            "review-admin",
            &[("user", Arg::Text(&answers.admin.user)), ("name", Arg::Text(&answers.admin.name))],
        )];
        people.extend(answers.users.iter().map(|user| format!("{} ({})", user.user, user.name)));
        rows.push(Row::Text(label("review-accounts", people.join(", ")), Look::Plain));
        for other in self.machine.disks.iter().filter(|other| other.name != target && !other.stick)
        {
            rows.push(Row::Text(
                label("review-untouched", format!("{}  {}", other.name, other.model)),
                Look::Good,
            ));
        }
        rows
    }

    /// The size the guided layout needs: its first partitions, the swap
    /// asked for, the release and room to spare.
    fn needed(&self) -> u64 {
        let swap =
            self.swap.trim().parse::<u64>().unwrap_or(0).min(u64::from(answers::SWAP_MAX_GIB));
        BEFORE_ROOT + (swap << 30) + self.release_bytes + ROOM
    }

    /// The answers: a setup file's, or those typed, with no passwords'
    /// hashes yet; the install hashes the passwords typed.
    ///
    /// # Errors
    ///
    /// If no disk can be chosen.
    pub fn answers(&self) -> Result<Answers, String> {
        if let Some(loaded) = &self.loaded {
            return Ok(loaded.clone());
        }
        let disk = self
            .disk
            .and_then(|i| self.machine.disks.get(i))
            .ok_or_else(|| messages::text("disk-none"))?;
        let mut network = std::collections::BTreeMap::new();
        if let Some(card) = self.machine.cards.get(self.card) {
            let setup = if self.fixed {
                Network::Fixed(Fixed {
                    address: self.address.trim().to_string(),
                    gateway: Some(self.gateway.trim().to_string()).filter(|g| !g.is_empty()),
                    dns: self
                        .dns
                        .split([',', ' '])
                        .filter(|server| !server.is_empty())
                        .map(str::to_string)
                        .collect(),
                })
            } else {
                Network::Dhcp("dhcp".into())
            };
            network.insert(card.name.clone(), setup);
        }
        let account = |name: &str, user: &str| Account {
            name: name.trim().to_string(),
            user: user.trim().to_string(),
            // The hash is made as the install starts; "" stands in until
            // then, so that the answers check.
            password_hash: Some(String::new()),
        };
        Ok(Answers {
            version: answers::VERSION,
            keyboard: self.machine.layouts[self.keyboard].0.clone(),
            language: messages::LANGUAGE.to_string(),
            disk: Disk {
                layout: Layout::Guided,
                target: disk.name.clone(),
                mirror: None,
                swap_gib: self.swap.trim().parse().unwrap_or(u32::MAX),
            },
            hostname: Some(self.hostname.trim().to_string()),
            time_zone: Some(self.machine.zones[self.zone].clone()),
            network,
            admin: account(&self.name, &self.user),
            users: self.people.iter().map(|person| account(&person.name, &person.user)).collect(),
            keep_password_hashes: self.keep_hashes,
            later: std::collections::BTreeMap::new(),
        })
    }

    /// The passwords typed, the administrator's first and then each other
    /// person's, for the install to hash.
    #[must_use]
    pub fn passwords(&self) -> Vec<String> {
        std::iter::once(self.password.clone())
            .chain(self.people.iter().map(|person| person.password.clone()))
            .collect()
    }

    /// Forgets the passwords typed, once the install has them.
    pub fn forget_passwords(&mut self) {
        for password in std::iter::once(&mut self.password)
            .chain(std::iter::once(&mut self.repeat))
            .chain(self.people.iter_mut().map(|person| &mut person.password))
        {
            let mut bytes = std::mem::take(password).into_bytes();
            accounts::wipe(&mut bytes);
        }
    }

    /// Checks the page's answers before Next leaves it: the problem shown
    /// if there is one.
    fn check_page(&self) -> Result<(), String> {
        let keys: &[&str] = match self.page {
            Page::Network => &["hostname", "time_zone", "network"],
            Page::Disk => &["disk"],
            Page::You => {
                if self.password != self.repeat {
                    return Err(messages::text("you-mismatch"));
                }
                &["admin", "users"]
            }
            Page::Review => {
                let answers = self.answers()?;
                if self.confirm.trim() != answers.target() {
                    return Err(messages::with(
                        "review-type",
                        &[("disk", Arg::Text(answers.target()))],
                    ));
                }
                return Ok(());
            }
            _ => return Ok(()),
        };
        let answers = self.answers()?;
        if self.page == Page::Disk
            && self
                .machine
                .disks
                .iter()
                .any(|d| d.name == answers.target() && d.bytes < self.needed())
        {
            return Err(messages::with(
                "disk-too-small",
                &[("size", Arg::Text(&size(self.needed())))],
            ));
        }
        match answers.check() {
            Ok(()) => Ok(()),
            Err(problems) => {
                match problems.into_iter().find(|p| keys.iter().any(|key| p.starts_with(key))) {
                    Some(problem) => Err(problem),
                    None => Ok(()),
                }
            }
        }
    }

    /// Acts on `key`.
    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        if self.modal.is_some() {
            return self.modal_key(key);
        }
        // Any key but a letter ends the letters typed on a list field.
        if !matches!(key.code, KeyCode::Char(_)) {
            self.seeking = None;
        }
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let count = self.focusable().len();
        match key.code {
            KeyCode::F(1) => {
                self.modal = Some(Modal::Help(self.page));
                Outcome::Stay
            }
            KeyCode::Tab if shift => self.move_focus(count, -1),
            KeyCode::BackTab | KeyCode::Up => self.move_focus(count, -1),
            KeyCode::Tab | KeyCode::Down => self.move_focus(count, 1),
            KeyCode::Esc => {
                self.back();
                Outcome::Stay
            }
            KeyCode::Char('l' | 'L')
                if matches!(self.page, Page::Installing | Page::Done | Page::Failed) =>
            {
                self.modal = Some(Modal::Log { top: usize::MAX });
                Outcome::Stay
            }
            _ => match self.focused() {
                Some(id) => {
                    let (page, at) = (self.page, self.focus);
                    let outcome = self.field_key(id, key);
                    // What was typed can change which fields take the
                    // focus, as a larger swap rules out a small disk:
                    // the focus stays on the field typed in, unless the
                    // key moved it.
                    if self.page == page && self.focus == at {
                        self.focus_on(id);
                    }
                    outcome
                }
                None => Outcome::Stay,
            },
        }
    }

    fn move_focus(&mut self, count: usize, by: isize) -> Outcome {
        if count > 0 {
            self.focus = (self.focus as isize + by).rem_euclid(count as isize) as usize;
        }
        Outcome::Stay
    }

    /// Acts on `key` for the focused field or button `id`.
    fn field_key(&mut self, id: Id, key: KeyEvent) -> Outcome {
        let enter = key.code == KeyCode::Enter;
        let space = key.code == KeyCode::Char(' ');
        match self.kind_of(id) {
            Some(Kind::Input { .. }) => {
                if enter {
                    return self.press(Id::Next);
                }
                self.edit(id, key);
                Outcome::Stay
            }
            Some(Kind::Cycle(_)) => {
                match key.code {
                    KeyCode::Left => self.cycle(id, -1),
                    KeyCode::Right | KeyCode::Char(' ') => self.cycle(id, 1),
                    KeyCode::Char(c) => self.seek(id, c),
                    KeyCode::Enter => return self.press(Id::Next),
                    _ => {}
                }
                Outcome::Stay
            }
            Some(Kind::Choice(_) | Kind::Toggle(_)) if space => {
                self.choose(id);
                Outcome::Stay
            }
            Some(Kind::Choice(_) | Kind::Toggle(_)) if enter => {
                self.choose(id);
                self.press(Id::Next)
            }
            Some(Kind::Action) if enter || space => self.press(id),
            Some(Kind::Action) if matches!(key.code, KeyCode::Delete | KeyCode::Backspace) => {
                if let Id::Person(i) = id {
                    self.people.remove(i);
                    self.focus = self.focus.min(self.focusable().len().saturating_sub(1));
                }
                Outcome::Stay
            }
            None if enter || space => self.press(id),
            _ => Outcome::Stay,
        }
    }

    /// The kind of the field `id` on this page; `None` for a button.
    fn kind_of(&self, id: Id) -> Option<Kind> {
        self.rows().into_iter().find_map(|row| match row {
            Row::Field(field) if field.id == id => Some(field.kind),
            _ => None,
        })
    }

    /// The text of the input field `id`.
    fn input_mut(&mut self, id: Id) -> Option<&mut String> {
        Some(match id {
            Id::TryKeys => &mut self.try_keys,
            Id::Address => &mut self.address,
            Id::Gateway => &mut self.gateway,
            Id::Dns => &mut self.dns,
            Id::Hostname => &mut self.hostname,
            Id::Swap => &mut self.swap,
            Id::Name => &mut self.name,
            Id::User => &mut self.user,
            Id::Password => &mut self.password,
            Id::Repeat => &mut self.repeat,
            Id::Confirm => &mut self.confirm,
            _ => return None,
        })
    }

    /// Types `key` into the input field `id`.
    fn edit(&mut self, id: Id, key: KeyEvent) {
        let Some(text) = self.input_mut(id) else { return };
        match key.code {
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL) && text.len() < 200 =>
            {
                text.push(c);
            }
            _ => return,
        }
        self.problem = None;
        match id {
            Id::User => self.user_typed = !self.user.is_empty(),
            Id::Name if !self.user_typed => self.user = user_name(&self.name),
            _ => {}
        }
    }

    /// Moves the cycle field `id` by `by`.
    fn cycle(&mut self, id: Id, by: isize) {
        let (index, count) = match id {
            Id::Keyboard => (&mut self.keyboard, self.machine.layouts.len()),
            Id::Zone => (&mut self.zone, self.machine.zones.len()),
            _ => return,
        };
        *index = (*index as isize + by).rem_euclid(count.max(1) as isize) as usize;
        if id == Id::Keyboard {
            self.keyboard_changed();
        }
    }

    /// Moves the cycle field `id` to the next entry after the current one
    /// that starts with `c`, as a list does when typed in.
    fn seek(&mut self, id: Id, c: char) {
        let now = Instant::now();
        let typed = match self.seeking.take() {
            Some(mut seek) if seek.id == id && now.duration_since(seek.at) < SEEK_PAUSE => {
                seek.typed.extend(c.to_lowercase());
                seek.typed
            }
            _ => c.to_lowercase().collect(),
        };
        let (index, names): (&mut usize, Vec<&str>) = match id {
            Id::Keyboard => (
                &mut self.keyboard,
                self.machine.layouts.iter().map(|(_, shown)| shown.as_str()).collect(),
            ),
            Id::Zone => (&mut self.zone, self.machine.zones.iter().map(String::as_str).collect()),
            _ => return,
        };
        // One letter goes on to the next name that starts with it, so that
        // pressing it again steps through them; more stay on the name
        // shown while it still starts with them all.
        let first = usize::from(typed.chars().count() == 1);
        let count = names.len();
        if let Some(found) = (first..first + count)
            .map(|step| (*index + step) % count)
            .find(|&i| names[i].to_lowercase().starts_with(&typed))
        {
            *index = found;
        }
        self.seeking = Some(Seek { id, typed, at: now });
        if id == Id::Keyboard {
            self.keyboard_changed();
        }
    }

    /// The layout changed: the keys type in it from now on, so that the
    /// passwords are typed as they will be at the sign-in.
    fn keyboard_changed(&mut self) {
        self.said = super::set_keyboard(&self.machine.layouts[self.keyboard].0);
    }

    /// Chooses the choice or toggle `id`.
    fn choose(&mut self, id: Id) {
        match id {
            Id::StepByStep => self.from_file = false,
            Id::FromFile => self.from_file = true,
            Id::SetupFile(i) => self.setup_file = i,
            Id::Card(i) => self.card = i,
            Id::Dhcp => self.fixed = false,
            Id::Fixed => self.fixed = true,
            Id::Disk(i) => {
                self.disk = Some(i);
                self.confirm.clear();
            }
            Id::KeepHashes => self.keep_hashes = !self.keep_hashes,
            _ => {}
        }
        self.problem = None;
    }

    /// Presses the button or action `id`.
    fn press(&mut self, id: Id) -> Outcome {
        match id {
            Id::Next => self.next(),
            Id::Back => {
                self.back();
                Outcome::Stay
            }
            Id::Help => {
                self.modal = Some(Modal::Help(self.page));
                Outcome::Stay
            }
            Id::Live => Outcome::Quit,
            Id::Load => {
                match self.setup_files.get(self.setup_file).map(|found| found.answers.clone()) {
                    Some(Ok(answers)) => {
                        self.loaded = Some(answers);
                        self.confirm.clear();
                        self.show(Page::Review);
                    }
                    Some(Err(problems)) => self.problem = problems.first().cloned(),
                    None => self.problem = Some(messages::text("setup-none")),
                }
                Outcome::Stay
            }
            Id::Install => match self.check_page() {
                Ok(()) => {
                    self.show(Page::Installing);
                    Outcome::Install
                }
                Err(problem) => {
                    self.problem = Some(problem);
                    self.focus_on(Id::Confirm);
                    Outcome::Stay
                }
            },
            Id::AddPerson => {
                self.modal = Some(Modal::Person(PersonForm::default()));
                Outcome::Stay
            }
            Id::Restart => Outcome::Restart,
            Id::ShowLog => {
                self.modal = Some(Modal::Log { top: usize::MAX });
                Outcome::Stay
            }
            Id::SaveSetup => Outcome::SaveSetup,
            _ => Outcome::Stay,
        }
    }

    /// Next: checks the page and shows the next one.
    fn next(&mut self) -> Outcome {
        if let Err(problem) = self.check_page() {
            self.problem = Some(problem);
            return Outcome::Stay;
        }
        let next = match self.page {
            Page::Welcome if self.from_file => Page::SetupFile,
            Page::Welcome => Page::Network,
            Page::Network => Page::Disk,
            Page::Disk => Page::You,
            Page::You => Page::Review,
            Page::Review => return self.press(Id::Install),
            Page::SetupFile => return self.press(Id::Load),
            other => other,
        };
        self.show(next);
        Outcome::Stay
    }

    /// Back: the page before, keeping its answers.
    fn back(&mut self) {
        let before = match self.page {
            Page::SetupFile | Page::Network => Page::Welcome,
            Page::Disk => Page::Network,
            Page::You => Page::Disk,
            Page::Review if self.loaded.take().is_some() => Page::SetupFile,
            Page::Review => Page::You,
            other => other,
        };
        if before != self.page {
            self.show(before);
        }
    }

    /// Acts on `key` in the box shown over the page.
    fn modal_key(&mut self, key: KeyEvent) -> Outcome {
        let Some(modal) = self.modal.as_mut() else { return Outcome::Stay };
        match modal {
            Modal::Help(_) => {
                if matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Enter | KeyCode::F(1) | KeyCode::Char(' ')
                ) {
                    self.modal = None;
                }
            }
            Modal::Log { top } => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('l' | 'L' | 'q') => self.modal = None,
                KeyCode::Up => *top = top.saturating_sub(1),
                KeyCode::PageUp => *top = top.saturating_sub(10),
                KeyCode::Down => *top = top.saturating_add(1),
                KeyCode::PageDown => *top = top.saturating_add(10),
                KeyCode::Home => *top = 0,
                KeyCode::End => *top = usize::MAX,
                _ => {}
            },
            Modal::Person(form) => match key.code {
                KeyCode::Esc => self.modal = None,
                KeyCode::Tab | KeyCode::Down => form.focus = (form.focus + 1) % 3,
                KeyCode::BackTab | KeyCode::Up => form.focus = (form.focus + 2) % 3,
                KeyCode::Enter => {
                    let form = form.clone();
                    self.add_person(form);
                }
                KeyCode::Backspace | KeyCode::Char(_) => {
                    let text = form_text(form);
                    match key.code {
                        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            text.push(c);
                        }
                        KeyCode::Backspace => {
                            text.pop();
                        }
                        _ => {}
                    }
                    form.problem = None;
                    match form.focus {
                        0 if !form.user_typed => form.user = user_name(&form.name),
                        1 => form.user_typed = !form.user.is_empty(),
                        _ => {}
                    }
                }
                _ => {}
            },
        }
        Outcome::Stay
    }

    /// Adds the person of `form`, if the name and user name are good and
    /// not taken.
    fn add_person(&mut self, form: PersonForm) {
        let user = form.user.trim().to_string();
        let taken = user == self.user.trim() || self.people.iter().any(|p| p.user == user);
        let problem = if taken {
            Some(messages::with("you-taken", &[("user", Arg::Text(&user))]))
        } else {
            accounts::check_name(&user)
                .err()
                .or_else(|| accounts::check_full_name(form.name.trim()).err())
        };
        match problem {
            Some(problem) => {
                if let Some(Modal::Person(shown)) = self.modal.as_mut() {
                    shown.problem = Some(problem);
                }
            }
            None => {
                self.people.push(Person {
                    name: form.name.trim().to_string(),
                    user,
                    password: form.password,
                });
                self.modal = None;
            }
        }
    }

    /// Takes in what the install's log told.
    pub fn told(&mut self, event: Event) {
        match event {
            Event::Step(step) => self.progress.started.push(step),
            Event::Line(line) => self.progress.last = line,
            Event::Copied { done, total } => self.progress.copied = (done, total),
        }
    }

    /// The install ended: `Ok` with its last line, or why it failed.
    pub fn finished(&mut self, result: Result<String, String>) {
        let page = if result.is_ok() { Page::Done } else { Page::Failed };
        self.progress.finished = Some(result);
        self.show(page);
    }
}

/// The text of the focused field of `form`.
fn form_text(form: &mut PersonForm) -> &mut String {
    match form.focus {
        0 => &mut form.name,
        1 => &mut form.user,
        _ => &mut form.password,
    }
}

/// A user name made from a full name: its first word, in lowercase
/// letters and digits, as the approved design suggests one.
#[must_use]
pub fn user_name(name: &str) -> String {
    name.split_whitespace()
        .next()
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .take(32)
        .collect()
}

/// The message id of the button `id`'s label.
fn button_name(id: Id) -> &'static str {
    match id {
        Id::Live => "button-live",
        Id::Back => "button-back",
        Id::Load => "button-load",
        Id::Install => "button-install",
        Id::Help => "button-help",
        Id::Restart => "button-restart",
        Id::ShowLog => "button-log",
        Id::SaveSetup => "button-save-setup",
        _ => "button-next",
    }
}

/// A row of plain text.
fn text(text: String) -> Row {
    Row::Text(text, Look::Plain)
}

/// A field labelled by the message `label`.
fn field(id: Id, kind: Kind, label: &str, note: &str) -> Row {
    Row::Field(Field {
        id,
        kind,
        label: messages::text(label),
        note: note.to_string(),
        enabled: true,
    })
}

/// An input field labelled by the message `label`, or by `label` itself
/// if it is no message's id.
fn input(id: Id, label: &str, value: &str, masked: bool, hint: &str) -> Row {
    let shown = if label.contains(' ') { label.to_string() } else { messages::text(label) };
    Row::Field(Field {
        id,
        kind: Kind::Input { value: value.to_string(), masked, hint: hint.to_string() },
        label: shown,
        note: String::new(),
        enabled: true,
    })
}

/// A disk as the disk page lists it: its name, size, kind and model.
fn disk_label(disk: &crate::machine::DiskInfo) -> String {
    let kind = match disk.rotational {
        Some(false) => "SSD",
        Some(true) => "HDD",
        None => "",
    };
    format!("{:<8} {:>8}  {kind:<3}  {}", disk.name, size(disk.bytes), disk.model)
}

/// What a disk holds, said.
fn holds(holds: Holds, partitions: usize) -> String {
    let count = Arg::Number(partitions as u64);
    match holds {
        Holds::Nothing => messages::text("disk-empty"),
        Holds::Windows => messages::with("disk-windows", &[("count", count)]),
        Holds::Dukovo => messages::with("disk-dukovo", &[("count", count)]),
        Holds::Linux => messages::with("disk-linux", &[("count", count)]),
        Holds::Other => messages::with("disk-partitions", &[("count", count)]),
    }
}

/// A size in bytes, as people read one: `512 MiB`, `8.0 GiB`, `111 GiB`.
#[must_use]
pub fn size(bytes: u64) -> String {
    const GIB: u64 = 1 << 30;
    if bytes < GIB {
        format!("{} MiB", bytes >> 20)
    } else if bytes < 100 * GIB {
        format!("{}.{} GiB", bytes / GIB, (bytes % GIB) * 10 / GIB)
    } else {
        format!("{} GiB", bytes / GIB)
    }
}
