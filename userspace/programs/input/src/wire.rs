// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The records of radix-input(2), as a program reads them.
//!
//! Each reader takes what the kernel wrote and leaves out a record whose
//! bytes are too few, so that nothing here panics whatever a read
//! returned. The kernel's side of each record is `radix_input::event`,
//! and the tests check the two against each other.

use radix_abi::input::{Event, InputAxis, InputChange, InputEvent, InputInfo, Motion};

fn le16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn le64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

fn signed(bytes: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// The events a read of a device's file put into `bytes`: every whole
/// one, the oldest first. What one report said ends with an EV_SYN event
/// whose code is SYN_REPORT.
pub fn events(bytes: &[u8]) -> impl Iterator<Item = InputEvent> + '_ {
    bytes.as_chunks::<{ InputEvent::SIZE }>().0.iter().filter_map(|event| {
        Some(InputEvent {
            time_ns: le64(event, InputEvent::OFFSET_TIME_NS)?,
            kind: le16(event, InputEvent::OFFSET_KIND)?,
            code: le16(event, InputEvent::OFFSET_CODE)?,
            value: signed(event, InputEvent::OFFSET_VALUE)?,
        })
    })
}

/// The keys a read of _/devices/kbd_ put into `bytes`: every whole one,
/// the oldest first.
pub fn keys(bytes: &[u8]) -> impl Iterator<Item = Event> + '_ {
    bytes.as_chunks::<{ Event::SIZE }>().0.iter().filter_map(|key| {
        Some(Event {
            time_ns: le64(key, Event::OFFSET_TIME_NS)?,
            code: le16(key, Event::OFFSET_CODE)?,
            value: le16(key, Event::OFFSET_VALUE)?,
            reserved: 0,
        })
    })
}

/// The motions a read of _/devices/mouse_ put into `bytes`: every whole
/// one, the oldest first.
pub fn motions(bytes: &[u8]) -> impl Iterator<Item = Motion> + '_ {
    bytes.as_chunks::<{ Motion::SIZE }>().0.iter().filter_map(|motion| {
        Some(Motion {
            time_ns: le64(motion, Motion::OFFSET_TIME_NS)?,
            dx: signed(motion, Motion::OFFSET_DX)?,
            dy: signed(motion, Motion::OFFSET_DY)?,
            wheel: signed(motion, Motion::OFFSET_WHEEL)?,
            pan: signed(motion, Motion::OFFSET_PAN)?,
            x: le32(motion, Motion::OFFSET_X)?,
            y: le32(motion, Motion::OFFSET_Y)?,
            buttons: le32(motion, Motion::OFFSET_BUTTONS)?,
            flags: le32(motion, Motion::OFFSET_FLAGS)?,
        })
    })
}

/// The devices that came and went, as a read of
/// _/devices/input/control_ put them into `bytes`: every whole one, the
/// oldest first.
pub fn changes(bytes: &[u8]) -> impl Iterator<Item = InputChange> + '_ {
    bytes.as_chunks::<{ InputChange::SIZE }>().0.iter().filter_map(|change| {
        Some(InputChange {
            time_ns: le64(change, InputChange::OFFSET_TIME_NS)?,
            device: le32(change, InputChange::OFFSET_DEVICE)?,
            what: le32(change, InputChange::OFFSET_WHAT)?,
        })
    })
}

/// The devices' numbers INPUT_LIST wrote into `bytes`: every whole one.
pub fn numbers(bytes: &[u8]) -> impl Iterator<Item = u32> + '_ {
    bytes.as_chunks::<4>().0.iter().map(|number| u32::from_le_bytes(*number))
}

/// INPUT_INFO's answer.
pub fn info(bytes: &[u8]) -> Option<InputInfo> {
    Some(InputInfo {
        device: le32(bytes, InputInfo::OFFSET_DEVICE)?,
        bus: le16(bytes, InputInfo::OFFSET_BUS)?,
        vendor: le16(bytes, InputInfo::OFFSET_VENDOR)?,
        product: le16(bytes, InputInfo::OFFSET_PRODUCT)?,
        version: le16(bytes, InputInfo::OFFSET_VERSION)?,
        rate_hz: le32(bytes, InputInfo::OFFSET_RATE_HZ)?,
    })
}

/// INPUT_AXIS's answer.
pub fn axis(bytes: &[u8]) -> Option<InputAxis> {
    Some(InputAxis {
        value: signed(bytes, InputAxis::OFFSET_VALUE)?,
        min: signed(bytes, InputAxis::OFFSET_MIN)?,
        max: signed(bytes, InputAxis::OFFSET_MAX)?,
        fuzz: signed(bytes, InputAxis::OFFSET_FUZZ)?,
        flat: signed(bytes, InputAxis::OFFSET_FLAT)?,
        resolution: signed(bytes, InputAxis::OFFSET_RESOLUTION)?,
    })
}

/// The text of a name, a place or a serial number as the kernel answered
/// it: up to the first zero if there is one, or nothing if it is not
/// UTF-8.
pub fn text(bytes: &[u8]) -> &str {
    let end = bytes.iter().position(|&byte| byte == 0).unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..end]).unwrap_or("")
}
