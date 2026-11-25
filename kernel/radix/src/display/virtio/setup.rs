// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Setting a virtio graphics card up as a card of the layer: its
//! transport and interrupts, what it keeps of its device, and what it
//! tells the layer it is.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use radix_abi::display::{
    CARD_HOT_PLUG, CARD_MIRRORS, CARD_SETS_MODES, CARD_SHARED_PAGES, CARD_VIRTIO, FORMAT_ARGB8888,
    FORMAT_XRGB8888, Mode,
};
use radix_hal::Devices;
use radix_hw::pci::{Address, Bar, Function, MsiX};
use radix_klog::{TextBuf, info, warn};
use radix_mm::Kmalloc;
use radix_sync::Mutex;
use radix_virtio::gpu::{Band, Gpu, POINTER_SIDE, SCANOUTS_MAX, Scanout, Vectors, format};
use radix_virtio::{Error, NO_VECTOR, Transport};

use super::super::{
    CardRef, CardSpec, Driver, PAGE, PlaneSpec, ScreenSpec, add_card, firmware, kmalloc, name,
};
use super::device::Device;
use super::log::Log;
use super::shown::{POINTER, Shown};
use super::{
    SHARE, SIDE_LEAST, SIDE_MOST, SLOTS, STORES, Slot, State, TAKEN, Virtio, answered, changed,
    mode_of, waiter, why,
};
use crate::Arch;

/// The text band: a quarter of a mebibyte of rows on their way to the
/// host.
const BAND_PAGES: usize = 64;

/// Sets up the virtio graphics card `function` as a card of the layer,
/// saying why in the log if it cannot be.
pub fn bind(function: &Function) {
    if let Err(why) = try_bind(function) {
        warn!(
            "Display: could not set up the virtio graphics card at {}: {why}",
            function.address()
        );
    }
}

/// Gives the card two MSI-X interrupts, for its screens and for its
/// answers, and returns their vectors: `None`, with nothing kept, if the
/// card and the kernel have not both.
fn interrupts(function: &Function, slot: usize) -> Option<[u32; 2]> {
    let msix = MsiX::find(function).filter(|msix| msix.entries() >= 2)?;
    let (bar, offset) = msix.table_location();
    let Some(Bar::Memory { address, .. }) = function.bar(bar) else { return None };
    let table = <Arch as Devices>::map_device(address + offset, msix.table_len())?;
    let source = function.address();
    let (screens, first) = <Arch as Devices>::allocate_interrupt(source, changed, slot)?;
    let Some((answers, second)) = <Arch as Devices>::allocate_interrupt(source, answered, slot)
    else {
        <Arch as Devices>::free_interrupt(screens);
        return None;
    };
    msix.enable_masked(function);
    msix.set_entry(&table, 0, first);
    msix.set_entry(&table, 1, second);
    msix.unmask_all(function);
    Some([screens, answers])
}

/// [`bind`]. Its frame holds the driver before it goes on the heap: kept
/// out of the scan's (tools/xtask/src/frames.rs allows it).
#[inline(never)]
fn try_bind(function: &Function) -> Result<(), &'static str> {
    let index = TAKEN.fetch_add(1, Ordering::AcqRel);
    let slot = SLOTS.get(index).ok_or("more graphics cards than the layer keeps")?;
    // Before the device is touched: nothing is to undo if there is none.
    let place = Box::try_new_uninit_in(kmalloc()).map_err(|_| "out of memory")?;
    function.enable_memory_and_dma();
    let mut map_bar = |_: usize, bar: Bar| match bar {
        Bar::Memory { address, size, .. } => {
            <Arch as Devices>::map_device(address, usize::try_from(size).ok()?)
        }
        Bar::Io { .. } => None,
    };
    let transport = Transport::find(function, &mut map_bar).map_err(|_| "no modern transport")?;
    let vectors = interrupts(function, index);
    let entries = match vectors {
        Some(_) => Vectors { config: 0, control: 1 },
        None => Vectors { config: NO_VECTOR, control: NO_VECTOR },
    };
    let source = function.address();
    let mut alloc = |pages: usize| <Arch as Devices>::alloc_dma(source, pages, false);
    let share = SHARE.load(Ordering::Relaxed);
    let gpu = match Gpu::new(transport, entries, share, &mut alloc) {
        Ok(gpu) => gpu,
        Err(err) => {
            vectors.into_iter().flatten().for_each(<Arch as Devices>::free_interrupt);
            return Err(why(err));
        }
    };
    // From here a failure drops the device's state, which lets it go.
    let vectors = vectors.map_or([None; 2], |[screens, answers]| [Some(screens), Some(answers)]);
    let device = start(Box::write(place, gpu), source, vectors, slot)?;
    let card = add(device, index, slot)?;
    let _ = slot.card.set(card.clone());
    // Its thread reads the monitors' data, which the layer has not.
    slot.changed.store(true, Ordering::Release);
    card.wake();
    if firmware::step_aside(function) {
        info!("Display: firmware0 is gone: its screen is virtio{index}'s");
    }
    Ok(())
}

/// Makes what the card keeps of its device: the screens it has, the
/// text band, and the pointer's resource.
fn start(
    mut gpu: Box<Gpu, Kmalloc>,
    source: Address,
    vectors: [Option<u32>; 2],
    slot: &'static Slot,
) -> Result<Device, &'static str> {
    let mut wait = waiter(slot);
    let pointer_bytes = (POINTER_SIDE * POINTER_SIDE * 4) as usize;
    let pointer = <Arch as Devices>::alloc_dma(source, pointer_bytes.div_ceil(PAGE), false);
    // A card with no memory for the band keeps a store for each picture.
    let band = (!STORES.load(Ordering::Relaxed))
        .then(|| <Arch as Devices>::alloc_dma(source, BAND_PAGES, false))
        .flatten()
        .map(Band::new);
    let count = gpu.scanouts() as usize;
    let made = pointer.ok_or(Error::NoMemory).and_then(|pointer| {
        let found = gpu.display_info(&mut wait)?;
        gpu.create(POINTER, format::BGRA, POINTER_SIDE, POINTER_SIDE)?;
        gpu.attach(POINTER, pointer.phys(), pointer_bytes as u32)?;
        gpu.finish(&mut wait)?;
        Ok((pointer, found))
    });
    let (pointer, found) = match made {
        Ok(made) => made,
        Err(err) => {
            gpu.reset();
            vectors.into_iter().flatten().for_each(<Arch as Devices>::free_interrupt);
            return Err(why(err));
        }
    };
    Ok(Device {
        gpu,
        source,
        vectors,
        band,
        stores: Vec::new_in(kmalloc()),
        stored: 0,
        next: POINTER + 1,
        shown: Shown { screens: [None; SCANOUTS_MAX], count, pointer },
        found,
        lost: 0,
    })
}

/// What the layer is told of the card's screens: each as the device
/// found it.
fn screens_of(found: &[Scanout; SCANOUTS_MAX]) -> [ScreenSpec; SCANOUTS_MAX] {
    let mut screens =
        [ScreenSpec { plugged: false, fixed_pipe: None, native: Mode::default() }; SCANOUTS_MAX];
    for (screen, found) in screens.iter_mut().zip(found) {
        (screen.plugged, screen.native) = (found.enabled, mode_of(found));
    }
    screens
}

/// Adds the card to the layer as `virtio` and its place among the
/// virtio cards, and says in the log what it is.
fn add(device: Device, index: usize, slot: &'static Slot) -> Result<CardRef, &'static str> {
    let mut label = TextBuf::<16>::new();
    let _ = write!(label, "virtio{index}");
    let count = device.shown.count;
    let screens = screens_of(&device.found);
    let shares = device.gpu.shares_pages();
    let flags = CARD_HOT_PLUG | CARD_MIRRORS | CARD_SETS_MODES;
    let formats = |format: u32| [format, 0, 0, 0, 0, 0, 0, 0];
    let spec = CardSpec {
        kind: CARD_VIRTIO,
        name: name(label.as_str()),
        model: name("Virtio graphics card"),
        flags: if shares { flags | CARD_SHARED_PAGES } else { flags },
        width_min: SIDE_LEAST,
        height_min: SIDE_LEAST,
        width_max: SIDE_MOST,
        height_max: SIDE_MOST,
        pipes: count,
        screens: &screens[..count],
        main: PlaneSpec {
            width_max: SIDE_MOST,
            height_max: SIDE_MOST,
            formats: formats(FORMAT_XRGB8888),
        },
        pointer: PlaneSpec {
            width_max: POINTER_SIDE,
            height_max: POINTER_SIDE,
            formats: formats(FORMAT_ARGB8888),
        },
    };
    let (source, polled, band) =
        (device.source, device.vectors[0].is_none(), device.band.is_some());
    let plugged = screens.iter().filter(|screen| screen.plugged).count();
    let card = add_card(&spec, |id| {
        let card = Virtio {
            id,
            slot,
            handed: AtomicBool::new(false),
            polled,
            ask_at: AtomicU64::new(0),
            state: Mutex::new(State { device, log: Log::boxed()? }),
        };
        let driver: Box<dyn Driver, Kmalloc> = Box::try_new_in(card, kmalloc()).ok()?;
        Some(driver)
    })?;
    info!(
        "Display: {label} is a virtio graphics card at {source}: {count} screens, {plugged} with \
         a monitor, {}, {}, {}",
        if band { "a text band" } else { "a store for each kept picture" },
        if shares { "pages shared with the host" } else { "pictures copied to the host" },
        if polled { "polled" } else { "MSI-X" },
    );
    Ok(card)
}
