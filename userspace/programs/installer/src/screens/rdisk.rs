// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The rdisk screen: the manual layout, as the approved design draws it
//! (ADR 0311). Each disk's partitions are listed as they are and as they
//! will be: kept, made, formatted or taken out. N makes a partition in
//! the free space, D takes one out (or keeps it again), F formats one for
//! a use, G lays out the guided partitions on the disk, U undoes the last
//! change, Tab shows the next disk, and Enter is done.
//!
//! Nothing is written here. The edits become the answers' `changes`
//! ([`Rdisk::changes`]), which the install applies as it would an answers
//! file's, and each edit is checked as it is made by applying it to the
//! disk's table in memory, with the engine rdisk(8) writes tables with
//! ([`plan::after`]): what this screen shows is what rdisk will write.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::app::size;
use crate::answers::{Change, Format, Part, Role};
use crate::disks;
use crate::machine::{DiskInfo, Machine};
use crate::messages::{self, Arg};
use crate::plan;

/// What a partition is for, as the screen offers it: a role, and the
/// filesystem of a data partition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Use {
    /// The role.
    pub role: Role,
    /// A data partition's filesystem; none leaves it empty.
    pub format: Option<Format>,
}

/// The uses the form offers, in its order.
pub const USES: [Use; 8] = [
    Use { role: Role::Root, format: None },
    Use { role: Role::Efi, format: None },
    Use { role: Role::Swap, format: None },
    Use { role: Role::BiosBoot, format: None },
    Use { role: Role::Data, format: Some(Format::Btrfs) },
    Use { role: Role::Data, format: Some(Format::Exfat) },
    Use { role: Role::Data, format: Some(Format::Fat32) },
    Use { role: Role::Data, format: None },
];

impl Use {
    /// What it is called in the form and the list.
    #[must_use]
    pub fn label(self) -> String {
        messages::text(match (self.role, self.format) {
            (Role::Root, _) => "rdisk-use-root",
            (Role::Efi, _) => "rdisk-use-efi",
            (Role::Swap, _) => "rdisk-use-swap",
            (Role::BiosBoot, _) => "rdisk-use-bios-boot",
            (Role::Data, Some(Format::Btrfs)) => "rdisk-use-btrfs",
            (Role::Data, Some(Format::Exfat)) => "rdisk-use-exfat",
            (Role::Data, Some(Format::Fat32)) => "rdisk-use-fat32",
            (Role::Data, None) => "rdisk-use-empty",
        })
    }

    /// The filesystem it gets, as the list shows it.
    fn filesystem(self) -> &'static str {
        match (self.role, self.format) {
            (Role::Root, _) | (Role::Data, Some(Format::Btrfs)) => "Btrfs",
            (Role::Efi, _) | (Role::Data, Some(Format::Fat32)) => "FAT32",
            (Role::Data, Some(Format::Exfat)) => "exFAT",
            (Role::Swap, _) => "swap",
            (Role::BiosBoot, _) | (Role::Data, None) => "none",
        }
    }

    /// Where the installed system has it: the root at `/`, the EFI
    /// partition at `/system/boot`, and a data partition where Spark
    /// mounts each, `/volumes/<partition>`.
    fn mount(self, partition: &str) -> String {
        match (self.role, self.format) {
            (Role::Root, _) => "/".to_string(),
            (Role::Efi, _) => "/system/boot".to_string(),
            (Role::Data, Some(_)) => format!("/volumes/{partition}"),
            _ => "-".to_string(),
        }
    }

    /// A new partition's name in the GPT, when none is typed.
    fn default_name(self) -> String {
        messages::text(match self.role {
            Role::Root => "review-root",
            Role::Efi => "review-efi",
            Role::Swap => "review-swap",
            Role::BiosBoot => "review-bios-boot",
            Role::Data => "rdisk-data",
        })
    }
}

/// What becomes of a partition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// It stays as it is.
    Keep,
    /// It is taken out.
    Delete,
    /// It is made, for this use.
    New(Use),
    /// It is there, and formatted for this use.
    Format(Use),
}

/// A partition, as it is and as it will be.
#[derive(Clone, Debug)]
pub struct Slot {
    /// Its number in the table.
    pub number: u32,
    /// Its first sector, in the disk's logical sectors.
    pub first: u64,
    /// How many sectors it has.
    pub sectors: u64,
    /// Its name: its GPT name, or what its type is called.
    pub name: String,
    /// What it holds now, as its type is called; empty for a new one.
    pub now: String,
    /// What becomes of it.
    pub state: State,
}

/// One disk's edits.
#[derive(Clone, Debug, Default)]
pub struct Edit {
    /// Whether a new, empty GPT replaces the disk's table.
    pub new_table: bool,
    /// Its partitions, in the order of their places on the disk.
    pub slots: Vec<Slot>,
}

impl Edit {
    /// The disk as it is: every partition kept.
    fn of(disk: &DiskInfo) -> Edit {
        let slots = disk.table.iter().flat_map(|table| &table.partitions).map(|p| Slot {
            number: p.number,
            first: p.first,
            sectors: p.sectors(),
            name: if p.name.is_empty() { p.kind.name().to_string() } else { p.name.clone() },
            now: p.kind.name().to_string(),
            state: State::Keep,
        });
        Edit { new_table: false, slots: slots.collect() }
    }

    /// Whether anything on the disk changes.
    fn changes_anything(&self) -> bool {
        self.new_table || self.slots.iter().any(|slot| slot.state != State::Keep)
    }

    /// The change of `disk` these edits make, as the answers hold it.
    #[must_use]
    pub fn change(&self, disk: &str) -> Change {
        let mut change =
            Change { disk: disk.to_string(), new_table: self.new_table, ..Change::default() };
        for slot in &self.slots {
            match slot.state {
                State::Keep => {}
                State::Delete if self.new_table => {}
                State::Delete => change.delete.push(slot.number),
                State::New(using) => change.partitions.push(Part {
                    number: slot.number,
                    first: Some(slot.first),
                    sectors: Some(slot.sectors),
                    name: Some(slot.name.clone()),
                    role: using.role,
                    format: using.format,
                }),
                State::Format(using) => change.partitions.push(Part {
                    number: slot.number,
                    first: None,
                    sectors: None,
                    name: None,
                    role: using.role,
                    format: using.format,
                }),
            }
        }
        change
    }
}

/// The form of a partition being made or formatted, over the screen.
#[derive(Clone, Debug)]
pub struct PartForm {
    /// The partition formatted, by its row; `None` for a new one.
    pub row: Option<usize>,
    /// Its size, as typed, for a new one.
    pub size: String,
    /// The size offered for all the free space, if it was: kept as it
    /// is, it takes the whole run, not the tenth of a GiB rounded off.
    pub rest: Option<String>,
    /// Its use, by its place in [`USES`].
    pub using: usize,
    /// Its name, for a new one.
    pub name: String,
    /// The field with the focus: the size, the use, the name.
    pub focus: usize,
    /// Why it cannot be done, if it cannot.
    pub problem: Option<String>,
}

/// A field of the partition form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormField {
    /// The new partition's size.
    Size,
    /// What it is for.
    Use,
    /// The new partition's name.
    Name,
}

impl PartForm {
    /// Its fields, in order: a new partition's three, or the use alone.
    #[must_use]
    pub fn field_list(&self) -> &'static [FormField] {
        if self.row.is_some() {
            &[FormField::Use]
        } else {
            &[FormField::Size, FormField::Use, FormField::Name]
        }
    }

    /// How many fields it has.
    #[must_use]
    pub fn fields(&self) -> usize {
        self.field_list().len()
    }

    /// The field with the focus.
    #[must_use]
    pub fn field(&self) -> FormField {
        self.field_list()[self.focus.min(self.fields() - 1)]
    }

    /// The use chosen.
    #[must_use]
    pub fn chosen(&self) -> Use {
        USES[self.using.min(USES.len() - 1)]
    }

    /// Chooses the use `by` places on, round the list. A new partition's
    /// name follows the use while it is the use's own.
    pub fn cycle(&mut self, by: isize) {
        let was = self.chosen().default_name();
        self.using = (self.using as isize + by).rem_euclid(USES.len() as isize) as usize;
        if self.row.is_none() && (self.name == was || self.name.is_empty()) {
            self.name = self.chosen().default_name();
        }
    }

    /// The text of the field with the focus, if it is one typed in.
    pub fn text_mut(&mut self) -> Option<&mut String> {
        match self.field() {
            FormField::Size => Some(&mut self.size),
            FormField::Name => Some(&mut self.name),
            FormField::Use => None,
        }
    }
}

/// The rdisk screen's state.
#[derive(Clone, Debug)]
pub struct Rdisk {
    /// The disk shown, by its place in the machine's list.
    pub disk: usize,
    /// The partition chosen, by its row.
    pub row: usize,
    /// Each disk's edits, in the machine's order.
    pub edits: Vec<Edit>,
    /// What was done, in words, latest last.
    pub done: Vec<String>,
    /// The edits and words before each change, for U.
    undo: Vec<(Vec<Edit>, usize)>,
}

impl Rdisk {
    /// The screen for `machine`, showing the disk `disk` first.
    #[must_use]
    pub fn new(machine: &Machine, disk: usize) -> Rdisk {
        Rdisk {
            disk,
            row: 0,
            edits: machine.disks.iter().map(Edit::of).collect(),
            done: Vec::new(),
            undo: Vec::new(),
        }
    }

    /// The changes of every disk changed, as the answers hold them.
    #[must_use]
    pub fn changes(&self, machine: &Machine) -> Vec<Change> {
        self.edits
            .iter()
            .zip(&machine.disks)
            .filter(|(edit, _)| edit.changes_anything())
            .map(|(edit, disk)| edit.change(&disk.name))
            .collect()
    }

    /// Shows the next disk that is not the install stick.
    pub fn next_disk(&mut self, machine: &Machine) {
        let count = machine.disks.len();
        for step in 1..=count {
            let at = (self.disk + step) % count;
            if !machine.disks[at].stick {
                self.disk = at;
                self.row = 0;
                return;
            }
        }
    }

    /// Moves the chosen row by `by`.
    pub fn move_row(&mut self, by: isize) {
        let count = self.edits[self.disk].slots.len();
        if count > 0 {
            self.row = (self.row as isize + by).clamp(0, count as isize - 1) as usize;
        }
    }

    /// Keeps the edits as they are, for U.
    fn remember(&mut self) {
        self.undo.push((self.edits.clone(), self.done.len()));
    }

    /// Undoes the last change.
    pub fn undo(&mut self) {
        if let Some((edits, done)) = self.undo.pop() {
            self.edits = edits;
            self.done.truncate(done);
            self.move_row(0);
        }
    }

    /// The table of the disk shown, as its edits leave it, worked out as
    /// rdisk will write it.
    fn table_after(&self, disk: &DiskInfo) -> Result<disk::Table, String> {
        let edit = &self.edits[self.disk];
        if edit.new_table || disk.table.is_none() {
            // A disk with no table gets a new GPT.
            let mut fresh = disk.clone();
            fresh.table = None;
            let change = Change { new_table: true, delete: Vec::new(), ..edit.change(&disk.name) };
            return plan::after(&fresh, &change);
        }
        plan::after(disk, &edit.change(&disk.name))
    }

    /// The free space of the disk shown, as its edits leave it: its
    /// largest run, in bytes.
    #[must_use]
    pub fn free(&self, disk: &DiskInfo) -> u64 {
        let Ok(table) = self.table_after(disk) else { return 0 };
        let align = table.alignment();
        let sectors = table
            .free()
            .into_iter()
            .map(|(first, last)| (last + 1).saturating_sub(first.div_ceil(align) * align))
            .max()
            .unwrap_or(0);
        sectors * u64::from(table.sector_size)
    }

    /// The form for a new partition on the disk shown, with what it most
    /// likely is: the EFI system partition first, then the root, then data.
    ///
    /// # Errors
    ///
    /// If the disk has no free space for one.
    pub fn new_form(&self, disk: &DiskInfo) -> Result<PartForm, String> {
        let free = self.free(disk);
        if free < 1 << 20 {
            return Err(messages::with("rdisk-no-room", &[("disk", Arg::Text(&disk.name))]));
        }
        let has = |role: Role| {
            self.edits.iter().flat_map(|edit| &edit.slots).any(|slot| {
                matches!(slot.state, State::New(using) | State::Format(using) if using.role == role)
            })
        };
        let using = if !has(Role::Efi) {
            Use { role: Role::Efi, format: None }
        } else if !has(Role::Root) {
            Use { role: Role::Root, format: None }
        } else {
            Use { role: Role::Data, format: Some(Format::Btrfs) }
        };
        let bytes = if using.role == Role::Efi { plan::EFI_BYTES.min(free) } else { free };
        Ok(PartForm {
            row: None,
            size: size_text(bytes),
            rest: (bytes == free).then(|| size_text(free)),
            using: USES.iter().position(|u| *u == using).unwrap_or(0),
            name: using.default_name(),
            focus: 0,
            problem: None,
        })
    }

    /// The form to format the chosen partition, if it is one there is.
    #[must_use]
    pub fn format_form(&self) -> Option<PartForm> {
        let slot = self.edits[self.disk].slots.get(self.row)?;
        let using = match slot.state {
            State::Keep => USES[4],
            State::Format(using) | State::New(using) => using,
            State::Delete => return None,
        };
        Some(PartForm {
            row: Some(self.row),
            size: String::new(),
            rest: None,
            using: USES.iter().position(|u| *u == using).unwrap_or(0),
            name: slot.name.clone(),
            focus: 0,
            problem: None,
        })
    }

    /// Makes or formats the partition of `form` on the disk shown.
    ///
    /// # Errors
    ///
    /// Why it cannot be done, in words the form shows.
    pub fn apply_form(&mut self, disk: &DiskInfo, form: &PartForm) -> Result<(), String> {
        let using = form.chosen();
        if let Some(row) = form.row {
            let edit = &mut self.edits[self.disk];
            let Some(slot) = edit.slots.get(row) else { return Ok(()) };
            let state = match slot.state {
                State::New(_) => State::New(using),
                _ => State::Format(using),
            };
            let said = messages::with(
                "rdisk-did-format",
                &[
                    ("number", Arg::Number(u64::from(slot.number))),
                    ("use", Arg::Text(&using.label())),
                ],
            );
            self.remember();
            self.edits[self.disk].slots[row].state = state;
            self.done.push(said);
            return Ok(());
        }
        let bytes = match disk::size::parse(&form.size) {
            _ if form.rest.as_deref() == Some(form.size.trim()) => self.free(disk),
            Some(disk::size::Amount::Bytes(bytes))
                if form.size.trim().ends_with(|c: char| c.is_ascii_alphabetic()) =>
            {
                bytes
            }
            _ => return Err(messages::text("rdisk-size-unit")),
        };
        let mut table = self.table_after(disk)?;
        let kind = disk::types::parse(plan::type_alias(using.role, using.format), table.scheme)
            .ok_or_else(|| messages::text("rdisk-not-on-mbr"))?;
        let sectors = bytes.div_ceil(u64::from(table.sector_size));
        let name = if form.name.trim().is_empty() {
            using.default_name()
        } else {
            form.name.trim().to_string()
        };
        // The EFI system partition is partition 2 where it can be, where
        // a BIOS's GRUB looks, and the root, swap and BIOS boot leave it.
        let esp = plan::ESP_NUMBER;
        let number = match using.role {
            Role::Efi if table.get(esp).is_none() => Some(esp),
            Role::Efi | Role::Data => None,
            Role::Root | Role::Swap | Role::BiosBoot => Some(plan::free_number(&table, Some(esp))),
        };
        let end = disk::End::Sectors(sectors);
        let new = disk::New { name: name.clone(), number, ..disk::New::of(kind, end) };
        let number = table.add(new, &mut disk::Rng::seeded(1)).map_err(|err| err.0)?;
        let made = table.get(number).ok_or("the partition just made is gone")?;
        let made_bytes = made.sectors() * u64::from(table.sector_size);
        let slot = Slot {
            number,
            first: made.first,
            sectors: made.sectors(),
            name: name.clone(),
            now: String::new(),
            state: State::New(using),
        };
        self.remember();
        let edit = &mut self.edits[self.disk];
        if disk.table.is_none() {
            edit.new_table = true;
        }
        let at = edit.slots.iter().position(|s| s.first > slot.first).unwrap_or(edit.slots.len());
        edit.slots.insert(at, slot);
        self.row = at;
        self.done.push(messages::with(
            "rdisk-did-new",
            &[
                ("name", Arg::Text(&name)),
                ("disk", Arg::Text(&disk.name)),
                ("size", Arg::Text(&size(made_bytes))),
            ],
        ));
        Ok(())
    }

    /// Takes out the chosen partition, or keeps it again if it was taken
    /// out; a new one is dropped.
    ///
    /// # Errors
    ///
    /// Why it cannot be kept again: a new partition uses its space, or
    /// G made a new table.
    pub fn delete(&mut self) -> Result<(), String> {
        let edit = &self.edits[self.disk];
        let Some(slot) = edit.slots.get(self.row).cloned() else { return Ok(()) };
        let state = match slot.state {
            State::Delete if edit.new_table => return Err(messages::text("rdisk-new-table")),
            State::Delete => {
                let end = slot.first + slot.sectors;
                let overlaps = edit.slots.iter().any(|other| {
                    matches!(other.state, State::New(_))
                        && other.first < end
                        && slot.first < other.first + other.sectors
                });
                if overlaps {
                    return Err(messages::text("rdisk-space-used"));
                }
                Some(State::Keep)
            }
            State::New(_) => None,
            State::Keep | State::Format(_) => Some(State::Delete),
        };
        let number = Arg::Number(u64::from(slot.number));
        let said = match state {
            Some(State::Keep) => messages::with("rdisk-did-keep", &[("number", number)]),
            Some(_) => messages::with(
                "rdisk-did-delete",
                &[("number", number), ("name", Arg::Text(&slot.name))],
            ),
            None => messages::with("rdisk-did-drop", &[("name", Arg::Text(&slot.name))]),
        };
        self.remember();
        let edit = &mut self.edits[self.disk];
        match state {
            Some(state) => edit.slots[self.row].state = state,
            None => {
                edit.slots.remove(self.row);
                self.move_row(0);
            }
        }
        self.done.push(said);
        Ok(())
    }

    /// Lays out the guided partitions on the disk shown, with swap of
    /// `swap_gib`: a new table, every partition it had taken out.
    ///
    /// # Errors
    ///
    /// If the disk is too small for them.
    pub fn guided(&mut self, disk: &DiskInfo, swap_gib: u32) -> Result<(), String> {
        let change = plan::guided(disk, swap_gib)?;
        self.remember();
        let edit = &mut self.edits[self.disk];
        edit.new_table = true;
        edit.slots.retain(|slot| !matches!(slot.state, State::New(_)));
        for slot in &mut edit.slots {
            slot.state = State::Delete;
        }
        for part in change.partitions {
            let using = Use { role: part.role, format: part.format };
            edit.slots.push(Slot {
                number: part.number,
                first: part.first.unwrap_or_default(),
                sectors: part.sectors.unwrap_or_default(),
                name: part.name.unwrap_or_else(|| using.default_name()),
                now: String::new(),
                state: State::New(using),
            });
        }
        edit.slots.sort_by_key(|slot| (matches!(slot.state, State::New(_)), slot.first));
        self.row = 0;
        self.done.push(messages::with("rdisk-did-guided", &[("disk", Arg::Text(&disk.name))]));
        Ok(())
    }

    /// Checks the edits before Enter leaves the screen, and finds the
    /// disk that holds the root, the install's target.
    ///
    /// # Errors
    ///
    /// What is missing, in words the screen shows.
    pub fn done(&self, machine: &Machine, root_bytes: u64) -> Result<usize, String> {
        let uses = |at: usize, role: Role| {
            self.edits[at].slots.iter().find(|slot| {
                matches!(slot.state, State::New(using) | State::Format(using) if using.role == role)
            })
        };
        let roots: Vec<usize> =
            (0..self.edits.len()).filter(|&at| uses(at, Role::Root).is_some()).collect();
        let [target] = roots[..] else {
            return Err(messages::text(if roots.is_empty() {
                "rdisk-need-root"
            } else {
                "rdisk-one-root"
            }));
        };
        let disk = &machine.disks[target];
        let name = Arg::Text(&disk.name);
        if uses(target, Role::Efi).is_none() {
            return Err(messages::with("rdisk-need-efi", &[("disk", name)]));
        }
        for (at, edit) in self.edits.iter().enumerate().filter(|(_, edit)| edit.changes_anything())
        {
            let other = &machine.disks[at];
            let after = plan::after(other, &edit.change(&other.name))?;
            if at == target && plan::check_bios_boot(&other.name, &after).is_err() {
                let name = Arg::Text(&other.name);
                return Err(messages::with("rdisk-efi-second", &[("disk", name)]));
            }
        }
        let edit = &self.edits[target];
        let gpt =
            edit.new_table || disk.table.as_ref().is_none_or(|t| t.scheme == disk::Scheme::Gpt);
        if !gpt {
            return Err(messages::with("rdisk-need-gpt", &[("disk", name)]));
        }
        let root =
            uses(target, Role::Root).map_or(0, |slot| slot.sectors * u64::from(disk.sector_size));
        if root < root_bytes {
            return Err(messages::with(
                "rdisk-root-small",
                &[("size", Arg::Text(&size(root_bytes)))],
            ));
        }
        let elsewhere = self.edits.iter().enumerate().any(|(at, edit)| {
            at != target
                && edit.slots.iter().any(|slot| {
                    matches!(slot.state, State::New(u) | State::Format(u)
                        if matches!(u.role, Role::Efi | Role::Swap))
                })
        });
        if elsewhere {
            return Err(messages::with("rdisk-on-target", &[("disk", name)]));
        }
        Ok(target)
    }
}

/// A size as the form's box takes it back: whole MiB, or GiB with a
/// tenth.
fn size_text(bytes: u64) -> String {
    if bytes < 1 << 30 {
        format!("{} MiB", bytes >> 20)
    } else {
        // Rounded down, so that it fits the space it came from.
        format!("{}.{} GiB", bytes >> 30, ((bytes % (1 << 30)) * 10) >> 30)
    }
}

/// The colours of the screens' design, as draw.rs has them.
const ACCENT: Color = Color::Indexed(173);
const DIM: Color = Color::Indexed(245);
const WARNING: Color = Color::Indexed(179);
const DANGER: Color = Color::Indexed(210);
const KEPT: Color = Color::Indexed(60);

/// Draws the rdisk screen's rows in `area`.
pub fn draw(frame: &mut Frame<'_>, rdisk: &Rdisk, machine: &Machine, area: Rect) {
    let width = usize::from(area.width);
    let disk = &machine.disks[rdisk.disk];
    let edit = &rdisk.edits[rdisk.disk];
    let mut lines = vec![tabs(rdisk, machine), Line::raw("")];
    let table = match (&disk.table, edit.new_table) {
        (Some(table), false) => table.scheme.name().to_uppercase(),
        _ => messages::text("rdisk-new-gpt"),
    };
    let model = if disk.model.is_empty() { String::new() } else { format!("{} · ", disk.model) };
    lines.push(Line::from(Span::styled(format!("{model}{table}"), Style::new().fg(DIM))));
    lines.push(bar(edit, disk, width));
    lines.push(Line::raw(""));
    let header = format!(
        "  {:<3}{:<21}{:<22}{:>9}  {:<20}{}",
        "#",
        messages::text("rdisk-name"),
        messages::text("rdisk-filesystem"),
        messages::text("rdisk-size"),
        messages::text("rdisk-mount"),
        messages::text("rdisk-change")
    );
    lines.push(Line::from(Span::styled(header, Style::new().add_modifier(Modifier::BOLD))));
    let shown = usize::from(area.height).saturating_sub(13).max(3);
    let top = rdisk.row.saturating_sub(shown - 1);
    for (i, slot) in edit.slots.iter().enumerate().skip(top).take(shown) {
        lines.push(slot_line(slot, i == rdisk.row, disk, width));
    }
    if edit.slots.is_empty() {
        lines.push(Line::from(Span::styled(messages::text("rdisk-empty"), Style::new().fg(DIM))));
    }
    let free = rdisk.free(disk);
    if free >= 1 << 20 {
        let text = format!("     {:<43}{:>9}", messages::text("rdisk-free"), size(free));
        let text: String = text.chars().take(width).collect();
        lines.push(Line::from(Span::styled(text, Style::new().fg(DIM))));
    }
    lines.push(Line::raw(""));
    let count = Arg::Number(rdisk.done.len() as u64);
    lines.push(Line::from(Span::styled(
        messages::with("rdisk-pending", &[("count", count)]),
        Style::new().add_modifier(Modifier::BOLD),
    )));
    let skip = rdisk.done.len().saturating_sub(3);
    for (i, said) in rdisk.done.iter().enumerate().skip(skip) {
        let text: String = format!("  {}  {said}", i + 1).chars().take(width).collect();
        lines.push(Line::from(Span::styled(text, Style::new().fg(ACCENT))));
    }
    lines.push(Line::from(Span::styled(messages::text("rdisk-nothing-yet"), Style::new().fg(DIM))));
    frame.render_widget(Paragraph::new(lines), area);
}

/// The disks, the one shown highlighted.
fn tabs(rdisk: &Rdisk, machine: &Machine) -> Line<'static> {
    let mut spans = vec![Span::raw(format!("{}  ", messages::text("rdisk-disks")))];
    for (i, disk) in machine.disks.iter().enumerate().filter(|(_, disk)| !disk.stick) {
        let text = format!(" {} {} ", disk.name, size(disk.bytes));
        let style = if i == rdisk.disk {
            Style::new().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        spans.extend([Span::styled(text, style), Span::raw(" ")]);
    }
    spans.push(Span::styled(messages::text("rdisk-tab"), Style::new().fg(DIM)));
    Line::from(spans)
}

/// The disk drawn as a bar `width` wide: each partition that stays, by
/// its share of the disk, named where it has room; free space dotted.
fn bar(edit: &Edit, disk: &DiskInfo, width: usize) -> Line<'static> {
    let total = disk.sectors().max(1);
    let mut spans = Vec::new();
    let mut used = 0;
    let mut slots: Vec<&Slot> = edit.slots.iter().filter(|s| s.state != State::Delete).collect();
    slots.sort_by_key(|slot| slot.first);
    let mut at = 0;
    for slot in slots {
        let gap =
            ((slot.first.saturating_sub(at)) as u128 * width as u128 / total as u128) as usize;
        if gap > 0 && used + gap <= width {
            spans.push(Span::styled(".".repeat(gap), Style::new().fg(DIM)));
            used += gap;
        }
        let cells = ((slot.sectors as u128 * width as u128 / total as u128) as usize).max(1);
        let cells = cells.min(width - used);
        if cells == 0 {
            break;
        }
        let colour = match slot.state {
            State::New(_) => ACCENT,
            State::Format(_) => WARNING,
            _ => KEPT,
        };
        let mut text: String = slot.name.chars().take(cells.saturating_sub(1)).collect();
        text.insert(0, ' ');
        let text = format!("{text:<cells$}").chars().take(cells).collect::<String>();
        spans.push(Span::styled(text, Style::new().fg(Color::Black).bg(colour)));
        used += cells;
        at = slot.first + slot.sectors;
    }
    if used < width {
        spans.push(Span::styled(".".repeat(width - used), Style::new().fg(DIM)));
    }
    Line::from(spans)
}

/// A partition's line in the list: the filesystem and mount it will
/// have, or the type it has now if it is kept.
fn slot_line(slot: &Slot, chosen: bool, disk: &DiskInfo, width: usize) -> Line<'static> {
    let bytes = slot.sectors * u64::from(disk.sector_size);
    let partition = disks::partition(&disk.name, slot.number);
    let now = || (slot.now.clone(), "-".to_string());
    let will = |using: Use| (using.filesystem().to_string(), using.mount(&partition));
    let ((filesystem, mount), change, colour) = match slot.state {
        State::Keep => (now(), "rdisk-keep", None),
        State::Delete => (now(), "rdisk-delete", Some(DANGER)),
        State::New(using) => (will(using), "rdisk-new", Some(ACCENT)),
        State::Format(using) => (will(using), "rdisk-format", Some(WARNING)),
    };
    let mark = if chosen { '>' } else { ' ' };
    let text = format!(
        "{mark} {:<3}{:<21}{:<22}{:>9}  {:<20}{}",
        slot.number,
        cut(&slot.name, 20),
        cut(&filesystem, 21),
        size(bytes),
        cut(&mount, 19),
        messages::text(change)
    );
    let text: String = text.chars().take(width).collect();
    let style = match (chosen, colour) {
        (true, _) => Style::new().fg(Color::Black).bg(ACCENT),
        (false, Some(colour)) => Style::new().fg(colour),
        (false, None) => Style::new(),
    };
    Line::from(Span::styled(text, style))
}

/// `text` cut to `most` characters.
fn cut(text: &str, most: usize) -> String {
    text.chars().take(most).collect()
}
