// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Which of the commands a program sends an optical drive MMC_COMMAND
//! passes (ADR 0449), which way their data goes, and the MODE SELECT
//! parameter lists it takes.

use proptest::prelude::*;
use std::vec::Vec;

use crate::op;
use crate::recording::{Direction, Passed, mode_select_allowed, opcode, passed};

/// A command block of 16 bytes beginning with `opcode`.
fn cdb(opcode: u8) -> [u8; 16] {
    let mut cdb = [0u8; 16];
    cdb[0] = opcode;
    cdb
}

#[test]
fn the_commands_that_ask_pass_with_their_data_from_the_drive() {
    for code in [
        op::INQUIRY,
        op::REQUEST_SENSE,
        op::GET_CONFIGURATION,
        op::READ_DISC_INFORMATION,
        opcode::READ_TRACK_INFORMATION,
        op::READ_TOC,
        op::MODE_SENSE_10,
        opcode::READ_FORMAT_CAPACITIES,
        opcode::READ_BUFFER_CAPACITY,
        opcode::GET_PERFORMANCE,
        op::READ_CAPACITY_10,
    ] {
        let found = passed(&cdb(code)).unwrap_or_else(|| panic!("{code:#04x} passes"));
        assert_eq!(found.direction, Direction::FromDrive, "{code:#04x}");
        assert!(!found.writes, "{code:#04x} only asks");
    }
    let ready = passed(&cdb(op::TEST_UNIT_READY));
    assert_eq!(ready, Some(Passed { length: 6, direction: Direction::None, writes: false }));
}

#[test]
fn the_commands_that_record_need_the_write_right() {
    let expect = [
        (op::WRITE_10, 10, Direction::ToDrive),
        (opcode::WRITE_12, 12, Direction::ToDrive),
        (op::FORMAT_UNIT, 6, Direction::ToDrive),
        (opcode::MODE_SELECT_10, 10, Direction::ToDrive),
        (opcode::SEND_CUE_SHEET, 10, Direction::ToDrive),
        (opcode::SET_STREAMING, 12, Direction::ToDrive),
        (op::SYNCHRONIZE_CACHE_10, 10, Direction::None),
        (opcode::RESERVE_TRACK, 10, Direction::None),
        (opcode::CLOSE_TRACK_SESSION, 10, Direction::None),
        (opcode::BLANK, 12, Direction::None),
        (opcode::SET_CD_SPEED, 12, Direction::None),
    ];
    for (code, length, direction) in expect {
        assert_eq!(passed(&cdb(code)), Some(Passed { length, direction, writes: true }));
    }
}

#[test]
fn reads_of_data_ejects_firmware_and_events_do_not_pass() {
    for code in [
        op::READ_10,
        op::READ_16,
        op::WRITE_16,
        op::READ_CD,
        op::START_STOP_UNIT,
        op::PREVENT_ALLOW_MEDIUM_REMOVAL,
        op::GET_EVENT_STATUS_NOTIFICATION,
        // WRITE BUFFER, which loads firmware; SEND KEY and REPORT KEY.
        0x3b,
        0xa3,
        0xa4,
        op::MODE_SENSE_6,
        // MODE SELECT(6).
        0x15,
    ] {
        assert_eq!(passed(&cdb(code)), None, "{code:#04x} does not pass");
    }
    assert_eq!(passed(&[]), None);
    // A block shorter than its opcode's group says.
    assert_eq!(passed(&[op::WRITE_10, 0, 0]), None);
}

#[test]
fn a_mode_select_that_saves_its_page_does_not_pass() {
    let mut saving = cdb(opcode::MODE_SELECT_10);
    saving[1] = 0x11;
    assert_eq!(passed(&saving), None);
    saving[1] = 0x10;
    assert!(passed(&saving).is_some());
}

/// A MODE SELECT(10) parameter list: the header, `descriptors` bytes of
/// block descriptors, and a page of `code` with `body` bytes.
fn mode_list(descriptors: u16, code: u8, body: usize) -> Vec<u8> {
    let mut list = vec![0u8; 8];
    list[6..8].copy_from_slice(&descriptors.to_be_bytes());
    list.extend(core::iter::repeat_n(0, usize::from(descriptors)));
    list.push(code);
    list.push(body as u8);
    list.extend(core::iter::repeat_n(0, body));
    list
}

#[test]
fn only_the_write_parameters_page_is_set() {
    assert!(mode_select_allowed(&mode_list(0, 0x05, 0x32)));
    assert!(mode_select_allowed(&mode_list(8, 0x05, 0x32)));
    // The PS bit, which MODE SENSE says and MODE SELECT must clear, is
    // the drive's to refuse.
    assert!(mode_select_allowed(&mode_list(0, 0x85, 0x32)));
    // The CD/DVD capabilities page, the power page, the caching page.
    for code in [0x2a, 0x1a, 0x08] {
        assert!(!mode_select_allowed(&mode_list(0, code, 0x10)), "{code:#04x}");
    }
    // Cut short, or with a second page after the first.
    let whole = mode_list(0, 0x05, 0x32);
    assert!(!mode_select_allowed(&whole[..whole.len() - 1]));
    let mut two = whole.clone();
    two.extend(mode_list(0, 0x08, 0x10)[8..].iter());
    assert!(!mode_select_allowed(&two));
    assert!(!mode_select_allowed(&whole[..7]));
}

proptest! {
    #[test]
    fn no_command_block_panics(bytes in proptest::collection::vec(any::<u8>(), 0..20)) {
        if let Some(found) = passed(&bytes) {
            prop_assert!(bytes.len() >= found.length);
            prop_assert!(matches!(found.length, 6 | 10 | 12));
        }
        let _ = mode_select_allowed(&bytes);
    }
}
