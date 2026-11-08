// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the install does to the disks, worked out before anything is
//! written (ADR 0311). Every layout comes down to a [`Change`] of each
//! disk it touches: the partitions it makes, formats and takes out. The
//! guided layout is a new table; the free-space layout adds Dukovo's
//! partitions in the largest free space of a disk that keeps the rest;
//! the layout next to Windows shrinks Windows' partition, once its volume
//! is shrunk, and adds them in the space it gives back (ADR 0314); the
//! manual layout is the changes made on the rdisk screen.
//!
//! [`after`] applies a change to the disk's table in memory, with the
//! disk engine rdisk(8) itself uses, from the layout file
//! (rdisk-layout(5)) that [`layout`] makes of it: a change that does not
//! fit is refused before anything is written, and the table the screens
//! show is the table rdisk writes. Every new partition's place is given
//! in sectors, so nothing is left for rdisk to choose.

use disk::layout::{Layout as LayoutFile, Part as LayoutPart, Resize};
use disk::{End, New, Rng, Scheme, Table, types};

use crate::answers::{Change, Format, Part, Role, Shrink};
use crate::machine::DiskInfo;
use crate::windows::Windows;

/// A mebibyte.
const MIB: u64 = 1 << 20;

/// The size of the BIOS boot partition, which holds GRUB for a BIOS.
pub const BIOS_BOOT_BYTES: u64 = MIB;

/// The size of the EFI system partition.
pub const EFI_BYTES: u64 = 512 * MIB;

/// The number of the EFI system partition on a disk with a BIOS boot
/// partition: the BIOS's GRUB finds its files on partition 2
/// (rgrub-setup(8)).
pub const ESP_NUMBER: u32 = 2;

/// What the root needs beyond the release itself: room for settings,
/// state, users' files and the next update's slot.
pub const ROOM: u64 = 1 << 30;

/// What new partitions may lose to rounding up to whole MiB, at most one
/// MiB each, as each starts on one.
const ALIGNMENT_SLACK: u64 = 4 * MIB;

/// A gibibyte.
const GIB: u64 = 1 << 30;

/// The least space Dukovo takes next to Windows (ADR 0314).
pub const BESIDE_LEAST: u64 = 20 * GIB;

/// What Windows keeps free when it is shrunk, beyond what its files use,
/// for its updates and its page file.
pub const WINDOWS_FREE: u64 = 10 * GIB;

/// The least Dukovo's root gets next to Windows, beyond its boot
/// partition and swap.
pub const BESIDE_ROOT_LEAST: u64 = 8 * GIB;

/// The name of Dukovo's own EFI system partition next to Windows', which
/// it leaves as it is.
pub const DUKOVO_BOOT: &str = "Dukovo boot";

/// The partitions an install makes before the root, with swap of
/// `swap_gib` if any: the BIOS boot partition, the EFI system partition
/// and swap, as (what it is for, its size, its name in the GPT).
fn before_root(swap_gib: u32) -> Vec<(Role, u64, &'static str)> {
    let mut parts =
        vec![(Role::BiosBoot, BIOS_BOOT_BYTES, "BIOS boot"), (Role::Efi, EFI_BYTES, "EFI system")];
    if swap_gib > 0 {
        parts.push((Role::Swap, u64::from(swap_gib) << 30, "Swap"));
    }
    parts
}

/// The bytes the guided and free-space layouts need: their partitions
/// before the root, with swap of `swap_gib`, and a root of `root_bytes`.
#[must_use]
pub fn needed(swap_gib: u32, root_bytes: u64) -> u64 {
    let before: u64 = before_root(swap_gib).iter().map(|(_, bytes, _)| bytes).sum();
    before + root_bytes + ALIGNMENT_SLACK
}

/// The guided layout of `disk`: a new GPT with a BIOS boot partition, the
/// EFI system partition second, where a BIOS's GRUB looks for its files
/// (rgrub-setup(8)), swap of `swap_gib` if any, and the root in the rest.
///
/// # Errors
///
/// If the disk is too small for them.
pub fn guided(disk: &DiskInfo, swap_gib: u32) -> Result<Change, String> {
    let mut table = Table::new(Scheme::Gpt, disk.sector_size, disk.sectors(), &mut Rng::seeded(1))
        .map_err(|err| format!("{}: {err}", disk.name))?;
    let partitions = place(&mut table, &before_root(swap_gib), None, End::Rest)
        .map_err(|why| format!("{}: {why}", disk.name))?;
    Ok(Change { disk: disk.name.clone(), new_table: true, partitions, ..Change::default() })
}

/// Why the free-space layout cannot use `disk`, if it cannot: it needs a
/// GPT that starts no system yet, with free space for the partitions
/// before the root, swap of `swap_gib`, and a root of `root_bytes`.
/// Otherwise the free space it would use: its first sector and how many.
///
/// # Errors
///
/// Why not, in words the screens show.
pub fn free_room(disk: &DiskInfo, swap_gib: u32, root_bytes: u64) -> Result<(u64, u64), String> {
    let Some(table) = &disk.table else {
        return Err(format!("{} has no partition table; the guided layout makes one", disk.name));
    };
    if table.scheme != Scheme::Gpt {
        return Err(format!(
            "{} has an MBR table, and Dukovo goes on a GPT disk; the guided layout makes one",
            disk.name
        ));
    }
    // Another system's boot partitions mean another boot loader, which
    // the manual layout leaves the choice of.
    for (alias, what) in
        [("efi", "an EFI system partition"), ("bios-boot", "a BIOS boot partition")]
    {
        let kind = types::parse(alias, Scheme::Gpt);
        if let Some(found) = table.partitions.iter().find(|p| Some(p.kind) == kind) {
            return Err(format!(
                "{} starts a system already: it has {what}, partition {}; choose Manual to keep it",
                disk.name, found.number
            ));
        }
    }
    let align = table.alignment();
    let largest = table
        .free()
        .into_iter()
        .map(|(first, last)| {
            let start = first.div_ceil(align) * align;
            (start, (last + 1).saturating_sub(start))
        })
        .max_by_key(|&(_, sectors)| sectors);
    let (start, sectors) = largest.unwrap_or((0, 0));
    let free_bytes = sectors * u64::from(table.sector_size);
    let wanted = needed(swap_gib, root_bytes);
    if free_bytes < wanted {
        return Err(format!(
            "{} has {} free in one piece, and Dukovo needs {} there",
            disk.name,
            disk::size::human(free_bytes),
            disk::size::human(wanted)
        ));
    }
    Ok((start, sectors))
}

/// The free-space layout of `disk`: every partition it has is kept, and
/// Dukovo's own BIOS boot partition, EFI system partition, swap of
/// `swap_gib` if any and root go in its largest free space, which must
/// hold a root of `root_bytes` ([`free_room`]).
///
/// # Errors
///
/// Why the disk cannot take it.
pub fn free_space(disk: &DiskInfo, swap_gib: u32, root_bytes: u64) -> Result<Change, String> {
    let (start, _) = free_room(disk, swap_gib, root_bytes)?;
    let mut table = disk.table.clone().ok_or("no table")?;
    let partitions = place(&mut table, &before_root(swap_gib), Some(start), End::Rest)
        .map_err(|why| format!("{}: {why}", disk.name))?;
    Ok(Change { disk: disk.name.clone(), partitions, ..Change::default() })
}

/// The space Dukovo can take next to Windows on `disk`, the least and the
/// most, in bytes, when `boot_entries` says the firmware's boot entries
/// can be made: Windows keeps [`WINDOWS_FREE`] beyond what its files use,
/// and never shrinks below the smallest its volume can be.
///
/// # Errors
///
/// Why Dukovo cannot go next to Windows there, in words the screens
/// show: no Windows, a volume that cannot be shrunk now, no boot entries,
/// or too little room.
pub fn beside_room(disk: &DiskInfo, boot_entries: bool) -> Result<(u64, u64), String> {
    let Some(windows) = &disk.windows else {
        return Err(format!("{} has no Windows that starts by UEFI", disk.name));
    };
    if let Some(refused) = &windows.refused {
        return Err(format!("Windows on {} cannot be shrunk now: {refused}", disk.name));
    }
    if !boot_entries {
        return Err("this computer did not start the stick by UEFI, which the boot entry for \
                    Dukovo next to Windows needs: start it by UEFI from the firmware's boot menu"
            .into());
    }
    let keeps = (windows.used + WINDOWS_FREE).max(windows.smallest);
    let most = windows.bytes.saturating_sub(keeps) / MIB * MIB;
    if most < BESIDE_LEAST {
        return Err(format!(
            "Windows on {} has {} it can give, and Dukovo needs {} while Windows keeps {} free",
            disk.name,
            disk::size::human(most),
            disk::size::human(BESIDE_LEAST),
            disk::size::human(WINDOWS_FREE)
        ));
    }
    Ok((BESIDE_LEAST, most))
}

/// The layout next to Windows on `disk`: Windows' partition shrunk by
/// `bytes`, kept on a whole MiB, and in the space it gives back Dukovo's
/// own EFI system partition, [`DUKOVO_BOOT`], swap of `swap_gib` if any,
/// and the root in the rest, which must hold `root_bytes`. Windows' own
/// EFI system partition, its recovery partition and the rest are kept as
/// they are; no BIOS boot partition is made, as Windows starts by UEFI.
///
/// # Errors
///
/// Why not: Dukovo cannot go next to Windows there ([`beside_room`]),
/// `bytes` is outside the room, or the root would be too small.
pub fn beside(
    disk: &DiskInfo,
    boot_entries: bool,
    swap_gib: u32,
    bytes: u64,
    root_bytes: u64,
) -> Result<Change, String> {
    let (least, most) = beside_room(disk, boot_entries)?;
    let windows: &Windows = disk.windows.as_ref().ok_or("no Windows")?;
    if !(least..=most).contains(&bytes) {
        return Err(format!(
            "give Dukovo between {} and {} GiB, so that Windows keeps {} GiB free",
            least / GIB,
            most / GIB,
            WINDOWS_FREE / GIB
        ));
    }
    let around = EFI_BYTES + (u64::from(swap_gib) << 30) + ALIGNMENT_SLACK;
    if bytes < around + BESIDE_ROOT_LEAST.max(root_bytes) {
        return Err(format!(
            "leave at least {} GiB beyond the swap for Dukovo itself",
            (EFI_BYTES + ALIGNMENT_SLACK + BESIDE_ROOT_LEAST.max(root_bytes)).div_ceil(GIB)
        ));
    }
    let mut table = disk.table.clone().ok_or("no table")?;
    let was = table.get(windows.partition).ok_or("Windows' partition is gone")?.clone();
    let sector = u64::from(table.sector_size);
    let sectors = (windows.bytes - bytes) / MIB * (MIB / sector);
    table.resize(windows.partition, sectors).map_err(|err| format!("{}: {err}", disk.name))?;
    let start = (was.first + sectors).div_ceil(table.alignment()) * table.alignment();
    let mut parts = vec![(Role::Efi, EFI_BYTES, DUKOVO_BOOT)];
    if swap_gib > 0 {
        parts.push((Role::Swap, u64::from(swap_gib) << 30, "Swap"));
    }
    // The root ends where Windows' partition did: Dukovo takes the space
    // asked for, and free space the disk had elsewhere stays free.
    let partitions = place(&mut table, &parts, Some(start), End::Last(was.last))
        .map_err(|why| format!("{}: {why}", disk.name))?;
    Ok(Change {
        disk: disk.name.clone(),
        partitions,
        shrink: Some(Shrink { number: windows.partition, sectors }),
        ..Change::default()
    })
}

/// Adds `parts` to `table`, then the root to `root_end`: in the rest of
/// the free space the last one ends in, or to a sector. They go from
/// `start` on, one after another, or where the table's first free space
/// lets them. Returns them as the change lists new partitions, each with
/// its place.
///
/// The EFI system partition takes number 2, [`ESP_NUMBER`], for a BIOS's
/// GRUB to find; if the table has a partition 2 already, the BIOS boot
/// partition is left out, and the disk starts by UEFI alone. The others
/// take the first numbers free.
fn place(
    table: &mut Table,
    parts: &[(Role, u64, &'static str)],
    start: Option<u64>,
    root_end: End,
) -> Result<Vec<Part>, String> {
    let sector = u64::from(table.sector_size);
    let align = table.alignment();
    let mut at = start;
    let mut made = Vec::new();
    let root = (Role::Root, 0, "Dukovo root");
    let bios = table.get(ESP_NUMBER).is_none();
    let parts = parts.iter().filter(|(role, _, _)| bios || *role != Role::BiosBoot);
    for &(role, bytes, name) in parts.chain(std::iter::once(&root)) {
        let end = if role == Role::Root { root_end } else { End::Sectors(bytes.div_ceil(sector)) };
        let kind = types::parse(type_alias(role, None), table.scheme).ok_or("no such type")?;
        let number = match role {
            Role::Efi if bios => ESP_NUMBER,
            // Partition 2 waits for the EFI system partition.
            _ => free_number(table, bios.then_some(ESP_NUMBER)),
        };
        let new =
            New { number: Some(number), first: at, name: name.to_string(), ..New::of(kind, end) };
        let number = table.add(new, &mut Rng::seeded(1)).map_err(|err| err.0)?;
        let added = table.get(number).ok_or("the partition just added is gone")?;
        at = at.map(|_| (added.last + 1).div_ceil(align) * align);
        made.push(Part {
            number,
            first: Some(added.first),
            sectors: Some(added.sectors()),
            name: Some(name.to_string()),
            role,
            format: None,
        });
    }
    Ok(made)
}

/// The first partition number `table` has free, but `reserved`.
#[must_use]
pub fn free_number(table: &Table, reserved: Option<u32>) -> u32 {
    (1..).find(|&n| table.get(n).is_none() && Some(n) != reserved).unwrap_or(1)
}

/// Checks that a disk whose table will be `after` can start by a BIOS if
/// it has a BIOS boot partition: its partition 2 must be its EFI system
/// partition, where the BIOS's GRUB finds its files (rgrub-setup(8)).
///
/// # Errors
///
/// If it has a BIOS boot partition and partition 2 is something else.
pub fn check_bios_boot(disk: &str, after: &Table) -> Result<(), String> {
    let kind = |alias: &str| types::parse(alias, after.scheme);
    let has_bios_boot = after.partitions.iter().any(|p| Some(p.kind) == kind("bios-boot"));
    let second = after.get(ESP_NUMBER).map(|p| p.kind);
    if has_bios_boot && second != kind("efi") {
        return Err(format!(
            "{disk} has a BIOS boot partition, and the BIOS's GRUB finds its files on partition \
             {ESP_NUMBER}, which is not its EFI system partition: number the EFI system \
             partition {ESP_NUMBER}, or take out the BIOS boot partition to start by UEFI alone"
        ));
    }
    Ok(())
}

/// The type, as rdisk names it, of a partition for `role`, with `format`
/// for a data partition: Microsoft's basic data for FAT and exFAT, which
/// other systems then mount, and Linux's for the rest.
#[must_use]
pub fn type_alias(role: Role, format: Option<Format>) -> &'static str {
    match role {
        Role::Root => "dukovo-root",
        Role::Efi => "efi",
        Role::Swap => "swap",
        Role::BiosBoot => "bios-boot",
        Role::Data => match format {
            Some(Format::Fat32 | Format::Exfat) => "windows-data",
            Some(Format::Btrfs) | None => "linux",
        },
    }
}

/// Whether a partition there is that is formatted for `role` gets that
/// role's type too: what the root, the EFI system partition, swap and
/// BIOS boot hold is told by their types. A data partition keeps its own.
fn retyped(role: Role) -> bool {
    role != Role::Data
}

/// The layout file (rdisk-layout(5)) that makes `change` on `disk`: the
/// disk's table kept unless a new one replaces it, the partitions taken
/// out, and each new one where the change puts it. A partition there is
/// that is formatted for the root, the EFI system partition, swap or BIOS
/// boot is taken out and made again in its place with that type.
#[must_use]
pub fn layout(disk: &DiskInfo, change: &Change) -> LayoutFile {
    let kept = disk.table.as_ref().filter(|_| !change.new_table);
    let scheme = kept.map_or(Scheme::Gpt, |table| table.scheme);
    let mut delete = change.delete.clone();
    let mut partitions = Vec::new();
    for part in &change.partitions {
        let place = match (part.first, part.sectors) {
            (Some(first), Some(sectors)) => Some((first, sectors, part.name.clone())),
            _ if retyped(part.role) => kept
                .and_then(|table| table.get(part.number))
                .map(|was| (was.first, was.sectors(), Some(was.name.clone()))),
            _ => None,
        };
        let Some((first, sectors, name)) = place else { continue };
        if !part.is_new() {
            delete.push(part.number);
        }
        partitions.push(LayoutPart {
            kind: type_alias(part.role, part.format).to_string(),
            size: format!("{sectors}s"),
            start: Some(format!("{first}s")),
            number: Some(part.number),
            name: name.filter(|name| scheme == Scheme::Gpt && !name.is_empty()),
            uuid: None,
            bootable: false,
            attributes: Vec::new(),
        });
    }
    let resize = change
        .shrink
        .iter()
        .map(|shrink| Resize { number: shrink.number, size: format!("{}s", shrink.sectors) })
        .collect();
    LayoutFile {
        table: scheme.name().to_string(),
        uuid: None,
        keep: kept.is_some(),
        delete,
        resize,
        partitions,
    }
}

/// The table `disk` has once `change` is made, worked out in memory as
/// rdisk will write it.
///
/// # Errors
///
/// If the change does not fit the disk: a partition it formats or takes
/// out that is not there, or a new one where there is no room.
pub fn after(disk: &DiskInfo, change: &Change) -> Result<Table, String> {
    let file = layout(disk, change);
    let existing = disk.table.as_ref().filter(|_| !change.new_table);
    for part in change.partitions.iter().filter(|part| !part.is_new()) {
        if existing.and_then(|table| table.get(part.number)).is_none() {
            return Err(format!("{}: there is no partition {} to format", disk.name, part.number));
        }
    }
    disk::layout::apply(
        &file,
        disk.sector_size,
        disk.sectors(),
        disk.table.as_ref(),
        &mut Rng::seeded(1),
    )
    .map_err(|why| format!("{}: {why}", disk.name))
}

/// The layout file of `change` on `disk`, as rdisk --script reads it.
///
/// # Errors
///
/// If it cannot be written as JSON, which does not happen.
pub fn layout_text(disk: &DiskInfo, change: &Change) -> Result<String, String> {
    let mut text =
        serde_json::to_string_pretty(&layout(disk, change)).map_err(|err| err.to_string())?;
    text.push('\n');
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A disk of `gib` GiB of 512-byte sectors, with `table`.
    fn disk(name: &str, gib: u64, table: Option<Table>) -> DiskInfo {
        DiskInfo {
            name: name.into(),
            bytes: gib << 30,
            model: String::new(),
            rotational: None,
            partitions: Vec::new(),
            stick: false,
            sector_size: 512,
            table,
            windows: None,
        }
    }

    /// A GPT on a disk of `gib` GiB with a 1 GiB partition of `alias`
    /// first.
    fn with_one(gib: u64, alias: &str) -> Table {
        let mut table =
            Table::new(Scheme::Gpt, 512, (gib << 30) / 512, &mut Rng::seeded(2)).unwrap();
        let kind = types::parse(alias, Scheme::Gpt).unwrap();
        table.add(New::of(kind, End::Sectors(2 << 20)), &mut Rng::seeded(2)).unwrap();
        table
    }

    fn roles(change: &Change) -> Vec<Role> {
        change.partitions.iter().map(|part| part.role).collect()
    }

    #[test]
    fn the_guided_layout_is_a_new_table() {
        let blank = disk("vda", 8, Some(with_one(8, "linux")));
        let change = guided(&blank, 2).unwrap();
        assert!(change.new_table && change.delete.is_empty());
        assert_eq!(roles(&change), [Role::BiosBoot, Role::Efi, Role::Swap, Role::Root]);
        let numbers: Vec<u32> = change.partitions.iter().map(|part| part.number).collect();
        assert_eq!(numbers, [1, 2, 3, 4], "a BIOS's GRUB finds its files on the second");
        assert_eq!(change.partitions[1].sectors, Some(EFI_BYTES / 512));
        assert_eq!(change.partitions[0].first, Some(2048), "on the first whole MiB");
        let table = after(&blank, &change).unwrap();
        assert_eq!(table.partitions.len(), 4);
        assert_eq!(table.partitions[3].last, table.last_usable, "the root takes the rest");
        let file = layout(&blank, &change);
        assert!(!file.keep);
        assert_eq!(file.partitions[2].kind, "swap");
        assert_eq!(guided(&blank, 0).unwrap().partitions.len(), 3);
    }

    #[test]
    fn free_space_keeps_what_is_there() {
        let data = disk("vda", 8, Some(with_one(8, "linux")));
        let change = free_space(&data, 1, 2 << 30).unwrap();
        assert!(!change.new_table);
        assert_eq!(roles(&change), [Role::BiosBoot, Role::Efi, Role::Swap, Role::Root]);
        let numbers: Vec<u32> = change.partitions.iter().map(|part| part.number).collect();
        assert_eq!(numbers, [3, 2, 4, 5], "the EFI system partition second, for a BIOS's GRUB");
        let table = after(&data, &change).unwrap();
        check_bios_boot("vda", &table).unwrap();
        assert_eq!(
            table.partitions[0],
            data.table.as_ref().unwrap().partitions[0],
            "kept as it was"
        );
        let file = layout(&data, &change);
        assert!(file.keep && file.delete.is_empty());
        assert_eq!(
            file.partitions[0].start.as_deref(),
            Some("2099200s"),
            "after the data, on a MiB"
        );
        // Too little room, another system's boot partition, or no GPT.
        // With a partition 2 there already, no BIOS boot partition: the
        // disk starts by UEFI alone.
        let mut two = with_one(8, "linux");
        let kind = types::parse("linux", Scheme::Gpt).unwrap();
        two.add(New::of(kind, End::Sectors(1 << 21)), &mut Rng::seeded(2)).unwrap();
        let change = free_space(&disk("vda", 8, Some(two)), 1, 2 << 30).unwrap();
        assert_eq!(roles(&change), [Role::Efi, Role::Swap, Role::Root]);
        let full = free_space(&data, 1, 7 << 30).unwrap_err();
        assert!(full.contains("free in one piece"), "{full}");
        let windows = disk("vda", 8, Some(with_one(8, "efi")));
        assert!(free_space(&windows, 1, 1 << 30).unwrap_err().contains("starts a system already"));
        assert!(
            free_space(&disk("vda", 8, None), 1, 1 << 30)
                .unwrap_err()
                .contains("no partition table")
        );
    }

    /// A 128 GiB disk as Windows lays it out: its EFI system partition,
    /// its reserved partition, C: of 100 GiB, 41 GiB full, and recovery
    /// after it, with free space at the end.
    fn windows_disk() -> DiskInfo {
        let mut table =
            Table::new(Scheme::Gpt, 512, (128 << 30) / 512, &mut Rng::seeded(2)).unwrap();
        for (alias, mib) in [
            ("efi", 100),
            ("windows-reserved", 16),
            ("windows-data", 100 << 10),
            ("windows-recovery", 600),
        ] {
            let kind = types::parse(alias, Scheme::Gpt).unwrap();
            table.add(New::of(kind, End::Sectors(mib << 11)), &mut Rng::seeded(2)).unwrap();
        }
        let mut disk = disk("nvme0n1", 128, Some(table));
        disk.windows = Some(Windows {
            partition: 3,
            bytes: 100 << 30,
            used: 41 << 30,
            smallest: 42 << 30,
            refused: None,
            boot_manager: 1,
        });
        disk
    }

    #[test]
    fn next_to_windows_shrinks_it_and_fills_the_space_it_gives() {
        let disk = windows_disk();
        // Windows keeps 10 GiB free beyond the 41 GiB its files use.
        assert_eq!(beside_room(&disk, true).unwrap(), (20 << 30, 49 << 30));
        let change = beside(&disk, true, 2, 30 << 30, 2 << 30).unwrap();
        assert!(!change.new_table && change.delete.is_empty());
        assert_eq!(roles(&change), [Role::Efi, Role::Swap, Role::Root]);
        let names: Vec<_> = change.partitions.iter().map(|p| p.name.as_deref()).collect();
        assert_eq!(names, [Some(DUKOVO_BOOT), Some("Swap"), Some("Dukovo root")]);
        let numbers: Vec<u32> = change.partitions.iter().map(|part| part.number).collect();
        assert_eq!(numbers, [5, 6, 7], "after Windows' own four");
        let sectors = (70 << 30) / 512;
        assert_eq!(change.shrink, Some(Shrink { number: 3, sectors }));
        let before = disk.table.as_ref().unwrap();
        let table = after(&disk, &change).unwrap();
        check_bios_boot("nvme0n1", &table).unwrap();
        let c = table.get(3).unwrap();
        assert_eq!((c.first, c.sectors()), (before.get(3).unwrap().first, sectors));
        for kept in [1, 2, 4] {
            assert_eq!(table.get(kept), before.get(kept), "partition {kept} is kept as it was");
        }
        let boot = table.get(5).unwrap();
        assert_eq!(boot.first, c.last + 1, "Dukovo's boot partition starts where C: now ends");
        assert_eq!(boot.sectors(), EFI_BYTES / 512);
        assert_eq!(
            table.get(7).unwrap().last,
            before.get(3).unwrap().last,
            "the root ends where C: did, and the free space at the end stays free"
        );
        let file = layout(&disk, &change);
        assert!(file.keep);
        assert_eq!(file.resize.len(), 1);
        assert_eq!((file.resize[0].number, file.resize[0].size.as_str()), (3, "146800640s"));
        // No swap: the root takes what the swap would have.
        let change = beside(&disk, true, 0, 20 << 30, 2 << 30).unwrap();
        assert_eq!(roles(&change), [Role::Efi, Role::Root]);
    }

    #[test]
    fn next_to_windows_says_why_not() {
        let disk = windows_disk();
        let range = beside(&disk, true, 2, 60 << 30, 2 << 30).unwrap_err();
        assert!(range.contains("between 20 and 49 GiB"), "{range}");
        let root = beside(&disk, true, 16, 20 << 30, 2 << 30).unwrap_err();
        assert!(root.contains("beyond the swap"), "{root}");
        let bios = beside(&disk, false, 2, 30 << 30, 2 << 30).unwrap_err();
        assert!(bios.contains("did not start the stick by UEFI"), "{bios}");
        let mut full = windows_disk();
        full.windows.as_mut().unwrap().used = 85 << 30;
        let full = beside_room(&full, true).unwrap_err();
        assert!(full.contains("has 5 GiB it can give"), "{full}");
        let mut refused = windows_disk();
        refused.windows.as_mut().unwrap().refused = Some(crate::windows::Refused::Dirty);
        assert!(beside_room(&refused, true).unwrap_err().contains("cannot be shrunk now"));
        let mut none = windows_disk();
        none.windows = None;
        assert!(beside_room(&none, true).unwrap_err().contains("has no Windows"));
    }

    #[test]
    fn a_manual_change_formats_takes_out_and_makes() {
        let mut table = with_one(8, "linux");
        let kind = types::parse("windows-data", Scheme::Gpt).unwrap();
        table.add(New::of(kind, End::Sectors(1 << 21)), &mut Rng::seeded(3)).unwrap();
        let two = disk("vda", 8, Some(table));
        // Partition 1 becomes the EFI system partition, 2 goes, and the
        // root is made where it was and after.
        let change = Change {
            disk: "vda".into(),
            new_table: false,
            delete: vec![2],
            partitions: vec![
                Part {
                    number: 1,
                    first: None,
                    sectors: None,
                    name: None,
                    role: Role::Efi,
                    format: None,
                },
                Part {
                    number: 3,
                    first: Some(3 << 20),
                    sectors: Some(4 << 20),
                    name: Some("Dukovo root".into()),
                    role: Role::Root,
                    format: None,
                },
            ],
            ..Change::default()
        };
        let file = layout(&two, &change);
        assert_eq!(file.delete, [2, 1], "the EFI partition is made again with its type");
        assert_eq!(file.partitions[0].kind, "efi");
        assert_eq!(file.partitions[0].start.as_deref(), Some("2048s"));
        let table = after(&two, &change).unwrap();
        let efi = types::parse("efi", Scheme::Gpt).unwrap();
        assert_eq!(table.get(1).unwrap().kind, efi);
        assert!(table.get(2).is_none());
        assert_eq!(table.get(3).unwrap().first, 3 << 20);
        // A partition that is not there cannot be formatted, and a new one
        // cannot go where one is kept.
        let missing = Change { delete: Vec::new(), ..change.clone() };
        assert!(after(&two, &missing).unwrap_err().contains("not free"));
        let mut gone = change.clone();
        gone.partitions[0].number = 9;
        assert!(after(&two, &gone).unwrap_err().contains("no partition 9"));
    }
}
