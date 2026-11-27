// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The programs that draw windows: their connections.
//!
//! windowd waits on each program's connection itself, beside the files
//! of the screens and the devices, and lets the one that wrote speak
//! (ADR 0717). So it keeps a copy of each connection's descriptor to
//! wait on, and lets go of it when the program is gone, so that a
//! program windowd ended for an error reads the end of its connection.

use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::{Client, Display};
use smithay::wayland::compositor::CompositorClientState;

use crate::shell::Shell;

/// How many programs are connected at most: one more is turned away.
/// Each takes a place of the list windowd waits on.
pub const CLIENTS_MAX: usize = 256;

/// What windowd keeps for a program.
#[derive(Debug, Default)]
pub struct Peer {
    /// Its surfaces' state between its requests.
    pub compositor: CompositorClientState,
    /// Whether it is gone: it closed its connection, or broke a
    /// protocol's rule.
    gone: AtomicBool,
}

impl ClientData for Peer {
    fn initialized(&self, _client: ClientId) {}

    fn disconnected(&self, _client: ClientId, _reason: DisconnectReason) {
        self.gone.store(true, Ordering::Release);
    }
}

/// One program's connection.
#[derive(Debug)]
struct Connection {
    /// windowd's copy of the descriptor, to wait on.
    waited_on: UnixStream,
    client: Client,
    peer: Arc<Peer>,
}

/// Every program's.
#[derive(Debug, Default)]
pub struct Clients {
    connections: Vec<Connection>,
}

impl Clients {
    /// Takes the program in that connected as `stream`.
    ///
    /// # Errors
    ///
    /// EMFILE when [`CLIENTS_MAX`] are connected or the process has no
    /// descriptor left, and what fcntl(2) fails with.
    pub fn take_in(&mut self, display: &Display<Shell>, stream: UnixStream) -> io::Result<()> {
        if self.connections.len() >= CLIENTS_MAX {
            return Err(io::Error::from_raw_os_error(libc::EMFILE));
        }
        stream.set_nonblocking(true)?;
        let waited_on = stream.try_clone()?;
        let peer = Arc::new(Peer::default());
        let client = display.handle().insert_client(stream, peer.clone())?;
        self.connections.push(Connection { waited_on, client, peer });
        Ok(())
    }

    /// What to wait on for each program, in an order that holds until
    /// one is taken in or [`Clients::let_go`] runs.
    pub fn waited_on(&self) -> impl Iterator<Item = BorrowedFd<'_>> {
        self.connections.iter().map(|connection| connection.waited_on.as_fd())
    }

    /// Lets each program speak whose place in [`Clients::waited_on`]
    /// `wrote` names.
    pub fn hear(
        &self,
        display: &mut Display<Shell>,
        shell: &mut Shell,
        wrote: impl IntoIterator<Item = usize>,
    ) {
        let backend = display.backend();
        for at in wrote {
            let Some(connection) = self.connections.get(at) else { continue };
            // What a program did wrong ends that program, and
            // [`Clients::let_go`] finds it gone.
            let _ = backend.dispatch_single_client(shell, connection.client.id());
        }
    }

    /// Lets go of the programs that are gone: their surfaces, and
    /// windowd's copies of their connections.
    pub fn let_go(&mut self, display: &mut Display<Shell>, shell: &mut Shell) {
        let backend = display.backend();
        self.connections.retain(|connection| {
            let gone = connection.peer.gone.load(Ordering::Acquire);
            if gone {
                // Its surfaces go with one more call, which finds no
                // request and tidies up.
                let _ = backend.dispatch_single_client(shell, connection.client.id());
            }
            !gone
        });
    }
}
