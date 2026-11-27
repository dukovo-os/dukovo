// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Reads and writes that reach memory every time. The compiler may answer
//! a plain read from the value written just before it, or drop a write
//! that nothing reads; a memory test that writes a pattern and reads it
//! back needs each access to happen, as it is written, in that order.

/// The value at `place`, read from memory.
pub fn read<T: Copy>(place: &T) -> T {
    // SAFETY: a reference is valid for reads and aligned, and a `Copy`
    // value may be duplicated bit for bit.
    unsafe { std::ptr::read_volatile(place) }
}

/// Writes `value` to `place` in memory.
pub fn write<T: Copy>(place: &mut T, value: T) {
    // SAFETY: a mutable reference is valid for writes and aligned, and the
    // old value is `Copy`, so there is nothing to drop.
    unsafe { std::ptr::write_volatile(place, value) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_reads_back_what_was_written() {
        let mut words = [0u64; 4];
        write(&mut words[2], 0x55AA_55AA_55AA_55AA);
        assert_eq!(read(&words[2]), 0x55AA_55AA_55AA_55AA);
        assert_eq!(read(&words[1]), 0);
    }
}
