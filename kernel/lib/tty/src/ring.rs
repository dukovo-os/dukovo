// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A ring of bytes over storage the caller gives it.

/// A first-in, first-out ring of bytes in `B`'s storage, which never
/// grows: what does not fit is left to the caller.
pub struct Ring<B> {
    buf: B,
    /// Where the first byte is.
    head: usize,
    /// How many bytes it holds.
    len: usize,
}

impl<B: AsRef<[u8]> + AsMut<[u8]>> Ring<B> {
    /// An empty ring holding up to `buf`'s length.
    pub const fn new(buf: B) -> Ring<B> {
        Ring { buf, head: 0, len: 0 }
    }

    /// Its storage, to make another ring over: a new ring reads only what
    /// is written to it, so the bytes this one held there stay unread.
    pub fn into_inner(self) -> B {
        self.buf
    }

    /// How many bytes it can hold.
    pub fn capacity(&self) -> usize {
        self.buf.as_ref().len()
    }

    /// How many bytes it holds.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// How many more bytes fit.
    pub fn free(&self) -> usize {
        self.capacity() - self.len
    }

    /// `at`, an index less than one lap past the end of the storage, as
    /// every index these methods make is, brought back into it: what `%`
    /// would give, without its division, which costs some 26 cycles on
    /// the lab's Skylake, on each pipe's write and read.
    fn wrap(&self, at: usize) -> usize {
        let cap = self.capacity();
        debug_assert!(at < 2 * cap, "an index more than a lap past the ring's end");
        if at >= cap { at - cap } else { at }
    }

    /// Drops every byte.
    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    /// The byte `i` from the front.
    pub fn get(&self, i: usize) -> Option<u8> {
        if i >= self.len {
            return None;
        }
        Some(self.buf.as_ref()[self.wrap(self.head + i)])
    }

    /// Adds `byte` at the back, if it fits.
    pub fn push_byte(&mut self, byte: u8) -> bool {
        if self.free() == 0 {
            return false;
        }
        let at = self.wrap(self.head + self.len);
        self.buf.as_mut()[at] = byte;
        self.len += 1;
        true
    }

    /// Takes the byte at the back, as erasing does.
    pub fn pop_back(&mut self) -> Option<u8> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(self.buf.as_ref()[self.wrap(self.head + self.len)])
    }

    /// Adds as much of `bytes` as fits at the back; returns how much.
    pub fn push(&mut self, bytes: &[u8]) -> usize {
        let mut done = 0;
        let _ = self.write_with::<()>(bytes.len(), &mut |room| {
            room.copy_from_slice(&bytes[done..done + room.len()]);
            done += room.len();
            Ok(())
        });
        done
    }

    /// Takes as many bytes as fit in `out` from the front; returns how
    /// many.
    pub fn pop(&mut self, out: &mut [u8]) -> usize {
        let mut done = 0;
        let _ = self.read_with::<()>(out.len(), &mut |run| {
            out[done..done + run.len()].copy_from_slice(run);
            done += run.len();
            Ok(())
        });
        done
    }

    /// Takes up to `max` bytes from the front, giving `sink` each run that
    /// lies in one piece. A run `sink` fails on stays in the ring, and
    /// ends the read: the error is returned only if nothing was taken
    /// before it, as a short read is a read.
    pub fn read_with<E>(
        &mut self,
        max: usize,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<usize, E> {
        let mut done = 0;
        while done < max && self.len > 0 {
            let cap = self.capacity();
            let run = (max - done).min(self.len).min(cap - self.head);
            if let Err(err) = sink(&self.buf.as_ref()[self.head..self.head + run]) {
                return if done == 0 { Err(err) } else { Ok(done) };
            }
            self.head = self.wrap(self.head + run);
            self.len -= run;
            done += run;
        }
        if self.len == 0 {
            self.head = 0;
        }
        Ok(done)
    }

    /// Shows up to `max` bytes from the front to `sink`, a run that lies
    /// in one piece at a time, as [`Ring::read_with`] does, but leaves
    /// them in the ring: a socket's MSG_PEEK.
    pub fn peek_with<E>(
        &self,
        max: usize,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<usize, E> {
        let (cap, mut done) = (self.capacity(), 0);
        while done < max.min(self.len) {
            let head = self.wrap(self.head + done);
            let run = (max.min(self.len) - done).min(cap - head);
            if let Err(err) = sink(&self.buf.as_ref()[head..head + run]) {
                return if done == 0 { Err(err) } else { Ok(done) };
            }
            done += run;
        }
        Ok(done)
    }

    /// Adds up to `max` bytes at the back, having `source` fill each run
    /// of free room that lies in one piece. As with [`Ring::read_with`], a
    /// run `source` fails on is not added, and ends the write.
    pub fn write_with<E>(
        &mut self,
        max: usize,
        source: &mut dyn FnMut(&mut [u8]) -> Result<(), E>,
    ) -> Result<usize, E> {
        let mut done = 0;
        while done < max && self.free() > 0 {
            let cap = self.capacity();
            let tail = self.wrap(self.head + self.len);
            let run = (max - done).min(self.free()).min(cap - tail);
            if let Err(err) = source(&mut self.buf.as_mut()[tail..tail + run]) {
                return if done == 0 { Err(err) } else { Ok(done) };
            }
            self.len += run;
            done += run;
        }
        Ok(done)
    }
}
