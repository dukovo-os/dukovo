// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The methods of the platform capability (Phase 11, radix-platform(2),
//! ADR 0208): the controls the machine's embedded controller keeps, its
//! fans' duties and curves, its LEDs and its ARGB header, through
//! [`crate::crosec`]; the firmware's SMBIOS tables whole (ADR 0231); and
//! its UEFI variables (ADR 0313), through the firmware's runtime services.
//!
//! The arguments are checked here before anything reaches the EC, which
//! takes what it is sent as it is: Framework's EC copies a fan curve
//! without looking at it, and divides by its two temperatures' difference.
//!
//! The firmware gets copies in the kernel's memory, never the caller's
//! pages: it runs with page tables of its own, which map only the kernel's
//! half. Of its variables the kernel writes only those the boot manager
//! reads ([`writable`]), and never so many that the firmware's store
//! fills: firmware has failed to start with it full, and deleting the
//! wrong variables has stopped others starting at all.

use alloc::vec::Vec;
use core::fmt::Write;

use radix_abi::Status;
use radix_abi::UserBuffer;
use radix_abi::errors::{File, Kernel};
use radix_abi::platform::{
    self as abi, AutoFanArgs, AutoLedArgs, Controls, DescribeArgs, DescribeResults, EfiAttributes,
    EfiVariableArgs, EfiVariableResults, EfiVariableSpaceArgs, EfiVariableSpaceResults,
    FanCurveArgs, FanCurveResults, LedArgs, LedResults, NextEfiVariableArgs,
    NextEfiVariableResults, SetEfiVariableArgs, SetFanCurveArgs, SetFanDutyArgs, SetLedArgs,
    SetRgbArgs, SmbiosArgs, SmbiosResults, SmbiosTable,
};
use radix_cap::{Capability, ObjectKind, Rights};
use radix_crosec::{Error, FanCurve};
use radix_hal::{Platform, UserMode, VariableError};
use radix_mm::{Flags, HEAP, Kmalloc};

use super::methods::{Answer, answer};
use super::{Result, UserRef, cap_error, error};
use crate::Arch;
use crate::crosec::{self, Failed};

/// The addressable LEDs a request to the EC takes at most, copied from
/// the caller a request at a time.
const RGB_CHUNK: usize = 82;

/// The UEFI global variable vendor's GUID, as it lies in memory.
const GLOBAL_VARIABLE: [u8; 16] = vendor(abi::GLOBAL_VARIABLE_LOW, abi::GLOBAL_VARIABLE_HIGH);

/// The longest name the methods take, in UCS-2 characters, without its
/// NUL.
const NAME_MAX: usize = abi::EFI_NAME_MAX as usize / 2;

/// The room a write must leave in the firmware's store, as Linux's
/// `EFI_MIN_RESERVE` leaves it: some firmware has failed to start with
/// less (Samsung's laptops, in 2013).
const MIN_RESERVE: u64 = 5 * 1024;

/// The variables of the global vendor the kernel writes, by whole name.
const WRITABLE: [&str; 6] =
    ["BootOrder", "BootNext", "Timeout", "DriverOrder", "SysPrepOrder", "OsIndications"];

/// And those named by a word and four hexadecimal digits: `Boot0001`.
const WRITABLE_NUMBERED: [&str; 3] = ["Boot", "Driver", "SysPrep"];

/// Calls the method `label` on `cap`, a platform capability, for `user`:
/// kept out of line, so that [`super::methods::call`]'s frame, under every
/// method and the deep ones like a mount's, does not carry its own.
#[inline(never)]
pub fn method(user: &UserRef, cap: &Capability, label: u32, args: &[u64; 8]) -> Result<Answer> {
    let check = |rights| cap.check(ObjectKind::Platform, rights).map_err(cap_error);
    match label {
        abi::DESCRIBE => {
            DescribeArgs::decode(args)?;
            check(Rights::READ)?;
            let (controls, fans, sensors, leds) =
                crosec::describe().ok_or(error(Kernel::NotSupported))?;
            let results = DescribeResults {
                controls: Controls(u32::from(controls)),
                fans: u32::from(fans),
                sensors,
                leds,
            };
            answer(label, DescribeResults::WORDS, results.encode())
        }
        abi::SET_FAN_DUTY => {
            let a = SetFanDutyArgs::decode(args)?;
            check(Rights::CONTROL)?;
            let percent = u8::try_from(a.percent)
                .ok()
                .filter(|&percent| percent <= 100)
                .ok_or(error(Kernel::InvalidArgument))?;
            let name = user.process.name();
            crosec::set_fan_duty(a.fan, percent, name.as_str()).map_err(failed)?;
            answer(label, 0, [0; 8])
        }
        abi::AUTO_FAN => {
            let a = AutoFanArgs::decode(args)?;
            check(Rights::CONTROL)?;
            crosec::auto_fan(a.fan, user.process.name().as_str()).map_err(failed)?;
            answer(label, 0, [0; 8])
        }
        abi::FAN_CURVE => {
            let a = FanCurveArgs::decode(args)?;
            check(Rights::READ)?;
            let curve = crosec::fan_curve(a.fan).map_err(failed)?;
            let results = FanCurveResults {
                min_temperature: u32::from(curve.min_temperature),
                max_temperature: u32::from(curve.max_temperature),
                min_duty: u32::from(curve.min_duty),
                max_duty: u32::from(curve.max_duty),
                always_on: curve.always_on,
                sensor: u32::from(curve.sensor),
                duty: u32::from(curve.duty),
            };
            answer(label, FanCurveResults::WORDS, results.encode())
        }
        abi::SET_FAN_CURVE => {
            let a = SetFanCurveArgs::decode(args)?;
            check(Rights::CONTROL)?;
            let byte = |value: u32| u8::try_from(value).map_err(|_| error(Kernel::InvalidArgument));
            let curve = FanCurve {
                max_duty: byte(a.max_duty)?,
                min_duty: byte(a.min_duty)?,
                max_temperature: byte(a.max_temperature)?,
                min_temperature: byte(a.min_temperature)?,
                always_on: a.always_on,
                sensor: u8::try_from(a.sensor).map_err(|_| error(Kernel::OutOfRange))?,
                duty: 0,
            };
            if curve.min_temperature >= curve.max_temperature
                || curve.min_duty > curve.max_duty
                || curve.max_duty > 100
            {
                return Err(error(Kernel::InvalidArgument));
            }
            crosec::set_fan_curve(a.fan, curve).map_err(failed)?;
            answer(label, 0, [0; 8])
        }
        abi::LED => {
            let a = LedArgs::decode(args)?;
            check(Rights::READ)?;
            let [red, green, blue, yellow, white, amber] =
                crosec::led_range(a.led.raw()).map_err(failed)?.map(u32::from);
            let results = LedResults { red, green, blue, yellow, white, amber };
            answer(label, LedResults::WORDS, results.encode())
        }
        abi::SET_LED => {
            let a = SetLedArgs::decode(args)?;
            check(Rights::CONTROL)?;
            let mut brightness = [0; 6];
            for (to, from) in
                brightness.iter_mut().zip([a.red, a.green, a.blue, a.yellow, a.white, a.amber])
            {
                *to = u8::try_from(from).map_err(|_| error(Kernel::InvalidArgument))?;
            }
            crosec::set_led(a.led.raw(), Some(brightness)).map_err(failed)?;
            answer(label, 0, [0; 8])
        }
        abi::AUTO_LED => {
            let a = AutoLedArgs::decode(args)?;
            check(Rights::CONTROL)?;
            crosec::set_led(a.led.raw(), None).map_err(failed)?;
            answer(label, 0, [0; 8])
        }
        abi::SET_RGB => {
            let a = SetRgbArgs::decode(args)?;
            check(Rights::CONTROL)?;
            set_rgb(a.first, a.colors.address, a.colors.len)?;
            answer(label, 0, [0; 8])
        }
        abi::SMBIOS => {
            let a = SmbiosArgs::decode(args)?;
            check(Rights::READ)?;
            let tables = Arch::smbios().ok_or(error(Kernel::NotSupported))?;
            let bytes = match a.table {
                SmbiosTable::EntryPoint => tables.entry,
                SmbiosTable::Structures => tables.table,
            };
            let part = &bytes[..bytes.len().min(a.buffer.len as usize)];
            if !radix_ipc::range::user_bytes(a.buffer.address, part.len() as u64)
                || !Arch::copy_to_user(a.buffer.address, part)
            {
                return Err(error(Kernel::BadAddress));
            }
            let results = SmbiosResults { length: bytes.len() as u64 };
            answer(label, SmbiosResults::WORDS, results.encode())
        }
        abi::EFI_VARIABLE => {
            let a = EfiVariableArgs::decode(args)?;
            check(Rights::READ)?;
            efi_variable(&a)
        }
        abi::SET_EFI_VARIABLE => {
            let a = SetEfiVariableArgs::decode(args)?;
            check(Rights::CONTROL)?;
            set_efi_variable(user, &a)?;
            answer(label, 0, [0; 8])
        }
        abi::NEXT_EFI_VARIABLE => {
            let a = NextEfiVariableArgs::decode(args)?;
            check(Rights::READ)?;
            next_efi_variable(&a)
        }
        abi::EFI_VARIABLE_SPACE => {
            let a = EfiVariableSpaceArgs::decode(args)?;
            check(Rights::READ)?;
            let space =
                Arch::firmware_variable_space(a.attributes.bits()).map_err(variable_failed)?;
            let results = EfiVariableSpaceResults {
                total: space.total,
                remaining: space.remaining,
                largest: space.largest,
            };
            answer(label, EfiVariableSpaceResults::WORDS, results.encode())
        }
        _ => Err(error(Kernel::NoSuchMethod)),
    }
}

/// `efi_variable`: the variable copied out whole, or only its length if
/// the caller's buffer is shorter.
fn efi_variable(a: &EfiVariableArgs) -> Result<Answer> {
    let name = efi_name(a.name, false)?;
    let room = a.data.len.min(u64::from(abi::EFI_VARIABLE_MAX)) as usize;
    let mut data = zeroed(room)?;
    let vendor = vendor(a.vendor_low, a.vendor_high);
    let (attributes, length) = match Arch::firmware_variable(&vendor, &name, &mut data) {
        Ok((attributes, length)) => {
            copy_out(a.data, &data[..length.min(room)])?;
            (attributes, length)
        }
        Err(VariableError::TooSmall(length)) => (0, length),
        Err(why) => return Err(variable_failed(why)),
    };
    // Bits newer than the interface (enhanced authentication, UEFI 2.8)
    // are left out, as a caller could not decode them.
    let attributes = EfiAttributes(attributes & EfiAttributes::ALL.bits());
    let results = EfiVariableResults { attributes, length: length as u64 };
    answer(abi::EFI_VARIABLE, EfiVariableResults::WORDS, results.encode())
}

/// `set_efi_variable`: a boot manager's variable written or deleted, if
/// the firmware keeps room enough.
fn set_efi_variable(user: &UserRef, a: &SetEfiVariableArgs) -> Result<()> {
    let name = efi_name(a.name, false)?;
    let vendor = vendor(a.vendor_low, a.vendor_high);
    // The name is ASCII once it is one the kernel writes.
    let Some(text) = writable(&vendor, &name[..name.len() - 1]) else {
        return Err(error(Kernel::NotPermitted));
    };
    if a.data.len > u64::from(abi::EFI_VARIABLE_MAX) {
        return Err(error(Kernel::InvalidArgument));
    }
    let mut data = zeroed(a.data.len as usize)?;
    if !radix_ipc::range::user_bytes(a.data.address, a.data.len)
        || !Arch::copy_from_user(&mut data, a.data.address)
    {
        return Err(error(Kernel::BadAddress));
    }
    let attributes = a.attributes.bits();
    if !data.is_empty() && a.attributes.contains(EfiAttributes::NON_VOLATILE) {
        match Arch::firmware_variable_space(attributes) {
            Ok(space) => {
                let size = (data.len() + 2 * name.len()) as u64;
                if space.remaining.saturating_sub(size) < MIN_RESERVE {
                    radix_klog::warn!(
                        "EFI: {} may not set {} ({} bytes): the firmware would have under 5 KiB \
                         left for variables",
                        user.process.name().as_str(),
                        text.as_str(),
                        data.len()
                    );
                    return Err(File::NoSpace.status(0));
                }
            }
            // UEFI 1 firmware cannot say; Linux writes anyway.
            Err(VariableError::NotSupported) => {}
            Err(why) => return Err(variable_failed(why)),
        }
    }
    Arch::set_firmware_variable(&vendor, &name, attributes, &data).map_err(variable_failed)?;
    radix_klog::info!(
        "EFI: {} {} {}{}",
        user.process.name().as_str(),
        if data.is_empty() { "deleted" } else { "set" },
        text.as_str(),
        Bytes(data.len())
    );
    Ok(())
}

/// The length a log line gives a variable written, after its name.
struct Bytes(usize);

impl core::fmt::Display for Bytes {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0 == 0 { Ok(()) } else { write!(f, " ({} bytes)", self.0) }
    }
}

/// `next_efi_variable`: the name after the caller's, copied out whole, or
/// only its length if the caller's buffer is shorter.
fn next_efi_variable(a: &NextEfiVariableArgs) -> Result<Answer> {
    let mut name = efi_name(a.name, true)?;
    // The firmware writes the next name over the last, so the buffer
    // holds both: the caller's room, or the last name if that is longer.
    let room = (a.next.len / 2).min(NAME_MAX as u64) as usize + 1;
    if name.len() < room {
        name.try_reserve_exact(room - name.len()).map_err(|_| error(Kernel::NoMemory))?;
        while name.len() < room {
            // Room was reserved above.
            let _ = name.push_within_capacity(0);
        }
    }
    let mut vendor = vendor(a.vendor_low, a.vendor_high);
    let length = match Arch::next_firmware_variable(&mut vendor, &mut name) {
        Ok(units) => {
            let length = 2 * (units - 1);
            if length as u64 <= a.next.len {
                let bytes: Vec<u8, Kmalloc> = ucs2_bytes(&name[..units - 1])?;
                copy_out(a.next, &bytes)?;
            }
            length
        }
        Err(VariableError::TooSmall(units)) => 2 * units.saturating_sub(1),
        Err(why) => return Err(variable_failed(why)),
    };
    let (vendor_low, vendor_high) = vendor_words(&vendor);
    let results = NextEfiVariableResults { vendor_low, vendor_high, length: length as u64 };
    answer(abi::NEXT_EFI_VARIABLE, NextEfiVariableResults::WORDS, results.encode())
}

/// A GUID from its two words, as the methods pass it.
const fn vendor(low: u64, high: u64) -> [u8; 16] {
    let (low, high) = (low.to_le_bytes(), high.to_le_bytes());
    let mut guid = [0; 16];
    let mut at = 0;
    while at < 8 {
        guid[at] = low[at];
        guid[at + 8] = high[at];
        at += 1;
    }
    guid
}

/// The two words of a GUID.
fn vendor_words(guid: &[u8; 16]) -> (u64, u64) {
    let (low, high) = guid.split_at(8);
    let word = |half: &[u8]| u64::from_le_bytes(half.try_into().unwrap_or_default());
    (word(low), word(high))
}

/// The caller's name at `buffer`, UCS-2 little-endian, as the firmware
/// takes it: with a NUL after it and none inside. Only `next_efi_variable`
/// takes an empty one.
fn efi_name(buffer: UserBuffer, empty: bool) -> Result<Vec<u16, Kmalloc>> {
    let len = buffer.len as usize;
    if !len.is_multiple_of(2) || len > 2 * NAME_MAX || (len == 0 && !empty) {
        return Err(error(Kernel::InvalidArgument));
    }
    let mut bytes = zeroed(len)?;
    if !radix_ipc::range::user_bytes(buffer.address, buffer.len)
        || !Arch::copy_from_user(&mut bytes, buffer.address)
    {
        return Err(error(Kernel::BadAddress));
    }
    let mut name = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
    name.try_reserve_exact(len / 2 + 1).map_err(|_| error(Kernel::NoMemory))?;
    // Room for each character and the NUL was reserved above.
    for unit in bytes.as_chunks::<2>().0 {
        match u16::from_le_bytes(*unit) {
            0 => return Err(error(Kernel::InvalidArgument)),
            unit => {
                let _ = name.push_within_capacity(unit);
            }
        }
    }
    let _ = name.push_within_capacity(0);
    Ok(name)
}

/// UCS-2 characters as the caller gets them, little-endian.
fn ucs2_bytes(name: &[u16]) -> Result<Vec<u8, Kmalloc>> {
    let mut bytes = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
    bytes.try_reserve_exact(2 * name.len()).map_err(|_| error(Kernel::NoMemory))?;
    for byte in name.iter().flat_map(|unit| unit.to_le_bytes()) {
        // Room was reserved above.
        let _ = bytes.push_within_capacity(byte);
    }
    Ok(bytes)
}

/// `len` zero bytes in the kernel's memory, for the firmware.
fn zeroed(len: usize) -> Result<Vec<u8, Kmalloc>> {
    let mut bytes = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
    bytes.try_reserve_exact(len).map_err(|_| error(Kernel::NoMemory))?;
    while bytes.len() < len {
        // Room was reserved above.
        let _ = bytes.push_within_capacity(0);
    }
    Ok(bytes)
}

/// Copies `bytes` to the caller's `buffer`, which holds them.
fn copy_out(buffer: UserBuffer, bytes: &[u8]) -> Result<()> {
    if radix_ipc::range::user_bytes(buffer.address, bytes.len() as u64)
        && Arch::copy_to_user(buffer.address, bytes)
    {
        Ok(())
    } else {
        Err(error(Kernel::BadAddress))
    }
}

/// The name, in ASCII, if the kernel writes the variable `name` of
/// `vendor` (without its NUL): one of the boot manager's, of the global
/// vendor (ADR 0313). Others are the firmware's or its maker's, which
/// the kernel leaves alone: deleting some has left machines unable to
/// start.
fn writable(vendor: &[u8; 16], name: &[u16]) -> Option<radix_klog::TextBuf<16>> {
    if *vendor != GLOBAL_VARIABLE || name.len() > 16 {
        return None;
    }
    let mut text = radix_klog::TextBuf::<16>::new();
    for &unit in name {
        let byte = u8::try_from(unit).ok().filter(u8::is_ascii_alphanumeric)?;
        let _ = text.write_char(char::from(byte));
    }
    let ascii = text.as_str();
    let numbered = |word: &str| {
        ascii.strip_prefix(word).is_some_and(|digits| {
            digits.len() == 4 && digits.bytes().all(|b| matches!(b, b'0'..=b'9' | b'A'..=b'F'))
        })
    };
    (WRITABLE.contains(&ascii) || WRITABLE_NUMBERED.iter().copied().any(numbered)).then_some(text)
}

/// The status for a call to the firmware's variable services that failed.
fn variable_failed(why: VariableError) -> Status {
    match why {
        VariableError::NotSupported => error(Kernel::NotSupported),
        VariableError::NotFound => File::NotFound.status(0),
        VariableError::TooSmall(_) | VariableError::InvalidParameter => {
            error(Kernel::InvalidArgument)
        }
        VariableError::WriteProtected => File::ReadOnly.status(0),
        VariableError::NoSpace => File::NoSpace.status(0),
        VariableError::SecurityViolation => error(Kernel::NotPermitted),
        VariableError::Device | VariableError::Faulted | VariableError::Other(_) => {
            File::Io.status(0)
        }
    }
}

/// `set_rgb`: the `len` bytes at `address` of the caller's, three for
/// each LED from `first`.
fn set_rgb(first: u32, address: u64, len: u64) -> Result<()> {
    if !len.is_multiple_of(3) {
        return Err(error(Kernel::InvalidArgument));
    }
    let count = len / 3;
    if u64::from(first) + count > u64::from(abi::RGB_LEDS) {
        return Err(error(Kernel::OutOfRange));
    }
    if !radix_ipc::range::user_bytes(address, len) {
        return Err(error(Kernel::BadAddress));
    }
    let mut colors = [[0u8; 3]; RGB_CHUNK];
    let mut done = 0;
    while done < count as usize {
        let part = (count as usize - done).min(RGB_CHUNK);
        let bytes = colors[..part].as_flattened_mut();
        if !Arch::copy_from_user(bytes, address + 3 * done as u64) {
            return Err(error(Kernel::BadAddress));
        }
        // Below RGB_LEDS, checked above.
        crosec::set_rgb((first as usize + done) as u8, &colors[..part]).map_err(failed)?;
        done += part;
    }
    Ok(())
}

/// The status for a control that failed.
fn failed(why: Failed) -> Status {
    match why {
        Failed::NotSupported | Failed::Ec(Error::Status(radix_crosec::STATUS_INVALID_COMMAND)) => {
            error(Kernel::NotSupported)
        }
        Failed::OutOfRange => error(Kernel::OutOfRange),
        Failed::Ec(Error::Status(radix_crosec::STATUS_INVALID_PARAM) | Error::TooBig) => {
            error(Kernel::InvalidArgument)
        }
        Failed::Ec(Error::Timeout) => error(Kernel::TimedOut),
        Failed::Ec(Error::Status(_) | Error::BadResponse) => File::Io.status(0),
    }
}
