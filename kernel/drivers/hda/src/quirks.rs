// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What particular codecs need beyond the specification, each from
//! FreeBSD's hdaa(4) (`hdaa_patch_direct` in sys/dev/sound/pci/hda/
//! hdaa_patches.c, BSD-2-Clause): the facts only, in code of our own.

use crate::Error;
use crate::codec::Link;
use crate::verb;

/// Realtek's vendor ID.
const REALTEK: u32 = 0x10ec;

/// Applies what codec `vendor` (vendor and device) in card `subsystem`
/// needs, once it is powered up and before its outputs are routed.
pub fn apply(link: &mut Link<'_>, vendor: u32, _subsystem: u32) -> Result<(), Error> {
    match vendor {
        // The ALC255 and ALC256 (Dell's ALC3246 is one) need bits 13 and
        // 12 of coefficient 0x46 set, as FreeBSD sets them.
        id if id == REALTEK << 16 | 0x0255 || id == REALTEK << 16 | 0x0256 => {
            let value = read_coef(link, 0x20, 0x46)?;
            write_coef(link, 0x20, 0x46, value as u16 | 0x3000)?;
        }
        _ => {}
    }
    Ok(())
}

/// Reads coefficient `index` of node `nid`.
fn read_coef(link: &mut Link<'_>, nid: u8, index: u16) -> Result<u32, Error> {
    link.verb4(nid, verb::SET_COEF_INDEX, index)?;
    link.verb4(nid, verb::GET_PROC_COEF, 0)
}

/// Writes `value` to coefficient `index` of node `nid`.
fn write_coef(link: &mut Link<'_>, nid: u8, index: u16, value: u16) -> Result<(), Error> {
    link.verb4(nid, verb::SET_COEF_INDEX, index)?;
    link.verb4(nid, verb::SET_PROC_COEF, value)?;
    Ok(())
}

/// What a codec of `vendor` is, in words, for `audio list`: its
/// maker's name and "HD Audio".
pub fn model(vendor: u32) -> &'static str {
    match vendor >> 16 {
        0x10ec => "Realtek HD Audio",
        0x14f1 => "Conexant HD Audio",
        0x111d | 0x8384 => "IDT HD Audio",
        0x11d4 => "Analog Devices HD Audio",
        0x1106 => "VIA HD Audio",
        0x1013 => "Cirrus Logic HD Audio",
        0x8086 => "Intel HD Audio",
        0x1002 | 0x1022 => "AMD HD Audio",
        0x10de => "NVIDIA HD Audio",
        0x1af4 => "QEMU HD Audio",
        _ => "HD Audio",
    }
}
