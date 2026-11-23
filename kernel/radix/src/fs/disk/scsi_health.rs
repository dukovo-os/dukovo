// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The health of a SCSI disk (ADR 0665), for the controllers that send it
//! SCSI commands as virtio SCSI, VMware's PVSCSI and Hyper-V's storvsc do:
//! [`health`] is their [`Controller::health`](super::queue::Controller).
//!
//! A SATA disk behind the SCSI layer (its INQUIRY vendor is "ATA") is read
//! as AHCI reads one: SMART READ DATA and READ THRESHOLDS, as ATA
//! PASS-THROUGH(16) here, into the record's two halves. Any other disk is
//! asked its log pages and grown defects (`radix_scsi::health::gather`),
//! each through the queue into the page's last 2 KiB, and the packed record
//! goes in the page's first 1 KiB. A disk with no list of log pages, as
//! QEMU's disks have none, keeps no record.

use radix_block::{Error, Health, Page};
use radix_scsi::Cdb;
use radix_scsi::health::{self, Failed, Query, RECORD};

use super::queue::{Controller, Disk, Op, Request};

/// Where the answers to a query land in the page: its last 2 KiB.
const ANSWER: usize = 2048;

/// Reads what `disk` says of its health into `page`: SMART for a SATA
/// disk (`ata`), as [`Health::Ata`]; else the log pages, as
/// [`Health::Scsi`]. `Unsupported` if the disk keeps no record.
pub fn health<C: Controller>(disk: &Disk<C>, page: &mut Page, ata: bool) -> Result<Health, Error> {
    page.bytes_mut().fill(0);
    if ata {
        // The data, then the thresholds, each 512 bytes.
        for (feature, at) in [(health::SMART_READ_DATA, 0), (health::SMART_READ_THRESHOLDS, 512)] {
            let data = [(page.phys() + at, 512)];
            let op = Op::Smart(feature, 0);
            disk.request(&Request { op, target: 0, sector: 0, data: &data, unit: 1 })?;
        }
        radix_hw::device_wrote(&mut page.bytes_mut()[..1024]);
        // A disk that refused leaves the page as it was.
        if page.bytes()[..512].iter().all(|&byte| byte == 0) {
            return Err(Error::Unsupported);
        }
        return Ok(Health::Ata);
    }
    let base = page.phys() + ANSWER as u64;
    let (record, answer) = page.bytes_mut().split_at_mut(ANSWER);
    let record: &mut [u8; RECORD] = (&mut record[..RECORD]).try_into().map_err(|_| Error::Io)?;
    // Why a query failed, if the queue did: a disk that refused is not an
    // error of the queue's.
    let mut failure = None;
    let mut fetch = |query: Query, out: &mut [u8]| -> Result<usize, Failed> {
        let length = out.len().min(answer.len());
        let data = [(base, length as u32)];
        let op = Op::Query(query);
        if let Err(err) = disk.request(&Request { op, target: 0, sector: 0, data: &data, unit: 1 })
        {
            failure = Some(err);
            return Err(Failed);
        }
        radix_hw::device_wrote(&mut answer[..length]);
        out[..length].copy_from_slice(&answer[..length]);
        Ok(length)
    };
    match health::gather(&mut fetch, record) {
        Ok(()) => Ok(Health::Scsi),
        Err(Failed) => Err(failure.unwrap_or(Error::Unsupported)),
    }
}

/// The INQUIRY vendor of a SATA disk behind a SCSI layer.
pub const ATA_VENDOR: &str = "ATA";

/// The command that asks `query` for as many bytes as the request's
/// buffers hold (`data`: physical address, length), for a lane to send.
pub fn query_cdb(query: Query, data: &[(u64, u32)]) -> Cdb {
    let length = data.iter().map(|&(_, len)| len).sum::<u32>();
    let length = u16::try_from(length).unwrap_or(u16::MAX);
    match query {
        Query::LogSense(page) => Cdb::log_sense(page, length),
        Query::GrownDefects => Cdb::read_defect_data_10(length),
    }
}
