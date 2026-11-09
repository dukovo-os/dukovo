// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! USB speakers, headsets and DACs (ADR 0323), the outputs `usb0` and on:
//! the worker's side of them.
//!
//! A USB device is driven by its controller's thread (`crate::usb`), which
//! alone talks to the controller; the output's worker talks to that
//! thread through a [`Link`]. The worker writes each period into the
//! link's ring of samples (widened to the setting's sample size), notes
//! where it ends, and wakes the thread, which queues the ring's bytes as
//! a packet each service interval and notes what the controller has
//! played. A period is handed back once everything up to its end has
//! played. Setting the format up and letting go of it are asked of the
//! thread, which answers, and the worker waits for the answer; the volume
//! and starting and stopping are only told.
//!
//! When the device is unplugged the thread takes its output away
//! (`super::detach`) and marks the link gone, and the worker lets go of
//! this side, the link's last holder, whose memory then goes.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use radix_audio::{Format, Fragments, MAX_FRAGMENT, Volume};
use radix_hw::Dma;
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::Mutex;
use radix_usb::audio::{MAX_PERIODS, Periods, Stream, widen};

use super::{ANSWER_MS, Driver};
use crate::sched;

/// What the worker asks the thread to set the stream up for: the format
/// and the period's bytes in the ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub format: Format,
    pub period: u32,
}

/// What the worker and the thread share, under the link's lock.
pub struct Shared {
    /// The ring of samples, which the controller reads.
    pub memory: Dma,
    /// What is written into it, queued and played.
    pub stream: Stream,
    /// The periods written and not yet played.
    pub periods: Periods,
    /// The bytes of each sample in the ring: 2 to 4.
    pub subframe: u8,
    /// The worker's last ask, counted, and what it asks: a format, or
    /// nothing (let go of the stream).
    pub asked: u64,
    pub wanted: Option<Wanted>,
    /// The last ask the thread answered, and its answer.
    pub answered: u64,
    pub answer: Result<(), &'static str>,
    /// Whether the worker wants the stream to play.
    pub running: bool,
    /// A volume the worker set and the thread has not yet sent.
    pub volume: Option<Volume>,
    /// The device was unplugged.
    pub gone: bool,
    /// The device stopped answering.
    pub failed: bool,
}

/// The worker's and the controller thread's hold on a USB output.
pub struct Link {
    pub shared: Mutex<Shared>,
    /// The index of the controller whose thread drives the device.
    controller: usize,
    /// The output's index, once it has one.
    output: AtomicUsize,
}

impl Link {
    /// A link for the device on controller `controller`, whose stream
    /// plays from `memory`, in `frame`-byte frames of `subframe`-byte
    /// samples. Its frame holds the stream's TDs (6 KiB) on their way to
    /// the heap, so it is kept out of its callers' frames and runs only
    /// on a controller's thread (`tools/xtask/src/frames.rs`).
    #[inline(never)]
    pub fn new(
        controller: usize,
        memory: Dma,
        frame: u32,
        subframe: u8,
    ) -> Result<Arc<Link, Kmalloc>, &'static str> {
        let size = memory.len() as u32 / frame * frame;
        let stream = Stream::new(memory.phys(), size, frame);
        let shared = Shared {
            memory,
            stream,
            periods: Periods::default(),
            subframe,
            asked: 0,
            wanted: None,
            answered: 0,
            answer: Ok(()),
            running: false,
            volume: None,
            gone: false,
            failed: false,
        };
        let link =
            Link { shared: Mutex::new(shared), controller, output: AtomicUsize::new(usize::MAX) };
        Arc::try_new_in(link, HEAP.with(Flags::MAY_SLEEP)).map_err(|_| "out of memory")
    }

    /// Notes the output's index, whose worker the thread wakes.
    pub fn set_output(&self, index: usize) {
        self.output.store(index, Ordering::Release);
    }

    /// Wakes the output's worker: periods played, or an ask answered.
    pub fn wake_worker(&self) {
        let index = self.output.load(Ordering::Acquire);
        if index != usize::MAX {
            super::interrupt(index);
        }
    }

    /// Wakes the controller's thread: there is something to do.
    fn wake_thread(&self) {
        crate::usb::wake_controller(self.controller);
    }
}

/// A USB output as its worker drives it.
pub struct Output {
    link: Arc<Link, Kmalloc>,
    /// The periods the stream is set up for, and their bytes in the ring.
    count: u32,
    period: u32,
    /// Whether the device has a volume of its own.
    hardware_volume: bool,
    /// Where a period is widened before it goes into the ring.
    staging: Vec<u8, Kmalloc>,
}

impl Output {
    /// The worker's side of `link`, of a device that has a volume of its
    /// own if `hardware_volume`.
    pub fn new(link: Arc<Link, Kmalloc>, hardware_volume: bool) -> Result<Output, &'static str> {
        let subframe = link.shared.lock().subframe;
        let room = MAX_FRAGMENT as usize / 2 * usize::from(subframe);
        let staging = radix_mm::bytes::zeroed(room, HEAP.with(Flags::MAY_SLEEP))
            .map_err(|_| "out of memory")?;
        Ok(Output { link, count: 0, period: 0, hardware_volume, staging })
    }

    /// Asks the thread to set the stream up for `wanted`, or to let go of
    /// it, and waits for its answer.
    fn ask(&mut self, wanted: Option<Wanted>) -> Result<(), &'static str> {
        let ask = {
            let mut shared = self.link.shared.lock();
            if shared.gone {
                return Err("it was unplugged");
            }
            shared.asked += 1;
            shared.wanted = wanted;
            shared.running = false;
            shared.asked
        };
        self.link.wake_thread();
        for _ in 0..ANSWER_MS {
            {
                let shared = self.link.shared.lock();
                if shared.answered == ask {
                    return shared.answer;
                }
                if shared.gone {
                    return Err("it was unplugged");
                }
            }
            sched::sleep_ns(1_000_000);
        }
        self.link.shared.lock().failed = true;
        Err("the device did not answer")
    }
}

/// The bytes that `bytes` of 16-bit samples take in the ring, in samples
/// of `subframe` bytes.
fn widened(bytes: u32, subframe: u8) -> u32 {
    bytes / 2 * u32::from(subframe)
}

impl Driver for Output {
    fn max_periods(&self) -> u32 {
        MAX_PERIODS as u32
    }

    fn configure(&mut self, format: Format, fragments: Fragments) -> Result<(), &'static str> {
        let subframe = self.link.shared.lock().subframe;
        let period = widened(fragments.bytes, subframe);
        self.ask(Some(Wanted { format, period }))?;
        self.count = fragments.count.min(MAX_PERIODS as u32);
        self.period = period;
        Ok(())
    }

    fn can_submit(&self) -> bool {
        let shared = self.link.shared.lock();
        shared.answered == shared.asked
            && shared.answer.is_ok()
            && shared.wanted.is_some()
            && !shared.gone
            && !shared.failed
            && (shared.periods.len() as u32) < self.count
            && shared.stream.room() >= self.period
    }

    fn submit(&mut self, samples: &[u8]) -> bool {
        let subframe = self.link.shared.lock().subframe;
        let bytes = widen(samples, subframe, &mut self.staging);
        let mut shared = self.link.shared.lock();
        let Shared { memory, stream, periods, .. } = &mut *shared;
        if bytes == 0 || (stream.room() as usize) < bytes || periods.len() == MAX_PERIODS {
            return false;
        }
        // Into the ring where the stream is written to, wrapping at its
        // end.
        let at = stream.write_offset() as usize;
        let size = stream.size() as usize;
        let first = bytes.min(size - at);
        memory.write(at, &self.staging[..first]);
        memory.write(0, &self.staging[first..bytes]);
        stream.wrote(bytes as u32);
        periods.push(stream.written())
    }

    fn notify(&mut self) {
        self.link.wake_thread();
    }

    fn collect(&mut self) -> Option<bool> {
        let mut shared = self.link.shared.lock();
        let played = shared.stream.played();
        shared.periods.collect(played).then_some(true)
    }

    fn in_flight(&self) -> u32 {
        self.link.shared.lock().periods.len() as u32
    }

    fn start(&mut self) -> bool {
        self.link.shared.lock().running = true;
        self.link.wake_thread();
        true
    }

    fn stop(&mut self) -> bool {
        self.link.shared.lock().running = false;
        self.link.wake_thread();
        true
    }

    fn release(&mut self) {
        let _ = self.ask(None);
        self.count = 0;
    }

    fn failed(&self) -> bool {
        let shared = self.link.shared.lock();
        shared.gone || shared.failed
    }

    fn set_volume(&mut self, volume: Volume) -> bool {
        if !self.hardware_volume {
            return false;
        }
        self.link.shared.lock().volume = Some(volume);
        self.link.wake_thread();
        true
    }
}
