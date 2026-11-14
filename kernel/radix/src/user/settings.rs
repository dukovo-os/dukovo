// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The methods of the kernel settings capability (Phase 16,
//! radix-kernel-settings(2), ADR 0476): listing the settings, and
//! changing one ([`crate::settings`]), which is logged with who changed
//! it.

use radix_abi::errors::Kernel;
use radix_abi::kernel_settings::{
    self as abi, LINE_MAX, ListArgs, ListResults, NAME_MAX, SetArgs, VALUE_MAX,
};
use radix_cap::{Capability, ObjectKind, Rights};
use radix_hal::UserMode;
use radix_klog::TextBuf;

use super::methods::{Answer, answer};
use super::{Result, UserRef, cap_error, error};
use crate::Arch;
use crate::settings::{self, Refused};

/// Calls the method `label` on `cap`, a kernel settings capability, for
/// `user`: kept out of line, so that [`super::methods::call`]'s frame does
/// not carry its own.
#[inline(never)]
pub fn method(user: &UserRef, cap: &Capability, label: u32, args: &[u64; 8]) -> Result<Answer> {
    let check = |rights| cap.check(ObjectKind::KernelSettings, rights).map_err(cap_error);
    match label {
        abi::LIST => {
            let a = ListArgs::decode(args)?;
            check(Rights::READ)?;
            let index = a.index as usize;
            if index >= settings::count() {
                return Err(error(Kernel::OutOfRange));
            }
            let mut line = TextBuf::<{ LINE_MAX as usize }>::new();
            settings::line(index, &mut line).map_err(|_| error(Kernel::InvalidArgument))?;
            let bytes = line.as_str().as_bytes();
            if bytes.len() as u64 > a.buffer.len {
                return Err(error(Kernel::InvalidArgument));
            }
            if !radix_ipc::range::user_bytes(a.buffer.address, a.buffer.len)
                || !Arch::copy_to_user(a.buffer.address, bytes)
            {
                return Err(error(Kernel::BadAddress));
            }
            let results = ListResults { len: bytes.len() as u64 };
            answer(label, ListResults::WORDS, results.encode())
        }
        abi::SET => {
            let a = SetArgs::decode(args)?;
            check(Rights::CONTROL)?;
            let mut name = [0u8; NAME_MAX as usize];
            let mut value = [0u8; VALUE_MAX as usize];
            // A name or value longer than any is no setting's, or one it
            // takes.
            if a.name.len > NAME_MAX {
                return Err(error(Kernel::OutOfRange));
            }
            if a.value.len > VALUE_MAX {
                return Err(error(Kernel::InvalidArgument));
            }
            let name = read_text(&mut name, a.name.address, a.name.len)?;
            let value = read_text(&mut value, a.value.address, a.value.len)?;
            let by = user.process.name();
            match settings::set(name, value, by.as_str()) {
                Ok(()) => answer(label, 0, [0; 8]),
                Err(Refused::NoSuchSetting) => Err(error(Kernel::OutOfRange)),
                Err(Refused::Value) => Err(error(Kernel::InvalidArgument)),
                Err(Refused::NotHere) => Err(error(Kernel::NotSupported)),
            }
        }
        _ => Err(error(Kernel::NoSuchMethod)),
    }
}

/// Reads the `len` bytes at `address` into `room`, which holds them, as
/// text: invalid_argument if they are not UTF-8.
fn read_text(room: &mut [u8], address: u64, len: u64) -> Result<&str> {
    let len = len as usize;
    if !radix_ipc::range::user_bytes(address, len as u64)
        || !Arch::copy_from_user(&mut room[..len], address)
    {
        return Err(error(Kernel::BadAddress));
    }
    core::str::from_utf8(&room[..len]).map_err(|_| error(Kernel::InvalidArgument))
}
