// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The machine's hardware watchdog (Phase 11, ADR 0143), which restarts
//! the machine unless it is petted in time.
//!
//! [`find`] asks the platform for one at boot, after the PCI scan, and
//! logs whether it ended the boot before. Spark starts it through the
//! power capability and pets it from its loop, so a Spark that stops
//! responding, or a kernel that stops running it, restarts the machine.
//! Before the machine is powered off or restarted, [`ending`] gives it its
//! longest timeout, up to [`ENDING_SECONDS`], so that a long write-back is
//! not cut short but a hung one still ends; before it is halted, it stops.
//! kdb pets it while it waits for a command ([`pet_from_kdb`]).
//! `/kernel/class/watchdog/watchdog0` shows its state, as Linux's sysfs
//! does.
//!
//! On a server the BMC's watchdog (ADR 0661) is taken before the
//! chipset's: it is the one firmware leaves working, where the chipset's
//! may be locked so that it never restarts, and its expiry is written to
//! the BMC's event log. [`crate::ipmi::find`] runs first, so that the BMC
//! is there to ask. Its commands sleep while the BMC answers, so the state
//! is behind a sleeping lock; kdb, which may not sleep, takes neither that
//! lock nor the BMC's unless they are free, and its waits spin.

use radix_hal::{Devices, Watchdog};
use radix_klog::{info, warn};
use radix_sync::{Mutex, Once};

use crate::Arch;

/// The longest timeout while the filesystems are written back at the end.
pub const ENDING_SECONDS: u32 = 600;

/// Which watchdog it is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Provider {
    /// The platform's: the chipset's or the board's, through [`Arch`].
    Platform,
    /// The BMC's, through IPMI.
    Bmc,
}

/// The watchdog found, and whose it is.
static FOUND: Once<(Watchdog, Provider)> = Once::new();

/// Whether it runs, and its timeout in seconds.
static STATE: Mutex<(bool, u32)> = Mutex::new((false, 0));

/// Asks the BMC, then the platform, for the watchdog, which it leaves
/// stopped.
pub fn find() {
    let (found, provider) = match bmc::find() {
        Some(found) => {
            if let Some(platform) = Arch::find_watchdog() {
                info!("Watchdog: {} is there too; the BMC's is used", platform.name);
            }
            (found, Provider::Bmc)
        }
        None => match Arch::find_watchdog() {
            Some(found) => (found, Provider::Platform),
            None => {
                info!("Watchdog: none the kernel drives");
                return;
            }
        },
    };
    if found.restarted {
        warn!("Watchdog: {} restarted the machine: the boot before this one hung", found.name);
    }
    info!(
        "Watchdog: {}, with timeouts from {} to {} s; Spark starts it",
        found.name, found.min_seconds, found.max_seconds
    );
    let _ = FOUND.set((found, provider));
}

/// The watchdog, if one was found.
pub fn found() -> Option<&'static Watchdog> {
    FOUND.get().map(|(found, _)| found)
}

fn provider() -> Option<Provider> {
    FOUND.get().map(|&(_, provider)| provider)
}

/// Whether it runs, and its timeout.
pub fn state() -> (bool, u32) {
    *STATE.lock()
}

/// Starts the watchdog of `provider`, or changes its timeout, with
/// `running` saying whether it runs now; returns the timeout set.
fn provider_start(provider: Provider, seconds: u32, running: bool) -> u32 {
    match provider {
        Provider::Platform => Arch::watchdog_start(seconds),
        Provider::Bmc => bmc::start(seconds, running),
    }
}

fn provider_pet(provider: Provider) {
    match provider {
        Provider::Platform => Arch::watchdog_pet(),
        Provider::Bmc => bmc::pet(),
    }
}

fn provider_stop(provider: Provider) {
    match provider {
        Provider::Platform => Arch::watchdog_stop(),
        Provider::Bmc => bmc::stop(),
    }
}

/// Starts it, or changes its timeout, for `who`; returns the timeout set,
/// or `None` without a watchdog.
pub fn start(seconds: u32, who: &str) -> Option<u32> {
    let &(ref found, provider) = FOUND.get()?;
    let (set, changed) = {
        let mut state = STATE.lock();
        let set = provider_start(provider, seconds, state.0);
        let changed = *state != (true, set);
        *state = (true, set);
        (set, changed)
    };
    // Not under the lock: a console the log writes to may take locks.
    if changed {
        info!(
            "Watchdog: `{who}` started {}: the machine restarts unless petted every {set} s",
            found.name
        );
    }
    Some(set)
}

/// Pets it; false if it is not running.
pub fn pet() -> bool {
    let Some(provider) = provider() else { return false };
    let state = STATE.lock();
    if state.0 {
        provider_pet(provider);
    }
    state.0
}

/// Pets it from kdb, which stops the CPU Spark may need, and after a
/// panic every CPU: whoever is at the debugger is not a hung machine. It
/// waits for no lock, since a stopped CPU may hold it, and skips a pet
/// that one was in the middle of.
pub fn pet_from_kdb() {
    let Some(provider) = provider() else { return };
    if let Some(state) = STATE.try_lock()
        && state.0
    {
        match provider {
            Provider::Platform => Arch::watchdog_pet(),
            Provider::Bmc => bmc::pet_spinning(),
        }
    }
}

/// Stops it, for `who`; false without a watchdog.
pub fn stop(who: &str) -> bool {
    let Some(&(ref found, provider)) = FOUND.get() else { return false };
    let was = {
        let mut state = STATE.lock();
        if state.0 {
            provider_stop(provider);
        }
        core::mem::replace(&mut state.0, false)
    };
    if was {
        info!("Watchdog: `{who}` stopped {}", found.name);
    }
    true
}

/// Before the machine ends: when it `halts`, stops the watchdog, so the
/// machine stays halted; otherwise gives a running one its longest
/// timeout, up to [`ENDING_SECONDS`].
pub fn ending(halts: bool) {
    let Some(&(ref found, provider)) = FOUND.get() else { return };
    let stopped = {
        let mut state = STATE.lock();
        if !state.0 {
            return;
        }
        if halts {
            provider_stop(provider);
            state.0 = false;
        } else {
            state.1 = provider_start(provider, found.max_seconds.min(ENDING_SECONDS), true);
        }
        halts
    };
    if stopped {
        info!("Watchdog: stopped, so the halted machine stays halted");
    }
}

/// The BMC's watchdog, through IPMI's commands for it (IPMI 2.0 section
/// 27): Set Watchdog Timer gives it its use, its action and its timeout,
/// and stops it unless told not to; Reset Watchdog Timer starts the
/// countdown again from the timeout; Get Watchdog Timer says what it is.
mod bmc {
    use core::sync::atomic::{AtomicBool, Ordering};

    use radix_hal::Watchdog;
    use radix_klog::warn;

    use crate::ipmi::{self, Failed};

    /// The application network function (IPMI 2.0 table 5-1).
    const APP: u8 = 0x06;
    /// Reset Watchdog Timer, Set Watchdog Timer and Get Watchdog Timer
    /// (IPMI 2.0 table G-1).
    const RESET: u8 = 0x22;
    const SET: u8 = 0x24;
    const GET: u8 = 0x25;

    /// The timer's use: the operating system's (SMS/OS, table 27-6).
    const USE_OS: u8 = 0x04;
    /// Set's bit that keeps a running timer running (table 27-6).
    const DONT_STOP: u8 = 0x40;
    /// Its actions at expiry: none, or a hard reset (table 27-6).
    const NO_ACTION: u8 = 0x00;
    const HARD_RESET: u8 = 0x01;
    /// The OS use's bit among the expiration flags (tables 27-6, 27-7).
    const EXPIRED_OS: u8 = 0x10;
    /// The countdown counts tenths of a second, in 16 bits.
    const MAX_TENTHS: u32 = 0xffff;

    /// Whether a failed pet has been logged since the last that worked, so
    /// that a BMC that stops answering is logged once, not every pet.
    static PET_FAILED: AtomicBool = AtomicBool::new(false);

    /// Sends `cmd` with `data`; the completion code must be 0. Returns the
    /// answer's bytes after it, in `answer`.
    fn command(cmd: u8, data: &[u8], answer: &mut [u8; 16]) -> Result<usize, Failed> {
        let len = ipmi::request(APP, cmd, data, answer)?;
        check(answer, len)
    }

    fn check(answer: &[u8; 16], len: usize) -> Result<usize, Failed> {
        match answer.first() {
            Some(0) if len > 0 => Ok(len),
            Some(&code) if len > 0 => Err(Failed::Bmc(radix_ipmi::Error::Completion(code))),
            _ => Err(Failed::Bmc(radix_ipmi::Error::ShortResponse)),
        }
    }

    /// Set Watchdog Timer's data: the OS's use, `action` at expiry,
    /// `tenths` of a second, no pre-timeout, and the expiration flags in
    /// `clear` cleared.
    fn set_data(keep_running: bool, action: u8, tenths: u32, clear: u8) -> [u8; 6] {
        let tenths = tenths.min(MAX_TENTHS) as u16;
        let [low, high] = tenths.to_le_bytes();
        let used = if keep_running { USE_OS | DONT_STOP } else { USE_OS };
        [used, action, 0, clear, low, high]
    }

    /// The BMC's watchdog, stopped, if the machine has a BMC that has one;
    /// clears the flag that says it restarted the machine, once read.
    pub fn find() -> Option<Watchdog> {
        ipmi::device()?;
        let mut answer = [0u8; 16];
        // Completion code, use, actions, pre-timeout, expiration flags,
        // timeout and countdown (table 27-7).
        if !matches!(command(GET, &[], &mut answer), Ok(len) if len >= 5) {
            return None;
        }
        let restarted = answer[4] & EXPIRED_OS != 0;
        // Stopped, as the platform's are left, with nothing to do at expiry
        // until Spark starts it.
        if let Err(err) = command(SET, &set_data(false, NO_ACTION, 600, EXPIRED_OS), &mut answer) {
            warn!("Watchdog: the BMC's could not be stopped ({err}); it is not used");
            return None;
        }
        Some(Watchdog {
            name: "the BMC's watchdog (IPMI)",
            min_seconds: 1,
            max_seconds: MAX_TENTHS / 10,
            restarted,
        })
    }

    /// Starts it, or gives a `running` one a new timeout; returns the
    /// timeout set.
    pub fn start(seconds: u32, running: bool) -> u32 {
        let seconds = seconds.clamp(1, MAX_TENTHS / 10);
        let mut answer = [0u8; 16];
        let set = set_data(running, HARD_RESET, seconds * 10, 0);
        // Set takes the timeout; Reset starts the countdown from it.
        let started =
            command(SET, &set, &mut answer).and_then(|_| command(RESET, &[], &mut answer));
        if let Err(err) = started {
            warn!("Watchdog: the BMC's did not start: {err}");
        }
        seconds
    }

    /// Starts its countdown again.
    pub fn pet() {
        let mut answer = [0u8; 16];
        match command(RESET, &[], &mut answer) {
            Ok(_) => PET_FAILED.store(false, Ordering::Relaxed),
            Err(err) => {
                if !PET_FAILED.swap(true, Ordering::Relaxed) {
                    warn!("Watchdog: the BMC's could not be petted: {err}");
                }
            }
        }
    }

    /// [`pet`], from kdb: skipped if a message is in progress, and its
    /// waits spin.
    pub fn pet_spinning() {
        let mut answer = [0u8; 16];
        let _ = ipmi::request_spinning(APP, RESET, &[], &mut answer);
    }

    /// Stops it, with nothing to do at expiry.
    pub fn stop() {
        let mut answer = [0u8; 16];
        if let Err(err) = command(SET, &set_data(false, NO_ACTION, 600, 0), &mut answer) {
            warn!("Watchdog: the BMC's did not stop: {err}");
        }
    }
}
