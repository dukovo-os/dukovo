// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The layer in words, for an administrator's tools: who is in front,
//! and every card, screen, mode and pipe, a line each.
//!
//! A line starts with what it is of and that thing's number, and goes
//! on with `key=value` words. A value that may hold spaces (a card's
//! model, a monitor's name) is the last of its line and runs to the
//! line's end. The first line says who has the screens:
//!
//! ```text
//! front log
//! front process 41
//! card 1 name=virtio0 kind=virtio hot_plug=1 mirrors=0 sets_modes=1 shared_pages=0 wakes=17 model=virtio graphics card
//! screen 6 card=1 index=0 plugged=1 failed=0 hdr=0 variable_refresh=0 pipe=2 depth=8 width_mm=260 height_mm=160 refresh_min_mhz=0 refresh_max_mhz=0 luminance_max=0 product=4660 maker=RHT name=QEMU Monitor
//! mode 6 width=1280 height=800 refresh_mhz=74991 preferred=1
//! pipe 2 card=1 index=0 active=1 width=1280 height=800 refresh_mhz=74991 beat_ns=13334815 beat_count=412
//! ```
//!
//! `front` is `log` while the kernel's log has the screens, `process`
//! and the number of the process that made the last change of the
//! holder in front, `holder` for a holder that has made no change yet,
//! and `last` while the last picture of a holder that ended stays. A
//! screen's modes follow it, the one it prefers first; the mode it
//! shows is that of the pipe that feeds it.
//!
//! A monitor's serial number is not here. These words are for everyone
//! on the machine to read and to put in a report as they are; the
//! holder of the screens, which tells one monitor from its twin by that
//! number, reads it with INFO.

use core::alloc::Allocator;
use core::fmt::{self, Write};
use core::ops::Deref;

use radix_abi::display::{
    CARD_FIRMWARE, CARD_HOT_PLUG, CARD_MIRRORS, CARD_SETS_MODES, CARD_SHARED_PAGES, CARD_VIRTIO,
    KIND_CARD, KIND_PIPE, KIND_SCREEN, MODE_PREFERRED, PIPE_IS_ACTIVE, SCREEN_FAILED, SCREEN_HDR,
    SCREEN_PLUGGED, SCREEN_VARIABLE_REFRESH, Thing,
};

use super::Layer;
use crate::frame::{Card, Picture};
use crate::holder::Holder;

/// The text of a record's name: its bytes up to the first zero, with
/// anything that is not printable ASCII as `?`, so that a monitor's own
/// words cannot break a line.
struct Name<'a>(&'a [u8]);

impl fmt::Display for Name<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let end = self.0.iter().position(|&byte| byte == 0).unwrap_or(self.0.len());
        for &byte in &self.0[..end] {
            f.write_char(if byte == b' ' || byte.is_ascii_graphic() { byte as char } else { '?' })?;
        }
        Ok(())
    }
}

/// 1 if `flags` has `flag`, else 0.
fn bit(flags: u32, flag: u32) -> u32 {
    u32::from(flags & flag != 0)
}

impl<R, C, A> Layer<R, C, A>
where
    R: Clone + Deref<Target: Picture + Sized>,
    C: Card<R::Target> + Clone,
    A: Allocator + Copy,
{
    /// Writes the layer in words to `out`, as the module says. `wakes`
    /// gives how many times a card's thread has woken.
    ///
    /// # Errors
    ///
    /// `out`'s.
    pub fn describe(&self, wakes: &dyn Fn(&C) -> u64, out: &mut dyn Write) -> fmt::Result {
        self.describe_front(out)?;
        let mut result = Ok(());
        self.list(&mut |thing| {
            if result.is_ok() {
                result = match thing.kind {
                    KIND_CARD => self.describe_card(thing.id, wakes, out),
                    KIND_SCREEN => self.describe_screen(&thing, out),
                    KIND_PIPE => self.describe_pipe(&thing, out),
                    _ => Ok(()),
                };
            }
        });
        result
    }

    fn describe_front(&self, out: &mut dyn Write) -> fmt::Result {
        let holder = self.front().and_then(|id| self.holders.iter().find(|one| one.id == id));
        match holder.map(Holder::who) {
            Some(0) => writeln!(out, "front holder"),
            Some(who) => writeln!(out, "front process {who}"),
            None if self.keeps_last() => writeln!(out, "front last"),
            None => writeln!(out, "front log"),
        }
    }

    fn describe_card(
        &self,
        id: u32,
        wakes: &dyn Fn(&C) -> u64,
        out: &mut dyn Write,
    ) -> fmt::Result {
        let (Some(info), Some(card)) = (self.card_info(id), self.card(id)) else { return Ok(()) };
        let kind = match info.kind {
            CARD_FIRMWARE => "firmware",
            CARD_VIRTIO => "virtio",
            _ => "other",
        };
        writeln!(
            out,
            "card {id} name={} kind={kind} hot_plug={} mirrors={} sets_modes={} shared_pages={} \
             wakes={} model={}",
            Name(&info.name),
            bit(info.flags, CARD_HOT_PLUG),
            bit(info.flags, CARD_MIRRORS),
            bit(info.flags, CARD_SETS_MODES),
            bit(info.flags, CARD_SHARED_PAGES),
            wakes(card),
            Name(&info.model),
        )
    }

    fn describe_screen(&self, thing: &Thing, out: &mut dyn Write) -> fmt::Result {
        let Some(info) = self.screen_info(thing.id) else { return Ok(()) };
        writeln!(
            out,
            "screen {} card={} index={} plugged={} failed={} hdr={} variable_refresh={} pipe={} \
             depth={} width_mm={} height_mm={} refresh_min_mhz={} refresh_max_mhz={} \
             luminance_max={} product={} maker={} name={}",
            info.id,
            info.card,
            thing.index,
            bit(info.flags, SCREEN_PLUGGED),
            bit(info.flags, SCREEN_FAILED),
            bit(info.flags, SCREEN_HDR),
            bit(info.flags, SCREEN_VARIABLE_REFRESH),
            info.pipe,
            info.depth,
            info.width_mm,
            info.height_mm,
            info.refresh_min_mhz,
            info.refresh_max_mhz,
            info.luminance_max,
            info.product,
            Name(&info.maker),
            Name(&info.name),
        )?;
        for mode in self.screen_modes(thing.id).unwrap_or_default() {
            writeln!(
                out,
                "mode {} width={} height={} refresh_mhz={} preferred={}",
                info.id,
                mode.width,
                mode.height,
                mode.refresh_mhz,
                bit(mode.flags, MODE_PREFERRED),
            )?;
        }
        Ok(())
    }

    fn describe_pipe(&self, thing: &Thing, out: &mut dyn Write) -> fmt::Result {
        let Some(info) = self.pipe_info(thing.id) else { return Ok(()) };
        writeln!(
            out,
            "pipe {} card={} index={} active={} width={} height={} refresh_mhz={} beat_ns={} \
             beat_count={}",
            info.id,
            info.card,
            thing.index,
            bit(info.flags, PIPE_IS_ACTIVE),
            info.mode.width,
            info.mode.height,
            info.mode.refresh_mhz,
            info.beat_ns,
            info.beat_count,
        )
    }
}
