// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Random bytes from the kernel's generator (radix-random(4)), for keys,
//! nonces and ids.

use std::io;

/// Fills `bytes` with random bytes, as getrandom(2) gives them; a signal
/// that interrupts the call does not end it.
///
/// # Errors
///
/// What getrandom fails with but EINTR, which a kernel whose generator
/// is ready does not.
#[inline]
pub fn fill(bytes: &mut [u8]) -> io::Result<()> {
    let mut got = 0;
    while got < bytes.len() {
        let rest = &mut bytes[got..];
        // SAFETY: getrandom writes at most `rest.len()` bytes into `rest`.
        let n = unsafe { libc::getrandom(rest.as_mut_ptr().cast(), rest.len(), 0) };
        match usize::try_from(n) {
            Ok(n) => got += n,
            Err(_) => {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }
    Ok(())
}

/// `N` random bytes.
///
/// # Errors
///
/// As [`fill`]'s.
pub fn bytes<const N: usize>() -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    fill(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::bytes;

    #[test]
    fn two_draws_differ() {
        assert_ne!(bytes::<32>().unwrap(), bytes::<32>().unwrap());
    }
}
