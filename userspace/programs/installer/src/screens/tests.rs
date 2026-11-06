// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The screens driven by keys, as a person drives them, on a computer
//! made by hand, and drawn on a test terminal of the design's 100 by 31.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, Id, Modal, Outcome, Page, SetupFile, user_name};
use super::draw::{self, wrap};
use crate::answers::{Answers, Layout, Network};
use crate::log::{Event, Step};
use crate::machine::{Card, DiskInfo, Machine};

/// A GPT on a disk of `gib` GiB, with a partition of each `(alias, MiB)`
/// in turn.
fn table(gib: u64, parts: &[(&str, u64)]) -> disk::Table {
    let mut rng = disk::Rng::seeded(3);
    let mut table = disk::Table::new(disk::Scheme::Gpt, 512, (gib << 30) / 512, &mut rng).unwrap();
    for (alias, mib) in parts {
        let kind = disk::types::parse(alias, disk::Scheme::Gpt).unwrap();
        let end = disk::End::Sectors(mib << 11);
        let new = disk::New { name: (*alias).to_string(), ..disk::New::of(kind, end) };
        table.add(new, &mut rng).unwrap();
    }
    table
}

/// A computer with a Windows SSD, the install stick, a blank disk, a
/// disk with a data partition and room beside it, and a network card.
fn machine() -> Machine {
    let disk = |name: &str, gib: u64, model: &str, table: Option<disk::Table>, stick: bool| {
        let partitions = table.iter().flat_map(|t| &t.partitions);
        DiskInfo {
            name: name.into(),
            bytes: gib << 30,
            model: model.into(),
            rotational: Some(false),
            partitions: partitions.map(|p| p.kind.to_string().to_lowercase()).collect(),
            stick,
            sector_size: 512,
            table,
        }
    };
    let windows =
        table(128, &[("efi", 100), ("windows-reserved", 16), ("windows-data", 100 << 10)]);
    Machine {
        disks: vec![
            disk("sda", 128, "SK hynix SC311", Some(windows), false),
            disk("sdb", 16, "SanDisk Ultra", Some(table(16, &[("dukovo-live", 2048)])), true),
            disk("vda", 4, "", None, false),
            disk("vdb", 16, "", Some(table(16, &[("linux", 4096)])), false),
        ],
        cards: vec![Card {
            name: "net0".into(),
            model: "Intel 82574L".into(),
            carrier: true,
            speed: Some(1000),
            addresses: vec!["10.0.2.15/24".into()],
        }],
        memory: 1 << 30,
        layouts: vec![("us".into(), "English (US)".into()), ("de".into(), "German".into())],
        zones: vec!["UTC".into(), "America/Denver".into(), "Europe/Berlin".into()],
    }
}

fn app() -> App {
    App::new(machine(), "0.14.0", 600 << 20)
}

/// What the screens show, a line a row.
fn screen(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(100, 31)).unwrap();
    terminal.draw(|frame| draw::draw(frame, app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut text = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    assert_no_missing_messages(&text);
    text
}

/// Fails if `text` shows a message the messages file lacks: its id in
/// brackets, `[page-welcome]`.
fn assert_no_missing_messages(text: &str) {
    for (start, _) in text.match_indices('[') {
        let id: String =
            text[start + 1..].chars().take_while(|c| c.is_ascii_lowercase() || *c == '-').collect();
        let closed = text[start + 1 + id.len()..].starts_with(']');
        assert!(!(closed && id.contains('-')), "a message is missing: [{id}]\n{text}");
    }
}

fn press(app: &mut App, code: KeyCode) -> Outcome {
    app.key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
}

/// Focuses the field `id`.
fn focus(app: &mut App, id: Id) {
    app.focus = app
        .focusable()
        .iter()
        .position(|f| *f == id)
        .unwrap_or_else(|| panic!("{id:?} is not on {:?}", app.page));
}

/// Answers each page as a person would, up to the review.
fn to_review(app: &mut App) {
    press(app, KeyCode::Enter);
    assert_eq!(app.page, Page::Network);
    focus(app, Id::Hostname);
    for _ in 0.."dukovo".len() {
        press(app, KeyCode::Backspace);
    }
    type_text(app, "web01");
    focus(app, Id::Zone);
    press(app, KeyCode::Char('a'));
    press(app, KeyCode::Enter);
    assert_eq!(app.page, Page::Disk, "{:?}", app.problem);
    focus(app, Id::Disk(2));
    press(app, KeyCode::Char(' '));
    press(app, KeyCode::Enter);
    assert_eq!(app.page, Page::You, "{:?}", app.problem);
    type_text(app, "Alice Liddell");
    press(app, KeyCode::Tab);
    press(app, KeyCode::Tab);
    type_text(app, "queen of hearts");
    press(app, KeyCode::Tab);
    type_text(app, "queen of hearts");
    press(app, KeyCode::Enter);
    assert_eq!(app.page, Page::Review, "{:?}", app.problem);
}

#[test]
fn every_step_is_answered_by_keys_and_the_answers_check() {
    let mut app = app();
    let shown = screen(&app);
    assert!(shown.contains("Install Dukovo 0.14.0"), "{shown}");
    assert!(shown.contains("Step 1 of 5 · Welcome"), "{shown}");
    assert!(shown.contains("‹ English (US) ›"), "{shown}");
    assert!(shown.contains("< Next >"), "{shown}");
    to_review(&mut app);
    let shown = screen(&app);
    for needle in [
        "Erase     vda",
        "Dukovo root",
        "web01 · net0 DHCP · America/Denver · keyboard us",
        "alice (Alice Liddell), administrator",
        "Untouched sda",
        "Type vda to confirm",
    ] {
        assert!(shown.contains(needle), "{needle}:\n{shown}");
    }
    assert!(
        !shown.contains("sdb  SanDisk"),
        "the stick is not untouched, it is not a target:\n{shown}"
    );
    // Install asks for the disk's name first.
    focus(&mut app, Id::Install);
    assert_eq!(press(&mut app, KeyCode::Enter), Outcome::Stay);
    assert!(app.problem.as_deref().is_some_and(|p| p.contains("Type vda")));
    assert_eq!(app.focused(), Some(Id::Confirm));
    type_text(&mut app, "vda");
    assert_eq!(press(&mut app, KeyCode::Enter), Outcome::Install);
    assert_eq!(app.page, Page::Installing);
    let answers = app.answers().unwrap();
    answers.check().unwrap();
    assert_eq!(answers.target(), "vda");
    assert_eq!(answers.disk.layout, Layout::Guided);
    assert_eq!(answers.disk.swap_gib, 1);
    assert_eq!(answers.hostname(), "web01");
    assert_eq!(answers.time_zone(), "America/Denver");
    assert_eq!(answers.keyboard, "us");
    assert_eq!(
        (answers.admin.name.as_str(), answers.admin.user.as_str()),
        ("Alice Liddell", "alice")
    );
    assert!(matches!(answers.network.get("net0"), Some(Network::Dhcp(word)) if word == "dhcp"));
    assert_eq!(app.passwords(), ["queen of hearts"]);
    app.forget_passwords();
    assert_eq!(app.passwords(), [""]);
}

#[test]
fn a_note_too_long_for_its_line_goes_under_its_field() {
    let mut app = app();
    press(&mut app, KeyCode::Enter);
    let shown = screen(&app);
    let note = "Left and Right, or type the start of its name";
    let line = shown.lines().find(|line| line.contains(note)).unwrap_or_else(|| panic!("{shown}"));
    assert!(!line.contains("Time zone"), "{shown}");
    // A note that fits stays beside its field.
    assert!(
        shown
            .lines()
            .any(|line| line.contains("Automatic (DHCP)") && line.contains("got 10.0.2.15/24"))
    );
    press(&mut app, KeyCode::Esc);
    assert!(screen(&app).contains("more languages come in a later release"));
}

#[test]
fn a_zone_is_found_by_the_start_of_its_name() {
    let mut app = app();
    let zones = ["UTC", "Africa/Abidjan", "America/Danmarkshavn", "America/Denver", "Asia/Tokyo"];
    app.machine.zones = zones.map(String::from).to_vec();
    press(&mut app, KeyCode::Enter);
    focus(&mut app, Id::Zone);
    type_text(&mut app, "america/de");
    assert_eq!(app.machine.zones[app.zone], "America/Denver");
    // Left starts the letters again.
    press(&mut app, KeyCode::Left);
    assert_eq!(app.machine.zones[app.zone], "America/Danmarkshavn");
    type_text(&mut app, "as");
    assert_eq!(app.machine.zones[app.zone], "Asia/Tokyo");
    // One letter steps through the names it starts.
    press(&mut app, KeyCode::Right);
    type_text(&mut app, "a");
    assert_eq!(app.machine.zones[app.zone], "Africa/Abidjan");
}

#[test]
fn each_page_refuses_what_an_answers_file_would() {
    let mut app = app();
    press(&mut app, KeyCode::Enter);
    focus(&mut app, Id::Hostname);
    type_text(&mut app, " box");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::Network);
    assert!(
        app.problem.as_deref().is_some_and(|p| p.starts_with("hostname:")),
        "{:?}",
        app.problem
    );
    assert!(screen(&app).contains("! hostname:"));
    // A fixed address must be one.
    focus(&mut app, Id::Hostname);
    for _ in 0..4 {
        press(&mut app, KeyCode::Backspace);
    }
    focus(&mut app, Id::Fixed);
    press(&mut app, KeyCode::Char(' '));
    focus(&mut app, Id::Address);
    type_text(&mut app, "10.0.0.20");
    press(&mut app, KeyCode::Enter);
    assert!(
        app.problem.as_deref().is_some_and(|p| p.starts_with("network.net0:")),
        "{:?}",
        app.problem
    );
    type_text(&mut app, "/24");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::Disk, "{:?}", app.problem);
    // The stick cannot be chosen, and swap is at most 64 GiB.
    assert!(!app.focusable().contains(&Id::Disk(1)));
    assert!(screen(&app).contains("the install stick, can't be chosen"));
    focus(&mut app, Id::Swap);
    type_text(&mut app, "00");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::Disk);
    assert!(app.problem.as_deref().is_some_and(|p| p.contains("disk")), "{:?}", app.problem);
    for _ in 0..2 {
        press(&mut app, KeyCode::Backspace);
    }
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::You, "{:?}", app.problem);
    // Passwords typed differently are refused, and a name must be given.
    type_text(&mut app, "Alice");
    focus(&mut app, Id::Password);
    type_text(&mut app, "abc");
    let shown = screen(&app);
    assert!(shown.contains("This password is short"), "{shown}");
    assert!(shown.contains("•••") && !shown.contains("abc"), "{shown}");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::You);
    assert!(app.problem.as_deref().is_some_and(|p| p.contains("differ")));
    focus(&mut app, Id::Repeat);
    type_text(&mut app, "abc");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::Review, "{:?}", app.problem);
    // Esc goes back, keeping the answers.
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.page, Page::You);
    assert_eq!(app.user, "alice");
}

#[test]
fn the_disk_page_warns_before_windows_is_erased() {
    let mut app = app();
    app.page = Page::Disk;
    app.focus = 0;
    let shown = screen(&app);
    assert!(shown.contains("Windows, 3 partitions"), "{shown}");
    assert!(shown.contains("Erasing sda deletes Windows and everything on it."), "{shown}");
    assert!(shown.contains("vda, vdb stay untouched"), "{shown}");
    focus(&mut app, Id::Disk(2));
    press(&mut app, KeyCode::Char(' '));
    let shown = screen(&app);
    assert!(!shown.contains("Erasing"), "{shown}");
    assert!(shown.contains("sda, vdb stay untouched"), "{shown}");
}

/// Goes on from the Disk page, with `disk` and the layout `layout`
/// chosen.
fn choose_layout(app: &mut App, disk: usize, layout: Id) {
    app.page = Page::Disk;
    focus(app, Id::Disk(disk));
    press(app, KeyCode::Char(' '));
    focus(app, layout);
    press(app, KeyCode::Char(' '));
    press(app, KeyCode::Enter);
}

/// Answers the You page, so that the review shows.
fn answer_you(app: &mut App) {
    assert_eq!(app.page, Page::You, "{:?}", app.problem);
    type_text(app, "Alice Liddell");
    press(app, KeyCode::Enter);
    assert_eq!(app.page, Page::Review, "{:?}", app.problem);
}

#[test]
fn the_free_space_layout_keeps_the_partitions_there() {
    let mut app = app();
    app.page = Page::Disk;
    // A disk that starts Windows is not for it.
    focus(&mut app, Id::Disk(0));
    press(&mut app, KeyCode::Char(' '));
    let shown = screen(&app);
    assert!(shown.contains("sda starts a system already"), "{shown}");
    assert!(!app.focusable().contains(&Id::Free), "it cannot be chosen");
    choose_layout(&mut app, 3, Id::Free);
    answer_you(&mut app);
    let shown = screen(&app);
    for needle in [
        "Add to    vdb    (its partition is kept)",
        "3  BIOS boot",
        "2  EFI system        512 MiB   FAT32",
        "5  Dukovo root",
        "GRUB for UEFI and BIOS",
        "Untouched sda",
    ] {
        assert!(shown.contains(needle), "{needle}:\n{shown}");
    }
    assert!(!shown.contains("Erase"), "{shown}");
    let answers = app.answers().unwrap();
    answers.check().unwrap();
    assert_eq!((answers.disk.layout, answers.target()), (Layout::Free, "vdb"));
    focus(&mut app, Id::Install);
    press(&mut app, KeyCode::Enter);
    assert!(app.problem.as_deref().is_some_and(|p| p.contains("confirm the changes")));
}

#[test]
fn the_manual_layout_is_chosen_on_the_rdisk_screen() {
    let mut app = app();
    choose_layout(&mut app, 3, Id::Manual);
    assert_eq!(app.page, Page::Rdisk, "{:?}", app.problem);
    let shown = screen(&app);
    for needle in [
        "Step 3 of 5 · Partitions (rdisk)",
        " vdb 16.0 GiB ",
        "#  Name",
        "1  linux                Linux filesystem",
        "keep",
        "Free space",
        "Pending changes: 0",
        "N new · D delete",
    ] {
        assert!(shown.contains(needle), "{needle}:\n{shown}");
    }
    assert!(!shown.contains(" sdb "), "the stick is not a tab:\n{shown}");
    // Done with no root says what is missing.
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::Rdisk);
    assert!(app.problem.as_deref().is_some_and(|p| p.contains("Dukovo root")));
    // D takes the partition out; D again keeps it; U undoes.
    press(&mut app, KeyCode::Char('d'));
    assert!(screen(&app).contains("DELETE"));
    press(&mut app, KeyCode::Char('u'));
    assert!(!screen(&app).contains("DELETE"));
    // F formats it for data, Right choosing exFAT.
    press(&mut app, KeyCode::Char('f'));
    assert!(matches!(app.modal, Some(Modal::Partition(_))));
    assert!(screen(&app).contains("‹ Data: Btrfs ›"));
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none());
    let shown = screen(&app);
    assert!(shown.contains("exFAT") && shown.contains("/volumes/vdb1"), "{shown}");
    // N offers the EFI system partition first, then the root in the rest.
    press(&mut app, KeyCode::Char('n'));
    let shown = screen(&app);
    assert!(shown.contains("New partition") && shown.contains("‹ EFI system"), "{shown}");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('n'));
    assert!(screen(&app).contains("‹ Dukovo root"));
    // Left and Right choose the use from the size's box too.
    press(&mut app, KeyCode::Right);
    assert!(screen(&app).contains("‹ EFI system"));
    press(&mut app, KeyCode::Left);
    // A size needs its unit.
    for _ in 0..12 {
        press(&mut app, KeyCode::Backspace);
    }
    type_text(&mut app, "4");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(&app.modal, Some(Modal::Partition(form)) if form.problem.is_some()));
    type_text(&mut app, " GiB");
    press(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none(), "{:?}", app.modal);
    let shown = screen(&app);
    assert!(shown.contains("Pending changes: 3"), "{shown}");
    assert!(shown.contains("make Dukovo root on vdb, 4.0 GiB"), "{shown}");
    press(&mut app, KeyCode::Enter);
    answer_you(&mut app);
    let shown = screen(&app);
    for needle in [
        "Change    vdb",
        "format  1  linux",
        "make    2  EFI system",
        "make    3  Dukovo root",
        "GRUB for UEFI (the disk has no BIOS boot partition)",
        "Type vdb to confirm",
    ] {
        assert!(shown.contains(needle), "{needle}:\n{shown}");
    }
    let answers = app.answers().unwrap();
    answers.check().unwrap();
    assert_eq!((answers.disk.layout, answers.target()), (Layout::Manual, "vdb"));
    assert_eq!(answers.disk.changes.len(), 1);
    assert_eq!(answers.disk.changes[0].partitions.len(), 3);
    // Back from You is the rdisk screen, with its edits.
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.page, Page::Rdisk);
    assert!(screen(&app).contains("Pending changes: 3"));
}

#[test]
fn g_lays_out_the_guided_partitions_on_a_blank_disk() {
    let mut app = app();
    choose_layout(&mut app, 2, Id::Manual);
    assert_eq!(app.page, Page::Rdisk);
    assert!(screen(&app).contains("no partition table yet"));
    press(&mut app, KeyCode::F(1));
    assert!(screen(&app).contains("The same disk engine as rdisk"));
    press(&mut app, KeyCode::Esc);
    assert!(app.modal.is_none());
    press(&mut app, KeyCode::Char('g'));
    let shown = screen(&app);
    assert!(shown.contains("guided layout on vda"), "{shown}");
    assert!(shown.contains("BIOS boot") && shown.contains("/system/boot"), "{shown}");
    press(&mut app, KeyCode::Enter);
    answer_you(&mut app);
    let answers = app.answers().unwrap();
    answers.check().unwrap();
    assert_eq!(answers.target(), "vda");
    assert!(answers.disk.changes[0].new_table);
    assert!(screen(&app).contains("GRUB for UEFI and BIOS"));
}

#[test]
fn other_people_are_added_and_removed() {
    let mut app = app();
    app.page = Page::You;
    type_text(&mut app, "Alice Liddell");
    focus(&mut app, Id::AddPerson);
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.modal, Some(Modal::Person(_))));
    assert!(screen(&app).contains("Add a person"));
    // The administrator's name is taken.
    type_text(&mut app, "Alice Again");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(&app.modal, Some(Modal::Person(form)) if form.problem.is_some()));
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "Bob Smith");
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab);
    type_text(&mut app, "builder");
    press(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none());
    assert!(screen(&app).contains("bob (Bob Smith)"));
    assert_eq!(app.passwords(), ["", "builder"]);
    let answers = app.answers().unwrap();
    assert_eq!(answers.users[0].user, "bob");
    focus(&mut app, Id::Person(0));
    press(&mut app, KeyCode::Delete);
    assert!(app.people.is_empty());
}

#[test]
fn a_setup_file_is_reviewed_before_it_installs() {
    let mut app = app();
    let text = r#"{"version": 1, "disk": {"layout": "guided", "target": "vda"},
        "admin": {"name": "Admin", "user": "admin", "password_hash": ""}}"#;
    let answers = Answers::parse(text.as_bytes()).unwrap();
    app.setup_files =
        vec![SetupFile { place: "install.json on this USB stick".into(), answers: Ok(answers) }];
    focus(&mut app, Id::FromFile);
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::SetupFile);
    let shown = screen(&app);
    assert!(shown.contains("(•) install.json on this USB stick"), "{shown}");
    assert!(shown.contains("found, and every answer checks"), "{shown}");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::Review);
    let shown = screen(&app);
    assert!(shown.contains("admin (Admin), administrator"), "{shown}");
    type_text(&mut app, "vda");
    assert_eq!(press(&mut app, KeyCode::Enter), Outcome::Install);
    // A setup file's passwords are its hashes; none are typed.
    assert!(app.loaded.is_some());
    // Back from its review goes back to the setup file.
    app.page = Page::Review;
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.page, Page::SetupFile);
    assert!(app.loaded.is_none());
}

#[test]
fn the_progress_follows_the_log_to_the_end() {
    let mut app = app();
    to_review(&mut app);
    type_text(&mut app, "vda");
    press(&mut app, KeyCode::Enter);
    for step in
        [Step::Release, Step::Disk, Step::Partition, Step::Format, Step::Subvolumes, Step::Copy]
    {
        app.told(Event::Step(step));
    }
    app.told(Event::Copied { done: 50, total: 100 });
    app.told(Event::Line("$ subvolume create /tmp/install/root/@users".into()));
    assert_eq!(app.progress.percent(), 6 + 1 + 2 + 8 + 2 + 35);
    let shown = screen(&app);
    for needle in [
        "√  Partitioning vda",
        "•  Copying Dukovo 0.14.0 into its slot",
        "○  Installing GRUB",
        "54%",
        "$ subvolume create",
    ] {
        assert!(shown.contains(needle), "{needle}:\n{shown}");
    }
    // Keys other than L do nothing while it runs.
    assert_eq!(press(&mut app, KeyCode::Enter), Outcome::Stay);
    app.finished(Ok("Dukovo 0.14.0 is installed on vda.".into()));
    assert_eq!(app.page, Page::Done);
    let shown = screen(&app);
    for needle in [
        "Dukovo is installed",
        "100%",
        "√  Saving the answers",
        "Remove the install stick, then restart.",
        "< Restart >",
    ] {
        assert!(shown.contains(needle), "{needle}:\n{shown}");
    }
    assert_eq!(press(&mut app, KeyCode::Enter), Outcome::Restart);
}

#[test]
fn a_failed_install_says_why_and_where_the_log_is() {
    let mut app = app();
    app.page = Page::Installing;
    app.told(Event::Step(Step::Release));
    app.told(Event::Step(Step::Disk));
    app.finished(Err("disk.target: vda holds 512 MiB, and this install needs 2000 MiB".into()));
    assert_eq!(app.page, Page::Failed);
    let shown = screen(&app);
    assert!(shown.contains("!  Checking"), "{shown}");
    assert!(shown.contains("vda holds 512 MiB"), "{shown}");
    assert!(shown.contains("/tmp/install.log"), "{shown}");
}

#[test]
fn every_page_has_its_help() {
    let mut app = app();
    for page in [
        Page::Welcome,
        Page::SetupFile,
        Page::Network,
        Page::Disk,
        Page::You,
        Page::Review,
        Page::Done,
    ] {
        app.page = page;
        app.focus = 0;
        press(&mut app, KeyCode::F(1));
        assert!(matches!(app.modal, Some(Modal::Help(_))));
        let shown = screen(&app);
        assert!(shown.contains(" Help "), "{page:?}:\n{shown}");
        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
    }
}

#[test]
fn the_screens_fit_a_small_console() {
    let app = app();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| draw::draw(frame, &app)).unwrap();
}

#[test]
fn names_make_user_names_and_text_wraps() {
    assert_eq!(user_name("Alice Liddell"), "alice");
    assert_eq!(user_name("  Zoë O'Brien"), "zo");
    assert_eq!(user_name(""), "");
    assert_eq!(wrap("one two three", 7), ["one two", "three"]);
    assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    assert_eq!(wrap("  one two", 7), ["  one", "  two"]);
}
