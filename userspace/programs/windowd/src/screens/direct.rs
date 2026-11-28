// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A full-screen window's own pixels as its screen's picture
//! (ADR 0728).
//!
//! When all a screen shows is one buffer of a program's, as many
//! pixels wide and tall as the screen's mode, windowd draws no frame:
//! it makes a picture of the buffer where it lies in the program's
//! memory file and asks the layer to show that. A card that reads a
//! picture's pages at any moment then shows what the program drew with
//! no copy at all, and any other card copies it once, not twice.
//!
//! A buffer the screen shows is not the program's to draw in. windowd
//! keeps it from being given back until the change after the one that
//! showed it has shown: until then the card may still read it.
//!
//! A picture is made of a buffer once and kept for as long as the
//! screen shows the program's buffers, which a program draws in turn.
//! The first time, the layer is asked whether it would show the picture
//! (`CHECK_ONLY`); a buffer it would not show, or whose file it does
//! not take, is drawn from as any other from then on.

use std::os::fd::BorrowedFd;

use display::abi::{CHECK_ONLY, PLANE_PICTURE};
use display::wire::ChangeBytes;
use display::{Holder, outputs};
use smithay::backend::renderer::utils::{Buffer, CommitCounter, RendererSurfaceStateUserData};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_shm::Format;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Rectangle, Size, Transform};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shm::with_buffer_contents;

use crate::link::{Link, Source};

/// How many buffers a screen keeps a picture of, or knows it can make
/// none of, at most. A program draws in two or three in turn.
const KEPT: usize = 8;

/// A buffer that is all a screen shows, pixel for pixel.
#[derive(Debug)]
pub struct Fit {
    /// The surface the buffer is of.
    pub surface: WlSurface,
    /// The buffer. While this is kept, the buffer is not given back to
    /// its program.
    pub buffer: Buffer,
    /// Where its pixels are in its file.
    pub source: Source,
    /// Which commit of its surface it is of.
    pub commit: CommitCounter,
    /// What of it changed since the commit that was asked about.
    pub changed: Vec<Rectangle<i32, Physical>>,
}

/// The buffer of `surface`, if its pixels are those of a screen of
/// `mode` (width, height) whose area of the desktop is `area` in size,
/// when the surface fills that area: the whole buffer, upright, a pixel
/// of it a pixel of the screen, hiding everything under it. `seen` is
/// the commit of the surface that showed last, if one did and the
/// screen still shows it.
pub fn fit(
    surface: &WlSurface,
    mode: (u32, u32),
    area: Size<i32, Logical>,
    seen: Option<CommitCounter>,
) -> Option<Fit> {
    with_states(surface, |states| {
        let state = states.data_map.get::<RendererSurfaceStateUserData>()?.lock().ok()?;
        let (buffer, view) = (state.buffer()?, state.view()?);
        let whole = Rectangle::from_size(state.buffer_size()?.to_f64());
        if state.buffer_transform() != Transform::Normal || view.src != whole || view.dst != area {
            return None;
        }
        let data = with_buffer_contents(buffer, |_, _, data| data).ok()?;
        let size = (u32::try_from(data.width).ok()?, u32::try_from(data.height).ok()?);
        let hides_all = match data.format {
            Format::Xrgb8888 => true,
            // What the program says hides everything under it is shown
            // as it is, whatever its pixels say of how much they cover:
            // the renderer does the same.
            Format::Argb8888 => {
                let all = Rectangle::from_size(view.dst);
                state
                    .opaque_regions()
                    .is_some_and(|hides| hides.iter().any(|region| region.contains_rect(all)))
            }
            _ => false,
        };
        let row_bytes = u32::try_from(data.stride).ok()?;
        if size != mode || !hides_all || !row_bytes.is_multiple_of(4) {
            return None;
        }
        let screen = Rectangle::from_size(Size::from((data.width, data.height)));
        let changed = state.damage_since(seen);
        let changed = changed.iter().filter_map(|rect| {
            let rect = Rectangle::<i32, Physical>::new(
                (rect.loc.x, rect.loc.y).into(),
                (rect.size.w, rect.size.h).into(),
            );
            rect.intersection(screen)
        });
        Some(Fit {
            surface: surface.clone(),
            buffer: buffer.clone(),
            source: Source { offset: u64::try_from(data.offset).ok()?, size, row_bytes },
            commit: state.current_commit(),
            changed: changed.collect(),
        })
    })
}

/// A buffer of a program's, and the picture made of it: `None` for a
/// buffer no picture can be made of, or whose picture the layer would
/// not show.
#[derive(Debug)]
struct Brought {
    buffer: WlBuffer,
    picture: Option<u32>,
}

/// What a screen shows straight from a program's memory.
#[derive(Debug, Default)]
pub struct Direct {
    brought: Vec<Brought>,
    /// The picture of a program's buffer the screen's main plane was
    /// last asked to show, or none while it shows windowd's own.
    on_plane: Option<u32>,
    /// The commit of the program's surface that was last asked to show,
    /// while what the screen shows is known.
    seen: Option<CommitCounter>,
    /// The buffers that are kept from their programs, each with the
    /// cookie of the change that showed it.
    held: Vec<(u64, Buffer)>,
}

impl Direct {
    /// Whether the main plane was last asked to show a program's
    /// buffer.
    pub fn shows(&self) -> bool {
        self.on_plane.is_some()
    }

    /// The picture of a program's buffer the main plane was last asked
    /// to show, and how many buffers are kept from their programs.
    #[cfg(test)]
    pub fn state(&self) -> (Option<u32>, usize) {
        (self.on_plane, self.held.len())
    }

    /// Whether the main plane was last asked to show another picture
    /// than `picture`.
    pub fn shows_another(&self, picture: u32) -> bool {
        self.on_plane != Some(picture)
    }

    /// The commit of the program's surface that was last asked to show,
    /// while what the screen shows is known.
    pub fn seen(&self) -> Option<CommitCounter> {
        self.seen
    }

    /// The picture of the buffer `fit` on the screen `on`, made now if
    /// the buffer is new. `file` is the buffer's file, if it is known.
    /// Returns nothing for a buffer the screen cannot show from where
    /// it is.
    pub fn picture<L: Link>(
        &mut self,
        holder: &Holder<L>,
        on: &outputs::Output,
        fit: &Fit,
        file: Option<BorrowedFd<'_>>,
    ) -> Option<u32> {
        self.tidy(holder);
        let buffer: &WlBuffer = &fit.buffer;
        if let Some(known) = self.brought.iter().find(|known| known.buffer == *buffer) {
            return known.picture;
        }
        if self.brought.len() >= KEPT {
            return None;
        }
        let picture = file.and_then(|file| {
            let picture = L::brought(holder, on.card, file, &fit.source).ok()?;
            let mut check = ChangeBytes::new(CHECK_ONLY, 0);
            check.set(on.plane, PLANE_PICTURE, u64::from(picture));
            if holder.change(&mut check).is_ok() {
                return Some(picture);
            }
            // It was never asked to show: nothing keeps it.
            let _ = holder.drop_object(picture);
            None
        });
        self.brought.push(Brought { buffer: buffer.clone(), picture });
        picture
    }

    /// Lets go of what is kept of buffers that are gone.
    fn tidy<L: Link>(&mut self, holder: &Holder<L>) {
        let on_plane = self.on_plane;
        self.brought.retain(|known| {
            let shows = known.picture.is_some() && known.picture == on_plane;
            if known.buffer.is_alive() || shows {
                return true;
            }
            if let Some(picture) = known.picture {
                // The plane is asked to show another.
                let _ = holder.drop_object(picture);
            }
            false
        });
    }

    /// A change with `cookie` asked the main plane to show `picture`,
    /// which is of `fit`.
    pub fn asked(&mut self, cookie: u64, picture: u32, fit: Fit) {
        (self.on_plane, self.seen) = (Some(picture), Some(fit.commit));
        self.held.push((cookie, fit.buffer));
    }

    /// A change asked the main plane to show a picture of windowd's
    /// own: the pictures of the program's buffers are let go of. The
    /// buffers are the program's again once that change shows.
    pub fn composed<L: Link>(&mut self, holder: &Holder<L>) {
        (self.on_plane, self.seen) = (None, None);
        self.brought.retain(|known| {
            let Some(picture) = known.picture else { return known.buffer.is_alive() };
            let _ = holder.drop_object(picture);
            false
        });
    }

    /// The change with `cookie` shows: the buffers earlier changes
    /// showed are their programs' again.
    pub fn shown(&mut self, cookie: u64) {
        self.held.retain(|(of, _)| *of >= cookie);
    }

    /// Every change that was asked for has shown.
    pub fn all_shown(&mut self) {
        self.held.clear();
    }

    /// What the screen shows is not known: the next change names the
    /// whole picture.
    pub fn lost(&mut self) {
        self.seen = None;
    }

    /// The pictures that are kept, by their numbers.
    pub fn numbers(&self) -> impl Iterator<Item = u32> + '_ {
        self.brought.iter().filter_map(|known| known.picture)
    }
}
