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

use super::rdisk::{self, FormField, PartForm, Rdisk};
use crate::answers::{
    self, Account, Answers, Change, Disk, Features, Fixed, Layout, Network, Profile, Role,
};
use crate::download::{self, Downloaded};
use crate::features::{self, Offered};
use crate::log::{Event, Step};
use crate::machine::{DiskInfo, Holds, Machine};
use crate::messages::{self, Arg};
use crate::plan;
use crate::windows::{Refused, Windows};

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
    /// The manual layout's partitions, on the rdisk screen.
    Rdisk,
    /// What the machine is for: a profile, its features and the web
    /// console.
    Software,
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
    pub const STEPS: [Page; 6] =
        [Page::Welcome, Page::Network, Page::Disk, Page::Software, Page::You, Page::Review];

    /// The id of its name among the messages.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Page::Welcome => "page-welcome",
            Page::SetupFile => "page-setup-file",
            Page::Network => "page-network",
            Page::Disk => "page-disk",
            Page::Rdisk => "page-rdisk",
            Page::Software => "page-software",
            Page::You => "page-you",
            Page::Review => "page-review",
            Page::Installing => "page-installing",
            Page::Done => "page-done",
            Page::Failed => "page-failed",
        }
    }

    /// Its place among [`Page::STEPS`], if it is one; the rdisk screen
    /// is the Disk step's.
    #[must_use]
    pub fn step(self) -> Option<usize> {
        let page = if self == Page::Rdisk { Page::Disk } else { self };
        Page::STEPS.iter().position(|step| *step == page)
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
    /// Setup file: download it from a web address (ADR 0319).
    SetupWeb,
    /// Setup file: the web address, `https://` only.
    SetupUrl,
    /// Setup file: the SHA-256 the file must have, if one is given.
    SetupSha,
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
    /// Disk: the layout next to Windows.
    Beside,
    /// Disk: the space Dukovo takes next to Windows, in GiB.
    Space,
    /// Disk: the mirror layout, of two disks.
    Mirror,
    /// Disk: the mirror layout's second disk.
    MirrorDisk,
    /// Disk: the free-space layout.
    Free,
    /// Disk: the manual layout, on the rdisk screen.
    Manual,
    /// Disk: the swap's size in GiB.
    Swap,
    /// Software: a profile, by its place in [`Profile::ALL`].
    Profile(usize),
    /// Software: a feature, by its place in the release's list.
    Feature(usize),
    /// Software: turn the web console on.
    WebConsole,
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
    /// A partition being made or formatted on the rdisk screen.
    Partition(PartForm),
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
    /// Download the setup file at this address, over TLS only, and check
    /// it against this SHA-256 if one is given; [`App::downloaded`] takes
    /// what came.
    Download(tuf::Url, Option<[u8; 32]>),
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
    /// Whether the live system started from the install DVD, not a stick:
    /// the messages that name the medium say disc, and the Done page
    /// offers no setup file to save on it.
    pub from_disc: bool,
    /// The setup file chosen, by its place in the list.
    pub setup_file: usize,
    /// Whether the setup file is downloaded from a web address, not one
    /// found on the stick: always when none was found.
    pub setup_web: bool,
    /// The web address of the setup file.
    pub setup_url: String,
    /// The SHA-256 it must have, if one is given, in hexadecimal.
    pub setup_sha: String,
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
    /// The space Dukovo takes next to Windows, in GiB, as typed: a
    /// suggestion when the layout is first chosen.
    pub space: String,
    /// How the disk is laid out: guided, next to Windows, a mirror, free
    /// space or manual.
    pub layout: Layout,
    /// The mirror layout's second disk, by its place in the machine's
    /// list: another disk than the target, and not the stick.
    pub mirror: Option<usize>,
    /// The rdisk screen, once the manual layout has shown it.
    pub rdisk: Option<Rdisk>,
    /// What the machine is for.
    pub profile: Profile,
    /// The features the release offers, read from the live system.
    pub features: Vec<Offered>,
    /// The features ticked, by their IDs.
    pub picked: Vec<String>,
    /// Whether the web console is to be turned on: the profile's choice
    /// until it is changed.
    pub web_console: bool,
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
    /// Where they were downloaded from and whether they were checked, as
    /// the review says it, if they came from the web.
    pub loaded_from: Option<String>,
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
            from_disc: false,
            setup_file: 0,
            setup_web: false,
            setup_url: String::new(),
            setup_sha: String::new(),
            card: 0,
            fixed: false,
            address: String::new(),
            gateway: String::new(),
            dns: String::new(),
            hostname: answers::DEFAULT_HOSTNAME.to_string(),
            zone,
            disk,
            swap: swap.to_string(),
            space: String::new(),
            layout: Layout::Guided,
            mirror: None,
            rdisk: None,
            profile: Profile::Server,
            features: Vec::new(),
            picked: Vec::new(),
            web_console: Profile::Server.web_console(),
            name: String::new(),
            user: String::new(),
            user_typed: false,
            password: String::new(),
            repeat: String::new(),
            keep_hashes: false,
            people: Vec::new(),
            confirm: String::new(),
            loaded: None,
            loaded_from: None,
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
            Page::Rdisk => Vec::new(),
            Page::Software => self.software_rows(),
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
            // A disc is read-only: no setup file is saved on it.
            Page::Done if self.from_disc => &[Id::Restart, Id::ShowLog],
            Page::Done => &[Id::Restart, Id::ShowLog, Id::SaveSetup],
            Page::Failed => &[Id::ShowLog, Id::Live],
            Page::Network | Page::Disk | Page::Rdisk | Page::Software | Page::You => {
                &[Id::Back, Id::Next, Id::Help]
            }
        };
        ids.iter().map(|id| (*id, messages::text(button_name(*id)))).collect()
    }

    /// The medium the live system started from, as the messages that name
    /// it select by: `disc` for the install DVD, `stick` otherwise.
    #[must_use]
    pub fn medium(&self) -> &'static str {
        if self.from_disc { "disc" } else { "stick" }
    }

    /// The message `id`, which may name the medium.
    #[must_use]
    pub fn say(&self, id: &str) -> String {
        messages::with(id, &[("medium", Arg::Text(self.medium()))])
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
        if page == Page::Rdisk {
            // Its keys are its own; Next is the button Enter presses.
            self.focus_on(Id::Next);
        }
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
                &self.say("welcome-file-note"),
            ),
        ]
    }

    fn setup_file_rows(&self) -> Vec<Row> {
        let mut rows = vec![text(self.say("setup-intro")), Row::Blank];
        if self.setup_files.is_empty() {
            rows.push(Row::Text(self.say("setup-none"), Look::Dim));
        }
        for (i, found) in self.setup_files.iter().enumerate() {
            let note = match &found.answers {
                Ok(_) => messages::text("setup-ready"),
                Err(problems) => problems.first().cloned().unwrap_or_default(),
            };
            rows.push(Row::Field(Field {
                id: Id::SetupFile(i),
                kind: Kind::Choice(!self.web_chosen() && self.setup_file == i),
                label: found.place.clone(),
                note,
                enabled: true,
            }));
        }
        rows.push(field(
            Id::SetupWeb,
            Kind::Choice(self.web_chosen()),
            "setup-web",
            &messages::text("setup-web-note"),
        ));
        if self.web_chosen() {
            rows.extend([
                input(
                    Id::SetupUrl,
                    "setup-web-address-label",
                    &self.setup_url,
                    false,
                    &messages::text("setup-web-address-hint"),
                ),
                input(
                    Id::SetupSha,
                    "setup-web-sha-label",
                    &self.setup_sha,
                    false,
                    &messages::text("setup-web-sha-hint"),
                ),
                Row::Text(messages::text("setup-web-explain"), Look::Dim),
            ]);
        }
        rows
    }

    /// Whether the setup file is downloaded from the web: when chosen, or
    /// when the stick has none.
    #[must_use]
    pub fn web_chosen(&self) -> bool {
        self.setup_web || self.setup_files.is_empty()
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
            // The other layouts are checked against the room they find.
            let small = self.layout == Layout::Guided && disk.bytes < self.needed();
            let note = if disk.stick {
                messages::text("disk-stick")
            } else if small {
                messages::with("disk-too-small", &[("size", Arg::Text(&size(self.needed())))])
            } else {
                holds(disk.holds(), disk.partitions.len())
            };
            rows.push(Row::Field(Field {
                id: Id::Disk(i),
                kind: Kind::Choice(self.disk == Some(i)),
                label: disk_label(disk),
                note,
                enabled: !disk.stick && !small,
            }));
        }
        let free_note = match self.free_room() {
            Ok(free) => messages::with("disk-free-note", &[("size", Arg::Text(&size(free)))]),
            Err(why) => why,
        };
        rows.extend([
            Row::Blank,
            Row::Text(messages::text("disk-how"), Look::Bold),
            field(
                Id::Guided,
                Kind::Choice(self.layout == Layout::Guided),
                "disk-guided",
                &messages::text("disk-guided-note"),
            ),
        ]);
        rows.extend(self.beside_rows());
        rows.extend(self.mirror_rows());
        rows.extend([
            Row::Field(Field {
                id: Id::Free,
                kind: Kind::Choice(self.layout == Layout::Free),
                label: messages::text("disk-free"),
                note: free_note,
                enabled: self.free_room().is_ok(),
            }),
            field(
                Id::Manual,
                Kind::Choice(self.layout == Layout::Manual),
                "disk-manual",
                &messages::text("disk-manual-note"),
            ),
        ]);
        // The manual layout makes its own swap partition, or none.
        if self.layout != Layout::Manual {
            rows.extend([
                input(Id::Swap, "disk-swap", &self.swap, false, "0"),
                Row::Text(messages::text("disk-swap-note"), Look::Dim),
            ]);
        }
        if let Some(disk) = self.disk.and_then(|i| self.machine.disks.get(i)) {
            // The guided layout erases the disk, and the mirror layout
            // both of its disks.
            let erased: Vec<&DiskInfo> = match self.layout {
                Layout::Guided => vec![disk],
                Layout::Mirror => std::iter::once(disk).chain(self.mirror_disk()).collect(),
                _ => Vec::new(),
            };
            if self.layout == Layout::Mirror {
                rows.extend([
                    Row::Blank,
                    Row::Text(messages::text("disk-mirror-both"), Look::Plain),
                ]);
            }
            for erased in &erased {
                let name = Arg::Text(&erased.name);
                let warning = match erased.holds() {
                    Holds::Nothing => None,
                    Holds::Windows => Some(messages::with("disk-erase-windows", &[("disk", name)])),
                    Holds::Dukovo | Holds::Linux | Holds::Other => {
                        Some(messages::with("disk-erase", &[("disk", name)]))
                    }
                };
                if let Some(warning) = warning {
                    rows.extend([Row::Blank, Row::Text(warning, Look::Danger)]);
                }
            }
            let mirrored = self.mirror_disk().filter(|_| self.layout == Layout::Mirror);
            let others: Vec<String> = self
                .machine
                .disks
                .iter()
                .filter(|other| other.name != disk.name && !other.stick)
                .filter(|other| mirrored.is_none_or(|mirror| mirror.name != other.name))
                .map(|other| other.name.clone())
                .collect();
            if !others.is_empty() && self.layout != Layout::Manual {
                let count = Arg::Number(others.len() as u64);
                let others = others.join(", ");
                rows.push(Row::Text(
                    messages::with(
                        "disk-untouched",
                        &[("disks", Arg::Text(&others)), ("count", count)],
                    ),
                    Look::Dim,
                ));
            }
        }
        rows
    }

    /// The free space the free-space layout would use on the disk
    /// chosen, in bytes, or why it cannot be used.
    fn free_room(&self) -> Result<u64, String> {
        let disk = self
            .disk
            .and_then(|i| self.machine.disks.get(i))
            .ok_or_else(|| self.say("disk-none"))?;
        let (_, sectors) = plan::free_room(disk, self.swap_gib(), self.root_bytes())?;
        Ok(sectors * u64::from(disk.sector_size))
    }

    /// The rows of the layout next to Windows: its choice, and under it
    /// why Windows cannot be shrunk now, if it cannot, or once it is
    /// chosen, the space for Dukovo and the checks Windows passed.
    fn beside_rows(&self) -> Vec<Row> {
        let room = self.beside_room();
        let note = match &room {
            Ok(_) => messages::text("disk-beside-note"),
            Err(why) => why.clone(),
        };
        let mut rows = vec![Row::Field(Field {
            id: Id::Beside,
            kind: Kind::Choice(self.layout == Layout::Beside),
            label: messages::text("disk-beside"),
            note,
            enabled: room.is_ok(),
        })];
        let Some(windows) = self.windows() else { return rows };
        if let Some(refused) = &windows.refused {
            rows.push(Row::Text(refusal(refused), Look::Warning));
        } else if !self.machine.boot_entries {
            rows.push(Row::Text(self.say("disk-windows-bios"), Look::Warning));
        } else if let (Ok((least, most)), Layout::Beside) = (room, self.layout) {
            let field = input(Id::Space, "disk-space", &self.space, false, "");
            let Row::Field(mut field) = field else { return rows };
            field.note = messages::with(
                "disk-space-note",
                &[
                    ("least", Arg::Text(&size(least))),
                    ("most", Arg::Text(&size(most))),
                    ("size", Arg::Text(&size(windows.bytes))),
                    ("used", Arg::Text(&size(windows.used))),
                ],
            );
            rows.push(Row::Field(field));
            rows.push(Row::Text(messages::text("disk-windows-ready"), Look::Good));
        }
        rows
    }

    /// The rows of the mirror layout: its choice, and once it is chosen,
    /// the second disk, which Left and Right change.
    fn mirror_rows(&self) -> Vec<Row> {
        let candidates = self.mirror_candidates();
        let chosen = self.disk.and_then(|i| self.machine.disks.get(i));
        let note = if candidates.is_empty() {
            messages::text("disk-mirror-alone")
        } else if chosen.is_some_and(|disk| {
            candidates.iter().any(|&i| self.machine.disks[i].bytes == disk.bytes)
        }) {
            messages::text("disk-mirror-note")
        } else {
            messages::text("disk-mirror-sizes")
        };
        let mut rows = vec![Row::Field(Field {
            id: Id::Mirror,
            kind: Kind::Choice(self.layout == Layout::Mirror),
            label: messages::text("disk-mirror"),
            note,
            enabled: !candidates.is_empty(),
        })];
        if self.layout == Layout::Mirror
            && let Some(second) = self.mirror_disk()
        {
            let value = format!("{}  {}", second.name, size(second.bytes));
            let field = field(Id::MirrorDisk, Kind::Cycle(value), "disk-mirror-onto", "");
            if let Row::Field(mut field) = field {
                field.label = format!("    {}", field.label);
                field.note = holds(second.holds(), second.partitions.len());
                rows.push(Row::Field(field));
            }
        }
        rows
    }

    /// The disks the mirror layout can take as its second, by their place
    /// in the machine's list: every disk but the target and the stick.
    fn mirror_candidates(&self) -> Vec<usize> {
        (0..self.machine.disks.len())
            .filter(|&i| Some(i) != self.disk && !self.machine.disks[i].stick)
            .collect()
    }

    /// The mirror layout's second disk, if one can be chosen: the one
    /// chosen, or the first there is.
    fn mirror_disk(&self) -> Option<&DiskInfo> {
        let candidates = self.mirror_candidates();
        let i = self.mirror.filter(|i| candidates.contains(i)).or(candidates.first().copied())?;
        self.machine.disks.get(i)
    }

    /// Checks the mirror layout of the two disks chosen, as the install
    /// will lay them out.
    fn check_mirror(&self) -> Result<(), String> {
        let disk = self
            .disk
            .and_then(|i| self.machine.disks.get(i))
            .ok_or_else(|| self.say("disk-none"))?;
        let second = self.mirror_disk().ok_or_else(|| messages::text("disk-mirror-alone"))?;
        plan::mirror(disk, second, self.swap_gib(), self.root_bytes()).map(drop)
    }

    /// Windows on the disk chosen, if it is there.
    fn windows(&self) -> Option<&Windows> {
        self.disk.and_then(|i| self.machine.disks.get(i)).and_then(|disk| disk.windows.as_ref())
    }

    /// The space Dukovo can take next to Windows on the disk chosen, the
    /// least and the most, in bytes, or why it cannot go there, as the
    /// layout's note says it.
    fn beside_room(&self) -> Result<(u64, u64), String> {
        let disk = self
            .disk
            .and_then(|i| self.machine.disks.get(i))
            .ok_or_else(|| self.say("disk-none"))?;
        let Some(windows) = &disk.windows else {
            return Err(messages::with("disk-beside-none", &[("disk", Arg::Text(&disk.name))]));
        };
        if windows.refused.is_some() {
            return Err(messages::text("disk-beside-refused"));
        }
        if !self.machine.boot_entries {
            return Err(self.say("disk-beside-bios"));
        }
        plan::beside_room(disk, true).map_err(|_| {
            let keeps = (windows.used + plan::WINDOWS_FREE).max(windows.smallest);
            let gives = size(windows.bytes.saturating_sub(keeps));
            messages::with("disk-beside-full", &[("size", Arg::Text(&gives))])
        })
    }

    /// The space Dukovo is given next to Windows, in GiB, as typed.
    fn space_gib(&self) -> Option<u32> {
        self.space.trim().parse().ok()
    }

    /// Checks the space typed for Dukovo next to Windows, as the layout
    /// will use it: within the room Windows gives, and holding Dukovo's
    /// boot partition, the swap and a root of 8 GiB at least.
    fn check_space(&self) -> Result<(), String> {
        let (least, most) = self.beside_room()?;
        let gib = |bytes: u64| (bytes >> 30).to_string();
        let range = || {
            messages::with(
                "disk-space-range",
                &[("least", Arg::Text(&gib(least))), ("most", Arg::Text(&gib(most)))],
            )
        };
        let bytes = u64::from(self.space_gib().ok_or_else(range)?) << 30;
        if !(least..=most).contains(&bytes) {
            return Err(range());
        }
        let beyond = plan::EFI_BYTES + plan::BESIDE_ROOT_LEAST.max(self.root_bytes()) + (1 << 30);
        if bytes < (u64::from(self.swap_gib()) << 30) + beyond {
            return Err(messages::with("disk-space-root", &[("gib", Arg::Text(&gib(beyond)))]));
        }
        let disk = self.disk.and_then(|i| self.machine.disks.get(i)).ok_or("no disk")?;
        plan::beside(disk, true, self.swap_gib(), bytes, self.root_bytes()).map(drop)
    }

    /// The swap asked for, in GiB, within what the installer makes.
    fn swap_gib(&self) -> u32 {
        self.swap.trim().parse::<u32>().unwrap_or(0).min(answers::SWAP_MAX_GIB)
    }

    /// What the root needs: the release and room to spare.
    fn root_bytes(&self) -> u64 {
        self.release_bytes + plan::ROOM
    }

    fn software_rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Text(messages::text("software-intro"), Look::Bold)];
        for (i, profile) in Profile::ALL.into_iter().enumerate() {
            let id = format!("software-{}", profile.id());
            rows.push(field(
                Id::Profile(i),
                Kind::Choice(self.profile == profile),
                &id,
                &messages::text(&format!("{id}-note")),
            ));
        }
        rows.extend([
            Row::Text(messages::text("software-base"), Look::Dim),
            Row::Blank,
            Row::Text(messages::text("software-features"), Look::Bold),
        ]);
        if self.features.is_empty() {
            rows.push(Row::Text(messages::text("software-none"), Look::Dim));
        }
        for (i, offered) in self.features.iter().enumerate() {
            let note = if offered.at_install() {
                let services = offered.feature.services.join(" and ");
                messages::with("software-turns-on", &[("services", Arg::Text(&services))])
            } else {
                messages::with("software-after", &[("id", Arg::Text(&offered.id))])
            };
            rows.push(Row::Field(Field {
                id: Id::Feature(i),
                kind: Kind::Toggle(self.picked.contains(&offered.id)),
                label: offered.feature.name.clone(),
                note,
                enabled: offered.at_install(),
            }));
        }
        rows.extend([
            Row::Blank,
            field(
                Id::WebConsole,
                Kind::Toggle(self.web_console),
                "software-web-console",
                &messages::text("software-web-console-note"),
            ),
        ]);
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
        if let Some(from) = &self.loaded_from {
            rows.extend([Row::Text(from.clone(), Look::Dim), Row::Blank]);
        }
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
        let label = |id: &str, rest: String| format!("{:<10}{rest}", messages::text(id));
        let (mut rows, bios_boot) = match answers.disk.layout {
            Layout::Manual => self.review_changes(&answers.disk.changes, target),
            Layout::Beside => (self.review_beside(answers, target), false),
            Layout::Mirror => (self.review_mirror(answers), true),
            layout => self.review_layout(layout, answers.disk.swap_gib, target),
        };
        rows.push(Row::Text(
            format!("{:12}{}", "", messages::text("review-subvolumes")),
            Look::Dim,
        ));
        rows.push(Row::Text(
            label(
                "review-install",
                messages::with(
                    "review-release",
                    &[("version", Arg::Text(&self.version)), ("medium", Arg::Text(self.medium()))],
                ),
            ),
            Look::Plain,
        ));
        let grub = match answers.disk.layout {
            Layout::Beside => "review-grub-beside",
            Layout::Mirror => "review-grub-mirror",
            _ if bios_boot => "review-grub",
            _ => "review-grub-uefi",
        };
        rows.push(Row::Text(format!("{:10}{}", "", messages::text(grub)), Look::Plain));
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
        if answers.serial_console(self.machine.serial_console) {
            settings.push(messages::text("review-serial-console"));
        }
        rows.push(Row::Text(label("review-settings", settings.join(" · ")), Look::Plain));
        let mut people = vec![messages::with(
            "review-admin",
            &[("user", Arg::Text(&answers.admin.user)), ("name", Arg::Text(&answers.admin.name))],
        )];
        people.extend(answers.users.iter().map(|user| format!("{} ({})", user.user, user.name)));
        rows.push(Row::Text(label("review-accounts", people.join(", ")), Look::Plain));
        rows.push(Row::Text(label("review-software", self.review_software(answers)), Look::Plain));
        let mut changed: Vec<&str> =
            answers.disk.changes.iter().map(|change| change.disk.as_str()).collect();
        changed.extend(answers.mirror().filter(|_| answers.disk.layout == Layout::Mirror));
        for other in self.machine.disks.iter().filter(|other| {
            other.name != target && !other.stick && !changed.contains(&other.name.as_str())
        }) {
            rows.push(Row::Text(
                label("review-untouched", format!("{}  {}", other.name, other.model)),
                Look::Good,
            ));
        }
        rows
    }

    /// The review's line of what the machine is for: its profile, each
    /// feature added, by name, and the web console.
    fn review_software(&self, answers: &Answers) -> String {
        let mut parts = vec![messages::text(&format!("software-{}", answers.profile.id()))];
        parts.extend(answers.chosen_features().into_iter().map(|id| {
            self.features
                .iter()
                .find(|offered| offered.id == id)
                .map_or(id, |offered| offered.feature.name.clone())
        }));
        let console = if answers.web_console() { "on" } else { "off" };
        parts.push(messages::text(&format!("review-web-console-{console}")));
        parts.join(" · ")
    }

    /// The review's lines of the guided or free-space layout on `target`
    /// with swap of `swap_gib`: what is erased or kept, and each
    /// partition made. With whether the disk gets a BIOS boot partition.
    fn review_layout(&self, layout: Layout, swap_gib: u32, target: &str) -> (Vec<Row>, bool) {
        let label = |id: &str, rest: String| format!("{:<10}{rest}", messages::text(id));
        let Some(disk) = self.machine.disks.iter().find(|disk| disk.name == target) else {
            return (Vec::new(), true);
        };
        let model = disk.model.as_str();
        let (head, look, change) = if layout == Layout::Free {
            let kept = Arg::Number(disk.partitions.len() as u64);
            let head = format!(
                "{target}  {model}  {}",
                messages::with("review-keeps", &[("count", kept)])
            );
            (
                label("review-add", head),
                Look::Warning,
                plan::free_space(disk, swap_gib, self.root_bytes()),
            )
        } else {
            let erase = match disk.holds() {
                Holds::Windows => messages::text("review-erase-windows"),
                Holds::Nothing => String::new(),
                _ => messages::text("review-erase-all"),
            };
            (
                label("review-erase", format!("{target}  {model}  {erase}")),
                Look::Danger,
                plan::guided(disk, swap_gib),
            )
        };
        let mut rows = vec![
            Row::Text(head, look),
            Row::Text(
                label("review-create", messages::with("review-on", &[("disk", Arg::Text(target))])),
                Look::Plain,
            ),
        ];
        if let Ok(change) = change {
            rows.extend(made_rows(&change, disk.sector_size));
        }
        (rows, true)
    }

    /// The review's lines of the mirror layout: each of its two disks
    /// erased, and each partition made on it, the root as half of the
    /// RAID1.
    fn review_mirror(&self, answers: &Answers) -> Vec<Row> {
        let label = |id: &str, rest: String| format!("{:<10}{rest}", messages::text(id));
        let find = |name: &str| self.machine.disks.iter().find(|disk| disk.name == name);
        let (Some(first), Some(second)) = (find(answers.target()), answers.mirror().and_then(find))
        else {
            return Vec::new();
        };
        let Ok(changes) = plan::mirror(first, second, answers.disk.swap_gib, self.root_bytes())
        else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for (disk, change) in [first, second].into_iter().zip(&changes) {
            let erase = match disk.holds() {
                Holds::Windows => messages::text("review-erase-windows"),
                Holds::Nothing => String::new(),
                _ => messages::text("review-erase-all"),
            };
            rows.push(Row::Text(
                label("review-erase", format!("{}  {}  {erase}", disk.name, disk.model)),
                Look::Danger,
            ));
            rows.push(Row::Text(
                label(
                    "review-create",
                    messages::with("review-on", &[("disk", Arg::Text(&disk.name))]),
                ),
                Look::Plain,
            ));
            for mut row in made_rows(change, disk.sector_size) {
                // The root is half of the RAID1.
                if let Row::Text(text, _) = &mut row
                    && text.ends_with(&messages::text("review-root-note"))
                {
                    let cut = text.len() - messages::text("review-root-note").len();
                    text.replace_range(cut.., &messages::text("review-mirror-note"));
                }
                rows.push(row);
            }
        }
        rows
    }

    /// The review's lines of the layout next to Windows on `target`: the
    /// shrink of Windows' partition, the partitions kept, each partition
    /// made, and the boot menu that chooses between them.
    fn review_beside(&self, answers: &Answers, target: &str) -> Vec<Row> {
        let label = |id: &str, rest: String| format!("{:<10}{rest}", messages::text(id));
        let Some(disk) = self.machine.disks.iter().find(|disk| disk.name == target) else {
            return Vec::new();
        };
        let (Some(windows), Some(table)) = (&disk.windows, &disk.table) else {
            return Vec::new();
        };
        let space = u64::from(answers.disk.space_gib.unwrap_or(0)) << 30;
        let Ok(change) = plan::beside(
            disk,
            self.machine.boot_entries,
            answers.disk.swap_gib,
            space,
            self.root_bytes(),
        ) else {
            return Vec::new();
        };
        let sector = u64::from(disk.sector_size);
        let to = change.shrink.map_or(windows.bytes, |shrink| shrink.sectors * sector);
        let shrink = messages::with(
            "review-shrink-windows",
            &[
                ("disk", Arg::Text(target)),
                ("from", Arg::Text(&size(windows.bytes))),
                ("to", Arg::Text(&size(to))),
                ("used", Arg::Text(&size(windows.used))),
            ],
        );
        let kept: Vec<String> = table
            .partitions
            .iter()
            .filter(|p| p.number != windows.partition)
            .map(|p| if p.name.is_empty() { p.kind.name().to_string() } else { p.name.clone() })
            .collect();
        let manager = crate::disks::partition(target, windows.boot_manager);
        let mut rows = vec![
            Row::Text(label("review-shrink", shrink), Look::Warning),
            Row::Text(label("review-keep", kept.join(", ")), Look::Good),
            Row::Text(
                label("review-create", messages::with("review-on", &[("disk", Arg::Text(target))])),
                Look::Plain,
            ),
        ];
        rows.extend(made_rows(&change, disk.sector_size));
        rows.push(Row::Text(
            format!(
                "{:10}{}",
                "",
                messages::with("review-boot-menu", &[("partition", Arg::Text(&manager))])
            ),
            Look::Plain,
        ));
        rows
    }

    /// The review's lines of the manual layout's `changes`: each disk
    /// changed, and each partition taken out, made or formatted. With
    /// whether `target` has a BIOS boot partition after.
    fn review_changes(&self, changes: &[answers::Change], target: &str) -> (Vec<Row>, bool) {
        let mut rows = Vec::new();
        let mut bios_boot = false;
        for change in changes {
            let Some(disk) = self.machine.disks.iter().find(|disk| disk.name == change.disk) else {
                continue;
            };
            let what =
                if change.new_table { messages::text("review-new-table") } else { String::new() };
            rows.push(Row::Text(
                format!(
                    "{:<10}{}  {}  {what}",
                    messages::text("review-change"),
                    disk.name,
                    disk.model
                ),
                Look::Danger,
            ));
            let sector = u64::from(disk.sector_size);
            let was = |number: u32| disk.table.as_ref().and_then(|table| table.get(number));
            let line = |verb: &str, number: u32, name: &str, bytes: u64, what: String| {
                format!(
                    "{:12}{:<8}{number:<3}{:<18}{:>9}   {what}",
                    "",
                    messages::text(verb),
                    name,
                    size(bytes)
                )
            };
            let old_name = |number: u32| {
                was(number).map_or(String::new(), |p| {
                    if p.name.is_empty() { p.kind.name().to_string() } else { p.name.clone() }
                })
            };
            if !change.new_table {
                for &number in &change.delete {
                    let bytes = was(number).map_or(0, |p| p.sectors() * sector);
                    rows.push(Row::Text(
                        line("review-delete", number, &old_name(number), bytes, String::new()),
                        Look::Danger,
                    ));
                }
            }
            for part in &change.partitions {
                let (verb, name, bytes) = match part.sectors {
                    Some(sectors) => {
                        ("review-new", part.name.clone().unwrap_or_default(), sectors * sector)
                    }
                    None => (
                        "review-format",
                        old_name(part.number),
                        was(part.number).map_or(0, |p| p.sectors() * sector),
                    ),
                };
                let look = if part.is_new() { Look::Plain } else { Look::Warning };
                rows.push(Row::Text(
                    line(verb, part.number, &name, bytes, holds_what(part.role, part.format)),
                    look,
                ));
            }
            if change.disk == target {
                let kind = disk::types::parse("bios-boot", disk::Scheme::Gpt);
                bios_boot = plan::after(disk, change)
                    .is_ok_and(|table| table.partitions.iter().any(|p| Some(p.kind) == kind));
            }
        }
        (rows, bios_boot)
    }

    /// The space suggested for Dukovo next to Windows, in GiB: 60, within
    /// the room Windows gives.
    fn suggested_space(&self) -> String {
        const SUGGESTED: u64 = 60 << 30;
        self.beside_room()
            .map_or(String::new(), |(least, most)| (SUGGESTED.clamp(least, most) >> 30).to_string())
    }

    /// The size the guided layout needs: its first partitions, the swap
    /// asked for, the release and room to spare.
    fn needed(&self) -> u64 {
        plan::needed(self.swap_gib(), self.root_bytes())
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
            .ok_or_else(|| self.say("disk-none"))?;
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
                layout: self.layout,
                target: disk.name.clone(),
                mirror: match self.layout {
                    Layout::Mirror => self.mirror_disk().map(|disk| disk.name.clone()),
                    _ => None,
                },
                swap_gib: match self.layout {
                    Layout::Manual => 0,
                    _ => self.swap.trim().parse().unwrap_or(u32::MAX),
                },
                changes: match (&self.rdisk, self.layout) {
                    (Some(rdisk), Layout::Manual) => rdisk.changes(&self.machine),
                    _ => Vec::new(),
                },
                space_gib: match self.layout {
                    Layout::Beside => Some(self.space_gib().unwrap_or(0)),
                    _ => None,
                },
            },
            hostname: Some(self.hostname.trim().to_string()),
            time_zone: Some(self.machine.zones[self.zone].clone()),
            network,
            admin: account(&self.name, &self.user),
            users: self.people.iter().map(|person| account(&person.name, &person.user)).collect(),
            keep_password_hashes: self.keep_hashes,
            profile: self.profile,
            features: Features {
                // In the release's order, as the page lists them.
                add: self
                    .features
                    .iter()
                    .filter(|offered| self.picked.contains(&offered.id))
                    .map(|offered| offered.id.clone())
                    .collect(),
                remove: Vec::new(),
            },
            web_console: Some(self.web_console),
            // A system installed over the serial port keeps a console there
            // (ADR 0318).
            serial_console: Some(self.machine.serial_console),
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
            Page::Disk => match self.layout {
                // The rdisk screen checks the manual layout's edits.
                Layout::Manual => return Ok(()),
                Layout::Free => {
                    self.free_room()?;
                    &["disk"]
                }
                Layout::Beside => {
                    self.check_space()?;
                    &["disk"]
                }
                Layout::Mirror => {
                    self.check_mirror()?;
                    &["disk"]
                }
                _ => &["disk"],
            },
            Page::Software => &["features"],
            Page::You => {
                if self.password != self.repeat {
                    return Err(messages::text("you-mismatch"));
                }
                &["admin", "users"]
            }
            Page::Review => {
                let answers = self.answers()?;
                if self.confirm.trim() != answers.target() {
                    // Only the guided layout erases the whole disk.
                    let id = match answers.disk.layout {
                        Layout::Guided | Layout::Mirror => "review-type",
                        _ => "review-type-change",
                    };
                    return Err(messages::with(id, &[("disk", Arg::Text(answers.target()))]));
                }
                return Ok(());
            }
            _ => return Ok(()),
        };
        let answers = self.answers()?;
        if self.page == Page::Disk
            && self.layout == Layout::Guided
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
        if self.page == Page::Rdisk {
            return self.rdisk_key(key);
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
            Id::SetupUrl => &mut self.setup_url,
            Id::SetupSha => &mut self.setup_sha,
            Id::Address => &mut self.address,
            Id::Gateway => &mut self.gateway,
            Id::Dns => &mut self.dns,
            Id::Hostname => &mut self.hostname,
            Id::Swap => &mut self.swap,
            Id::Space => &mut self.space,
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
            Id::MirrorDisk => {
                self.cycle_mirror(by);
                return;
            }
            _ => return,
        };
        *index = (*index as isize + by).rem_euclid(count.max(1) as isize) as usize;
        if id == Id::Keyboard {
            self.keyboard_changed();
        }
    }

    /// Moves the mirror layout's second disk by `by` among the disks it
    /// can be.
    fn cycle_mirror(&mut self, by: isize) {
        let candidates = self.mirror_candidates();
        let at = self
            .mirror_disk()
            .and_then(|disk| self.machine.disks.iter().position(|d| d.name == disk.name))
            .and_then(|i| candidates.iter().position(|&c| c == i))
            .unwrap_or(0);
        let next = (at as isize + by).rem_euclid(candidates.len().max(1) as isize) as usize;
        self.mirror = candidates.get(next).copied();
        self.problem = None;
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
            Id::SetupFile(i) => {
                self.setup_file = i;
                self.setup_web = false;
            }
            Id::SetupWeb => self.setup_web = true,
            Id::Card(i) => self.card = i,
            Id::Dhcp => self.fixed = false,
            Id::Fixed => self.fixed = true,
            Id::Disk(i) => {
                self.disk = Some(i);
                self.confirm.clear();
            }
            Id::KeepHashes => self.keep_hashes = !self.keep_hashes,
            Id::Profile(i) => {
                if let Some(profile) = Profile::ALL.get(i) {
                    self.profile = *profile;
                    self.web_console = profile.web_console();
                }
            }
            Id::Feature(i) => {
                if let Some(offered) = self.features.get(i) {
                    match self.picked.iter().position(|id| *id == offered.id) {
                        Some(at) => {
                            self.picked.remove(at);
                        }
                        None => self.picked.push(offered.id.clone()),
                    }
                }
            }
            Id::WebConsole => self.web_console = !self.web_console,
            Id::Guided => self.layout = Layout::Guided,
            Id::Beside => {
                self.layout = Layout::Beside;
                if self.space.is_empty() {
                    self.space = self.suggested_space();
                }
            }
            Id::Mirror => {
                self.layout = Layout::Mirror;
                self.mirror = self
                    .mirror_disk()
                    .and_then(|disk| self.machine.disks.iter().position(|d| d.name == disk.name));
            }
            Id::Free => self.layout = Layout::Free,
            Id::Manual => self.layout = Layout::Manual,
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
            Id::Load if self.web_chosen() => self.download(),
            Id::Load => {
                let found =
                    self.setup_files.get(self.setup_file).map(|found| found.answers.clone());
                match found {
                    Some(answers) => self.load(answers, None),
                    None => self.problem = Some(self.say("setup-none")),
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

    /// Load, with the web chosen: checks the address and the SHA-256 and
    /// asks for the file to be downloaded, saying so while it is.
    fn download(&mut self) -> Outcome {
        let asked =
            download::address(&self.setup_url).map_err(|why| (Id::SetupUrl, why)).and_then(|url| {
                let sha = download::sha256(&self.setup_sha).map_err(|why| (Id::SetupSha, why))?;
                Ok((url, sha))
            });
        match asked {
            Ok((url, sha)) => {
                self.problem = None;
                self.said = Some(messages::text("setup-web-downloading"));
                Outcome::Download(url, sha)
            }
            Err((id, why)) => {
                self.problem = Some(why);
                self.focus_on(id);
                Outcome::Stay
            }
        }
    }

    /// The setup file downloaded from the web, or why it could not be:
    /// loaded, as one found on the stick, and the review shows where it
    /// came from.
    pub fn downloaded(&mut self, fetched: Result<Downloaded, String>) {
        self.said = None;
        let address = self.setup_url.trim().to_string();
        match fetched {
            Ok(file) => {
                let from = if file.checked { "setup-web-checked" } else { "setup-web-unchecked" };
                let from = messages::with(from, &[("address", Arg::Text(&address))]);
                // A page that is not JSON says so in JSON's terms, so
                // what it is not is said first.
                let answers = Answers::parse(&file.bytes)
                    .map_err(|why| {
                        vec![messages::with("setup-web-not-answers", &[("why", Arg::Text(&why))])]
                    })
                    .and_then(|answers| answers.check().map(|()| answers));
                self.load(answers, Some(from));
            }
            Err(why) => self.problem = Some(why),
        }
    }

    /// Loads a setup file's answers, read and checked, and shows the
    /// review; or says the first reason they cannot be used. `from` is
    /// where they were downloaded from, for the review.
    fn load(&mut self, answers: Result<Answers, Vec<String>>, from: Option<String>) {
        // The features it adds must be this release's, which only the
        // install stick knows, not the file's checks.
        let answers = answers.and_then(|answers| {
            features::check_offered(&answers, &self.features)
                .map(|_| answers)
                .map_err(|problem| vec![problem])
        });
        match answers {
            Ok(answers) => {
                self.loaded = Some(answers);
                self.loaded_from = from;
                self.confirm.clear();
                self.show(Page::Review);
            }
            Err(problems) => self.problem = problems.first().cloned(),
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
            Page::Disk if self.layout == Layout::Manual => {
                if !self.open_rdisk() {
                    self.problem = Some(self.say("disk-none"));
                    return Outcome::Stay;
                }
                Page::Rdisk
            }
            Page::Disk => Page::Software,
            Page::Rdisk => {
                let Some(rdisk) = &self.rdisk else { return Outcome::Stay };
                match rdisk.done(&self.machine, self.root_bytes()) {
                    Ok(target) => {
                        // The disk that holds the root is the one the
                        // review asks to have typed.
                        self.disk = Some(target);
                        self.confirm.clear();
                        Page::Software
                    }
                    Err(problem) => {
                        self.problem = Some(problem);
                        return Outcome::Stay;
                    }
                }
            }
            Page::Software => Page::You,
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
            Page::Rdisk => Page::Disk,
            Page::Software if self.layout == Layout::Manual => Page::Rdisk,
            Page::Software => Page::Disk,
            Page::You => Page::Software,
            Page::Review if self.loaded.take().is_some() => {
                self.loaded_from = None;
                Page::SetupFile
            }
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
            Modal::Partition(form) => {
                if key.code == KeyCode::Esc {
                    self.modal = None;
                } else if key.code == KeyCode::Enter {
                    let form = form.clone();
                    self.apply_partition(&form);
                } else {
                    partition_key(form, key);
                }
            }
        }
        Outcome::Stay
    }

    /// Readies the rdisk screen, on the disk chosen, or the first disk
    /// that is not the install stick; it keeps its edits from before.
    /// False if there is no disk to show.
    fn open_rdisk(&mut self) -> bool {
        let first = self.machine.disks.iter().position(|disk| !disk.stick);
        let Some(at) = self.disk.or(first) else { return false };
        match self.rdisk.as_mut() {
            Some(rdisk) => {
                rdisk.disk = at;
                rdisk.move_row(0);
            }
            None => self.rdisk = Some(Rdisk::new(&self.machine, at)),
        }
        true
    }

    /// Acts on `key` on the rdisk screen.
    fn rdisk_key(&mut self, key: KeyEvent) -> Outcome {
        let Some(mut rdisk) = self.rdisk.take() else { return Outcome::Stay };
        let disk = &self.machine.disks[rdisk.disk];
        self.problem = None;
        let mut done = Ok(());
        match key.code {
            KeyCode::Up => rdisk.move_row(-1),
            KeyCode::Down => rdisk.move_row(1),
            KeyCode::PageUp => rdisk.move_row(-10),
            KeyCode::PageDown => rdisk.move_row(10),
            KeyCode::Tab | KeyCode::BackTab => rdisk.next_disk(&self.machine),
            KeyCode::Char('n' | 'N') => match rdisk.new_form(disk) {
                Ok(form) => self.modal = Some(Modal::Partition(form)),
                Err(why) => done = Err(why),
            },
            KeyCode::Char('f' | 'F') => {
                if let Some(form) = rdisk.format_form() {
                    self.modal = Some(Modal::Partition(form));
                }
            }
            KeyCode::Char('d' | 'D') | KeyCode::Delete => done = rdisk.delete(),
            KeyCode::Char('g' | 'G') => done = rdisk.guided(disk, self.swap_gib()),
            KeyCode::Char('u' | 'U') => rdisk.undo(),
            KeyCode::F(1) | KeyCode::Char('?') => self.modal = Some(Modal::Help(Page::Rdisk)),
            KeyCode::Enter => {
                self.rdisk = Some(rdisk);
                return self.next();
            }
            KeyCode::Esc => {
                self.rdisk = Some(rdisk);
                self.back();
                return Outcome::Stay;
            }
            _ => {}
        }
        self.problem = done.err();
        self.rdisk = Some(rdisk);
        Outcome::Stay
    }

    /// Makes or formats the partition of `form`, closing the form; what
    /// stops it is shown in the form.
    fn apply_partition(&mut self, form: &PartForm) {
        let Some(rdisk) = self.rdisk.as_mut() else { return };
        let disk = &self.machine.disks[rdisk.disk];
        match rdisk.apply_form(disk, form) {
            Ok(()) => self.modal = None,
            Err(problem) => {
                if let Some(Modal::Partition(shown)) = self.modal.as_mut() {
                    shown.problem = Some(problem);
                }
            }
        }
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

/// Acts on `key` in the partition form: moving between its fields,
/// choosing the use, and typing the size and name.
fn partition_key(form: &mut PartForm, key: KeyEvent) {
    let fields = form.fields();
    let field = form.field();
    form.problem = None;
    match key.code {
        KeyCode::Tab | KeyCode::Down => form.focus = (form.focus + 1) % fields,
        KeyCode::BackTab | KeyCode::Up => form.focus = (form.focus + fields - 1) % fields,
        // The boxes have no cursor to move, so Left and Right choose the
        // use from any field, as the form's hint says.
        KeyCode::Left => form.cycle(-1),
        KeyCode::Right => form.cycle(1),
        KeyCode::Char(' ') if field == FormField::Use => form.cycle(1),
        KeyCode::Backspace => {
            form.text_mut().map(String::pop);
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(text) = form.text_mut() {
                text.push(c);
            }
        }
        _ => {}
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

/// What a partition for `role` holds, with `format` for data, as the
/// review says it.
/// The review's lines of each partition `change` makes, on a disk of
/// `sector_size`-byte sectors: its number, name, size and what it holds.
fn made_rows(change: &Change, sector_size: u32) -> Vec<Row> {
    let sector = u64::from(sector_size);
    change
        .partitions
        .iter()
        .map(|part| {
            let bytes = part.sectors.unwrap_or_default() * sector;
            Row::Text(
                format!(
                    "{:12}{}  {:<16}{:>9}   {}",
                    "",
                    part.number,
                    part.name.clone().unwrap_or_default(),
                    size(bytes),
                    holds_what(part.role, part.format)
                ),
                Look::Plain,
            )
        })
        .collect()
}

/// What the disk page says of Windows that cannot be shrunk now: what is
/// in the way, and how to put it right in Windows.
fn refusal(refused: &Refused) -> String {
    match refused {
        Refused::BitLocker => messages::text("disk-windows-bitlocker"),
        Refused::FastStartup => messages::text("disk-windows-fast-startup"),
        Refused::Dirty => messages::text("disk-windows-dirty"),
        Refused::Damaged(problem) => {
            messages::with("disk-windows-damaged", &[("problem", Arg::Text(problem))])
        }
        Refused::Kept(why) | Refused::Unreadable(why) => {
            messages::with("disk-windows-other", &[("why", Arg::Text(why))])
        }
    }
}

fn holds_what(role: Role, format: Option<answers::Format>) -> String {
    match role {
        Role::BiosBoot => "-".to_string(),
        Role::Efi => messages::text("review-efi-note"),
        Role::Swap => messages::text("review-swap-note"),
        Role::Root => messages::text("review-root-note"),
        Role::Data => rdisk::Use { role, format }.label(),
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
