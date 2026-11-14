// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The read socket (spark(8)): what Spark knows of the services, for
//! anyone, as Linux's `systemctl status` answers any user.
//!
//! `/system/live/spark/read` is mode 0666, as logd's query socket is, and
//! answers `status` and `show` alone; the requests that change something
//! go to the control socket, which only root may write to. service(8)
//! reads through it, so that `service list` and `service NAME status`
//! work for everyone, and so does the web console, whose reads run as the
//! person signed in (ADR 0515).
//!
//! It keeps connections of its own, at most ROOM at once, so that a user
//! who keeps them busy keeps no one out of the control socket, whose
//! connections are root's.
//!
//! What a service's own output said is root's, as its records in the log
//! are (logd(8)): `show` leaves out the last line a service wrote and the
//! last line a failed health check wrote for anyone else.
//!
//! The `admin-actions` boot test covers this file: the web console reads
//! the services as bob, who is not root, and as alice.

use alloc::format;
use alloc::string::String;

use spark_api::show::Show;

use super::{CONNECTIONS_MAX, Supervisor, show};

/// The read socket, beneath the root.
pub(super) const SOCKET: &str = "system/live/spark/read";

/// Its mode: anyone may write to it, which connecting takes (unix(7)).
pub(super) const MODE: u16 = 0o666;

/// The most connections to the read socket served at once.
const ROOM: usize = 4;

/// The socket a connection came to, which decides what it may ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Door {
    /// The control socket, root's: every request.
    Control,
    /// certd's socket: `certificate NAME`, from certd alone.
    Certd,
    /// The read socket: `status` and `show`, from anyone.
    Read,
}

impl Door {
    /// The connections it takes at once.
    pub(super) fn room(self) -> usize {
        match self {
            Door::Control => CONNECTIONS_MAX,
            Door::Certd => 1,
            Door::Read => ROOM,
        }
    }
}

impl Supervisor {
    /// The answer to `request` at the read socket, from a process acting
    /// for `user`.
    pub(super) fn answer_read(&self, request: &str, user: u32) -> String {
        let mut words = request.split_whitespace();
        match words.next().unwrap_or("") {
            "status" => self.answer_status(),
            "show" => match self.show_document(words.next()) {
                Ok(mut document) => {
                    if user != 0 {
                        leave_out_output(&mut document);
                    }
                    show::json_line(&document)
                }
                Err(refusal) => refusal,
            },
            "" => String::from("error: an empty request\n"),
            other => format!(
                "error: the read socket answers status and show; `{other}` goes to the control \
                 socket, as root\n"
            ),
        }
    }
}

/// Leaves out of `document` what the services' output said: the last
/// line each wrote, and the last line a failed health check wrote.
fn leave_out_output(document: &mut Show) {
    for service in &mut document.services {
        service.last_line = None;
        if let Some(health) = &mut service.health {
            health.last_failure = None;
        }
    }
}
