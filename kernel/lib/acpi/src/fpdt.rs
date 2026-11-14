// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Firmware Performance Data Table (`FPDT`) and the Firmware Basic
//! Boot Performance Table (`FBPT`) it points to: how long the firmware
//! took before it started the boot loader, and the loader before it ended
//! the firmware's boot services (ACPI 6.5, section 5.2.24), which
//! startup(8) puts before the kernel's own times (ADR 0474). Linux shows
//! the same at `/sys/firmware/acpi/fpdt/boot/`.
//!
//! The FPDT's body is a list of performance records, each starting with
//! a type (2 bytes), a length (1) and a revision (1). The one read here:
//!
//! | Offset | Size | Field (type 0, length 16, revision 1) |
//! | --- | --- | --- |
//! | 4 | 4 | Reserved |
//! | 8 | 8 | Physical address of the FBPT |
//!
//! The FBPT is not an ACPI table: its header is its signature, `FBPT`,
//! and its length (4 bytes each), then records in the same form. The one
//! read here, the Firmware Basic Boot Performance Data Record, gives
//! times in nanoseconds:
//!
//! | Offset | Size | Field (type 2, length 48, revision 2) |
//! | --- | --- | --- |
//! | 4 | 4 | Reserved |
//! | 8 | 8 | `ResetEnd`: the firmware's image began to run |
//! | 16 | 8 | `OsLoaderLoadImageStart`: the firmware began to load the boot loader |
//! | 24 | 8 | `OsLoaderStartImageStart`: the firmware started the boot loader |
//! | 32 | 8 | `ExitBootServicesEntry`: the loader asked to end the boot services |
//! | 40 | 8 | `ExitBootServicesExit`: they ended |
//!
//! A time the firmware did not take is 0.

use crate::{Error, Sdt, u16_at, u32_at, u64_at};

/// The type of the FPDT's record that points to the FBPT.
const BASIC_BOOT_POINTER: u16 = 0;

/// The type of the FBPT's record of the boot's times.
const BASIC_BOOT_DATA: u16 = 2;

/// The bytes of the FBPT's header: its signature and its length.
pub const FBPT_HEADER_LEN: usize = 8;

/// The longest FBPT the kernel reads: the firmware's record and a
/// generous number of the loader's own after it.
pub const FBPT_MAX_LEN: usize = 64 * 1024;

/// What the FPDT says: where the FBPT is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fpdt {
    /// The FBPT's physical address, if the table names one.
    pub basic_boot: Option<u64>,
}

impl Fpdt {
    /// Parses an `FPDT` table.
    pub fn parse(sdt: &Sdt<'_>) -> Result<Fpdt, Error> {
        if &sdt.signature != b"FPDT" {
            return Err(Error::BadSignature);
        }
        let mut basic_boot = None;
        for (kind, revision, record) in records(sdt.body()) {
            if kind == BASIC_BOOT_POINTER && revision == 1 && record.len() >= 16 {
                basic_boot = u64_at(record, 8).filter(|&address| address != 0);
            }
        }
        Ok(Fpdt { basic_boot })
    }
}

/// The times of a boot, from the FBPT, in nanoseconds of the firmware's
/// timer: 0 where the firmware took none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BootTimes {
    /// The firmware's image began to run.
    pub reset_end: u64,
    /// The firmware began to load the boot loader.
    pub loader_load_start: u64,
    /// The firmware started the boot loader.
    pub loader_start: u64,
    /// The loader asked to end the firmware's boot services.
    pub exit_boot_services_entry: u64,
    /// The boot services ended.
    pub exit_boot_services_exit: u64,
}

impl BootTimes {
    /// The length the FBPT at `header`, its first [`FBPT_HEADER_LEN`]
    /// bytes, says it has, if `header` is an FBPT's and the length is one
    /// the kernel reads.
    pub fn fbpt_len(header: &[u8]) -> Result<usize, Error> {
        if header.get(..4) != Some(b"FBPT".as_slice()) {
            return Err(Error::BadSignature);
        }
        let len = u32_at(header, 4).ok_or(Error::TooShort)? as usize;
        if !(FBPT_HEADER_LEN..=FBPT_MAX_LEN).contains(&len) {
            return Err(Error::TooShort);
        }
        Ok(len)
    }

    /// Parses an FBPT, its header and its records, as long as its length
    /// says; `Ok(None)` if it holds no record of the boot's times.
    pub fn parse_fbpt(bytes: &[u8]) -> Result<Option<BootTimes>, Error> {
        let len = Self::fbpt_len(bytes)?;
        let body = bytes.get(FBPT_HEADER_LEN..len).ok_or(Error::TooShort)?;
        let found = records(body).find(|&(kind, revision, record)| {
            kind == BASIC_BOOT_DATA && revision == 2 && record.len() >= 48
        });
        let Some((_, _, record)) = found else { return Ok(None) };
        let at = |offset| u64_at(record, offset).unwrap_or(0);
        Ok(Some(BootTimes {
            reset_end: at(8),
            loader_load_start: at(16),
            loader_start: at(24),
            exit_boot_services_entry: at(32),
            exit_boot_services_exit: at(40),
        }))
    }

    /// How long the firmware took, from its start to starting the boot
    /// loader, if it took both times.
    pub fn firmware_ns(&self) -> Option<u64> {
        // ResetEnd may be 0 when the firmware's timer starts at reset: a
        // start of 0 is a time then, but a loader's start of 0 is none.
        (self.loader_start != 0).then(|| self.loader_start.checked_sub(self.reset_end)).flatten()
    }

    /// How long the boot loader took, from its start to the end of the
    /// boot services, which is when the kernel runs alone, if the
    /// firmware took both times.
    pub fn loader_ns(&self) -> Option<u64> {
        let (start, end) = (self.loader_start, self.exit_boot_services_exit);
        (start != 0 && end != 0).then(|| end.checked_sub(start)).flatten()
    }
}

/// The records of a performance table's `body`: each one's type, revision
/// and bytes, header included, up to the first that does not fit.
fn records(body: &[u8]) -> impl Iterator<Item = (u16, u8, &[u8])> {
    let mut at = 0;
    core::iter::from_fn(move || {
        let kind = u16_at(body, at)?;
        let len = usize::from(*body.get(at + 2)?);
        let revision = *body.get(at + 3)?;
        // A record is at least its header: a shorter one would never end.
        let record = body.get(at..at.checked_add(len)?).filter(|_| len >= 4)?;
        at += len;
        Some((kind, revision, record))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::table;

    /// The FPDT's record that points to an FBPT at `address`.
    fn pointer(address: u64) -> Vec<u8> {
        let mut record = vec![0, 0, 16, 1, 0, 0, 0, 0];
        record.extend_from_slice(&address.to_le_bytes());
        record
    }

    /// An FBPT with the boot's record of `times`, then `more`.
    fn fbpt(times: [u64; 5], more: &[u8]) -> Vec<u8> {
        let mut record = vec![2, 0, 48, 2, 0, 0, 0, 0];
        for time in times {
            record.extend_from_slice(&time.to_le_bytes());
        }
        let mut bytes = b"FBPT".to_vec();
        bytes.extend_from_slice(&((8 + record.len() + more.len()) as u32).to_le_bytes());
        bytes.extend_from_slice(&record);
        bytes.extend_from_slice(more);
        bytes
    }

    fn sdt(bytes: &[u8]) -> Sdt<'_> {
        Sdt::parse(0x1000, bytes).unwrap()
    }

    #[test]
    fn the_fpdt_points_to_the_fbpt() {
        // An S3 record (type 1) first, which is passed over.
        let mut body = vec![1, 0, 16, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        body.extend_from_slice(&pointer(0x7f00_0000));
        let fpdt = table(*b"FPDT", 1, &body);
        assert_eq!(Fpdt::parse(&sdt(&fpdt)), Ok(Fpdt { basic_boot: Some(0x7f00_0000) }));
        // No pointer, a pointer to 0, or one of another revision: none.
        for body in [Vec::new(), pointer(0), {
            let mut other = pointer(0x7f00_0000);
            other[3] = 2;
            other
        }] {
            let fpdt = table(*b"FPDT", 1, &body);
            assert_eq!(Fpdt::parse(&sdt(&fpdt)), Ok(Fpdt { basic_boot: None }));
        }
        let other = table(*b"SPCR", 1, &pointer(0x7f00_0000));
        assert_eq!(Fpdt::parse(&sdt(&other)), Err(Error::BadSignature));
    }

    #[test]
    fn the_fbpt_gives_the_firmware_s_and_the_loader_s_times() {
        // As OVMF gives them: the loader started at 1.2 s and the boot
        // services ended at 3.5 s, of a timer that started at reset.
        let ms = 1_000_000;
        let times = [0, 1_100 * ms, 1_200 * ms, 3_400 * ms, 3_500 * ms];
        let bytes = fbpt(times, &[]);
        assert_eq!(BootTimes::fbpt_len(&bytes[..8]), Ok(bytes.len()));
        let boot = BootTimes::parse_fbpt(&bytes).unwrap().unwrap();
        assert_eq!(boot.firmware_ns(), Some(1_200 * ms));
        assert_eq!(boot.loader_ns(), Some(2_300 * ms));
        // A loader's record after the firmware's is passed over.
        let more = [0x10, 0x10, 8, 1, 0, 0, 0, 0];
        assert_eq!(BootTimes::parse_fbpt(&fbpt(times, &more)), Ok(Some(boot)));
    }

    #[test]
    fn times_the_firmware_did_not_take_are_none() {
        let boot = BootTimes::parse_fbpt(&fbpt([0; 5], &[])).unwrap().unwrap();
        assert_eq!((boot.firmware_ns(), boot.loader_ns()), (None, None));
        // The end before the start, as a timer that went round: none.
        let backwards = BootTimes::parse_fbpt(&fbpt([500, 0, 400, 0, 300], &[])).unwrap().unwrap();
        assert_eq!((backwards.firmware_ns(), backwards.loader_ns()), (None, None));
    }

    #[test]
    fn a_short_or_strange_fbpt_is_refused() {
        let good = fbpt([0, 1, 2, 3, 4], &[]);
        // Wrong signature, a length shorter than the header or past what
        // the kernel reads, or past the bytes given.
        let mut wrong = good.clone();
        wrong[..4].copy_from_slice(b"FPDT");
        assert_eq!(BootTimes::parse_fbpt(&wrong), Err(Error::BadSignature));
        for len in [4u32, (FBPT_MAX_LEN + 1) as u32] {
            let mut bad = good.clone();
            bad[4..8].copy_from_slice(&len.to_le_bytes());
            assert_eq!(BootTimes::parse_fbpt(&bad), Err(Error::TooShort), "{len}");
        }
        assert_eq!(BootTimes::parse_fbpt(&good[..good.len() - 1]), Err(Error::TooShort));
        // A record of the wrong type, revision or length is no time.
        for (at, value) in [(8, 3u8), (11, 1), (10, 40)] {
            let mut bad = good.clone();
            bad[at] = value;
            assert_eq!(BootTimes::parse_fbpt(&bad), Ok(None), "byte {at} = {value}");
        }
        // A record of length 0 ends the list rather than repeating.
        let mut stuck = b"FBPT".to_vec();
        stuck.extend_from_slice(&16u32.to_le_bytes());
        stuck.extend_from_slice(&[9, 0, 0, 1, 0, 0, 0, 0]);
        assert_eq!(BootTimes::parse_fbpt(&stuck), Ok(None));
        // All zeros is no FBPT.
        assert_eq!(BootTimes::parse_fbpt(&[0; 64]), Err(Error::BadSignature));
    }
}
