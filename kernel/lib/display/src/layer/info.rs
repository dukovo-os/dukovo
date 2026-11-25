// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What LIST and GET answer with: the things the layer has, and what it
//! knows of each.

use core::alloc::Allocator;
use core::ops::Deref;

use radix_abi::display::{
    CardInfo, KIND_CARD, KIND_PIPE, KIND_PLANE, KIND_SCREEN, Mode, PIPE_IS_ACTIVE, PLANE_SCALES,
    PipeInfo, PlaneInfo, SCREEN_FAILED, SCREEN_HDR, SCREEN_PLUGGED, ScreenInfo, Thing,
};

use super::Layer;
use crate::frame::{Card, Picture};

/// How many of `things` are of `card`.
fn count_of<T>(things: &[T], card: u32, card_of: impl Fn(&T) -> u32) -> u32 {
    things.iter().filter(|thing| card_of(thing) == card).count() as u32
}

impl<R, C, A> Layer<R, C, A>
where
    R: Clone + Deref<Target: Picture + Sized>,
    C: Card<R::Target> + Clone,
    A: Allocator + Copy,
{
    /// How many things there are: cards, screens, pipes and planes.
    pub fn count(&self) -> usize {
        self.cards.len() + self.screens.len() + self.pipes.len() + self.planes.len()
    }

    /// LIST: calls `each` with a Thing for every card, then every
    /// screen, pipe and plane.
    pub fn list(&self, each: &mut dyn FnMut(Thing)) {
        for card in &self.cards {
            each(Thing { id: card.id, kind: KIND_CARD, card: card.id, index: 0 });
        }
        for (at, screen) in self.screens.iter().enumerate() {
            let index = count_of(&self.screens[..at], screen.card, |screen| screen.card);
            each(Thing { id: screen.id, kind: KIND_SCREEN, card: screen.card, index });
        }
        for (at, pipe) in self.pipes.iter().enumerate() {
            let index = count_of(&self.pipes[..at], pipe.card, |pipe| pipe.card);
            each(Thing { id: pipe.id, kind: KIND_PIPE, card: pipe.card, index });
        }
        for (at, plane) in self.planes.iter().enumerate() {
            let index = count_of(&self.planes[..at], plane.card, |plane| plane.card);
            each(Thing { id: plane.id, kind: KIND_PLANE, card: plane.card, index });
        }
    }

    /// What thing `id` is, a KIND_*, if there is one.
    pub fn kind_of(&self, id: u32) -> Option<u32> {
        if self.cards.iter().any(|card| card.id == id) {
            Some(KIND_CARD)
        } else if self.screens.iter().any(|screen| screen.id == id) {
            Some(KIND_SCREEN)
        } else if self.pipes.iter().any(|pipe| pipe.id == id) {
            Some(KIND_PIPE)
        } else if self.planes.iter().any(|plane| plane.id == id) {
            Some(KIND_PLANE)
        } else {
            None
        }
    }

    /// GET's PART_INFO of card `id`.
    pub fn card_info(&self, id: u32) -> Option<CardInfo> {
        let at = self.cards.iter().position(|card| card.id == id)?;
        let (card, more) = (&self.cards[at], &self.card_more[at]);
        Some(CardInfo {
            id,
            kind: more.kind,
            flags: card.flags,
            screens: count_of(&self.screens, id, |screen| screen.card),
            pipes: count_of(&self.pipes, id, |pipe| pipe.card),
            planes: count_of(&self.planes, id, |plane| plane.card),
            width_max: card.width_max,
            height_max: card.height_max,
            name: more.name,
            model: more.model,
        })
    }

    /// GET's PART_INFO of screen `id`.
    pub fn screen_info(&self, id: u32) -> Option<ScreenInfo> {
        let at = self.screens.iter().position(|screen| screen.id == id)?;
        let (screen, more) = (&self.screens[at], &self.screen_more[at]);
        let monitor = &more.monitor;
        let flag = |set: bool, flag: u32| if set { flag } else { 0 };
        Some(ScreenInfo {
            id,
            card: screen.card,
            flags: flag(screen.plugged, SCREEN_PLUGGED)
                | flag(monitor.hdr, SCREEN_HDR)
                | flag(screen.failed, SCREEN_FAILED),
            pipe: self.shown.screens[at].pipe,
            width_mm: monitor.width_mm,
            height_mm: monitor.height_mm,
            modes: more.modes.len() as u32,
            monitor_data_bytes: more.data.len() as u32,
            refresh_min_mhz: monitor.refresh_min_mhz,
            refresh_max_mhz: monitor.refresh_max_mhz,
            luminance_max: monitor.luminance_max,
            product: monitor.product,
            serial: monitor.serial,
            depth: monitor.depth,
            reserved: [0; 3],
            maker: monitor.maker,
            name: monitor.name,
        })
    }

    /// GET's PART_MODES of screen `id`: its modes, the preferred one
    /// first.
    pub fn screen_modes(&self, id: u32) -> Option<&[Mode]> {
        let at = self.screens.iter().position(|screen| screen.id == id)?;
        Some(&self.screen_more[at].modes)
    }

    /// GET's PART_MONITOR_DATA of screen `id`: the bytes its monitor
    /// said what it is with.
    pub fn monitor_data(&self, id: u32) -> Option<&[u8]> {
        let at = self.screens.iter().position(|screen| screen.id == id)?;
        Some(&self.screen_more[at].data)
    }

    /// GET's PART_INFO of pipe `id`.
    pub fn pipe_info(&self, id: u32) -> Option<PipeInfo> {
        let at = self.pipes.iter().position(|pipe| pipe.id == id)?;
        let (pipe, run, state) = (&self.pipes[at], &self.runs[at], &self.shown.pipes[at]);
        let (beat_ns, beat_count, beat_time_ns) = match run.beat {
            Some(beat) => {
                let count = beat.latest(self.now);
                (beat.period_ns(), count, beat.time_of(count))
            }
            None => (0, run.count, 0),
        };
        Some(PipeInfo {
            id,
            card: pipe.card,
            flags: if state.active { PIPE_IS_ACTIVE } else { 0 },
            color_table_entries: pipe.color_table_entries,
            beat_ns,
            beat_count,
            beat_time_ns,
            mode: state.mode,
        })
    }

    /// How many times the card of pipe `id` was asked to give it another
    /// mode, to start it or to stop it.
    pub fn mode_sets(&self, id: u32) -> Option<u64> {
        let at = self.pipes.iter().position(|pipe| pipe.id == id)?;
        Some(self.runs[at].mode_sets)
    }

    /// GET's PART_INFO of plane `id`.
    pub fn plane_info(&self, id: u32) -> Option<PlaneInfo> {
        let at = self.planes.iter().position(|plane| plane.id == id)?;
        let plane = &self.planes[at];
        Some(PlaneInfo {
            id,
            card: plane.card,
            kind: plane.kind,
            flags: if plane.scales { PLANE_SCALES } else { 0 },
            pipe: self.shown.planes[at].pipe,
            fixed_pipe: plane.fixed_pipe,
            width_max: plane.width_max,
            height_max: plane.height_max,
            formats: plane.formats,
        })
    }
}
