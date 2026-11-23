// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The KCS state machine of section 9 of the IPMI 2.0 specification: the
//! write phase that sends a request, the read phase that takes the
//! response, and the exit that gets the BMC back to idle after an error.
//!
//! The status register has OBF (the BMC has a byte for the host) in bit
//! 0, IBF (the host's last byte is not yet taken) in bit 1, SMS_ATN in
//! bit 2, C/D (whether the last write was a command) in bit 3, and the
//! BMC's state in bits 7:6. The host writes only while IBF is clear, and
//! reads a byte only while OBF is set. Every wait is bounded by the time
//! left of the message's 5 seconds.

use crate::locate::Interface;
use crate::{Error, MAX_MESSAGE, Platform};

/// Status register: the BMC has a byte in the data register.
const OBF: u8 = 1 << 0;
/// Status register: the host's byte is not taken yet.
const IBF: u8 = 1 << 1;
/// The state, bits 7:6 of the status register.
const STATE_SHIFT: u8 = 6;
const STATE_IDLE: u8 = 0;
const STATE_READ: u8 = 1;
const STATE_WRITE: u8 = 2;

/// Control codes written to the command register (section 9.x, the
/// control codes' table). Abort and Get Status share one.
const GET_STATUS_ABORT: u8 = 0x60;
const WRITE_START: u8 = 0x61;
const WRITE_END: u8 = 0x62;
/// Written to the data register in the read phase: send the next byte.
const READ: u8 = 0x68;

/// How many times a message is tried, an abort between each.
const TRIES: u32 = 3;
/// All the waiting a message may do: 5 seconds.
const BUDGET_US: u64 = 5_000_000;
/// A wait begins at this many microseconds and doubles up to the second
/// number, so a quick BMC is polled quickly and a slow one is not
/// polled constantly.
const FIRST_POLL_US: u64 = 10;
const LAST_POLL_US: u64 = 1000;

/// One exchange: the registers, and the time left.
pub(crate) struct Kcs<'a, P> {
    platform: &'a P,
    data: u16,
    command: u16,
    left_us: u64,
}

fn state(status: u8) -> u8 {
    status >> STATE_SHIFT
}

impl<'a, P: Platform> Kcs<'a, P> {
    pub(crate) fn new(platform: &'a P, interface: Interface) -> Kcs<'a, P> {
        Kcs { platform, data: interface.data(), command: interface.command(), left_us: BUDGET_US }
    }

    /// Sends a request of `header` (the first two bytes) and `data`, and
    /// reads the response: its first two bytes are returned, the rest
    /// go into `response` as far as it reaches. Also returns how many
    /// bytes the response had in all.
    pub(crate) fn exchange(
        &mut self,
        header: [u8; 2],
        data: &[u8],
        response: &mut [u8],
    ) -> Result<([u8; 2], usize), Error> {
        for _ in 0..TRIES {
            match self.attempt(header, data, response) {
                Err(Error::Interface) => {
                    // An abort that fails the same way is left for the
                    // next try to meet; one that times out has spent the
                    // budget.
                    if let Err(timeout @ Error::Timeout) = self.abort() {
                        return Err(timeout);
                    }
                }
                outcome => return outcome,
            }
        }
        Err(Error::Interface)
    }

    fn attempt(
        &mut self,
        header: [u8; 2],
        data: &[u8],
        response: &mut [u8],
    ) -> Result<([u8; 2], usize), Error> {
        // All ones, which no BMC's busy interface reads: the ports lead
        // nowhere, and waiting would take the whole budget.
        if self.platform.in8(self.command) == 0xff {
            return Err(Error::NoDevice);
        }
        let status = self.wait_ready()?;
        if state(status) != STATE_IDLE {
            return Err(Error::Interface);
        }
        self.clear_obf(status);
        self.platform.out8(self.command, WRITE_START);
        self.expect_write_state()?;
        let total = header.len() + data.len();
        for (at, &byte) in header.iter().chain(data).enumerate() {
            if at + 1 == total {
                // The last byte follows WRITE_END, which tells the BMC
                // that the request ends with it.
                self.platform.out8(self.command, WRITE_END);
                self.expect_write_state()?;
            }
            self.platform.out8(self.data, byte);
            if at + 1 < total {
                self.expect_write_state()?;
            }
        }
        self.read_response(response)
    }

    /// Waits for the BMC to take the last byte, and checks that it is in
    /// the write state; then clears OBF, which the BMC may have set.
    fn expect_write_state(&mut self) -> Result<(), Error> {
        let status = self.wait_ready()?;
        if state(status) != STATE_WRITE {
            return Err(Error::Interface);
        }
        self.clear_obf(status);
        Ok(())
    }

    fn read_response(&mut self, response: &mut [u8]) -> Result<([u8; 2], usize), Error> {
        let mut header = [0; 2];
        let mut count = 0;
        loop {
            let status = self.wait_ready()?;
            match state(status) {
                STATE_READ => {
                    self.wait_obf()?;
                    let byte = self.platform.in8(self.data);
                    match count {
                        0 | 1 => header[count] = byte,
                        _ => {
                            if let Some(slot) = response.get_mut(count - 2) {
                                *slot = byte;
                            }
                        }
                    }
                    count += 1;
                    // A BMC that never ends its response is not a BMC
                    // that follows the protocol.
                    if count > MAX_MESSAGE {
                        return Err(Error::Interface);
                    }
                    self.platform.out8(self.data, READ);
                }
                STATE_IDLE if count > 0 => {
                    // The response is over; the BMC leaves a dummy byte,
                    // which is read to clear OBF.
                    self.wait_obf()?;
                    self.platform.in8(self.data);
                    return Ok((header, count));
                }
                _ => return Err(Error::Interface),
            }
        }
    }

    /// The exit from a failed exchange: ABORT, a zero data byte, the
    /// error code the BMC gives back, then idle.
    fn abort(&mut self) -> Result<(), Error> {
        let status = self.wait_ready()?;
        self.clear_obf(status);
        self.platform.out8(self.command, GET_STATUS_ABORT);
        let status = self.wait_ready()?;
        self.clear_obf(status);
        self.platform.out8(self.data, 0);
        let status = self.wait_ready()?;
        if state(status) != STATE_READ {
            return Err(Error::Interface);
        }
        // The byte is the BMC's error code for the exchange it dropped.
        self.wait_obf()?;
        self.platform.in8(self.data);
        self.platform.out8(self.data, READ);
        let status = self.wait_ready()?;
        if state(status) != STATE_IDLE {
            return Err(Error::Interface);
        }
        self.wait_obf()?;
        self.platform.in8(self.data);
        Ok(())
    }

    fn clear_obf(&self, status: u8) {
        if status & OBF != 0 {
            self.platform.in8(self.data);
        }
    }

    /// Waits for IBF to clear, and returns the status it read.
    fn wait_ready(&mut self) -> Result<u8, Error> {
        self.wait(|status| status & IBF == 0)
    }

    fn wait_obf(&mut self) -> Result<u8, Error> {
        self.wait(|status| status & OBF != 0)
    }

    fn wait(&mut self, done: fn(u8) -> bool) -> Result<u8, Error> {
        let mut step = FIRST_POLL_US;
        loop {
            let status = self.platform.in8(self.command);
            if done(status) {
                return Ok(status);
            }
            if self.left_us == 0 {
                return Err(Error::Timeout);
            }
            let us = step.min(self.left_us);
            self.platform.wait_us(us);
            self.left_us -= us;
            step = (step * 2).min(LAST_POLL_US);
        }
    }
}
