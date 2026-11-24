// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A simulated virtio graphics card for the driver's tests: the same PCI
//! configuration space as the block tests, a BAR with the card's two
//! queues, and a card that keeps resources, copies rectangles from
//! their backing as a host does, and has a glass for each screen: what a
//! person looking at it would see, which is what the screen was last
//! told to show and what was flushed since.
//!
//! A test turns the card's knobs through its state: a card that takes
//! no command, one that holds its answers, one that cuts an answer
//! short or gives another, one that keeps the pointer's commands.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use radix_hw::{Dma, Mmio, Registers, fake_memory, fake_memory_write};

use crate::gpu::{
    EDID_MAX, EVENT_DISPLAY, Gpu, POINTER_SIDE, Place, Rect, Vectors, code, feature, format,
};
use crate::snd_tests::{Buffer, Ring, next_chain, put_used, read_mem};
use crate::tests::{BAR_SIZE, alloc, function};
use crate::{Error, NO_VECTOR, Transport, feature as common_feature, status};

/// The bytes of a command's and of an answer's header.
pub(crate) const HEADER: usize = 24;
/// The most pixels of a resource the card has memory for.
const MOST_PIXELS: u64 = 4096 * 4096;

/// A picture the card keeps.
pub(crate) struct Resource {
    width: u32,
    height: u32,
    /// The card's copy of its pixels; none for a shared one.
    pixels: Vec<u8>,
    /// The guest's memory behind it, in pieces.
    backing: Vec<(u64, u32)>,
    /// Whether the card shows the backing itself.
    shared: bool,
}

/// What a screen shows: a rectangle of a resource, whose rows are
/// `row_bytes` apart.
#[derive(Clone, Copy)]
struct Shown {
    resource: u32,
    rect: Rect,
    row_bytes: u32,
}

#[derive(Clone, Default)]
pub(crate) struct Screen {
    /// The size the host would like, while a monitor is plugged in.
    plugged: Option<(u32, u32)>,
    pub(crate) edid: Vec<u8>,
    shown: Option<Shown>,
    /// What a person sees: the rectangle's pixels, row after row.
    glass: Vec<u8>,
}

/// The pointer as the card draws it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Pointer {
    pub(crate) resource: u32,
    pub(crate) hot: (u32, u32),
    pub(crate) place: Place,
}

#[derive(Default)]
pub(crate) struct State {
    pub(crate) device_select: u32,
    pub(crate) driver_select: u32,
    pub(crate) driver_features: u64,
    pub(crate) status: u8,
    pub(crate) select: u16,
    pub(crate) rings: [Ring; 2],
    pub(crate) config_vector: u16,
    pub(crate) vectors: [u16; 2],
    pub(crate) screens: Vec<Screen>,
    pub(crate) events: u32,
    pub(crate) resources: BTreeMap<u32, Resource>,
    pub(crate) pointer: Pointer,
    /// The pointer's picture: the copy the card took when it was set.
    pub(crate) pointer_pixels: Vec<u8>,
    /// The pointer's commands, in order: their code and place.
    pub(crate) pointed: Vec<(u32, Place)>,
    /// Whether the card takes no command at all.
    pub(crate) silent: bool,
    /// Whether the card keeps the commands it takes unanswered, until
    /// [`Card::release`].
    pub(crate) hold: bool,
    pub(crate) held: Vec<(u16, Vec<Buffer>)>,
    /// A command code, and the code the card answers it with.
    pub(crate) answer_with: Option<(u32, u32)>,
    /// A command code, and the bytes the card cuts its answer to.
    pub(crate) short: Option<(u32, usize)>,
    /// Whether the card keeps the pointer's commands.
    pub(crate) keeps_pointer: bool,
    /// Whether the card takes no interrupt vector.
    pub(crate) no_vectors: bool,
    /// The codes of the control commands, in order.
    pub(crate) commands: Vec<u32>,
    /// The doorbells rung, and the interrupts the card would raise, for
    /// each queue.
    pub(crate) doorbells: [u32; 2],
    pub(crate) interrupts: [u32; 2],
}

pub(crate) struct Card {
    pub(crate) state: Mutex<State>,
    features: u64,
}

fn u16_at(phys: u64) -> u16 {
    u16::from_le_bytes(read_mem(phys, 2).try_into().unwrap())
}

fn u32_of(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn u64_of(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn rect_of(bytes: &[u8], at: usize) -> Rect {
    Rect {
        x: u32_of(bytes, at),
        y: u32_of(bytes, at + 4),
        width: u32_of(bytes, at + 8),
        height: u32_of(bytes, at + 12),
    }
}

/// Whether `rect` has pixels, all of them inside `width` by `height`.
fn inside(rect: Rect, width: u32, height: u32) -> bool {
    rect.width > 0
        && rect.height > 0
        && u64::from(rect.x) + u64::from(rect.width) <= u64::from(width)
        && u64::from(rect.y) + u64::from(rect.height) <= u64::from(height)
}

/// Reads `out.len()` bytes at `offset` of a backing in pieces. A read
/// past its end is the driver's mistake, and fails the test.
fn read_backing(backing: &[(u64, u32)], mut offset: u64, out: &mut [u8]) {
    let mut done = 0;
    for &(at, len) in backing {
        let len = u64::from(len);
        if offset >= len {
            offset -= len;
            continue;
        }
        let take = ((len - offset) as usize).min(out.len() - done);
        fake_memory(at + offset, &mut out[done..done + take]);
        (done, offset) = (done + take, 0);
        if done == out.len() {
            return;
        }
    }
    panic!("a read past the end of the backing");
}

/// The `count` entries of a backing's table from byte `at` of a command,
/// which they end.
fn entries(request: &[u8], at: usize, count: u32) -> Vec<(u64, u32)> {
    assert_eq!(request.len(), at + 16 * count as usize, "the command, then its entries");
    let entry = |i: usize| (u64_of(request, at + 16 * i), u32_of(request, at + 16 * i + 8));
    (0..count as usize).map(entry).collect()
}

/// Copies `area` of what `screen` shows from its resource to the glass.
fn paint(resources: &BTreeMap<u32, Resource>, screen: &mut Screen, area: Rect) {
    let Some(Shown { resource, rect, row_bytes }) = screen.shown else { return };
    let resource = &resources[&resource];
    let (left, right) = (area.x.max(rect.x), (area.x + area.width).min(rect.x + rect.width));
    let (top, bottom) = (area.y.max(rect.y), (area.y + area.height).min(rect.y + rect.height));
    if left >= right {
        return;
    }
    let bytes = (right - left) as usize * 4;
    for y in top..bottom {
        let to = ((y - rect.y) * rect.width + (left - rect.x)) as usize * 4;
        let row = &mut screen.glass[to..to + bytes];
        if resource.shared {
            let from = u64::from(y) * u64::from(row_bytes) + u64::from(left) * 4;
            read_backing(&resource.backing, from, row);
        } else {
            let from = (y * resource.width + left) as usize * 4;
            row.copy_from_slice(&resource.pixels[from..from + bytes]);
        }
    }
}

impl State {
    fn display_info(&self, answer: &mut Vec<u8>) -> u32 {
        for index in 0..16 {
            let plugged = self.screens.get(index).and_then(|screen| screen.plugged);
            let (width, height) = plugged.unwrap_or((0, 0));
            for field in [0, 0, width, height, u32::from(plugged.is_some()), 0] {
                answer.extend_from_slice(&field.to_le_bytes());
            }
        }
        code::OK_DISPLAY_INFO
    }

    fn edid(&self, r: &[u8], answer: &mut Vec<u8>) -> u32 {
        assert_eq!(r.len(), 32);
        if self.driver_features & feature::EDID == 0 {
            return code::ERR_UNSPEC;
        }
        let Some(screen) = self.screens.get(u32_of(r, 24) as usize) else {
            return code::ERR_INVALID_PARAMETER;
        };
        answer.extend_from_slice(&(screen.edid.len() as u32).to_le_bytes());
        answer.extend_from_slice(&[0; 4]);
        answer.extend_from_slice(&screen.edid);
        answer.resize(HEADER + 8 + EDID_MAX, 0);
        code::OK_EDID
    }

    fn create(&mut self, r: &[u8]) -> u32 {
        assert_eq!(r.len(), 40);
        let (id, format) = (u32_of(r, 24), u32_of(r, 28));
        let (width, height) = (u32_of(r, 32), u32_of(r, 36));
        if id == 0 || self.resources.contains_key(&id) {
            return code::ERR_INVALID_RESOURCE_ID;
        }
        if !matches!(format, format::BGRA | format::BGRX) || width == 0 || height == 0 {
            return code::ERR_INVALID_PARAMETER;
        }
        if u64::from(width) * u64::from(height) > MOST_PIXELS {
            return code::ERR_OUT_OF_MEMORY;
        }
        let pixels = std::vec![0; width as usize * height as usize * 4];
        let resource = Resource { width, height, pixels, backing: Vec::new(), shared: false };
        self.resources.insert(id, resource);
        code::OK_NODATA
    }

    fn create_shared(&mut self, r: &[u8]) -> u32 {
        if self.driver_features & feature::RESOURCE_BLOB == 0 {
            return code::ERR_UNSPEC;
        }
        let (id, memory, flags) = (u32_of(r, 24), u32_of(r, 28), u32_of(r, 32));
        let (backing, size) = (entries(r, 56, u32_of(r, 36)), u64_of(r, 48));
        if id == 0 || self.resources.contains_key(&id) {
            return code::ERR_INVALID_RESOURCE_ID;
        }
        let held: u64 = backing.iter().map(|&(_, len)| u64::from(len)).sum();
        // The guest's memory, which the host may share.
        if memory != 1 || flags != 2 || size == 0 || held < size {
            return code::ERR_INVALID_PARAMETER;
        }
        let resource = Resource { width: 0, height: 0, pixels: Vec::new(), backing, shared: true };
        self.resources.insert(id, resource);
        code::OK_NODATA
    }

    fn attach(&mut self, r: &[u8]) -> u32 {
        let backing = entries(r, 32, u32_of(r, 28));
        let Some(resource) = self.resources.get_mut(&u32_of(r, 24)) else {
            return code::ERR_INVALID_RESOURCE_ID;
        };
        if !resource.backing.is_empty() {
            return code::ERR_UNSPEC;
        }
        resource.backing = backing;
        code::OK_NODATA
    }

    fn unref(&mut self, r: &[u8]) -> u32 {
        assert_eq!(r.len(), 32);
        let id = u32_of(r, 24);
        if self.resources.remove(&id).is_none() {
            return code::ERR_INVALID_RESOURCE_ID;
        }
        // A screen that showed it shows nothing.
        for screen in &mut self.screens {
            if screen.shown.is_some_and(|shown| shown.resource == id) {
                (screen.shown, screen.glass) = (None, Vec::new());
            }
        }
        code::OK_NODATA
    }

    fn set_scanout(&mut self, r: &[u8]) -> u32 {
        assert_eq!(r.len(), 48);
        self.scan_out(u32_of(r, 40), u32_of(r, 44), rect_of(r, 24), None)
    }

    fn set_scanout_shared(&mut self, r: &[u8]) -> u32 {
        assert_eq!(r.len(), 96);
        assert!(r[68..].iter().all(|byte| *byte == 0), "one plane, from the first byte");
        if !matches!(u32_of(r, 56), format::BGRA | format::BGRX) {
            return code::ERR_INVALID_PARAMETER;
        }
        let shape = (u32_of(r, 48), u32_of(r, 52), u32_of(r, 64));
        self.scan_out(u32_of(r, 40), u32_of(r, 44), rect_of(r, 24), Some(shape))
    }

    /// Screen `index` shows `rect` of resource `id`, or nothing for
    /// resource 0. `shape` is what the command says of a shared
    /// resource: its width, its height and the bytes of its rows.
    fn scan_out(&mut self, index: u32, id: u32, rect: Rect, shape: Option<(u32, u32, u32)>) -> u32 {
        let State { resources, screens, .. } = self;
        let Some(screen) = screens.get_mut(index as usize) else {
            return code::ERR_INVALID_SCANOUT_ID;
        };
        if id == 0 {
            (screen.shown, screen.glass) = (None, Vec::new());
            return code::OK_NODATA;
        }
        let Some(resource) = resources.get(&id) else { return code::ERR_INVALID_RESOURCE_ID };
        let (width, height, row_bytes) = match shape {
            Some(shape) if resource.shared => shape,
            None if !resource.shared => (resource.width, resource.height, resource.width * 4),
            _ => return code::ERR_INVALID_PARAMETER,
        };
        if !inside(rect, width, height) || u64::from(row_bytes) < u64::from(width) * 4 {
            return code::ERR_INVALID_PARAMETER;
        }
        // The last pixel shown is inside a shared resource's memory.
        let held: u64 = resource.backing.iter().map(|&(_, len)| u64::from(len)).sum();
        let end = u64::from(rect.y + rect.height - 1) * u64::from(row_bytes)
            + u64::from(rect.x + rect.width) * 4;
        if resource.shared && end > held {
            return code::ERR_INVALID_PARAMETER;
        }
        screen.shown = Some(Shown { resource: id, rect, row_bytes });
        screen.glass = std::vec![0; rect.width as usize * rect.height as usize * 4];
        paint(resources, screen, rect);
        code::OK_NODATA
    }

    /// Copies a rectangle from a resource's backing: row `n` of it is
    /// read at the command's offset plus `n` rows of the resource.
    fn transfer(&mut self, r: &[u8]) -> u32 {
        assert_eq!(r.len(), 56);
        let (rect, offset) = (rect_of(r, 24), u64_of(r, 40));
        let Some(resource) = self.resources.get_mut(&u32_of(r, 48)) else {
            return code::ERR_INVALID_RESOURCE_ID;
        };
        if resource.shared {
            // Its pages are what the card shows: nothing to copy.
            return code::OK_NODATA;
        }
        if !inside(rect, resource.width, resource.height) {
            return code::ERR_INVALID_PARAMETER;
        }
        let (stride, bytes) = (resource.width as usize * 4, rect.width as usize * 4);
        for row in 0..rect.height as usize {
            let to = (rect.y as usize + row) * stride + rect.x as usize * 4;
            let from = offset + (stride * row) as u64;
            read_backing(&resource.backing, from, &mut resource.pixels[to..to + bytes]);
        }
        code::OK_NODATA
    }

    fn flush(&mut self, r: &[u8]) -> u32 {
        assert_eq!(r.len(), 48);
        let (rect, id) = (rect_of(r, 24), u32_of(r, 40));
        let State { resources, screens, .. } = self;
        let Some(resource) = resources.get(&id) else { return code::ERR_INVALID_RESOURCE_ID };
        if !resource.shared && !inside(rect, resource.width, resource.height) {
            return code::ERR_INVALID_PARAMETER;
        }
        let shows = |screen: &&mut Screen| screen.shown.is_some_and(|shown| shown.resource == id);
        for screen in screens.iter_mut().filter(shows) {
            paint(resources, screen, rect);
        }
        code::OK_NODATA
    }

    fn carry_out(&mut self, code: u32, r: &[u8], answer: &mut Vec<u8>) -> u32 {
        match code {
            code::GET_DISPLAY_INFO => self.display_info(answer),
            code::GET_EDID => self.edid(r, answer),
            code::RESOURCE_CREATE_2D => self.create(r),
            code::RESOURCE_CREATE_BLOB => self.create_shared(r),
            code::RESOURCE_ATTACH_BACKING => self.attach(r),
            code::RESOURCE_UNREF => self.unref(r),
            code::SET_SCANOUT => self.set_scanout(r),
            code::SET_SCANOUT_BLOB => self.set_scanout_shared(r),
            code::TRANSFER_TO_HOST_2D => self.transfer(r),
            code::RESOURCE_FLUSH => self.flush(r),
            _ => code::ERR_UNSPEC,
        }
    }

    /// Hands a chain of queue `queue` back, and counts the interrupt the
    /// card would raise for it: one the driver asked for.
    fn used(&mut self, queue: usize, head: u16, len: u32) {
        let ring = &mut self.rings[queue];
        let asked = if self.driver_features & common_feature::EVENT_IDX != 0 {
            u16_at(ring.driver + 4 + 2 * u64::from(ring.size)) == ring.used_idx
        } else {
            u16_at(ring.driver) & 1 == 0
        };
        if asked && self.vectors[queue] != NO_VECTOR {
            self.interrupts[queue] += 1;
        }
        put_used(ring, head, len);
    }

    /// Carries a control command out and answers it.
    fn answer(&mut self, (head, buffers): (u16, Vec<Buffer>)) {
        let (&(answer_at, answer_len, writes), reads) = buffers.split_last().unwrap();
        assert!(writes && reads.iter().all(|read| !read.2), "what the card reads, then its answer");
        let request: Vec<u8> =
            reads.iter().flat_map(|&(at, len, _)| read_mem(at, len as usize)).collect();
        assert!(request[4..HEADER].iter().all(|byte| *byte == 0), "no flags, fence or context");
        let code = u32_of(&request, 0);
        self.commands.push(code);
        let mut answer = std::vec![0u8; HEADER];
        let kind = match self.answer_with {
            Some((command, kind)) if command == code => kind,
            _ => self.carry_out(code, &request, &mut answer),
        };
        answer[..4].copy_from_slice(&kind.to_le_bytes());
        if let Some((command, bytes)) = self.short
            && command == code
        {
            answer.truncate(bytes);
        }
        answer.truncate(answer_len as usize);
        fake_memory_write(answer_at, &answer);
        self.used(0, head, answer.len() as u32);
    }

    /// Takes the control commands the driver made available.
    fn control(&mut self) {
        while !self.silent
            && let Some(chain) = next_chain(&mut self.rings[0])
        {
            if self.hold {
                self.held.push(chain);
            } else {
                self.answer(chain);
            }
        }
    }

    /// Takes the pointer's commands, and hands each back unanswered.
    fn point(&mut self) {
        while let Some((head, buffers)) = next_chain(&mut self.rings[1]) {
            assert_eq!(buffers, [(buffers[0].0, 56, false)], "a command, and no answer");
            let r = read_mem(buffers[0].0, 56);
            let code = u32_of(&r, 0);
            let place = Place { scanout: u32_of(&r, 24), x: u32_of(&r, 28), y: u32_of(&r, 32) };
            self.pointed.push((code, place));
            self.pointer.place = place;
            if code == code::UPDATE_CURSOR {
                self.pointer.resource = u32_of(&r, 40);
                self.pointer.hot = (u32_of(&r, 44), u32_of(&r, 48));
                // The card takes a copy of a picture of the one size it
                // draws.
                let side = (POINTER_SIDE, POINTER_SIDE);
                if let Some(resource) = self.resources.get(&self.pointer.resource)
                    && (resource.width, resource.height) == side
                {
                    self.pointer_pixels.clone_from(&resource.pixels);
                }
            } else {
                assert_eq!(code, code::MOVE_CURSOR);
            }
            if !self.keeps_pointer {
                self.used(1, head, 0);
            }
        }
    }

    /// What a reset does: the card forgets its queues and all it keeps.
    fn reset(&mut self) {
        self.rings = [Ring::default(); 2];
        self.resources.clear();
        for screen in &mut self.screens {
            (screen.shown, screen.glass) = (None, Vec::new());
        }
        self.pointer = Pointer::default();
        self.held.clear();
        (self.config_vector, self.vectors) = (NO_VECTOR, [NO_VECTOR; 2]);
        self.driver_features = 0;
    }
}

impl Card {
    /// A card that offers `features` of its own, with a screen for each
    /// of `screens`: the size the host would like, or no monitor.
    pub(crate) fn new(features: u64, screens: &[Option<(u32, u32)>]) -> Arc<Card> {
        let screen = |&plugged| Screen { plugged, ..Screen::default() };
        let mut state = State { screens: screens.iter().map(screen).collect(), ..State::default() };
        (state.config_vector, state.vectors) = (NO_VECTOR, [NO_VECTOR; 2]);
        let features = features | common_feature::VERSION_1 | common_feature::EVENT_IDX;
        Arc::new(Card { state: Mutex::new(state), features })
    }

    /// Plugs a monitor into screen `index`, or takes it out, and tells
    /// the driver.
    pub(crate) fn plug(&self, index: usize, plugged: Option<(u32, u32)>) {
        let mut s = self.state.lock().unwrap();
        s.screens[index].plugged = plugged;
        s.events |= EVENT_DISPLAY;
        if s.config_vector != NO_VECTOR {
            s.interrupts[0] += 1;
        }
    }

    /// Answers the commands the card held, and holds no more.
    pub(crate) fn release(&self) {
        let mut s = self.state.lock().unwrap();
        s.hold = false;
        for chain in std::mem::take(&mut s.held) {
            s.answer(chain);
        }
    }

    /// What a person sees on screen `index`.
    pub(crate) fn glass(&self, index: usize) -> Vec<u8> {
        self.state.lock().unwrap().screens[index].glass.clone()
    }

    /// The interrupts the card would have raised for its answers.
    pub(crate) fn interrupts(&self) -> u32 {
        self.state.lock().unwrap().interrupts[0]
    }
}

impl Registers for Card {
    fn read(&self, offset: usize, size: usize) -> u64 {
        let s = self.state.lock().unwrap();
        let select = usize::from(s.select);
        match (offset, size) {
            (0x04, 4) => (self.features >> (32 * s.device_select)) & 0xffff_ffff,
            (0x10, 2) => u64::from(s.config_vector),
            (0x12, 2) => 2,
            (0x14, 1) => u64::from(s.status),
            // The control queue's entries, and the pointer's.
            (0x18, 2) => [256, 16][select],
            (0x1a, 2) => u64::from(s.vectors[select]),
            (0x1e, 2) => u64::from(s.select),
            (0x2000, 4) => u64::from(s.events),
            (0x2008, 4) => s.screens.len() as u64,
            _ => 0,
        }
    }

    fn write(&self, offset: usize, _size: usize, value: u64) {
        let mut s = self.state.lock().unwrap();
        let select = usize::from(s.select);
        let vector = if s.no_vectors { NO_VECTOR } else { value as u16 };
        match offset {
            0x00 => s.device_select = value as u32,
            0x08 => s.driver_select = value as u32,
            0x0c => {
                let shift = 32 * s.driver_select;
                s.driver_features =
                    (s.driver_features & !(0xffff_ffff << shift)) | (value << shift);
            }
            0x10 => s.config_vector = vector,
            0x14 => {
                let value = value as u8;
                let refused =
                    value & status::FEATURES_OK != 0 && s.driver_features & !self.features != 0;
                s.status = if refused { value & !status::FEATURES_OK } else { value };
                if value == 0 {
                    s.reset();
                }
            }
            0x16 => s.select = value as u16,
            0x18 => s.rings[select].size = value as u16,
            0x1a => s.vectors[select] = vector,
            0x20 => s.rings[select].desc = value,
            0x28 => s.rings[select].driver = value,
            0x30 => s.rings[select].device = value,
            0x2004 => s.events &= !(value as u32),
            0x3000 | 0x3004 => {
                assert!(s.status & status::DRIVER_OK != 0, "told before the driver is ready");
                let queue = (offset - 0x3000) / 4;
                s.doorbells[queue] += 1;
                if queue == 0 { s.control() } else { s.point() }
            }
            _ => {}
        }
    }
}

/// The interrupts the tests give a card.
pub(crate) const VECTORS: Vectors = Vectors { config: 0, control: 1 };

/// A wait the test does not expect: the card answered at once.
pub(crate) fn never() -> bool {
    panic!("nothing to wait for: the card has answered")
}

/// A card that offers `features` with `screens`, and the driver on it,
/// which takes shared pages if `shared`.
pub(crate) fn setup_with(
    features: u64,
    screens: &[Option<(u32, u32)>],
    shared: bool,
) -> (Arc<Card>, Result<Gpu, Error>) {
    let card = Card::new(features, screens);
    let f = function(16, 0x1050);
    let regs = card.clone();
    let transport =
        Transport::find(&f, &mut |_, _| Some(Mmio::fake(regs.clone(), BAR_SIZE))).unwrap();
    (card, Gpu::new(transport, VECTORS, shared, &mut alloc))
}

/// A card with one screen of 640 by 480, a second with no monitor, and
/// monitor data.
pub(crate) fn setup() -> (Arc<Card>, Gpu) {
    let (card, gpu) = setup_with(feature::EDID, &[Some((640, 480)), None], false);
    (card, gpu.unwrap())
}

/// The pixel at (`x`, `y`) of picture `seed`: it differs from the ones
/// around it and from the same pixel of another picture.
fn pixel(seed: u8, x: u32, y: u32) -> [u8; 4] {
    [x as u8, y as u8, seed ^ ((x >> 8) as u8) ^ ((y >> 8) as u8) << 4, 0xff]
}

/// The pixels of `rect` of picture `seed`, row after row.
pub(crate) fn pixels(seed: u8, rect: Rect) -> Vec<u8> {
    let row = |y| (rect.x..rect.x + rect.width).flat_map(move |x| pixel(seed, x, y));
    (rect.y..rect.y + rect.height).flat_map(row).collect()
}

/// Memory with picture `seed`, of `width` by `height`, in it.
pub(crate) fn store(seed: u8, width: u32, height: u32) -> Dma {
    let memory = Dma::fake(width as usize * height as usize * 4);
    memory.write(0, &pixels(seed, Rect::sized(width, height)));
    memory
}

/// Writes `rows`, the pixels of `rect`, into `image`, a picture `width`
/// pixels wide: what a test expects of the glass.
pub(crate) fn paste(image: &mut [u8], width: u32, rect: Rect, rows: &[u8]) {
    let bytes = rect.width as usize * 4;
    for (line, row) in rows.chunks_exact(bytes).enumerate() {
        let to = ((rect.y as usize + line) * width as usize + rect.x as usize) * 4;
        image[to..to + bytes].copy_from_slice(row);
    }
}

/// Makes resource `id` of `width` by `height` with `memory` behind it,
/// and shows all of it on screen `scanout`.
pub(crate) fn show(
    gpu: &mut Gpu,
    id: u32,
    memory: &Dma,
    (width, height): (u32, u32),
    scanout: u32,
) {
    let all = Rect::sized(width, height);
    gpu.create(id, format::BGRX, width, height).unwrap();
    gpu.attach(id, memory.phys(), memory.len() as u32).unwrap();
    gpu.transfer(id, all, 0).unwrap();
    gpu.set_scanout(scanout, id, all).unwrap();
    gpu.flush(id, all).unwrap();
}
