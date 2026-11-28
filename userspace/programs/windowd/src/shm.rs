// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The files the programs' pixels are in (`wl_shm`, ADR 0728).
//!
//! A program draws in a memory file it shares with windowd: a pool, of
//! which a buffer is a part. Smithay maps the file for the renderer and
//! keeps the description to itself. windowd keeps one of its own beside
//! it, for the one thing the renderer cannot do: hand the file to the
//! display layer, which then shows a full-screen window's pixels from
//! where the program drew them ([`crate::screens`]).
//!
//! `wl_shm` and `wl_shm_pool` are served by Smithay's own code. windowd
//! only looks at a request first: at the file a new pool is of, and at
//! the pool a new buffer is of. A handler is not told which object a
//! request makes, only handed it to pass on, so the new object is found
//! as the one of the program's objects of its kind that is not known
//! yet.

use std::collections::HashMap;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::rc::Rc;

use smithay::reexports::wayland_server::backend::{ClientId, ObjectId};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_shm::{self, WlShm};
use smithay::reexports::wayland_server::protocol::wl_shm_pool::{self, WlShmPool};
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, Resource, delegate_dispatch,
    delegate_global_dispatch,
};
use smithay::wayland::shm::{ShmBufferUserData, ShmPoolUserData, ShmState};

use crate::shell::Shell;

/// The file of each pool and of each buffer a program has.
#[derive(Debug, Default)]
pub struct Files {
    /// `None` for a pool whose file could not be kept: it is known,
    /// and its buffers are drawn from as any other.
    pools: HashMap<ObjectId, Option<Rc<OwnedFd>>>,
    /// A buffer keeps its pool's file for as long as it is there, as it
    /// keeps the pool's pixels: a program may let go of the pool first.
    buffers: HashMap<ObjectId, Option<Rc<OwnedFd>>>,
}

impl Files {
    /// The file the pixels of `buffer` are in, if it is a buffer of
    /// shared memory.
    pub fn of(&self, buffer: &WlBuffer) -> Option<BorrowedFd<'_>> {
        self.buffers.get(&buffer.id())?.as_ref().map(|file| file.as_fd())
    }

    /// `buffer` is gone.
    pub fn forget(&mut self, buffer: &WlBuffer) {
        self.buffers.remove(&buffer.id());
    }

    /// Lets go of the files of objects that are gone with nothing said
    /// of it: those a request made that Smithay then refused, whose
    /// program it disconnected.
    fn sweep(&mut self, display: &DisplayHandle) {
        let there = |object: &ObjectId| display.object_info(object.clone()).is_ok();
        self.pools.retain(|pool, _| there(pool));
        self.buffers.retain(|buffer, _| there(buffer));
    }
}

/// The one object of `client` of the kind `interface` names for which
/// `known` is false: the one the request at hand makes.
fn made(
    display: &DisplayHandle,
    client: &Client,
    interface: &str,
    known: impl Fn(&ObjectId) -> bool,
) -> Option<ObjectId> {
    let mut new = Vec::new();
    let listed = display.backend_handle().with_all_objects_for(client.id(), |object| {
        if object.interface().name == interface && !known(&object) {
            new.push(object);
        }
    });
    // More than one is none windowd can tell apart: the buffer is then
    // drawn from as any other, and never shown from its file.
    match (listed, new.len()) {
        (Ok(()), 1) => new.pop(),
        _ => None,
    }
}

impl Dispatch<WlShm, ()> for Shell {
    fn request(
        shell: &mut Shell,
        client: &Client,
        shm: &WlShm,
        request: wl_shm::Request,
        (): &(),
        display: &DisplayHandle,
        init: &mut DataInit<'_, Shell>,
    ) {
        if let wl_shm::Request::CreatePool { fd, .. } = &request {
            let files = &mut shell.files;
            files.sweep(display);
            let pool = made(display, client, "wl_shm_pool", |pool| files.pools.contains_key(pool));
            if let Some(pool) = pool {
                files.pools.insert(pool, fd.try_clone().ok().map(Rc::new));
            }
        }
        <ShmState as Dispatch<WlShm, (), Shell>>::request(
            shell,
            client,
            shm,
            request,
            &(),
            display,
            init,
        );
    }
}

impl Dispatch<WlShmPool, ShmPoolUserData> for Shell {
    fn request(
        shell: &mut Shell,
        client: &Client,
        pool: &WlShmPool,
        request: wl_shm_pool::Request,
        data: &ShmPoolUserData,
        display: &DisplayHandle,
        init: &mut DataInit<'_, Shell>,
    ) {
        if matches!(request, wl_shm_pool::Request::CreateBuffer { .. }) {
            let files = &mut shell.files;
            let file = files.pools.get(&pool.id()).cloned().flatten();
            let known = |buffer: &ObjectId| files.buffers.contains_key(buffer);
            if let Some(buffer) = made(display, client, "wl_buffer", known) {
                files.buffers.insert(buffer, file);
            }
        }
        <ShmState as Dispatch<WlShmPool, ShmPoolUserData, Shell>>::request(
            shell, client, pool, request, data, display, init,
        );
    }

    fn destroyed(shell: &mut Shell, client: ClientId, pool: &WlShmPool, data: &ShmPoolUserData) {
        shell.files.pools.remove(&pool.id());
        <ShmState as Dispatch<WlShmPool, ShmPoolUserData, Shell>>::destroyed(
            shell, client, pool, data,
        );
    }
}

delegate_global_dispatch!(Shell: [WlShm: ()] => ShmState);
delegate_dispatch!(Shell: [WlBuffer: ShmBufferUserData] => ShmState);
