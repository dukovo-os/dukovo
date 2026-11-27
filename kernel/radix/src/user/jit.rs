// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The methods of the jit capability (Phase 18, radix-jit(2), ADR 0624):
//! giving a process memory.jit, which lets it make its own private
//! anonymous memory writable and executable at once
//! (`methods::anonymous_access`, the Linux table's `mmap` and
//! `mprotect`). The process keeps it until it ends, through `fork` and
//! `exec` (`ProcessObject::allow_jit`); and telling whether a process has
//! it, and whether it was refused such memory for want of it.

use alloc::sync::Arc;

use radix_abi::errors::Kernel;
use radix_abi::jit::{self as abi, AllowArgs, StateArgs, StateResults};
use radix_cap::{Capability, ObjectKind, Rights};
use radix_klog::info;

use super::methods::{Answer, answer};
use super::objects::{self, Object};
use super::{Result, UserRef, cap_error, error};

/// Calls the method `label` on `cap`, a jit capability, for `user`.
#[inline(never)]
pub fn method(user: &UserRef, cap: &Capability, label: u32, args: &[u64; 8]) -> Result<Answer> {
    cap.check(ObjectKind::Jit, Rights::CONTROL).map_err(cap_error)?;
    match label {
        abi::ALLOW => {
            let a = AllowArgs::decode(args)?;
            let (_, Object::Process(process)) =
                objects::lookup(user.cspace, a.process, ObjectKind::Process, Rights::CONTROL)?
            else {
                return Err(error(Kernel::Gone));
            };
            // A holder gives memory.jit to a process it starts; it does
            // not take it for itself.
            if Arc::ptr_eq(&process, &user.process) {
                return Err(error(Kernel::InvalidArgument));
            }
            if !process.jit() {
                process.allow_jit();
                info!(
                    "Jit: process {} gave process {} memory.jit",
                    user.process.number(),
                    process.number()
                );
            }
            answer(label, 0, [0; 8])
        }
        abi::STATE => {
            let a = StateArgs::decode(args)?;
            let (_, Object::Process(process)) =
                objects::lookup(user.cspace, a.process, ObjectKind::Process, Rights::READ)?
            else {
                return Err(error(Kernel::Gone));
            };
            let results = StateResults { given: process.jit(), refused: process.jit_refused() };
            answer(label, StateResults::WORDS, results.encode())
        }
        _ => Err(error(Kernel::NoSuchMethod)),
    }
}
