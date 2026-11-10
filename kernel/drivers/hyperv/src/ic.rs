// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The integration services: the host's requests that the guest shut
//! down, its heartbeat, and its clock.
//!
//! Each service is a channel whose host sends requests and whose guest
//! sends each back as its answer, changed where the request asks. Every
//! message starts with a 28-byte header: the framework and message
//! versions, the message's type and length, a status, and flags, which
//! the answer sets to "transaction" and "response". The first request is
//! NEGOTIATE, which lists the framework versions and the message versions
//! the host speaks; the answer keeps the newest of each that the guest
//! speaks too. Then:
//!
//! - the heartbeat service sends HEARTBEAT with a sequence number, which
//!   the answer gives back one higher;
//! - the shutdown service sends SHUTDOWN with its flags; 0 and 1 ask to
//!   power the machine off, which the guest agrees to and then does;
//! - the time service sends TIMESYNC with the host's time in 100 ns steps
//!   since 1601, flagged SYNC once when the guest starts or resumes,
//!   which the guest takes as its clock, and SAMPLE from time to time,
//!   which it takes when its clock is more than a tenth of a second off.

/// Message types.
const NEGOTIATE: u16 = 0;
const HEARTBEAT: u16 = 1;
const SHUTDOWN: u16 = 3;
const TIMESYNC: u16 = 4;

/// The answer's statuses.
const STATUS_OK: u32 = 0;
const STATUS_FAIL: u32 = 0x8000_4005;

/// The answer's flags: transaction and response.
const FLAGS_RESPONSE: u8 = 0x01 | 0x04;

/// A time sample's flags.
const TIME_SYNC: u8 = 0x01;
const TIME_SAMPLE: u8 = 0x02;
/// 100 ns steps from 1601 to 1970.
const EPOCH_1970: u64 = 116_444_736_000_000_000;

/// The header's size.
pub const HEADER: usize = 28;
/// The size of an answer to NEGOTIATE with one version of each.
const NEGOTIATED: usize = 44;

/// A version as the services write it: major in the low half, minor in
/// the high.
const fn version(major: u16, minor: u16) -> u32 {
    major as u32 | (minor as u32) << 16
}

/// Version `v`, comparable: major in the high half.
fn order(v: u32) -> u32 {
    (v & 0xffff) << 16 | v >> 16
}

/// Which service a channel is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    /// The host's requests to shut down.
    Shutdown,
    /// The host's clock.
    TimeSync,
    /// The host's heartbeat.
    Heartbeat,
}

impl Service {
    /// The newest framework and message versions the kernel speaks for
    /// it.
    fn versions(self) -> (u32, u32) {
        match self {
            Service::Shutdown | Service::Heartbeat => (version(3, 0), version(3, 0)),
            Service::TimeSync => (version(3, 0), version(4, 0)),
        }
    }

    /// Its name, for the log.
    pub fn name(self) -> &'static str {
        match self {
            Service::Shutdown => "shutdown",
            Service::TimeSync => "time",
            Service::Heartbeat => "heartbeat",
        }
    }
}

/// What a request asks the kernel to do, besides answering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Nothing more.
    None,
    /// The versions were agreed.
    Negotiated {
        /// The framework's, as (major, minor).
        framework: (u16, u16),
        /// The messages', as (major, minor).
        message: (u16, u16),
    },
    /// No version was agreed, and the service is not used.
    NoVersion,
    /// Power the machine off, once the answer is sent.
    PowerOff,
    /// The host's clock.
    Time {
        /// Nanoseconds since 1970.
        unix_ns: u64,
        /// Whether it is a SYNC, to take as it is, or a SAMPLE, to take
        /// only when the clock is well off.
        sync: bool,
    },
}

/// A service's state: the message version agreed.
pub struct Ic {
    service: Service,
    message_version: u32,
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    data.get(at..at + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    data.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(data: &[u8], at: usize) -> u64 {
    let mut bytes = [0u8; 8];
    if let Some(b) = data.get(at..at + 8) {
        bytes.copy_from_slice(b);
    }
    u64::from_le_bytes(bytes)
}

impl Ic {
    /// The state of a channel of `service`, before NEGOTIATE.
    pub fn new(service: Service) -> Ic {
        Ic { service, message_version: 0 }
    }

    /// Which service it is.
    pub fn service(&self) -> Service {
        self.service
    }

    /// Takes the request in the first `len` bytes of `message` and turns
    /// it into its answer, in place. Returns the answer's length and what
    /// the request asks; `None` for a request too short to answer, which
    /// is dropped. The answer to NEGOTIATE lists one version of each.
    /// `message` must hold 44 bytes or more.
    pub fn handle(&mut self, message: &mut [u8], len: usize) -> Option<(usize, Action)> {
        if len < HEADER || message.len() < NEGOTIATED.max(len) {
            return None;
        }
        let mut len = len;
        let mut status = STATUS_OK;
        let action = match u16_at(message, 12) {
            NEGOTIATE => match self.negotiate(message, len) {
                Some((framework, msg)) => {
                    self.message_version = msg;
                    len = NEGOTIATED;
                    Action::Negotiated {
                        framework: (framework as u16, (framework >> 16) as u16),
                        message: (msg as u16, (msg >> 16) as u16),
                    }
                }
                None => {
                    status = STATUS_FAIL;
                    Action::NoVersion
                }
            },
            HEARTBEAT if self.service == Service::Heartbeat && len >= HEADER + 8 => {
                let sequence = u64_at(message, HEADER).wrapping_add(1);
                message[HEADER..HEADER + 8].copy_from_slice(&sequence.to_le_bytes());
                Action::None
            }
            SHUTDOWN if self.service == Service::Shutdown && len >= HEADER + 12 => {
                match u32_at(message, HEADER + 8) {
                    0 | 1 => Action::PowerOff,
                    _ => {
                        status = STATUS_FAIL;
                        Action::None
                    }
                }
            }
            TIMESYNC if self.service == Service::TimeSync => {
                // Version 4 has the host's time, its reference count when
                // it sent it, and the flags; earlier versions the host's
                // time, the guest's, the round trip, and the flags.
                let flags_at = if order(self.message_version) >= order(version(4, 0)) {
                    HEADER + 16
                } else {
                    HEADER + 24
                };
                if len <= flags_at {
                    return None;
                }
                let host = u64_at(message, HEADER);
                let flags = message[flags_at];
                let unix_ns = host.saturating_sub(EPOCH_1970).saturating_mul(100);
                if flags & TIME_SYNC != 0 {
                    Action::Time { unix_ns, sync: true }
                } else if flags & TIME_SAMPLE != 0 {
                    Action::Time { unix_ns, sync: false }
                } else {
                    Action::None
                }
            }
            _ => Action::None,
        };
        message[20..24].copy_from_slice(&status.to_le_bytes());
        message[25] = FLAGS_RESPONSE;
        Some((len, action))
    }

    /// Picks the newest framework and message versions the host lists in
    /// a NEGOTIATE of `len` bytes in `message` that the kernel speaks,
    /// and writes them as the answer's only two.
    fn negotiate(&self, message: &mut [u8], len: usize) -> Option<(u32, u32)> {
        let frameworks = usize::from(u16_at(message, HEADER));
        let messages = usize::from(u16_at(message, HEADER + 2));
        let listed = HEADER + 8;
        if frameworks == 0 || messages == 0 || len < listed + (frameworks + messages) * 4 {
            return None;
        }
        let (most_framework, most_message) = self.service.versions();
        let best = |from: usize, count: usize, most: u32| {
            (from..from + count)
                .map(|i| u32_at(message, listed + i * 4))
                .filter(|&v| order(v) <= order(most))
                .max_by_key(|&v| order(v))
        };
        let framework = best(0, frameworks, most_framework)?;
        let msg = best(frameworks, messages, most_message)?;
        message[HEADER..HEADER + 2].copy_from_slice(&1u16.to_le_bytes());
        message[HEADER + 2..HEADER + 4].copy_from_slice(&1u16.to_le_bytes());
        message[listed..listed + 4].copy_from_slice(&framework.to_le_bytes());
        message[listed + 4..listed + 8].copy_from_slice(&msg.to_le_bytes());
        // The data's size, after the header.
        message[18..20].copy_from_slice(&((NEGOTIATED - HEADER) as u16).to_le_bytes());
        Some((framework, msg))
    }
}
