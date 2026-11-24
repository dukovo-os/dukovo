// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The handles programs have open, and what each reads.
//!
//! A file of `/proc` or `/sys` is read from a snapshot made when the
//! handle is opened, as Linux's seq files are, so that a program that
//! reads a file in several calls reads one text. The kernel's FUSE keeps
//! one handle for each node it has open, which every `open(2)` of the
//! node shares, so a read of offset 0 after the first stands for a new
//! `open(2)` and makes the snapshot again.
//!
//! A handle's number holds a token that differs between one linuxd and
//! the next. A handle of an earlier linuxd, which the kernel still has
//! after a restart, is no handle of this one's: it is not found, and
//! its first read makes its snapshot.

use std::collections::HashMap;

use super::node::Node;

/// The bits a handle's count has, below its token's.
const COUNT_BITS: u32 = 32;

/// A handle a program has open.
struct Handle {
    node: Node,
    /// What reads give, once it is made; none for a socket's file.
    snapshot: Option<Vec<u8>>,
    /// Whether the snapshot was made at the open and no read has used
    /// it yet.
    fresh: bool,
}

/// The handles open.
pub struct Handles {
    open: HashMap<u64, Handle>,
    /// The handles open on each socket's file, so that the last one's
    /// release is the socket's.
    sockets: HashMap<u64, u32>,
    token: u64,
    /// The count of the next handle: the handle numbers wrap after 2^32
    /// opens, by when the first are long released.
    next: u64,
}

impl Handles {
    /// No handles yet; every one this gives has `token` in its number.
    pub fn new(token: u32) -> Handles {
        Handles {
            open: HashMap::new(),
            sockets: HashMap::new(),
            token: u64::from(token) << COUNT_BITS,
            next: 1,
        }
    }

    /// A new handle on `node`, with `snapshot` for a read to use (none
    /// for a socket's file); its number.
    pub fn open(&mut self, node: Node, snapshot: Option<Vec<u8>>) -> u64 {
        let number = self.token | self.next;
        self.next = (self.next + 1) & ((1 << COUNT_BITS) - 1);
        if let Node::Socket(id) = node {
            *self.sockets.entry(id).or_insert(0) += 1;
        }
        self.open.insert(number, Handle { node, snapshot, fresh: true });
        number
    }

    /// The bytes of handle `number`'s snapshot of `node`, which `make`
    /// makes if the handle has none (an earlier linuxd opened it), or if
    /// the read starts again from `offset` 0 after the first.
    pub fn snapshot(
        &mut self,
        number: u64,
        node: Node,
        offset: u64,
        make: impl FnOnce() -> Result<Vec<u8>, i32>,
    ) -> Result<&[u8], i32> {
        let handle =
            self.open.entry(number).or_insert(Handle { node, snapshot: None, fresh: false });
        if handle.snapshot.is_none() || (offset == 0 && !handle.fresh) {
            handle.snapshot = Some(make()?);
        }
        handle.fresh = false;
        Ok(handle.snapshot.as_deref().unwrap_or_default())
    }

    /// Releases handle `number`. If it was the last on a socket's file,
    /// the socket's number: the socket is for the caller to close.
    pub fn release(&mut self, number: u64) -> Option<u64> {
        let Handle { node: Node::Socket(id), .. } = self.open.remove(&number)? else { return None };
        let open = self.sockets.get_mut(&id)?;
        *open -= 1;
        if *open > 0 {
            return None;
        }
        self.sockets.remove(&id);
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::super::node::Fixed;
    use super::*;

    const FILE: Node = Node::Fixed(Fixed::ProcMeminfo);

    /// A maker that counts its calls and gives the text of the call.
    fn making(calls: &Cell<u32>) -> impl FnOnce() -> Result<Vec<u8>, i32> + '_ {
        move || {
            calls.set(calls.get() + 1);
            Ok(format!("text {}\n", calls.get()).into_bytes())
        }
    }

    #[test]
    fn a_read_after_the_open_uses_the_snapshot_of_the_open() {
        let mut handles = Handles::new(7);
        let handle = handles.open(FILE, Some(b"opened\n".to_vec()));
        let calls = Cell::new(0);
        assert_eq!(handles.snapshot(handle, FILE, 0, making(&calls)), Ok(&b"opened\n"[..]));
        assert_eq!(handles.snapshot(handle, FILE, 3, making(&calls)), Ok(&b"opened\n"[..]));
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn a_read_from_the_start_again_is_a_new_open() {
        let mut handles = Handles::new(7);
        let handle = handles.open(FILE, Some(b"opened\n".to_vec()));
        let calls = Cell::new(0);
        handles.snapshot(handle, FILE, 0, making(&calls)).unwrap();
        assert_eq!(handles.snapshot(handle, FILE, 0, making(&calls)), Ok(&b"text 1\n"[..]));
        // The reads that go on from there keep reading that text.
        assert_eq!(handles.snapshot(handle, FILE, 7, making(&calls)), Ok(&b"text 1\n"[..]));
        assert_eq!(calls.get(), 1);
        assert_eq!(handles.snapshot(handle, FILE, 0, making(&calls)), Ok(&b"text 2\n"[..]));
    }

    #[test]
    fn a_handle_of_an_earlier_linuxd_is_read_from_a_snapshot_made_then() {
        let mut handles = Handles::new(7);
        let earlier = (6_u64 << COUNT_BITS) | 5;
        let calls = Cell::new(0);
        assert_eq!(handles.snapshot(earlier, FILE, 100, making(&calls)), Ok(&b"text 1\n"[..]));
        assert_eq!(handles.snapshot(earlier, FILE, 100, making(&calls)), Ok(&b"text 1\n"[..]));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn a_snapshot_that_fails_is_the_error() {
        let mut handles = Handles::new(7);
        let handle = handles.open(FILE, None);
        assert_eq!(handles.snapshot(handle, FILE, 0, || Err(libc::ENOENT)), Err(libc::ENOENT));
    }

    #[test]
    fn the_numbers_hold_the_token_and_are_not_reused() {
        let mut handles = Handles::new(7);
        let first = handles.open(FILE, None);
        let second = handles.open(FILE, None);
        assert_eq!((first >> COUNT_BITS, second >> COUNT_BITS), (7, 7));
        assert_ne!(first, second);
        assert_ne!(Handles::new(8).open(FILE, None), first);
    }

    #[test]
    fn the_last_release_of_a_socket_is_the_sockets() {
        let mut handles = Handles::new(7);
        let socket = Node::Socket(12);
        let (first, second) = (handles.open(socket, None), handles.open(socket, None));
        let other = handles.open(Node::Socket(13), None);
        assert_eq!(handles.release(first), None);
        assert_eq!(handles.release(first), None);
        assert_eq!(handles.release(second), Some(12));
        assert_eq!(handles.release(other), Some(13));
        // Opened again, it is a new socket's file with one handle.
        let again = handles.open(socket, None);
        assert_eq!(handles.release(again), Some(12));
    }

    #[test]
    fn releasing_a_file_or_an_unknown_handle_closes_nothing() {
        let mut handles = Handles::new(7);
        let file = handles.open(FILE, None);
        assert_eq!(handles.release(file), None);
        assert_eq!(handles.release(99), None);
    }
}
