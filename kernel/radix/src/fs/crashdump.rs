// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `/devices/crashdump` (ADR 0483): the crash dump the previous boot left
//! when it panicked, then that boot's whole log, which dumps(8) saves to
//! the disk at boot; nothing when that boot left none. Writing `panic` to
//! it panics the machine on purpose, so that an administrator can check
//! once that a panic's dump is kept and saved (`dumps test`).
//!
//! The dump is in the RAM the crash log keeps across a restart
//! (`crate::crashlog`), which this boot never writes, so a read gives the
//! same bytes for the whole boot. Reads follow the offset, `lseek` works
//! as on a file of that length, and `fstat` gives the length.

use core::any::Any;

use radix_klog::warn;
use radix_vfs::{Access, Error, In, Kind, Node, Out, Result, Stat};

use crate::{crashlog, sched};

/// What a write must hold to panic the machine.
const PANIC: &[u8] = b"panic\n";
/// The panic's message, which the dump and the log then hold.
pub const TEST_PANIC: &str = "dumps test: a panic asked for on purpose";
/// Bytes a read copies at a time, through a buffer on the stack, which a
/// kernel function keeps under 2 KiB (ADR 0058).
const CHUNK: usize = 1024;

/// The node, numbered as its id.
pub struct CrashDump(pub u64);

impl Node for CrashDump {
    // Root's alone: a dump holds what the kernel held, and a write panics
    // the machine.
    fn access(&self) -> Access {
        Access { owner: 0, group: 0, mode: 0o600 }
    }

    fn stat(&self) -> Stat {
        Stat {
            kind: Kind::CharDevice,
            size: crashlog::previous_dump_len(),
            allocated: 0,
            id: self.0,
            created: 0,
            modified: 0,
            accessed: 0,
            changed: 0,
            links: 1,
            device: 0,
        }
    }

    fn kind(&self) -> Kind {
        Kind::CharDevice
    }

    fn read(&self, offset: u64, out: &mut dyn Out) -> Result<usize> {
        let mut chunk = [0u8; CHUNK];
        let mut done = 0;
        while done < out.len() {
            let want = (out.len() - done).min(CHUNK);
            let got = crashlog::read_previous_dump(offset + done as u64, &mut chunk[..want]);
            if got == 0 {
                break;
            }
            out.put(done, &chunk[..got])?;
            done += got;
        }
        Ok(done)
    }

    fn write(&self, _offset: u64, data: &dyn In) -> Result<usize> {
        if data.len() != PANIC.len() {
            return Err(Error::InvalidArgument);
        }
        let mut text = [0u8; PANIC.len()];
        data.get(0, &mut text)?;
        if text != PANIC {
            return Err(Error::InvalidArgument);
        }
        let me = sched::running();
        let process = me.as_ref().and_then(|me| me.user()).map_or(0, |user| user.process.number());
        let name = me.as_ref().map(|me| me.name());
        let name = name.as_ref().map_or("?", |name| name.as_str());
        warn!("Crash dump: a test panic was asked for by process {process} ({name})");
        panic!("{TEST_PANIC}");
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
