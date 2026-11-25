// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! One value written as printf(3) writes it, for a tool whose formats are
//! printf's own, as hexdump(1)'s are: its flags, width and precision with
//! one conversion, checked before the C library sees them.

use std::io;

/// A value for [`printf`], as the C library takes it: a long long, an
/// unsigned long long, a double, a character, or a string.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value<'a> {
    /// For `d`, `i`, `o`, `u`, `x` and `X`.
    Signed(i64),
    /// For `d`, `i`, `o`, `u`, `x` and `X`.
    Unsigned(u64),
    /// For `e`, `E`, `f`, `F`, `g`, `G`, `a` and `A`.
    Float(f64),
    /// For `c`.
    Char(u8),
    /// For `s`: its bytes up to the first NUL.
    Str(&'a [u8]),
}

/// The most digits of a width or a precision: a wider one is a mistake,
/// not a format.
const DIGITS_MAX: usize = 4;

/// Whether `spec` is printf's flags, then a width, then a precision, each
/// of them optional, and nothing else: `#08.3`, `-4`, ``.
fn valid_spec(spec: &str) -> bool {
    let rest = spec.trim_start_matches(['#', '-', '+', ' ', '0']);
    let (width, precision) = match rest.split_once('.') {
        Some((width, precision)) => (width, Some(precision)),
        None => (rest, None),
    };
    let digits = |text: &str| text.len() <= DIGITS_MAX && text.bytes().all(|b| b.is_ascii_digit());
    digits(width) && precision.is_none_or(digits)
}

/// `value` written as printf(3) writes it with the conversion
/// `%{spec}{letter}`: `spec` its flags, width and precision.
///
/// # Errors
///
/// `ErrorKind::InvalidInput` for a spec of other characters or another
/// shape, a width or precision of more than four digits, or a letter that
/// is not one for the value's kind.
pub fn printf(spec: &str, letter: char, value: Value<'_>) -> io::Result<Vec<u8>> {
    let fits = match value {
        Value::Signed(_) | Value::Unsigned(_) => {
            matches!(letter, 'd' | 'i' | 'o' | 'u' | 'x' | 'X')
        }
        Value::Float(_) => matches!(letter, 'e' | 'E' | 'f' | 'F' | 'g' | 'G' | 'a' | 'A'),
        Value::Char(_) => letter == 'c',
        Value::Str(_) => letter == 's',
    };
    if !fits || !valid_spec(spec) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("%{spec}{letter} is no conversion of this value"),
        ));
    }
    let length = if matches!(value, Value::Signed(_) | Value::Unsigned(_)) { "ll" } else { "" };
    let format = crate::c_string(format!("%{spec}{length}{letter}").as_bytes())?;
    let string = match value {
        Value::Str(bytes) => {
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            crate::c_string(&bytes[..end])?
        }
        _ => crate::c_string(b"")?,
    };
    let mut buffer = vec![0u8; 64];
    loop {
        let (out, size) = (buffer.as_mut_ptr().cast::<libc::c_char>(), buffer.len());
        // SAFETY: `format` is exactly one conversion, checked above to be
        // of the type each value is passed as (`ll` for the integers),
        // with flags, width and precision of digits only, no `*` or `$`;
        // snprintf writes at most `size` bytes into `buffer`, and the
        // string is a C string.
        let n = unsafe {
            match value {
                Value::Signed(v) => {
                    libc::snprintf(out, size, format.as_ptr(), libc::c_longlong::from(v))
                }
                Value::Unsigned(v) => {
                    libc::snprintf(out, size, format.as_ptr(), libc::c_ulonglong::from(v))
                }
                Value::Float(v) => libc::snprintf(out, size, format.as_ptr(), v),
                Value::Char(c) => libc::snprintf(out, size, format.as_ptr(), libc::c_int::from(c)),
                Value::Str(_) => libc::snprintf(out, size, format.as_ptr(), string.as_ptr()),
            }
        };
        let n = usize::try_from(n).map_err(|_| io::Error::last_os_error())?;
        if n < buffer.len() {
            buffer.truncate(n);
            return Ok(buffer);
        }
        buffer.resize(n + 1, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::{Value, printf};

    #[test]
    fn each_kind_is_written_as_printf_writes_it() {
        assert_eq!(printf("08", 'x', Value::Unsigned(0xbeef)).unwrap(), b"0000beef");
        assert_eq!(printf("+", 'd', Value::Signed(-5)).unwrap(), b"-5");
        assert_eq!(printf("#", 'o', Value::Unsigned(8)).unwrap(), b"010");
        assert_eq!(printf(".2", 'f', Value::Float(1.0 / 3.0)).unwrap(), b"0.33");
        assert_eq!(printf("-3", 'c', Value::Char(b'a')).unwrap(), b"a  ");
        assert_eq!(printf(".3", 's', Value::Str(b"abcdef\0gh")).unwrap(), b"abc");
        assert_eq!(printf("", 's', Value::Str(b"ab\0cd")).unwrap(), b"ab");
        assert_eq!(
            printf("200", 'd', Value::Signed(1)).unwrap().len(),
            200,
            "beyond the first buffer"
        );
    }

    #[test]
    fn a_conversion_that_is_not_one_value_s_is_refused() {
        for (spec, letter, value) in [
            ("", 's', Value::Signed(1)),
            ("", 'n', Value::Signed(1)),
            ("", 'd', Value::Str(b"x")),
            ("*", 'd', Value::Signed(1)),
            ("1$", 'd', Value::Signed(1)),
            ("l", 'd', Value::Signed(1)),
            ("1.2.3", 'd', Value::Signed(1)),
            ("%", 'd', Value::Signed(1)),
            ("99999", 'd', Value::Signed(1)),
            ("5-", 'd', Value::Signed(1)),
        ] {
            assert!(printf(spec, letter, value).is_err(), "%{spec}{letter}");
        }
    }
}
