// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The controller's management commands (*DCMD*) and views of what they
//! answer.
//!
//! A DCMD is an opcode ([`opcode`]), a 12-byte mailbox ([`mbox`]) and a
//! buffer the firmware fills. Every number in the buffer is little-endian.
//! The parsers here borrow the buffer and read fields when asked, so they
//! copy nothing. Each checks the buffer's length, and every count and
//! stride the firmware wrote, against the bytes it was given before it
//! reads a field, and none of them panics on any input.

use core::fmt;

#[cfg(test)]
mod tests;

/// Opcodes of the commands the driver sends.
pub mod opcode {
    /// Read the controller's identity, limits and counters.
    pub const CTRL_GET_INFO: u32 = 0x0101_0000;
    /// Read the controller's properties.
    pub const CTRL_GET_PROPERTIES: u32 = 0x0102_0100;
    /// Read the event log's sequence numbers.
    pub const CTRL_EVENT_GET_INFO: u32 = 0x0104_0100;
    /// Read the oldest event at or after a sequence number.
    pub const CTRL_EVENT_GET: u32 = 0x0104_0300;
    /// Wait for an event at or after a sequence number.
    pub const CTRL_EVENT_WAIT: u32 = 0x0104_0500;
    /// Prepare the controller for power off.
    pub const CTRL_SHUTDOWN: u32 = 0x0105_0000;
    /// Flush the controller's cache, and the disks' caches.
    pub const CTRL_CACHE_FLUSH: u32 = 0x0110_1000;
    /// List the physical disks.
    pub const PD_GET_LIST: u32 = 0x0201_0000;
    /// List the physical disks that match a query.
    pub const PD_LIST_QUERY: u32 = 0x0201_0100;
    /// Read one physical disk's details.
    pub const PD_GET_INFO: u32 = 0x0202_0000;
    /// List the logical drives.
    pub const LD_GET_LIST: u32 = 0x0301_0000;
    /// Read one logical drive's details.
    pub const LD_GET_INFO: u32 = 0x0302_0000;
    /// Read the whole configuration: arrays, drives and spares.
    pub const CFG_READ: u32 = 0x0401_0000;
    /// Read the battery's state.
    pub const BBU_GET_STATUS: u32 = 0x0501_0000;
    /// Read the battery's capacity and charge.
    pub const BBU_GET_CAPACITY_INFO: u32 = 0x0502_0000;
    /// Read the battery's design data.
    pub const BBU_GET_DESIGN_INFO: u32 = 0x0503_0000;
}

/// Builders of the 12-byte mailboxes.
pub mod mbox {
    /// The mailbox of a DCMD frame.
    pub type Mailbox = [u8; 12];

    /// Flush the controller's cache.
    pub const FLUSH_CONTROLLER: u8 = 0x01;
    /// Flush the disks' caches.
    pub const FLUSH_DISKS: u8 = 0x02;
    /// Spin the disks down on shutdown.
    pub const SHUTDOWN_SPIN_DOWN: u8 = 0x01;
    /// The PD list query that asks for every device.
    pub const PD_QUERY_ALL: u16 = 0;

    /// `LD_GET_INFO` for the logical drive with target ID `target`.
    pub fn ld_info(target: u16) -> Mailbox {
        let mut m = [0; 12];
        m[..2].copy_from_slice(&target.to_le_bytes());
        m
    }

    /// `PD_GET_INFO` for the physical disk with device ID `device`.
    pub fn pd_info(device: u16) -> Mailbox {
        let mut m = [0; 12];
        m[..2].copy_from_slice(&device.to_le_bytes());
        m
    }

    /// `PD_LIST_QUERY` for every device.
    pub fn pd_list_query_all() -> Mailbox {
        let mut m = [0; 12];
        m[..2].copy_from_slice(&PD_QUERY_ALL.to_le_bytes());
        m
    }

    /// `CTRL_EVENT_GET` or `CTRL_EVENT_WAIT`: events from sequence number
    /// `seq` on, whose locale is in the mask `locale` (see
    /// [`super::locale`]) and whose class is `min_class` or worse (see
    /// [`super::Class::as_i8`]).
    pub fn event(seq: u32, locale: u16, min_class: i8) -> Mailbox {
        let mut m = [0; 12];
        m[..4].copy_from_slice(&seq.to_le_bytes());
        m[4..6].copy_from_slice(&locale.to_le_bytes());
        m[7] = min_class.to_le_bytes()[0];
        m
    }

    /// `CTRL_CACHE_FLUSH` of the controller's cache and the disks'.
    pub fn cache_flush() -> Mailbox {
        let mut m = [0; 12];
        m[0] = FLUSH_CONTROLLER | FLUSH_DISKS;
        m
    }

    /// `CTRL_SHUTDOWN`, with the disks spun down if `spin_down` is set.
    pub fn shutdown(spin_down: bool) -> Mailbox {
        let mut m = [0; 12];
        if spin_down {
            m[0] = SHUTDOWN_SPIN_DOWN;
        }
        m
    }
}

/// Event locale bits, for the mask of an event query.
pub mod locale {
    /// Logical drives.
    pub const LD: u16 = 0x0001;
    /// Physical drives.
    pub const PD: u16 = 0x0002;
    /// Enclosures.
    pub const ENCLOSURE: u16 = 0x0004;
    /// The battery.
    pub const BBU: u16 = 0x0008;
    /// SAS links.
    pub const SAS: u16 = 0x0010;
    /// The controller.
    pub const CONTROLLER: u16 = 0x0020;
    /// The configuration.
    pub const CONFIG: u16 = 0x0040;
    /// The cluster.
    pub const CLUSTER: u16 = 0x0080;
    /// Every locale.
    pub const ALL: u16 = 0xffff;
}

/// Event codes the driver acts on, from the vendor header. Codes it does
/// not list are logged by their number.
pub mod event {
    /// The configuration was cleared.
    pub const CFG_CLEARED: u32 = 0x0004;
    /// A logical drive changed state (`Class`, arguments: LD and state).
    pub const LD_STATE_CHANGE: u32 = 0x0051;
    /// A physical disk was inserted.
    pub const PD_INSERTED: u32 = 0x005b;
    /// A physical disk was removed.
    pub const PD_REMOVED: u32 = 0x0070;
    /// A logical drive was created.
    pub const LD_CREATED: u32 = 0x008a;
    /// A logical drive was deleted.
    pub const LD_DELETED: u32 = 0x008b;
    /// A foreign configuration was imported.
    pub const FOREIGN_CFG_IMPORTED: u32 = 0x00db;
    /// A logical drive went offline.
    pub const LD_OFFLINE: u32 = 0x00fc;
    /// The firmware asks the host to scan its bus again.
    pub const CTRL_HOST_BUS_SCAN_REQUESTED: u32 = 0x0152;
}

/// Why a buffer is not what a parser was given it for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// The buffer is shorter than the structure.
    Short,
    /// A count is larger than the buffer can hold entries for.
    BadCount,
    /// A size or stride is smaller than the entry it describes.
    BadSize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ParseError::Short => "the answer is shorter than its structure",
            ParseError::BadCount => "the answer counts more entries than it holds",
            ParseError::BadSize => "the answer's entry size is too small",
        })
    }
}

// Reads of little-endian fields. Out of range reads give zero, so no read
// can panic; the parsers check their lengths first, so they never do.

fn field(b: &[u8], off: usize, len: usize) -> &[u8] {
    off.checked_add(len).and_then(|end| b.get(off..end)).unwrap_or(&[])
}

fn arr<const N: usize>(b: &[u8], off: usize) -> [u8; N] {
    <[u8; N]>::try_from(field(b, off, N)).unwrap_or([0; N])
}

fn u8_at(b: &[u8], off: usize) -> u8 {
    b.get(off).copied().unwrap_or(0)
}

fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(arr(b, off))
}

fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(arr(b, off))
}

fn u64_at(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(arr(b, off))
}

fn need(b: &[u8], len: usize) -> Result<(), ParseError> {
    if b.len() < len { Err(ParseError::Short) } else { Ok(()) }
}

/// The `count` runs of `stride` bytes from `start` that lie inside `b`.
fn runs(b: &[u8], start: usize, stride: usize, count: usize) -> impl Iterator<Item = &[u8]> {
    (0..count).filter_map(move |i| {
        let at = start.checked_add(i.checked_mul(stride)?)?;
        b.get(at..at.checked_add(stride)?)
    })
}

/// Whether `count` entries of `stride` bytes from `start` fit in `len`.
fn fits(len: usize, start: usize, stride: usize, count: usize) -> bool {
    count.checked_mul(stride).and_then(|n| n.checked_add(start)).is_some_and(|end| end <= len)
}

/// Defines a wrapper of a bit mask with one named test per bit.
macro_rules! flags {
    ($(#[$m:meta])* $name:ident($t:ty) { $($(#[$fm:meta])* $f:ident = $bit:expr;)* }) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub struct $name(pub $t);

        impl $name {
            /// The mask as the firmware wrote it.
            pub fn raw(self) -> $t {
                self.0
            }
            $(
                $(#[$fm])*
                pub fn $f(self) -> bool {
                    self.0 & (1 << $bit) != 0
                }
            )*
        }
    };
}

/// A fixed text field: ASCII, padded with spaces or NULs.
///
/// It reads from the first byte that is not a NUL or a space to the first
/// NUL after it, less trailing spaces, and prints only the printable ASCII
/// in that.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Text<'a>(&'a [u8]);

impl<'a> Text<'a> {
    /// The text of a fixed field.
    pub fn new(field: &'a [u8]) -> Text<'a> {
        let start = field.iter().position(|&c| c != 0 && c != b' ').unwrap_or(field.len());
        let rest = field.get(start..).unwrap_or(&[]);
        let end = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
        let mut text = rest.get(..end).unwrap_or(&[]);
        while let [head @ .., b' '] = text {
            text = head;
        }
        Text(text)
    }

    /// Whether nothing printable is in it.
    pub fn is_empty(&self) -> bool {
        !self.0.iter().any(|&c| printable(c))
    }

    /// The bytes, trimmed, as the firmware wrote them.
    pub fn as_bytes(&self) -> &'a [u8] {
        self.0
    }

    /// Whether the printable part is exactly `s`.
    pub fn is(&self, s: &str) -> bool {
        let mut mine = self.0.iter().filter(|&&c| printable(c));
        let mut theirs = s.bytes();
        loop {
            match (mine.next(), theirs.next()) {
                (None, None) => return true,
                (Some(&a), Some(b)) if a == b => {}
                _ => return false,
            }
        }
    }
}

fn printable(c: u8) -> bool {
    (0x20..0x7f).contains(&c)
}

impl fmt::Display for Text<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for &c in self.0 {
            if printable(c) {
                fmt::Write::write_char(f, char::from(c))?;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for Text<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Text(\"{self}\")")
    }
}

/// How far a long operation has come, and how long it has run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    /// The part done, in 65535ths.
    pub fraction: u16,
    /// Seconds since it began.
    pub elapsed_secs: u16,
}

impl Progress {
    fn at(b: &[u8], off: usize) -> Progress {
        Progress { fraction: u16_at(b, off), elapsed_secs: u16_at(b, off + 2) }
    }

    /// The part done, as a percentage from 0 to 100.
    pub fn percent(&self) -> u8 {
        u8::try_from(u32::from(self.fraction) * 100 / 0xffff).unwrap_or(100)
    }
}

/// The state of a logical drive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LdState {
    /// Not usable.
    Offline,
    /// Redundancy is lost in part: some spans are degraded.
    PartiallyDegraded,
    /// Redundancy is lost.
    Degraded,
    /// Healthy.
    Optimal,
    /// A state this driver does not know.
    Other(u32),
}

impl LdState {
    /// The state with the firmware's number `raw`.
    pub fn from_raw(raw: u32) -> LdState {
        match raw {
            0 => LdState::Offline,
            1 => LdState::PartiallyDegraded,
            2 => LdState::Degraded,
            3 => LdState::Optimal,
            n => LdState::Other(n),
        }
    }

    /// Its name, for the log.
    pub fn name(&self) -> &'static str {
        match self {
            LdState::Offline => "offline",
            LdState::PartiallyDegraded => "partially degraded",
            LdState::Degraded => "degraded",
            LdState::Optimal => "optimal",
            LdState::Other(_) => "unknown",
        }
    }
}

/// The state of a physical disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PdState {
    /// Free, working, in no array.
    UnconfiguredGood,
    /// Free, and failed.
    UnconfiguredBad,
    /// A hot spare.
    HotSpare,
    /// In an array, but offline.
    Offline,
    /// Failed.
    Failed,
    /// Being rebuilt.
    Rebuild,
    /// In an array and online.
    Online,
    /// Being copied back from a spare.
    Copyback,
    /// Passed through to the host (JBOD).
    System,
    /// A state this driver does not know.
    Other(u32),
}

impl PdState {
    /// The state with the firmware's number `raw`.
    pub fn from_raw(raw: u32) -> PdState {
        match raw {
            0x00 => PdState::UnconfiguredGood,
            0x01 => PdState::UnconfiguredBad,
            0x02 => PdState::HotSpare,
            0x10 => PdState::Offline,
            0x11 => PdState::Failed,
            0x14 => PdState::Rebuild,
            0x18 => PdState::Online,
            0x20 => PdState::Copyback,
            0x40 => PdState::System,
            n => PdState::Other(n),
        }
    }

    /// Its name, for the log.
    pub fn name(&self) -> &'static str {
        match self {
            PdState::UnconfiguredGood => "unconfigured good",
            PdState::UnconfiguredBad => "unconfigured bad",
            PdState::HotSpare => "hot spare",
            PdState::Offline => "offline",
            PdState::Failed => "failed",
            PdState::Rebuild => "rebuild",
            PdState::Online => "online",
            PdState::Copyback => "copyback",
            PdState::System => "system (JBOD)",
            PdState::Other(_) => "unknown",
        }
    }

    /// Whether the disk goes to the host as it is.
    pub fn is_jbod(&self) -> bool {
        *self == PdState::System
    }
}

/// The kind of interface a physical disk has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PdInterface {
    /// Not known.
    Unknown,
    /// Parallel SCSI.
    ParallelScsi,
    /// SAS.
    Sas,
    /// SATA.
    Sata,
    /// Fibre Channel.
    FibreChannel,
    /// NVMe.
    Nvme,
    /// A code this driver does not know.
    Other(u8),
}

impl PdInterface {
    fn from_raw(raw: u8) -> PdInterface {
        match raw {
            0 => PdInterface::Unknown,
            1 => PdInterface::ParallelScsi,
            2 => PdInterface::Sas,
            3 => PdInterface::Sata,
            4 => PdInterface::FibreChannel,
            5 => PdInterface::Nvme,
            n => PdInterface::Other(n),
        }
    }
}

/// What a disk's medium is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaType {
    /// Not known.
    Unknown,
    /// A rotating disk.
    Hdd,
    /// Flash.
    Ssd,
    /// A code this driver does not know.
    Other(u8),
}

/// The class of an event: how bad it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// For debugging.
    Debug,
    /// The progress of an operation.
    Progress,
    /// Information.
    Info,
    /// A warning.
    Warning,
    /// Critical.
    Critical,
    /// Fatal.
    Fatal,
    /// The controller or a part of it is dead.
    Dead,
    /// A class this driver does not know.
    Other(i8),
}

impl Class {
    /// The class with the firmware's number `raw`.
    pub fn from_i8(raw: i8) -> Class {
        match raw {
            -2 => Class::Debug,
            -1 => Class::Progress,
            0 => Class::Info,
            1 => Class::Warning,
            2 => Class::Critical,
            3 => Class::Fatal,
            4 => Class::Dead,
            n => Class::Other(n),
        }
    }

    /// The firmware's number for it.
    pub fn as_i8(&self) -> i8 {
        match self {
            Class::Debug => -2,
            Class::Progress => -1,
            Class::Info => 0,
            Class::Warning => 1,
            Class::Critical => 2,
            Class::Fatal => 3,
            Class::Dead => 4,
            Class::Other(n) => *n,
        }
    }
}

// ---------------------------------------------------------------------
// Controller info

flags! {
    /// What hardware the controller has.
    HwPresent(u32) {
        /// A battery backup unit.
        bbu = 0;
        /// An alarm.
        alarm = 1;
        /// An NVRAM.
        nvram = 2;
        /// A UART.
        uart = 3;
    }
}

flags! {
    /// What the controller can do, as its adapter operations say.
    AdapterOps(u32) {
        /// Sets the rebuild rate.
        rebuild_rate = 0;
        /// Sets the consistency check rate.
        cc_rate = 1;
        /// Sets the background initialization rate.
        bgi_rate = 2;
        /// Sets the reconstruction rate.
        recon_rate = 3;
        /// Sets the patrol read rate.
        patrol_rate = 4;
        /// Controls the alarm.
        alarm_control = 5;
        /// Works in a cluster.
        cluster_supported = 6;
        /// Has a battery.
        bbu = 7;
        /// Allows spanning.
        spanning_allowed = 8;
        /// Has dedicated hot spares.
        dedicated_spares = 9;
        /// Has revertible hot spares.
        revertible_spares = 10;
        /// Imports foreign configurations.
        foreign_import = 11;
        /// Runs self-diagnostics.
        self_diagnostic = 12;
        /// Allows arrays of mixed redundancy.
        mixed_array = 13;
        /// Has global hot spares.
        global_spares = 14;
    }
}

flags! {
    /// The RAID levels the controller makes.
    RaidLevels(u32) {
        /// RAID 0.
        raid0 = 0;
        /// RAID 1.
        raid1 = 1;
        /// RAID 5.
        raid5 = 2;
        /// RAID 1E.
        raid1e = 3;
        /// RAID 6.
        raid6 = 4;
    }
}

/// A firmware component: the application, the boot loader and so on.
#[derive(Clone, Copy, Debug)]
pub struct Component<'a>(&'a [u8]);

impl<'a> Component<'a> {
    /// The component's name.
    pub fn name(&self) -> Text<'a> {
        Text::new(field(self.0, 0, 8))
    }

    /// Its version.
    pub fn version(&self) -> Text<'a> {
        Text::new(field(self.0, 8, 32))
    }

    /// The date it was built.
    pub fn build_date(&self) -> Text<'a> {
        Text::new(field(self.0, 40, 16))
    }

    /// The time it was built.
    pub fn build_time(&self) -> Text<'a> {
        Text::new(field(self.0, 56, 16))
    }
}

/// The answer to `CTRL_GET_INFO`.
#[derive(Clone, Copy, Debug)]
pub struct CtrlInfo<'a>(&'a [u8]);

impl<'a> CtrlInfo<'a> {
    /// The size of the structure, which the firmware fills in full.
    pub const LEN: usize = 0x800;
    /// The size a parser needs: the structure up to its package version.
    /// The fields after that are read only when the buffer has them.
    pub const MIN_LEN: usize = 0x6a0;
    /// The most firmware components it lists.
    pub const MAX_COMPONENTS: usize = 8;

    const COMPONENTS: usize = 184;
    const COMPONENT_LEN: usize = 72;

    /// A view of `bytes`.
    pub fn parse(bytes: &'a [u8]) -> Result<CtrlInfo<'a>, ParseError> {
        need(bytes, Self::MIN_LEN)?;
        Ok(CtrlInfo(bytes))
    }

    /// The PCI vendor, device, subvendor and subdevice IDs it reports.
    pub fn pci_ids(&self) -> [u16; 4] {
        [u16_at(self.0, 0), u16_at(self.0, 2), u16_at(self.0, 4), u16_at(self.0, 6)]
    }

    /// The firmware components in use, as many as it counts and has room for.
    pub fn components(&self) -> impl Iterator<Item = Component<'a>> + use<'a> {
        let count = (u32_at(self.0, 180) as usize).min(Self::MAX_COMPONENTS);
        runs(self.0, Self::COMPONENTS, Self::COMPONENT_LEN, count).map(Component)
    }

    /// The version of the application firmware: that of the component
    /// named `APP`, or of the first one that has a version.
    pub fn firmware_version(&self) -> Option<Text<'a>> {
        let mut first = None;
        for c in self.components() {
            if c.version().is_empty() {
                continue;
            }
            if c.name().as_bytes().starts_with(b"APP") {
                return Some(c.version());
            }
            first = first.or(Some(c.version()));
        }
        first
    }

    /// The most arms of an array.
    pub fn max_arms(&self) -> u8 {
        u8_at(self.0, 1340)
    }

    /// The most spans of a logical drive.
    pub fn max_spans(&self) -> u8 {
        u8_at(self.0, 1341)
    }

    /// The most arrays.
    pub fn max_arrays(&self) -> u8 {
        u8_at(self.0, 1342)
    }

    /// The most logical drives.
    pub fn max_lds(&self) -> u8 {
        u8_at(self.0, 1343)
    }

    /// The product name, such as "PERC H730 Mini".
    pub fn product_name(&self) -> Text<'a> {
        Text::new(field(self.0, 1344, 80))
    }

    /// The serial number.
    pub fn serial_number(&self) -> Text<'a> {
        Text::new(field(self.0, 1424, 32))
    }

    /// The hardware it has.
    pub fn hw_present(&self) -> HwPresent {
        HwPresent(u32_at(self.0, 1456))
    }

    /// The firmware's clock, in seconds.
    pub fn firmware_time(&self) -> u32 {
        u32_at(self.0, 1460)
    }

    /// The most commands in flight.
    pub fn max_commands(&self) -> u16 {
        u16_at(self.0, 1464)
    }

    /// The most scatter-gather entries of a command.
    pub fn max_sge_count(&self) -> u16 {
        u16_at(self.0, 1466)
    }

    /// The largest request, as the firmware states it.
    pub fn max_request_size(&self) -> u32 {
        u32_at(self.0, 1468)
    }

    /// How many logical drives there are.
    pub fn lds_present(&self) -> u16 {
        u16_at(self.0, 1472)
    }

    /// How many of them are degraded.
    pub fn lds_degraded(&self) -> u16 {
        u16_at(self.0, 1474)
    }

    /// How many of them are offline.
    pub fn lds_offline(&self) -> u16 {
        u16_at(self.0, 1476)
    }

    /// How many physical devices there are.
    pub fn pds_present(&self) -> u16 {
        u16_at(self.0, 1478)
    }

    /// How many of them are disks.
    pub fn pd_disks_present(&self) -> u16 {
        u16_at(self.0, 1480)
    }

    /// How many disks have a predicted failure (are critical).
    pub fn pd_disks_critical(&self) -> u16 {
        u16_at(self.0, 1482)
    }

    /// How many disks have failed.
    pub fn pd_disks_failed(&self) -> u16 {
        u16_at(self.0, 1484)
    }

    /// The size of the NVRAM, in kilobytes.
    pub fn nvram_kb(&self) -> u16 {
        u16_at(self.0, 1486)
    }

    /// The size of the memory, in megabytes.
    pub fn memory_mb(&self) -> u16 {
        u16_at(self.0, 1488)
    }

    /// The size of the flash, in kilobytes.
    pub fn flash_kb(&self) -> u16 {
        u16_at(self.0, 1490)
    }

    /// The memory's corrected error count.
    pub fn memory_correctable_errors(&self) -> u16 {
        u16_at(self.0, 1492)
    }

    /// The memory's uncorrectable error count.
    pub fn memory_uncorrectable_errors(&self) -> u16 {
        u16_at(self.0, 1494)
    }

    /// The most strips of a request.
    pub fn max_strips_per_io(&self) -> u16 {
        u16_at(self.0, 1498)
    }

    /// The RAID levels it makes.
    pub fn raid_levels(&self) -> RaidLevels {
        RaidLevels(u32_at(self.0, 1500))
    }

    /// What it can do.
    pub fn adapter_ops(&self) -> AdapterOps {
        AdapterOps(u32_at(self.0, 1504))
    }

    /// The smallest and largest stripe sizes, as log2 of 512-byte blocks.
    pub fn stripe_size_range(&self) -> (u8, u8) {
        (u8_at(self.0, 1512), u8_at(self.0, 1513))
    }

    /// Whether the property that exposes JBOD disks to the host is set.
    pub fn jbod_enabled(&self) -> bool {
        u32_at(self.0, 1536 + 32) & (1 << 13) != 0
    }

    /// The package version.
    pub fn package_version(&self) -> Text<'a> {
        Text::new(field(self.0, 1600, 96))
    }

    /// The cache memory, in megabytes, if the answer has the field and it
    /// is not zero.
    pub fn cache_memory_mb(&self) -> Option<u16> {
        self.has(0x7a2, 2).then(|| u16_at(self.0, 0x7a2)).filter(|&v| v != 0)
    }

    /// Whether the firmware says it supports JBOD, if the answer has the
    /// field.
    pub fn jbod_supported(&self) -> Option<bool> {
        self.has(0x7a4, 4).then(|| u32_at(self.0, 0x7a4) & (1 << 11) != 0)
    }

    /// The temperature of the RAID chip, in degrees Celsius, if the answer
    /// has the field and the sensor reports (it is not zero).
    pub fn roc_temperature(&self) -> Option<u8> {
        self.has(0x7c9, 1).then(|| u8_at(self.0, 0x7c9)).filter(|&v| v != 0)
    }

    /// The temperature of the controller board, in degrees Celsius, if
    /// the answer has the field and the sensor reports.
    pub fn controller_temperature(&self) -> Option<u8> {
        self.has(0x7ca, 1).then(|| u8_at(self.0, 0x7ca)).filter(|&v| v != 0)
    }

    fn has(&self, off: usize, len: usize) -> bool {
        self.0.len() >= off + len
    }
}

// ---------------------------------------------------------------------
// Logical drives

/// One entry of the logical drive list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LdListEntry {
    /// The drive's target ID.
    pub target_id: u8,
    /// Its sequence number, which changes when its configuration does.
    pub sequence: u16,
    /// Its state.
    pub state: LdState,
    /// Its size, in blocks.
    pub size_blocks: u64,
}

/// The answer to `LD_GET_LIST`.
#[derive(Clone, Copy, Debug)]
pub struct LdList<'a> {
    bytes: &'a [u8],
    count: usize,
}

impl<'a> LdList<'a> {
    const HEADER: usize = 8;
    const ENTRY: usize = 16;
    /// The size of the structure, for 64 drives.
    pub const LEN: usize = 8 + 64 * 16;

    /// A view of `bytes`.
    pub fn parse(bytes: &'a [u8]) -> Result<LdList<'a>, ParseError> {
        need(bytes, Self::HEADER)?;
        let count = u32_at(bytes, 0) as usize;
        if !fits(bytes.len(), Self::HEADER, Self::ENTRY, count) {
            return Err(ParseError::BadCount);
        }
        Ok(LdList { bytes, count })
    }

    /// How many drives it lists.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether it lists none.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Entry `i`.
    pub fn get(&self, i: usize) -> Option<LdListEntry> {
        if i >= self.count {
            return None;
        }
        runs(self.bytes, Self::HEADER + i * Self::ENTRY, Self::ENTRY, 1).next().map(Self::entry)
    }

    /// The entries.
    pub fn iter(&self) -> impl Iterator<Item = LdListEntry> + use<'a> {
        runs(self.bytes, Self::HEADER, Self::ENTRY, self.count).map(Self::entry)
    }

    fn entry(e: &[u8]) -> LdListEntry {
        LdListEntry {
            target_id: u8_at(e, 0),
            sequence: u16_at(e, 2),
            state: LdState::from_raw(u32::from(u8_at(e, 4))),
            size_blocks: u64_at(e, 8),
        }
    }
}

/// A logical drive's cache policy bits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CachePolicy(pub u8);

impl CachePolicy {
    /// The mask as the firmware wrote it.
    pub fn raw(self) -> u8 {
        self.0
    }

    /// Writes are cached, not written through.
    pub fn write_back(self) -> bool {
        self.0 & 0x01 != 0
    }

    /// Writes are cached while a battery is good, and not otherwise.
    pub fn write_adaptive(self) -> bool {
        self.0 & 0x02 != 0
    }

    /// Reads go ahead of what was asked.
    pub fn read_ahead(self) -> bool {
        self.0 & 0x04 != 0
    }

    /// Reads go ahead only when reads are sequential.
    pub fn read_adaptive(self) -> bool {
        self.0 & 0x08 != 0
    }

    /// Writes stay cached when the battery is bad.
    pub fn write_cache_bad_bbu(self) -> bool {
        self.0 & 0x10 != 0
    }

    /// Writes go through the cache (cached I/O).
    pub fn allow_write_cache(self) -> bool {
        self.0 & 0x20 != 0
    }

    /// Reads go through the cache (cached I/O).
    pub fn allow_read_cache(self) -> bool {
        self.0 & 0x40 != 0
    }
}

/// What a logical drive does with the disks' own caches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskCache {
    /// Left as the disks have it.
    Unchanged,
    /// On.
    Enabled,
    /// Off.
    Disabled,
    /// A code this driver does not know.
    Other(u8),
}

/// Who may use a logical drive, and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LdAccess {
    /// Reads and writes.
    ReadWrite,
    /// Reads only.
    ReadOnly,
    /// Neither.
    Blocked,
    /// A code this driver does not know.
    Other(u8),
}

/// A logical drive's RAID level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RaidLevel {
    /// Striping.
    Raid0,
    /// Mirroring.
    Raid1,
    /// Striping with parity.
    Raid5,
    /// Striping with two parities.
    Raid6,
    /// Mirroring striped over an odd number of disks.
    Raid1E,
    /// Striping over spans of striping.
    Raid00,
    /// Striping over mirrored spans.
    Raid10,
    /// Striping over spans of RAID 5.
    Raid50,
    /// Striping over spans of RAID 6.
    Raid60,
    /// A combination this driver does not know.
    Unknown,
}

impl RaidLevel {
    /// Its name, such as "RAID 10".
    pub fn name(&self) -> &'static str {
        match self {
            RaidLevel::Raid0 => "RAID 0",
            RaidLevel::Raid1 => "RAID 1",
            RaidLevel::Raid5 => "RAID 5",
            RaidLevel::Raid6 => "RAID 6",
            RaidLevel::Raid1E => "RAID 1E",
            RaidLevel::Raid00 => "RAID 00",
            RaidLevel::Raid10 => "RAID 10",
            RaidLevel::Raid50 => "RAID 50",
            RaidLevel::Raid60 => "RAID 60",
            RaidLevel::Unknown => "RAID ?",
        }
    }
}

/// One span of a logical drive: the part of one array it uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    /// Where on the array the span starts, in blocks.
    pub start_block: u64,
    /// Its size, in blocks.
    pub num_blocks: u64,
    /// The array it lies on.
    pub array_ref: u16,
}

/// A logical drive's configuration: its properties and parameters, and
/// its spans. `LD_GET_INFO` starts with one, and `CFG_READ` has one for
/// each drive.
#[derive(Clone, Copy, Debug)]
pub struct LdConfig<'a>(&'a [u8]);

impl<'a> LdConfig<'a> {
    /// The size of the structure.
    pub const LEN: usize = 256;
    /// The size a parser needs: the properties and parameters. A span is
    /// read only if the buffer has it.
    pub const MIN_LEN: usize = 64;
    /// The most spans.
    pub const MAX_SPANS: usize = 8;

    /// A view of `bytes`.
    pub fn parse(bytes: &'a [u8]) -> Result<LdConfig<'a>, ParseError> {
        need(bytes, Self::MIN_LEN)?;
        Ok(LdConfig(bytes))
    }

    /// The drive's target ID.
    pub fn target_id(&self) -> u8 {
        u8_at(self.0, 0)
    }

    /// Its sequence number.
    pub fn sequence(&self) -> u16 {
        u16_at(self.0, 2)
    }

    /// Its name.
    pub fn name(&self) -> Text<'a> {
        Text::new(field(self.0, 4, 16))
    }

    /// The cache policy it was made with.
    pub fn default_cache_policy(&self) -> CachePolicy {
        CachePolicy(u8_at(self.0, 20))
    }

    /// Who may use it.
    pub fn access(&self) -> LdAccess {
        match u8_at(self.0, 21) & 3 {
            0 => LdAccess::ReadWrite,
            2 => LdAccess::ReadOnly,
            3 => LdAccess::Blocked,
            n => LdAccess::Other(n),
        }
    }

    /// What it does with the disks' caches.
    pub fn disk_cache(&self) -> DiskCache {
        match u8_at(self.0, 22) {
            0 => DiskCache::Unchanged,
            1 => DiskCache::Enabled,
            2 => DiskCache::Disabled,
            n => DiskCache::Other(n),
        }
    }

    /// The cache policy now.
    pub fn current_cache_policy(&self) -> CachePolicy {
        CachePolicy(u8_at(self.0, 23))
    }

    /// The primary RAID level number.
    pub fn primary_level(&self) -> u8 {
        u8_at(self.0, 32)
    }

    /// The RAID level qualifier.
    pub fn level_qualifier(&self) -> u8 {
        u8_at(self.0, 33)
    }

    /// The secondary RAID level number.
    pub fn secondary_level(&self) -> u8 {
        u8_at(self.0, 34)
    }

    /// The stripe size as the firmware states it: log2 of the size in
    /// 512-byte blocks.
    pub fn stripe_shift(&self) -> u8 {
        u8_at(self.0, 35)
    }

    /// The stripe size, in bytes, if the shift is a sane one.
    pub fn stripe_bytes(&self) -> Option<u64> {
        let shift = self.stripe_shift();
        (shift <= 32).then(|| 512u64 << shift)
    }

    /// How many drives it has.
    pub fn num_drives(&self) -> u8 {
        u8_at(self.0, 36)
    }

    /// How many spans it has.
    pub fn span_depth(&self) -> u8 {
        u8_at(self.0, 37)
    }

    /// Its state.
    pub fn state(&self) -> LdState {
        LdState::from_raw(u32::from(u8_at(self.0, 38)))
    }

    /// How it was initialized: 0 not, 1 quickly, 2 fully.
    pub fn init_state(&self) -> u8 {
        u8_at(self.0, 39)
    }

    /// Whether its parity or mirrors are consistent.
    pub fn is_consistent(&self) -> bool {
        u8_at(self.0, 40) != 0
    }

    /// Its RAID level, from the primary level, the qualifier and how many
    /// spans it has.
    pub fn raid_level(&self) -> RaidLevel {
        let spans = self.span_depth();
        let primary = self.primary_level();
        if spans > 1 {
            return match primary {
                0 => RaidLevel::Raid00,
                1 => RaidLevel::Raid10,
                5 => RaidLevel::Raid50,
                6 => RaidLevel::Raid60,
                _ => RaidLevel::Unknown,
            };
        }
        match primary {
            0 => RaidLevel::Raid0,
            1 if self.level_qualifier() == 3 => RaidLevel::Raid1E,
            1 => RaidLevel::Raid1,
            5 => RaidLevel::Raid5,
            6 => RaidLevel::Raid6,
            _ => RaidLevel::Unknown,
        }
    }

    /// Span `i`, if the drive has it and the buffer holds it.
    pub fn span(&self, i: usize) -> Option<Span> {
        if i >= usize::from(self.span_depth()).min(Self::MAX_SPANS) {
            return None;
        }
        let s = runs(self.0, 64 + i * 24, 24, 1).next()?;
        Some(Span { start_block: u64_at(s, 0), num_blocks: u64_at(s, 8), array_ref: u16_at(s, 16) })
    }

    /// The spans.
    pub fn spans(&self) -> impl Iterator<Item = Span> + use<'a> {
        let me = *self;
        (0..Self::MAX_SPANS).map_while(move |i| me.span(i))
    }
}

/// The answer to `LD_GET_INFO`.
#[derive(Clone, Copy, Debug)]
pub struct LdInfo<'a>(&'a [u8]);

impl<'a> LdInfo<'a> {
    /// The size of the structure.
    pub const LEN: usize = 384;

    /// A view of `bytes`.
    pub fn parse(bytes: &'a [u8]) -> Result<LdInfo<'a>, ParseError> {
        need(bytes, Self::LEN)?;
        Ok(LdInfo(bytes))
    }

    /// The drive's configuration.
    pub fn config(&self) -> LdConfig<'a> {
        LdConfig(self.0.get(..LdConfig::LEN).unwrap_or(self.0))
    }

    /// The drive's size, in blocks.
    pub fn size_blocks(&self) -> u64 {
        u64_at(self.0, 256)
    }

    /// The consistency check, if one runs.
    pub fn consistency_check(&self) -> Option<Progress> {
        self.progress(0, 268)
    }

    /// The background initialization, if one runs.
    pub fn background_init(&self) -> Option<Progress> {
        self.progress(1, 272)
    }

    /// The foreground initialization, if one runs.
    pub fn foreground_init(&self) -> Option<Progress> {
        self.progress(2, 276)
    }

    /// The reconstruction, if one runs.
    pub fn reconstruction(&self) -> Option<Progress> {
        self.progress(3, 280)
    }

    /// Whether the drive is being reconstructed, by the flag the firmware
    /// keeps apart from the progress.
    pub fn reconstruct_active(&self) -> bool {
        u8_at(self.0, 302) != 0
    }

    fn progress(&self, bit: u32, off: usize) -> Option<Progress> {
        (u32_at(self.0, 264) & (1 << bit) != 0).then(|| Progress::at(self.0, off))
    }
}

// ---------------------------------------------------------------------
// Physical disks

/// One entry of the physical disk list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PdAddress {
    /// The disk's device ID.
    pub device_id: u16,
    /// The device ID of its enclosure.
    pub enclosure_device_id: u16,
    /// The enclosure's index.
    pub enclosure_index: u8,
    /// The slot it is in.
    pub slot: u8,
    /// Its SCSI device type: 0 for a disk.
    pub scsi_device_type: u8,
    /// The ports it is connected on, as a bit map.
    pub connected_ports: u8,
    /// The SAS addresses of its two ports.
    pub sas_address: [u64; 2],
}

/// The answer to `PD_GET_LIST` and `PD_LIST_QUERY`.
#[derive(Clone, Copy, Debug)]
pub struct PdList<'a> {
    bytes: &'a [u8],
    count: usize,
}

impl<'a> PdList<'a> {
    const HEADER: usize = 8;
    const ENTRY: usize = 24;
    /// The size of the structure, for 240 disks.
    pub const LEN: usize = 8 + 240 * 24;

    /// A view of `bytes`.
    pub fn parse(bytes: &'a [u8]) -> Result<PdList<'a>, ParseError> {
        need(bytes, Self::HEADER)?;
        let count = u32_at(bytes, 4) as usize;
        if !fits(bytes.len(), Self::HEADER, Self::ENTRY, count) {
            return Err(ParseError::BadCount);
        }
        Ok(PdList { bytes, count })
    }

    /// The size the firmware says it wrote, in bytes.
    pub fn size(&self) -> u32 {
        u32_at(self.bytes, 0)
    }

    /// How many disks it lists.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether it lists none.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Entry `i`.
    pub fn get(&self, i: usize) -> Option<PdAddress> {
        if i >= self.count {
            return None;
        }
        runs(self.bytes, Self::HEADER + i * Self::ENTRY, Self::ENTRY, 1).next().map(Self::entry)
    }

    /// The entries.
    pub fn iter(&self) -> impl Iterator<Item = PdAddress> + use<'a> {
        runs(self.bytes, Self::HEADER, Self::ENTRY, self.count).map(Self::entry)
    }

    fn entry(e: &[u8]) -> PdAddress {
        PdAddress {
            device_id: u16_at(e, 0),
            enclosure_device_id: u16_at(e, 2),
            enclosure_index: u8_at(e, 4),
            slot: u8_at(e, 5),
            scsi_device_type: u8_at(e, 6),
            connected_ports: u8_at(e, 7),
            sas_address: [u64_at(e, 8), u64_at(e, 16)],
        }
    }
}

/// A disk's standard INQUIRY data.
#[derive(Clone, Copy, Debug)]
pub struct Inquiry<'a>(&'a [u8]);

impl<'a> Inquiry<'a> {
    /// The peripheral device type: 0 for a disk.
    pub fn device_type(&self) -> u8 {
        u8_at(self.0, 0) & 0x1f
    }

    /// The vendor.
    pub fn vendor(&self) -> Text<'a> {
        Text::new(field(self.0, 8, 8))
    }

    /// The product.
    pub fn product(&self) -> Text<'a> {
        Text::new(field(self.0, 16, 16))
    }

    /// The revision.
    pub fn revision(&self) -> Text<'a> {
        Text::new(field(self.0, 32, 4))
    }

    /// The data as it came.
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

/// The answer to `PD_GET_INFO`.
#[derive(Clone, Copy, Debug)]
pub struct PdInfo<'a>(&'a [u8]);

impl<'a> PdInfo<'a> {
    /// The size of the structure.
    pub const LEN: usize = 512;
    /// The size a parser needs: the structure up to its second VPD page.
    /// The fields after that are read only when the buffer has them.
    pub const MIN_LEN: usize = 358;

    /// A view of `bytes`.
    pub fn parse(bytes: &'a [u8]) -> Result<PdInfo<'a>, ParseError> {
        need(bytes, Self::MIN_LEN)?;
        Ok(PdInfo(bytes))
    }

    /// The disk's device ID.
    pub fn device_id(&self) -> u16 {
        u16_at(self.0, 0)
    }

    /// Its sequence number, which changes when it is replaced.
    pub fn sequence(&self) -> u16 {
        u16_at(self.0, 2)
    }

    /// Its INQUIRY data.
    pub fn inquiry(&self) -> Inquiry<'a> {
        Inquiry(field(self.0, 4, 96))
    }

    /// Its serial number, from the VPD page it carries: unit serial
    /// number (page 0x80), or the first ASCII identifier of the device
    /// identification page (0x83).
    pub fn serial(&self) -> Option<Text<'a>> {
        vpd_serial(field(self.0, 100, 64))
    }

    /// Its SCSI device type.
    pub fn scsi_device_type(&self) -> u8 {
        u8_at(self.0, 165)
    }

    /// The ports it is connected on, as a bit map.
    pub fn connected_ports(&self) -> u8 {
        u8_at(self.0, 166)
    }

    /// The speed the device negotiates, as a code.
    pub fn device_speed(&self) -> u8 {
        u8_at(self.0, 167)
    }

    /// The media errors it has had.
    pub fn media_errors(&self) -> u32 {
        u32_at(self.0, 168)
    }

    /// Its other errors.
    pub fn other_errors(&self) -> u32 {
        u32_at(self.0, 172)
    }

    /// The predictive failures it has reported.
    pub fn predictive_failures(&self) -> u32 {
        u32_at(self.0, 176)
    }

    /// The sequence number of the event of its last predictive failure.
    pub fn last_predictive_failure_seq(&self) -> u32 {
        u32_at(self.0, 180)
    }

    /// Whether a predictive failure (S.M.A.R.T. alert) has been flagged:
    /// the failure count is not zero.
    pub fn smart_alert(&self) -> bool {
        self.predictive_failures() != 0
    }

    /// Its firmware state.
    pub fn state(&self) -> PdState {
        PdState::from_raw(u32::from(u16_at(self.0, 184)))
    }

    /// Whether it is set to be removed.
    pub fn disabled_for_removal(&self) -> bool {
        u8_at(self.0, 186) != 0
    }

    /// The link speed, as a code.
    pub fn link_speed(&self) -> u8 {
        u8_at(self.0, 187)
    }

    /// The disk's DDF type word: flags in its low 16 bits, then the
    /// interface in the top four of them.
    pub fn ddf_type(&self) -> u32 {
        u32_at(self.0, 188)
    }

    /// Whether it is in a virtual drive.
    pub fn in_vd(&self) -> bool {
        self.ddf_type() & (1 << 1) != 0
    }

    /// Whether it is a global hot spare.
    pub fn is_global_spare(&self) -> bool {
        self.ddf_type() & (1 << 2) != 0
    }

    /// Whether it is a hot spare.
    pub fn is_spare(&self) -> bool {
        self.ddf_type() & (1 << 3) != 0
    }

    /// Whether its configuration is foreign: from another controller.
    pub fn is_foreign(&self) -> bool {
        self.ddf_type() & (1 << 4) != 0
    }

    /// Its interface.
    pub fn interface(&self) -> PdInterface {
        PdInterface::from_raw(u8::try_from((self.ddf_type() >> 12) & 0xf).unwrap_or(0))
    }

    /// How many paths it has.
    pub fn path_count(&self) -> u8 {
        u8_at(self.0, 192)
    }

    /// The SAS address of path `i`, if it has it.
    pub fn sas_address(&self, i: usize) -> Option<u64> {
        (i < usize::from(self.path_count()).min(4)).then(|| u64_at(self.0, 200 + i * 8))
    }

    /// Its raw size, in blocks.
    pub fn raw_size(&self) -> u64 {
        u64_at(self.0, 232)
    }

    /// Its size without coercion, in blocks.
    pub fn non_coerced_size(&self) -> u64 {
        u64_at(self.0, 240)
    }

    /// Its size as coerced, in blocks.
    pub fn coerced_size(&self) -> u64 {
        u64_at(self.0, 248)
    }

    /// The device ID of its enclosure.
    pub fn enclosure_device_id(&self) -> u16 {
        u16_at(self.0, 256)
    }

    /// Its enclosure's index.
    pub fn enclosure_index(&self) -> u8 {
        u8_at(self.0, 258)
    }

    /// Its slot.
    pub fn slot(&self) -> u8 {
        u8_at(self.0, 259)
    }

    /// The rebuild, if one runs.
    pub fn rebuild(&self) -> Option<Progress> {
        self.progress(0, 264)
    }

    /// The patrol read, if one runs.
    pub fn patrol(&self) -> Option<Progress> {
        self.progress(1, 268)
    }

    /// The clear, if one runs.
    pub fn clear(&self) -> Option<Progress> {
        self.progress(2, 272)
    }

    /// Its medium, if the answer has the field.
    pub fn media_type(&self) -> Option<MediaType> {
        (self.0.len() > 370).then(|| match u8_at(self.0, 370) {
            0 => MediaType::Unknown,
            1 => MediaType::Hdd,
            2 => MediaType::Ssd,
            n => MediaType::Other(n),
        })
    }

    /// Its interface as the extended field says, if the answer has it.
    pub fn interface_ext(&self) -> Option<PdInterface> {
        (self.0.len() > 401).then(|| PdInterface::from_raw(u8_at(self.0, 401)))
    }

    /// Its temperature, in degrees Celsius, if the answer has the field
    /// and the disk reports one (the field is neither 0 nor 255).
    pub fn temperature(&self) -> Option<u8> {
        (self.0.len() > 402).then(|| u8_at(self.0, 402)).filter(|&t| t != 0 && t != 0xff)
    }

    fn progress(&self, bit: u32, off: usize) -> Option<Progress> {
        (u32_at(self.0, 260) & (1 << bit) != 0).then(|| Progress::at(self.0, off))
    }
}

/// The serial number a 64-byte VPD page carries.
fn vpd_serial(page: &[u8]) -> Option<Text<'_>> {
    let code = u8_at(page, 1);
    let len = usize::from(u8_at(page, 3));
    match code {
        0x80 => {
            let t = Text::new(page.get(4..4usize.checked_add(len)?)?);
            (!t.is_empty()).then_some(t)
        }
        0x83 => {
            let mut at = 4usize;
            let end = at.checked_add(len)?.min(page.len());
            while at.checked_add(4)? <= end {
                let n = usize::from(u8_at(page, at + 3));
                let data = page.get(at + 4..(at + 4).checked_add(n)?)?;
                if u8_at(page, at) & 0x0f == 2 {
                    let t = Text::new(data);
                    if !t.is_empty() {
                        return Some(t);
                    }
                }
                at = at.checked_add(4)?.checked_add(n)?;
            }
            None
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------
// Events

/// The answer to `CTRL_EVENT_GET_INFO`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventLogState {
    /// The newest event's sequence number.
    pub newest: u32,
    /// The oldest event the log still has.
    pub oldest: u32,
    /// The sequence number at the last clear.
    pub clear: u32,
    /// The sequence number at the last shutdown.
    pub shutdown: u32,
    /// The sequence number at the last boot.
    pub boot: u32,
}

impl EventLogState {
    /// The size of the structure.
    pub const LEN: usize = 20;

    /// The state in `bytes`.
    pub fn parse(bytes: &[u8]) -> Result<EventLogState, ParseError> {
        need(bytes, Self::LEN)?;
        Ok(EventLogState {
            newest: u32_at(bytes, 0),
            oldest: u32_at(bytes, 4),
            clear: u32_at(bytes, 8),
            shutdown: u32_at(bytes, 12),
            boot: u32_at(bytes, 16),
        })
    }
}

/// A logical drive named by an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LdRef {
    /// Its target ID.
    pub target_id: u16,
    /// Its index.
    pub index: u8,
}

/// A physical disk named by an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PdRef {
    /// Its device ID.
    pub device_id: u16,
    /// Its enclosure's index.
    pub enclosure_index: u8,
    /// Its slot.
    pub slot: u8,
}

fn ld_ref(b: &[u8], off: usize) -> LdRef {
    LdRef { target_id: u16_at(b, off), index: u8_at(b, off + 2) }
}

fn pd_ref(b: &[u8], off: usize) -> PdRef {
    PdRef { device_id: u16_at(b, off), enclosure_index: u8_at(b, off + 2), slot: u8_at(b, off + 3) }
}

/// What an event's arguments hold, by its argument type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventArgs<'a> {
    /// None.
    None,
    /// A disk, and the command and sense data that failed.
    CdbSense {
        /// The disk.
        pd: PdRef,
        /// The command descriptor block.
        cdb: &'a [u8],
        /// The sense data.
        sense: &'a [u8],
    },
    /// A logical drive.
    Ld(LdRef),
    /// A logical drive and a count.
    LdCount {
        /// The drive.
        ld: LdRef,
        /// The count.
        count: u64,
    },
    /// A logical drive and a block.
    LdLba {
        /// The drive.
        ld: LdRef,
        /// The block.
        lba: u64,
    },
    /// A logical drive and its owners before and after.
    LdOwner {
        /// The drive.
        ld: LdRef,
        /// The previous owner.
        prev_owner: u32,
        /// The new owner.
        new_owner: u32,
    },
    /// A block of a logical drive and of a disk.
    LdLbaPdLba {
        /// The drive.
        ld: LdRef,
        /// The disk.
        pd: PdRef,
        /// The block of the drive.
        ld_lba: u64,
        /// The block of the disk.
        pd_lba: u64,
    },
    /// A logical drive and the progress of an operation on it.
    LdProgress {
        /// The drive.
        ld: LdRef,
        /// The progress.
        progress: Progress,
    },
    /// A logical drive and its states before and after.
    LdState {
        /// The drive.
        ld: LdRef,
        /// The state before.
        prev: LdState,
        /// The state after.
        new: LdState,
    },
    /// A logical drive and a strip.
    LdStrip {
        /// The drive.
        ld: LdRef,
        /// The strip.
        strip: u64,
    },
    /// A physical disk.
    Pd(PdRef),
    /// A physical disk and an error code.
    PdError {
        /// The disk.
        pd: PdRef,
        /// The error.
        error: u32,
    },
    /// A physical disk and a block.
    PdLba {
        /// The disk.
        pd: PdRef,
        /// The block.
        lba: u64,
    },
    /// A block of a physical disk, and a logical drive.
    PdLbaLd {
        /// The disk.
        pd: PdRef,
        /// The drive.
        ld: LdRef,
        /// The block.
        lba: u64,
    },
    /// A physical disk and the progress of an operation on it.
    PdProgress {
        /// The disk.
        pd: PdRef,
        /// The progress.
        progress: Progress,
    },
    /// A physical disk and its states before and after.
    PdState {
        /// The disk.
        pd: PdRef,
        /// The state before.
        prev: PdState,
        /// The state after.
        new: PdState,
    },
    /// A PCI function.
    Pci {
        /// Its vendor ID.
        vendor: u16,
        /// Its device ID.
        device: u16,
        /// Its subsystem vendor ID.
        subvendor: u16,
        /// Its subsystem device ID.
        subdevice: u16,
    },
    /// A rate.
    Rate(u32),
    /// A string.
    Str(Text<'a>),
    /// A time, and seconds since it.
    Time {
        /// The real-time clock's reading.
        rtc: u32,
        /// Seconds elapsed.
        elapsed_secs: u16,
    },
    /// A memory error: its address and log words, and a string.
    Ecc {
        /// The error address register.
        ecar: u32,
        /// The error log.
        elog: u32,
        /// A description.
        text: Text<'a>,
    },
    /// An argument type this driver does not decode: its number.
    Other(u8),
}

/// The answer to `CTRL_EVENT_GET` and `CTRL_EVENT_WAIT`.
#[derive(Clone, Copy, Debug)]
pub struct EventDetail<'a>(&'a [u8]);

impl<'a> EventDetail<'a> {
    /// The size of the structure.
    pub const LEN: usize = 256;

    const ARGS: usize = 32;

    /// A view of `bytes`.
    pub fn parse(bytes: &'a [u8]) -> Result<EventDetail<'a>, ParseError> {
        need(bytes, Self::LEN)?;
        Ok(EventDetail(bytes))
    }

    /// The event's sequence number.
    pub fn sequence(&self) -> u32 {
        u32_at(self.0, 0)
    }

    /// Its time stamp, in seconds of the firmware's clock.
    pub fn time(&self) -> u32 {
        u32_at(self.0, 4)
    }

    /// Its code (see [`event`]).
    pub fn code(&self) -> u32 {
        u32_at(self.0, 8)
    }

    /// Its locale bits (see [`locale`]).
    pub fn locale(&self) -> u16 {
        u16_at(self.0, 12)
    }

    /// Its class.
    pub fn class(&self) -> Class {
        Class::from_i8(i8::from_le_bytes([u8_at(self.0, 15)]))
    }

    /// Its argument type, as the firmware numbers it.
    pub fn arg_type(&self) -> u8 {
        u8_at(self.0, 16)
    }

    /// Its description.
    pub fn description(&self) -> Text<'a> {
        Text::new(field(self.0, 128, 128))
    }

    /// Its arguments, decoded by their type.
    pub fn args(&self) -> EventArgs<'a> {
        let a = field(self.0, Self::ARGS, 96);
        match self.arg_type() {
            0 => EventArgs::None,
            1 => EventArgs::CdbSense {
                pd: pd_ref(a, 0),
                cdb: field(a, 8, usize::from(u8_at(a, 4)).min(16)),
                sense: field(a, 24, usize::from(u8_at(a, 5)).min(64)),
            },
            2 => EventArgs::Ld(ld_ref(a, 0)),
            3 => EventArgs::LdCount { ld: ld_ref(a, 0), count: u64_at(a, 4) },
            4 => EventArgs::LdLba { ld: ld_ref(a, 8), lba: u64_at(a, 0) },
            5 => EventArgs::LdOwner {
                ld: ld_ref(a, 0),
                prev_owner: u32_at(a, 4),
                new_owner: u32_at(a, 8),
            },
            6 => EventArgs::LdLbaPdLba {
                ld_lba: u64_at(a, 0),
                pd_lba: u64_at(a, 8),
                ld: ld_ref(a, 16),
                pd: pd_ref(a, 20),
            },
            7 => EventArgs::LdProgress { ld: ld_ref(a, 0), progress: Progress::at(a, 4) },
            8 => EventArgs::LdState {
                ld: ld_ref(a, 0),
                prev: LdState::from_raw(u32_at(a, 4)),
                new: LdState::from_raw(u32_at(a, 8)),
            },
            9 => EventArgs::LdStrip { strip: u64_at(a, 0), ld: ld_ref(a, 8) },
            10 => EventArgs::Pd(pd_ref(a, 0)),
            11 => EventArgs::PdError { pd: pd_ref(a, 0), error: u32_at(a, 4) },
            12 => EventArgs::PdLba { lba: u64_at(a, 0), pd: pd_ref(a, 8) },
            13 => EventArgs::PdLbaLd { lba: u64_at(a, 0), pd: pd_ref(a, 8), ld: ld_ref(a, 12) },
            14 => EventArgs::PdProgress { pd: pd_ref(a, 0), progress: Progress::at(a, 4) },
            15 => EventArgs::PdState {
                pd: pd_ref(a, 0),
                prev: PdState::from_raw(u32_at(a, 4)),
                new: PdState::from_raw(u32_at(a, 8)),
            },
            16 => EventArgs::Pci {
                vendor: u16_at(a, 0),
                device: u16_at(a, 2),
                subvendor: u16_at(a, 4),
                subdevice: u16_at(a, 6),
            },
            17 => EventArgs::Rate(u32_at(a, 0)),
            18 => EventArgs::Str(Text::new(field(a, 0, 96))),
            19 => EventArgs::Time { rtc: u32_at(a, 0), elapsed_secs: u16_at(a, 4) },
            20 => EventArgs::Ecc {
                ecar: u32_at(a, 0),
                elog: u32_at(a, 4),
                text: Text::new(field(a, 8, 64)),
            },
            n => EventArgs::Other(n),
        }
    }
}

// ---------------------------------------------------------------------
// Configuration

/// A drive of an array.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArrayMember {
    /// The disk's device ID, 0xffff if it is missing.
    pub device_id: u16,
    /// Its sequence number.
    pub sequence: u16,
    /// Its state.
    pub state: PdState,
    /// Its enclosure's index.
    pub enclosure: u8,
    /// Its slot.
    pub slot: u8,
}

impl ArrayMember {
    /// Whether the drive is missing.
    pub fn is_missing(&self) -> bool {
        self.device_id == 0xffff
    }
}

/// An array: disks that logical drives are cut from.
#[derive(Clone, Copy, Debug)]
pub struct Array<'a>(&'a [u8]);

impl<'a> Array<'a> {
    /// The size of the structure, for 32 drives.
    pub const LEN: usize = 288;
    /// The size of its part before the drives.
    pub const HEADER: usize = 32;
    /// The most drives.
    pub const MAX_MEMBERS: usize = 32;

    /// Its size, in blocks.
    pub fn size_blocks(&self) -> u64 {
        u64_at(self.0, 0)
    }

    /// How many drives it says it has.
    pub fn num_drives(&self) -> u8 {
        u8_at(self.0, 8)
    }

    /// Its reference, which spans name it by.
    pub fn array_ref(&self) -> u16 {
        u16_at(self.0, 10)
    }

    /// Its drives, as many as it counts and has room for.
    pub fn members(&self) -> impl Iterator<Item = ArrayMember> + use<'a> {
        let count = usize::from(self.num_drives()).min(Self::MAX_MEMBERS);
        runs(self.0, Self::HEADER, 8, count).map(|m| ArrayMember {
            device_id: u16_at(m, 0),
            sequence: u16_at(m, 2),
            state: PdState::from_raw(u32::from(u16_at(m, 4))),
            enclosure: u8_at(m, 6),
            slot: u8_at(m, 7),
        })
    }
}

/// A hot spare.
#[derive(Clone, Copy, Debug)]
pub struct Spare<'a>(&'a [u8]);

impl<'a> Spare<'a> {
    /// The size of the structure, for 16 arrays.
    pub const LEN: usize = 40;
    /// The size of its part before the arrays.
    pub const HEADER: usize = 8;
    /// The most arrays.
    pub const MAX_ARRAYS: usize = 16;

    /// The spare disk's device ID.
    pub fn device_id(&self) -> u16 {
        u16_at(self.0, 0)
    }

    /// Its sequence number.
    pub fn sequence(&self) -> u16 {
        u16_at(self.0, 2)
    }

    /// Whether it serves only the arrays it lists.
    pub fn is_dedicated(&self) -> bool {
        u8_at(self.0, 4) & 1 != 0
    }

    /// Whether it serves every array.
    pub fn is_global(&self) -> bool {
        !self.is_dedicated()
    }

    /// Whether a copyback returns its data to a replaced disk.
    pub fn is_revertible(&self) -> bool {
        u8_at(self.0, 4) & 2 != 0
    }

    /// Whether it serves only arrays in its enclosure.
    pub fn enclosure_affinity(&self) -> bool {
        u8_at(self.0, 4) & 4 != 0
    }

    /// The arrays it serves, as many as it counts and has room for.
    pub fn arrays(&self) -> impl Iterator<Item = u16> + use<'a> {
        let count = usize::from(u8_at(self.0, 7)).min(Self::MAX_ARRAYS);
        runs(self.0, Self::HEADER, 2, count).map(|r| u16_at(r, 0))
    }
}

/// The answer to `CFG_READ`.
#[derive(Clone, Copy, Debug)]
pub struct Config<'a> {
    bytes: &'a [u8],
    array_count: usize,
    array_size: usize,
    ld_count: usize,
    ld_size: usize,
    spare_count: usize,
    spare_size: usize,
}

impl<'a> Config<'a> {
    /// The size of the header, which the arrays follow.
    pub const HEADER: usize = 32;

    /// A view of `bytes`.
    ///
    /// The counts and sizes in the header must put every entry inside
    /// `bytes`, and each size must hold the part of an entry that comes
    /// before its variable-length list.
    pub fn parse(bytes: &'a [u8]) -> Result<Config<'a>, ParseError> {
        need(bytes, Self::HEADER)?;
        let c = Config {
            bytes,
            array_count: usize::from(u16_at(bytes, 4)),
            array_size: usize::from(u16_at(bytes, 6)),
            ld_count: usize::from(u16_at(bytes, 8)),
            ld_size: usize::from(u16_at(bytes, 10)),
            spare_count: usize::from(u16_at(bytes, 12)),
            spare_size: usize::from(u16_at(bytes, 14)),
        };
        if (c.array_count > 0 && c.array_size < Array::HEADER)
            || (c.ld_count > 0 && c.ld_size < LdConfig::MIN_LEN)
            || (c.spare_count > 0 && c.spare_size < Spare::HEADER)
        {
            return Err(ParseError::BadSize);
        }
        let (a, l, s) = (
            c.array_count as u64 * c.array_size as u64,
            c.ld_count as u64 * c.ld_size as u64,
            c.spare_count as u64 * c.spare_size as u64,
        );
        if Self::HEADER as u64 + a + l + s > bytes.len() as u64 {
            return Err(ParseError::BadCount);
        }
        Ok(c)
    }

    fn ld_start(&self) -> usize {
        Self::HEADER + self.array_count * self.array_size
    }

    fn spare_start(&self) -> usize {
        self.ld_start() + self.ld_count * self.ld_size
    }

    /// The size of the data the firmware says it wrote.
    pub fn size(&self) -> u32 {
        u32_at(self.bytes, 0)
    }

    /// How many arrays there are.
    pub fn array_count(&self) -> usize {
        self.array_count
    }

    /// The size of an array's entry.
    pub fn array_size(&self) -> usize {
        self.array_size
    }

    /// How many logical drives there are.
    pub fn ld_count(&self) -> usize {
        self.ld_count
    }

    /// The size of a logical drive's entry.
    pub fn ld_size(&self) -> usize {
        self.ld_size
    }

    /// How many hot spares there are.
    pub fn spare_count(&self) -> usize {
        self.spare_count
    }

    /// The size of a hot spare's entry.
    pub fn spare_size(&self) -> usize {
        self.spare_size
    }

    /// The arrays.
    pub fn arrays(&self) -> impl Iterator<Item = Array<'a>> + use<'a> {
        runs(self.bytes, Self::HEADER, self.array_size, self.array_count).map(Array)
    }

    /// The logical drives.
    pub fn lds(&self) -> impl Iterator<Item = LdConfig<'a>> + use<'a> {
        runs(self.bytes, self.ld_start(), self.ld_size, self.ld_count).map(LdConfig)
    }

    /// The hot spares.
    pub fn spares(&self) -> impl Iterator<Item = Spare<'a>> + use<'a> {
        runs(self.bytes, self.spare_start(), self.spare_size, self.spare_count).map(Spare)
    }
}

// ---------------------------------------------------------------------
// Battery

/// What kind of battery backs the cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BbuType {
    /// None.
    None,
    /// An integrated battery (iBBU).
    Ibbu,
    /// A battery pack (BBU).
    Bbu,
    /// A type this driver does not know: its number.
    Other(u8),
}

flags! {
    /// The battery firmware's status bits.
    BbuFlags(u32) {
        /// The pack is missing.
        pack_missing = 0;
        /// The voltage is low.
        voltage_low = 1;
        /// The temperature is high.
        temperature_high = 2;
        /// It is charging.
        charge_active = 3;
        /// It is discharging.
        discharge_active = 4;
        /// A learn cycle is requested.
        learn_cycle_requested = 5;
        /// A learn cycle is running.
        learn_cycle_active = 6;
        /// A learn cycle failed.
        learn_cycle_failed = 7;
        /// A learn cycle timed out.
        learn_cycle_timeout = 8;
        /// An I2C error was detected.
        i2c_error = 9;
    }
}

/// The answer to `BBU_GET_STATUS`.
#[derive(Clone, Copy, Debug)]
pub struct BbuStatus<'a>(&'a [u8]);

impl<'a> BbuStatus<'a> {
    /// The size of the structure.
    pub const LEN: usize = 64;

    /// A view of `bytes`.
    pub fn parse(bytes: &'a [u8]) -> Result<BbuStatus<'a>, ParseError> {
        need(bytes, Self::LEN)?;
        Ok(BbuStatus(bytes))
    }

    /// The kind of battery.
    pub fn battery_type(&self) -> BbuType {
        match u8_at(self.0, 0) {
            0 => BbuType::None,
            1 => BbuType::Ibbu,
            2 => BbuType::Bbu,
            n => BbuType::Other(n),
        }
    }

    /// The voltage, in millivolts.
    pub fn voltage_mv(&self) -> u16 {
        u16_at(self.0, 2)
    }

    /// The current, in milliamps; negative while it discharges.
    pub fn current_ma(&self) -> i16 {
        i16::from_le_bytes(arr(self.0, 4))
    }

    /// The temperature, in degrees Celsius.
    pub fn temperature_c(&self) -> u16 {
        u16_at(self.0, 6)
    }

    /// The firmware's status bits.
    pub fn flags(&self) -> BbuFlags {
        BbuFlags(u32_at(self.0, 8))
    }

    /// The gas gauge's status word.
    pub fn gas_gauge_status(&self) -> u16 {
        u16_at(self.0, 32)
    }

    /// The relative state of charge, in percent, for a battery that has
    /// one.
    pub fn relative_charge(&self) -> Option<u16> {
        matches!(self.battery_type(), BbuType::Ibbu | BbuType::Bbu).then(|| u16_at(self.0, 34))
    }

    /// The absolute state of charge, in percent, of an integrated battery.
    pub fn absolute_charge(&self) -> Option<u16> {
        (self.battery_type() == BbuType::Ibbu).then(|| u16_at(self.0, 42))
    }

    /// The remaining capacity, in milliamp-hours, of a battery pack.
    pub fn remaining_capacity(&self) -> Option<u16> {
        (self.battery_type() == BbuType::Bbu).then(|| u16_at(self.0, 38))
    }

    /// The full-charge capacity, in milliamp-hours, of a battery pack.
    pub fn full_charge_capacity(&self) -> Option<u16> {
        (self.battery_type() == BbuType::Bbu).then(|| u16_at(self.0, 40))
    }

    /// Whether the pack's state of health is good, for a battery pack.
    pub fn state_of_health_good(&self) -> Option<bool> {
        (self.battery_type() == BbuType::Bbu).then(|| u8_at(self.0, 42) != 0)
    }
}

/// The answer to `BBU_GET_CAPACITY_INFO`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BbuCapacity {
    /// The relative state of charge, in percent.
    pub relative_charge: u16,
    /// The absolute state of charge, in percent.
    pub absolute_charge: u16,
    /// The remaining capacity, in milliamp-hours.
    pub remaining_capacity: u16,
    /// The capacity when full, in milliamp-hours.
    pub full_charge_capacity: u16,
    /// The minutes to empty at the present rate.
    pub run_time_to_empty: u16,
    /// The minutes to empty at the average rate.
    pub average_time_to_empty: u16,
    /// The minutes to full at the average rate.
    pub average_time_to_full: u16,
    /// The charge cycles it has had.
    pub cycle_count: u16,
    /// The error of the state of charge, in percent.
    pub max_error: u16,
    /// The capacity that raises the low alarm.
    pub remaining_capacity_alarm: u16,
    /// The minutes remaining that raise the low alarm.
    pub remaining_time_alarm: u16,
}

impl BbuCapacity {
    /// The size of the structure.
    pub const LEN: usize = 48;
    /// The size a parser needs: the fields, without the padding after.
    pub const MIN_LEN: usize = 22;

    /// The capacity data in `bytes`.
    pub fn parse(bytes: &[u8]) -> Result<BbuCapacity, ParseError> {
        need(bytes, Self::MIN_LEN)?;
        Ok(BbuCapacity {
            relative_charge: u16_at(bytes, 0),
            absolute_charge: u16_at(bytes, 2),
            remaining_capacity: u16_at(bytes, 4),
            full_charge_capacity: u16_at(bytes, 6),
            run_time_to_empty: u16_at(bytes, 8),
            average_time_to_empty: u16_at(bytes, 10),
            average_time_to_full: u16_at(bytes, 12),
            cycle_count: u16_at(bytes, 14),
            max_error: u16_at(bytes, 16),
            remaining_capacity_alarm: u16_at(bytes, 18),
            remaining_time_alarm: u16_at(bytes, 20),
        })
    }
}

/// The answer to `BBU_GET_DESIGN_INFO`.
#[derive(Clone, Copy, Debug)]
pub struct BbuDesign<'a>(&'a [u8]);

impl<'a> BbuDesign<'a> {
    /// The size of the structure.
    pub const LEN: usize = 67;
    /// The size a parser needs: the fields, without the padding after.
    pub const MIN_LEN: usize = 50;

    /// A view of `bytes`.
    pub fn parse(bytes: &'a [u8]) -> Result<BbuDesign<'a>, ParseError> {
        need(bytes, Self::MIN_LEN)?;
        Ok(BbuDesign(bytes))
    }

    /// The manufacture date word as the firmware wrote it.
    pub fn manufacture_date_raw(&self) -> u32 {
        u32_at(self.0, 0)
    }

    /// The manufacture date as year, month and day, if the low 16 bits
    /// hold a valid one in the Smart Battery format (year from 1980 in
    /// the top 7 bits, month in 4, day in 5).
    pub fn manufacture_date(&self) -> Option<(u16, u8, u8)> {
        let w = u16_at(self.0, 0);
        let (month, day) = (u8::try_from((w >> 5) & 0xf).ok()?, u8::try_from(w & 0x1f).ok()?);
        ((1..=12).contains(&month) && (1..=31).contains(&day)).then_some((
            1980 + (w >> 9),
            month,
            day,
        ))
    }

    /// The design capacity, in milliamp-hours.
    pub fn design_capacity(&self) -> u16 {
        u16_at(self.0, 4)
    }

    /// The design voltage, in millivolts.
    pub fn design_voltage(&self) -> u16 {
        u16_at(self.0, 6)
    }

    /// The specification information word.
    pub fn spec_info(&self) -> u16 {
        u16_at(self.0, 8)
    }

    /// The serial number.
    pub fn serial_number(&self) -> u16 {
        u16_at(self.0, 10)
    }

    /// The pack status and configuration word.
    pub fn pack_stat_config(&self) -> u16 {
        u16_at(self.0, 12)
    }

    /// The manufacturer's name.
    pub fn manufacturer(&self) -> Text<'a> {
        Text::new(field(self.0, 14, 12))
    }

    /// The device's name.
    pub fn device_name(&self) -> Text<'a> {
        Text::new(field(self.0, 26, 8))
    }

    /// The chemistry, such as "LION".
    pub fn chemistry(&self) -> Text<'a> {
        Text::new(field(self.0, 34, 8))
    }

    /// The manufacturer's data.
    pub fn manufacturer_data(&self) -> &'a [u8] {
        field(self.0, 42, 8)
    }
}
