// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The commands a disk's health takes, and the record that packs what the
//! disk answers (ADR 0665), against a disk that is simulated.

use std::vec::Vec;

use crate::health::*;
use crate::{Cdb, op};

/// A log page: its header and `body`.
fn page(code: u8, body: &[u8]) -> Vec<u8> {
    let mut bytes = std::vec![code, 0];
    bytes.extend((body.len() as u16).to_be_bytes());
    bytes.extend_from_slice(body);
    bytes
}

/// A disk that has `pages`, grown defects `defects` as the header it
/// answers READ DEFECT DATA with (or refuses), and the queries it was asked.
struct Sim {
    pages: Vec<(u8, Vec<u8>)>,
    defects: Option<[u8; 4]>,
    asked: Vec<Query>,
}

impl Sim {
    fn new(pages: &[(u8, usize)], defects: Option<[u8; 4]>) -> Sim {
        // The list of pages the disk has, and each page with a body of the
        // length given, its bytes its page code.
        let list: Vec<u8> = pages.iter().map(|&(code, _)| code).collect();
        let mut all = std::vec![(0, page(0, &list))];
        for &(code, len) in pages {
            all.push((code, page(code, &std::vec![code; len])));
        }
        Sim { pages: all, defects, asked: Vec::new() }
    }

    fn fetch(&mut self, query: Query, buffer: &mut [u8]) -> Result<usize, Failed> {
        self.asked.push(query);
        match query {
            Query::LogSense(code) => {
                let (_, bytes) = self.pages.iter().find(|(c, _)| *c == code).ok_or(Failed)?;
                let n = bytes.len().min(buffer.len());
                buffer[..n].copy_from_slice(&bytes[..n]);
                Ok(n)
            }
            Query::GrownDefects => {
                let header = self.defects.ok_or(Failed)?;
                buffer.copy_from_slice(&header[..buffer.len()]);
                Ok(buffer.len())
            }
        }
    }

    fn gather(&mut self, record: &mut [u8; RECORD]) -> Result<(), Failed> {
        let mut fetch = |query, buffer: &mut [u8]| self.fetch(query, buffer);
        gather(&mut fetch, record)
    }
}

/// The entries of a record: page code, and what they hold.
fn entries(record: &[u8; RECORD]) -> Vec<(u8, Vec<u8>)> {
    let mut found = Vec::new();
    let mut at = 0;
    while at + ENTRY_HEADER <= RECORD {
        let (code, length) =
            (record[at], usize::from(u16::from_le_bytes([record[at + 2], record[at + 3]])));
        if code == 0 && length == 0 {
            break;
        }
        found.push((code, record[at + 4..at + 4 + length].to_vec()));
        at += ENTRY_HEADER + length;
    }
    found
}

#[test]
fn the_commands_are_the_bytes_a_disk_takes() {
    assert_eq!(
        Cdb::log_sense(0x2f, 1024).bytes(),
        [op::LOG_SENSE, 0, 0x6f, 0, 0, 0, 0, 0x04, 0x00, 0]
    );
    assert_eq!(
        Cdb::read_defect_data_10(4).bytes(),
        [op::READ_DEFECT_DATA_10, 0, 0x0f, 0, 0, 0, 0, 0, 4, 0]
    );
    for feature in [SMART_READ_DATA, SMART_READ_THRESHOLDS] {
        let cdb = Cdb::smart_read(feature);
        assert_eq!(
            cdb.bytes(),
            [0x85, 0x08, 0x0e, 0, feature, 0, 1, 0, 0, 0, 0x4f, 0, 0xc2, 0, 0xb0, 0]
        );
    }
}

#[test]
fn the_supported_pages_make_a_set() {
    let list = page(0, &[0, 2, 3, 0x0d, 0x2f, 0x3f]);
    let set = Supported::parse(&list).unwrap();
    for code in [0, 2, 3, 0x0d, 0x2f, 0x3f] {
        assert!(set.contains(code));
    }
    assert!(!set.contains(5) && !set.contains(0x10));
    // Cut short of the length it gives: what is there counts.
    assert!(Supported::parse(&list[..6]).unwrap().contains(2));
    assert!(!Supported::parse(&list[..6]).unwrap().contains(3));
    // Not the list: another page, no pages, no header, or all zeros, as a
    // disk that refused leaves its buffer.
    assert_eq!(Supported::parse(&page(2, &[2])), None);
    assert_eq!(Supported::parse(&page(0, &[])), None);
    assert_eq!(Supported::parse(&[0, 0]), None);
    assert_eq!(Supported::parse(&[0; 64]), None);
}

#[test]
fn the_defect_count_comes_from_the_header() {
    // Bytes from index: 8 a defect; short block: 4; any format, 8.
    assert_eq!(grown_defects(&[0, 0x0c, 0, 24]), Some(3));
    assert_eq!(grown_defects(&[0, 0x08, 0, 24]), Some(6));
    assert_eq!(grown_defects(&[0, 0x0d, 0, 0]), Some(0));
    // The grown list is not in the answer, or there is no header.
    assert_eq!(grown_defects(&[0, 0x04, 0, 24]), None);
    assert_eq!(grown_defects(&[0, 0, 0, 0]), None);
    assert_eq!(grown_defects(&[0, 0x0c, 0]), None);
}

#[test]
fn a_record_has_the_defects_first_and_the_pages_as_the_disk_gave_them() {
    let mut disk =
        Sim::new(&[(0x2f, 8), (0x0d, 12), (0x10, 20), (0x01, 4)], Some([0, 0x0c, 0, 16]));
    let mut record = [0xaa; RECORD];
    disk.gather(&mut record).unwrap();
    let found = entries(&record);
    let codes: Vec<u8> = found.iter().map(|(code, _)| *code).collect();
    // Only the pages that are wanted, in the order they are wanted.
    assert_eq!(codes, [DEFECTS_ENTRY, 0x2f, 0x0d, 0x10]);
    assert_eq!(found[0].1, [2, 0, 0, 0]);
    assert_eq!(found[1].1, page(0x2f, &[0x2f; 8]));
    assert_eq!(found[2].1, page(0x0d, &[0x0d; 12]));
    // The record ends with zeros after the last entry.
    let used: usize = found.iter().map(|(_, bytes)| ENTRY_HEADER + bytes.len()).sum();
    assert!(record[used..].iter().all(|&byte| byte == 0));
    // The pages it did not list were not asked for, and the defects last.
    assert_eq!(
        disk.asked,
        [
            Query::LogSense(0),
            Query::LogSense(0x2f),
            Query::LogSense(0x0d),
            Query::LogSense(0x10),
            Query::GrownDefects
        ]
    );
}

#[test]
fn a_refusal_of_the_defects_leaves_their_entry_out() {
    for defects in [None, Some([0; 4])] {
        let mut disk = Sim::new(&[(0x2f, 8), (0x0d, 12)], defects);
        let mut record = [0xaa; RECORD];
        disk.gather(&mut record).unwrap();
        let found = entries(&record);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, 0x2f);
        assert_eq!(record[0], 0x2f);
        let used: usize = found.iter().map(|(_, bytes)| ENTRY_HEADER + bytes.len()).sum();
        assert!(record[used..].iter().all(|&byte| byte == 0));
    }
}

#[test]
fn an_entry_that_does_not_fit_is_cut_and_the_record_ends_there() {
    // Two pages of 600 bytes: the first fits, the second is cut to what is
    // left, and the pages after are not asked for.
    let mut disk = Sim::new(&[(0x2f, 600), (0x0d, 600), (0x10, 20)], Some([0, 0x0c, 0, 8]));
    let mut record = [0; RECORD];
    disk.gather(&mut record).unwrap();
    let found = entries(&record);
    assert_eq!(found.len(), 3);
    assert_eq!(found[1].1.len(), 604);
    let left = RECORD - 8 - (ENTRY_HEADER + 604) - ENTRY_HEADER;
    assert_eq!(found[2].0, 0x0d);
    assert_eq!(found[2].1.len(), left);
    assert_eq!(found[2].1, page(0x0d, &[0x0d; 600])[..left]);
    // The record is full to its last byte.
    assert_eq!(RECORD, 8 + (ENTRY_HEADER + 604) + ENTRY_HEADER + left);
    assert!(!disk.asked.contains(&Query::LogSense(0x10)));
    assert_eq!(disk.asked.last(), Some(&Query::GrownDefects));
}

#[test]
fn a_page_longer_than_the_answer_is_what_was_answered() {
    // The header says 600 bytes; the buffer holds 1024, and the disk sent
    // 100: what a disk that answers less than it said does.
    let mut bytes = page(0x2f, &[7; 596]);
    bytes.truncate(100);
    let mut disk = Sim::new(&[(0x2f, 0)], None);
    disk.pages[1].1 = bytes;
    let mut record = [0; RECORD];
    disk.gather(&mut record).unwrap();
    let found = entries(&record);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1.len(), 100);
}

#[test]
fn a_disk_with_no_list_of_pages_keeps_no_record() {
    // Refused, as QEMU's disks do, or answered with zeros.
    let mut disk = Sim::new(&[(0x2f, 8)], Some([0, 0x0c, 0, 8]));
    disk.pages.clear();
    let mut record = [0; RECORD];
    assert_eq!(disk.gather(&mut record), Err(Failed));
    assert_eq!(disk.asked, [Query::LogSense(0)]);
    let mut zeros = |_: Query, buffer: &mut [u8]| Ok(buffer.len());
    assert_eq!(gather(&mut zeros, &mut record), Err(Failed));
}

#[test]
fn a_page_that_fails_is_left_out() {
    let mut disk = Sim::new(&[(0x2f, 8), (0x0d, 12), (0x03, 4)], None);
    disk.pages.retain(|(code, _)| *code != 0x0d);
    let mut record = [0; RECORD];
    disk.gather(&mut record).unwrap();
    let codes: Vec<u8> = entries(&record).iter().map(|(code, _)| *code).collect();
    assert_eq!(codes, [0x2f, 0x03]);
}

#[test]
fn a_disk_with_no_wanted_page_gives_an_empty_record() {
    let mut disk = Sim::new(&[(0x01, 8)], None);
    let mut record = [0xaa; RECORD];
    disk.gather(&mut record).unwrap();
    assert!(record.iter().all(|&byte| byte == 0));
}

#[test]
fn the_wanted_pages_are_ten_and_different() {
    let mut sorted = WANTED;
    sorted.sort_unstable();
    assert!(sorted.windows(2).all(|pair| pair[0] != pair[1]));
    assert_eq!(WANTED[0], INFORMATIONAL_EXCEPTIONS);
    assert_eq!(WANTED[9], SELF_TEST_RESULTS);
}
