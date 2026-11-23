// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! How a SCSI disk says it is (ADR 0665): the SAS and SCSI counterpart of
//! SMART. A disk keeps its counters in log pages (LOG SENSE), and the
//! number of blocks it has replaced in its grown defect list (READ DEFECT
//! DATA). [`gather`] asks for the pages worth keeping, and packs them into
//! one [`RECORD`] that `GET_HEALTH` hands to diskhealth(8). A SATA disk
//! behind a SCSI layer has no log pages worth reading: SMART reaches it
//! through ATA PASS-THROUGH ([`crate::Cdb::smart_read`]).
//!
//! Nothing here sends a command: [`gather`] is given a function that does,
//! so a controller that reaches its disks another way (a RAID controller's
//! pass-through frames) uses it too.

/// SMART READ DATA's feature code: the 512 bytes of attribute values.
pub const SMART_READ_DATA: u8 = 0xd0;
/// SMART READ THRESHOLDS' feature code: the 512 bytes of thresholds.
pub const SMART_READ_THRESHOLDS: u8 = 0xd1;
/// READ DEFECT DATA's defect list format: bytes from index, which disks
/// that keep a grown list answer in, as smartctl asks for it. Only the
/// list's length is read, so a disk that answers in another format (which
/// the answer's header says) still gives the count.
pub const DEFECT_FORMAT: u8 = 0b100;

/// The log page that lists the pages a disk has.
pub const SUPPORTED_PAGES: u8 = 0x00;
/// Log page 0x02, counters of write errors.
pub const WRITE_ERRORS: u8 = 0x02;
/// Log page 0x03, counters of read errors.
pub const READ_ERRORS: u8 = 0x03;
/// Log page 0x05, counters of verify errors.
pub const VERIFY_ERRORS: u8 = 0x05;
/// Log page 0x06, counters of errors that are not the medium's.
pub const NON_MEDIUM_ERRORS: u8 = 0x06;
/// Log page 0x0D, the temperature and its limit.
pub const TEMPERATURE: u8 = 0x0d;
/// Log page 0x0E, start-stop and load-unload cycles.
pub const START_STOP_CYCLES: u8 = 0x0e;
/// Log page 0x10, the self-test results.
pub const SELF_TEST_RESULTS: u8 = 0x10;
/// Log page 0x11, a solid state disk's wear.
pub const SOLID_STATE_MEDIA: u8 = 0x11;
/// Log page 0x15, the background scan, whose first parameter counts the
/// minutes the disk was on.
pub const BACKGROUND_SCAN: u8 = 0x15;
/// Log page 0x2F, informational exceptions: the disk's own warning that
/// it will fail.
pub const INFORMATIONAL_EXCEPTIONS: u8 = 0x2f;

/// The page code of the record's synthetic entry for the grown defect
/// count. No log page has it: pages are six bits.
pub const DEFECTS_ENTRY: u8 = 0xf0;

/// The pages [`gather`] keeps, in the order it asks for them, the most
/// useful first: an entry that does not fit the record is cut, and the
/// ones after it are left out. The self-test log, the longest page, is last.
pub const WANTED: [u8; 10] = [
    INFORMATIONAL_EXCEPTIONS,
    TEMPERATURE,
    READ_ERRORS,
    WRITE_ERRORS,
    VERIFY_ERRORS,
    NON_MEDIUM_ERRORS,
    SOLID_STATE_MEDIA,
    START_STOP_CYCLES,
    BACKGROUND_SCAN,
    SELF_TEST_RESULTS,
];

/// The bytes of a record.
pub const RECORD: usize = 1024;
/// The most bytes [`gather`] asks of one query: what a record holds.
pub const FETCH: usize = RECORD;
/// The bytes of an entry's header: page code, subpage, length.
pub const ENTRY_HEADER: usize = 4;
/// The bytes of the defect count's entry: its header and a u32.
const DEFECTS_LEN: usize = ENTRY_HEADER + 4;

/// What [`gather`] asks of a disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Query {
    /// LOG SENSE of this page ([`crate::Cdb::log_sense`]), [`FETCH`] bytes.
    LogSense(u8),
    /// READ DEFECT DATA of the grown defect list's header
    /// ([`crate::Cdb::read_defect_data_10`]), 4 bytes.
    GrownDefects,
}

/// A query that failed, or a disk that keeps no record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failed;

/// What runs a [`Query`] for [`gather`]: into the buffer, zeros, it returns
/// how many bytes the disk answered.
pub type Fetch<'a> = dyn FnMut(Query, &mut [u8]) -> Result<usize, Failed> + 'a;

/// The log pages a disk says it has (log page 0x00).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Supported([u8; 32]);

impl Supported {
    /// Reads log page 0x00 as the disk answered it: its header and a byte
    /// for each page. `None` if it is another page, or lists none.
    pub fn parse(bytes: &[u8]) -> Option<Supported> {
        if bytes.len() < ENTRY_HEADER || bytes[0] & 0x3f != SUPPORTED_PAGES || bytes[1] != 0 {
            return None;
        }
        let length = usize::from(u16::from_be_bytes([bytes[2], bytes[3]]));
        let list = &bytes[ENTRY_HEADER..bytes.len().min(ENTRY_HEADER + length)];
        let mut set = [0u8; 32];
        for &code in list {
            set[usize::from(code >> 3)] |= 1 << (code & 7);
        }
        (!list.is_empty()).then_some(Supported(set))
    }

    /// Whether it lists `page`.
    pub fn contains(&self, page: u8) -> bool {
        self.0[usize::from(page >> 3)] & 1 << (page & 7) != 0
    }
}

/// The number of grown defects READ DEFECT DATA's header `bytes` says:
/// its list length over the size of the address descriptors it is in.
/// The first descriptor format (short block, 0) takes 4 bytes each; every
/// other, 8. `None` if it does not say its grown list is there (GLISTV),
/// as a disk that refused leaves it, or is shorter than the header.
pub fn grown_defects(bytes: &[u8]) -> Option<u32> {
    let [_, flags, high, low, ..] = *bytes else { return None };
    if flags & 0x08 == 0 {
        return None;
    }
    let size = if flags & 7 == 0 { 4 } else { 8 };
    Some(u32::from(u16::from_be_bytes([high, low])) / size)
}

/// The log page in `bytes`, a LOG SENSE answer: from its header on, as long
/// as it says or as the answer is. `None` if it is not page `page`.
fn log_page(bytes: &[u8], page: u8) -> Option<&[u8]> {
    if bytes.len() < ENTRY_HEADER || bytes[0] & 0x3f != page || bytes[1] != 0 {
        return None;
    }
    let length = usize::from(u16::from_be_bytes([bytes[2], bytes[3]]));
    Some(&bytes[..bytes.len().min(ENTRY_HEADER + length)])
}

/// Packs entries into a [`RECORD`].
///
/// The record is an ABI (GET_HEALTH's `HEALTH_SCSI`, radix-block(2)): a
/// series of entries, each a page code (u8), a subpage (u8), a length
/// (u16, little-endian) and that many bytes, which are the log page as the
/// disk returned it, from its own 4-byte header on. An entry that did not
/// fit is cut to what fits (at least a page's header). The record ends
/// at the first entry whose page code and length are 0, or at its end; the
/// bytes after are 0. The first entry, if the disk gave a count, is
/// [`DEFECTS_ENTRY`], subpage 0, length 4: the number of grown defects, a
/// u32.
struct Packer<'a> {
    bytes: &'a mut [u8; RECORD],
    at: usize,
}

impl<'a> Packer<'a> {
    /// An empty record, with room kept for the defect count's entry,
    /// which goes first.
    fn new(bytes: &'a mut [u8; RECORD]) -> Packer<'a> {
        bytes.fill(0);
        Packer { bytes, at: DEFECTS_LEN }
    }

    /// Puts `data`, log page `page`, in, cut to what fits. Returns whether
    /// it fit whole.
    fn push(&mut self, page: u8, data: &[u8]) -> bool {
        let room = RECORD - self.at;
        let len = data.len().min(room.saturating_sub(ENTRY_HEADER));
        if len < ENTRY_HEADER.min(data.len()) {
            self.at = RECORD;
            return false;
        }
        let at = self.at;
        self.bytes[at] = page;
        self.bytes[at + 1] = 0;
        self.bytes[at + 2..at + 4].copy_from_slice(&(len as u16).to_le_bytes());
        self.bytes[at + 4..at + 4 + len].copy_from_slice(&data[..len]);
        self.at += ENTRY_HEADER + len;
        len == data.len()
    }

    /// Whether nothing more fits.
    fn full(&self) -> bool {
        RECORD - self.at <= ENTRY_HEADER
    }

    /// Puts the defect count first, or, with none, closes the gap kept
    /// for it.
    fn finish(self, defects: Option<u32>) {
        match defects {
            Some(count) => {
                self.bytes[0] = DEFECTS_ENTRY;
                self.bytes[1] = 0;
                self.bytes[2..4].copy_from_slice(&4u16.to_le_bytes());
                self.bytes[4..8].copy_from_slice(&count.to_le_bytes());
            }
            None => {
                self.bytes.copy_within(DEFECTS_LEN..self.at, 0);
                let end = self.at - DEFECTS_LEN;
                self.bytes[end..].fill(0);
            }
        }
    }
}

/// Reads what a SCSI disk says of its health into `record`, as
/// [`Packer`] describes it, by asking `fetch`. `fetch` runs a query into
/// the buffer it is given, which is zeros, and returns how many bytes the
/// disk answered, at most the buffer's length: it may return the length
/// of the buffer, for the headers say how long each answer is. It fails
/// if the disk refused ([`Fetch`]).
///
/// It asks for log page 0x00 first, and fails if there is no list of
/// pages: the disk keeps no record. Then it asks for each of [`WANTED`]
/// the disk lists, in order, leaving out one that fails, and last for the
/// grown defects, which a disk may refuse: the record then has no entry
/// for them.
#[inline(never)]
pub fn gather(fetch: &mut Fetch<'_>, record: &mut [u8; RECORD]) -> Result<(), Failed> {
    let mut buffer = [0u8; FETCH];
    let got = ask(fetch, &mut buffer[..], Query::LogSense(SUPPORTED_PAGES)).ok_or(Failed)?;
    let supported = Supported::parse(&buffer[..got]).ok_or(Failed)?;
    let mut packer = Packer::new(record);
    for page in WANTED {
        if packer.full() {
            break;
        }
        if !supported.contains(page) {
            continue;
        }
        let Some(got) = ask(fetch, &mut buffer[..], Query::LogSense(page)) else { continue };
        if let Some(data) = log_page(&buffer[..got], page) {
            packer.push(page, data);
        }
    }
    let defects = ask(fetch, &mut buffer[..4], Query::GrownDefects)
        .and_then(|got| grown_defects(&buffer[..got]));
    packer.finish(defects);
    Ok(())
}

/// Runs `query` into `buffer`, zeroed first. The bytes answered, or `None`
/// if the query failed.
fn ask(fetch: &mut Fetch<'_>, buffer: &mut [u8], query: Query) -> Option<usize> {
    buffer.fill(0);
    let length = buffer.len();
    fetch(query, buffer).ok().map(|got| got.min(length))
}
