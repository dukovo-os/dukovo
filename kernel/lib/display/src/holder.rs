// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A holder: one description of `/devices/display/control` (or of
//! `attention`), with what is its own. Its pictures and values, under
//! the numbers it knows them by; the state it last asked for; and the
//! events it has not read. The layer keeps them all.

use alloc::vec::Vec;
use core::alloc::Allocator;
use core::ops::Deref;

use radix_abi::Status;
use radix_abi::display::{BLOB_MODE, BLOBS_MAX, DisplayEvent, EVENTS_KEPT, Mode, PICTURES_MAX};
use radix_abi::errors::Kernel;

use crate::frame::Picture;
use crate::layer::Stored;
use crate::ring::Ring;
use crate::state::{PictureShape, Values};

/// How many events a holder's ring holds at first, and how it grows:
/// four times over, twice, to EVENTS_KEPT.
const EVENTS_FIRST: usize = 16;

/// What one of a holder's numbers stands for.
enum Object<R> {
    /// Nothing: the number is free.
    Free,
    /// A picture.
    Picture(R),
    /// A value that is a mode.
    Mode(Mode),
    /// A value of another kind, a BLOB_*. No card of release 1.1 reads
    /// one, so its bytes are not kept.
    Value(u32),
}

/// A holder, whose pictures are `R`: each shared with the plane that
/// shows it, so that it lives while it shows.
pub struct Holder<R, A: Allocator> {
    /// Its number, which the stack and the changes that wait know it
    /// by.
    pub(crate) id: u32,
    /// What its opener knows it by, which finds it for a request.
    pub(crate) key: usize,
    /// Who made its last change, in whatever numbers the layer's user
    /// tells programs apart by, or 0.
    who: u64,
    /// Its pictures and values: number `n` is `objects[n - 1]`.
    objects: Vec<Object<R>, A>,
    /// How many of them are pictures.
    pictures: u32,
    /// How many are values.
    values: u32,
    /// The state it last asked for, once it has made a change.
    pub(crate) wanted: Option<Stored<A>>,
    ring: Ring,
    events: Vec<DisplayEvent, A>,
    /// Whether an event was added since its file's polls were last told
    /// ([`Holder::take_posted`]).
    posted: bool,
    alloc: A,
}

impl<R: Deref<Target: Picture>, A: Allocator + Copy> Holder<R, A> {
    /// A holder with nothing yet.
    pub(crate) fn new(id: u32, key: usize, alloc: A) -> Holder<R, A> {
        Holder {
            id,
            key,
            who: 0,
            objects: Vec::new_in(alloc),
            pictures: 0,
            values: 0,
            wanted: None,
            ring: Ring::new(),
            events: Vec::new_in(alloc),
            posted: false,
            alloc,
        }
    }

    /// Notes who makes its changes: the kernel gives the number of the
    /// process, for whoever asks which program is in front.
    pub fn set_who(&mut self, who: u64) {
        self.who = who;
    }

    /// Who made its last change, or 0 if it has made none.
    pub fn who(&self) -> u64 {
        self.who
    }

    /// Keeps `object` under the lowest free number, and returns it.
    fn keep(&mut self, object: Object<R>) -> Result<u32, Status> {
        let free = self.objects.iter().position(|object| matches!(object, Object::Free));
        let at = match free {
            Some(at) => at,
            None => {
                self.objects.try_reserve(1).map_err(|_| Kernel::NoMemory.status(0))?;
                // Room was reserved.
                let _ = self.objects.push_within_capacity(Object::Free);
                self.objects.len() - 1
            }
        };
        self.objects[at] = object;
        Ok(at as u32 + 1)
    }

    /// Keeps `picture` and returns its number.
    ///
    /// # Errors
    ///
    /// too_many at PICTURES_MAX, and no_memory.
    pub fn keep_picture(&mut self, picture: R) -> Result<u32, Status> {
        if self.pictures >= PICTURES_MAX {
            return Err(Kernel::TooMany.status(0));
        }
        let number = self.keep(Object::Picture(picture))?;
        self.pictures += 1;
        Ok(number)
    }

    /// Keeps a value of `kind`, whose mode is `mode` if it is one, and
    /// returns its number.
    ///
    /// # Errors
    ///
    /// too_many at BLOBS_MAX, and no_memory.
    pub fn keep_value(&mut self, kind: u32, mode: Option<Mode>) -> Result<u32, Status> {
        if self.values >= BLOBS_MAX {
            return Err(Kernel::TooMany.status(0));
        }
        let object = match mode {
            Some(mode) if kind == BLOB_MODE => Object::Mode(mode),
            _ => Object::Value(kind),
        };
        let number = self.keep(object)?;
        self.values += 1;
        Ok(number)
    }

    /// Lets go of picture or value `number`.
    ///
    /// # Errors
    ///
    /// invalid_argument if there is none, and busy while the state the
    /// holder asked for shows the picture or uses the value.
    pub fn drop_object(&mut self, number: u32) -> Result<(), Status> {
        let at = (number as usize).checked_sub(1).ok_or(Kernel::InvalidArgument.status(0))?;
        let object = self.objects.get(at).ok_or(Kernel::InvalidArgument.status(0))?;
        let used = self.wanted.as_ref().is_some_and(|wanted| match object {
            Object::Picture(_) => wanted.view().shows(number),
            _ => wanted.view().uses(number),
        });
        match object {
            Object::Free => return Err(Kernel::InvalidArgument.status(0)),
            _ if used => return Err(Kernel::Busy.status(0)),
            Object::Picture(_) => self.pictures -= 1,
            Object::Mode(_) | Object::Value(_) => self.values -= 1,
        }
        self.objects[at] = Object::Free;
        Ok(())
    }

    /// Picture `number`, if the holder has it.
    pub fn picture_ref(&self, number: u32) -> Option<&R> {
        match self.objects.get((number as usize).checked_sub(1)?)? {
            Object::Picture(picture) => Some(picture),
            _ => None,
        }
    }

    /// Adds `event` to those it has not read, growing their storage
    /// while it may. Past EVENTS_KEPT the oldest is dropped for it, which
    /// the holder reads of first.
    pub(crate) fn post(&mut self, event: DisplayEvent) {
        if self.ring.is_full(&self.events) && self.events.len() < EVENTS_KEPT as usize {
            self.grow_events();
        }
        self.ring.push(&mut self.events, event);
        self.posted = true;
    }

    /// Whether an event was added since this was last asked.
    pub(crate) fn take_posted(&mut self) -> bool {
        core::mem::take(&mut self.posted)
    }

    /// Gives the ring four times its storage, if there is memory for it:
    /// without, it drops the oldest event as a full one does.
    fn grow_events(&mut self) {
        let longer = (self.events.len() * 4).clamp(EVENTS_FIRST, EVENTS_KEPT as usize);
        let mut events = Vec::new_in(self.alloc);
        if events.try_reserve_exact(longer).is_err() {
            return;
        }
        for _ in 0..longer {
            // Room was reserved.
            let _ = events.push_within_capacity(DisplayEvent::default());
        }
        if self.ring.move_to(&self.events, &mut events) {
            self.events = events;
        }
    }

    /// Takes the oldest event it has not read.
    pub fn next_event(&mut self) -> Option<DisplayEvent> {
        self.ring.pop(&self.events)
    }

    /// Whether an event waits to be read.
    pub fn has_events(&self) -> bool {
        !self.ring.is_empty()
    }
}

impl<R: Deref<Target: Picture>, A: Allocator + Copy> Values for Holder<R, A> {
    fn picture(&self, number: u32) -> Option<PictureShape> {
        self.picture_ref(number).map(|picture| picture.shape())
    }

    fn mode(&self, number: u32) -> Option<Mode> {
        match self.objects.get((number as usize).checked_sub(1)?)? {
            Object::Mode(mode) => Some(*mode),
            _ => None,
        }
    }

    fn blob_kind(&self, number: u32) -> Option<u32> {
        match self.objects.get((number as usize).checked_sub(1)?)? {
            Object::Mode(_) => Some(BLOB_MODE),
            Object::Value(kind) => Some(*kind),
            _ => None,
        }
    }
}
