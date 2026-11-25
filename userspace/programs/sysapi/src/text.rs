// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! One value written as printf(3) writes it, for a tool whose formats are
//! printf's own, as hexdump(1)'s and a shell's `printf` are: its flags,
//! width and precision with one conversion, checked before the C library
//! sees them; and a number read as strtod(3) reads it.

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

/// The most digits of a width or a precision: ten, as many as an int has.
const DIGITS_MAX: usize = 10;

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
/// shape, a width or precision of more than ten digits, or a letter that
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

/// What [`read_float`] read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Float {
    /// The number; ±HUGE_VAL or 0 when it is out of range.
    pub value: f64,
    /// How many bytes of the text it took.
    pub used: usize,
    /// Whether the number is beyond what a double holds (ERANGE).
    pub out_of_range: bool,
}

/// The number at the start of `text`, as strtod(3) reads it: decimal or
/// hexadecimal, `inf` and `nan`, after any white space. The text ends at
/// a NUL byte, as a C string does.
#[must_use]
pub fn read_float(text: &[u8]) -> Float {
    let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
    let Ok(c_text) = crate::c_string(&text[..end]) else {
        return Float { value: 0.0, used: 0, out_of_range: false };
    };
    let start = c_text.as_ptr();
    let mut stop = start.cast_mut();
    // SAFETY: the text is a C string, and strtod leaves `stop` inside it;
    // errno is this thread's, cleared first so that ERANGE is strtod's.
    let (value, out_of_range, used) = unsafe {
        *libc::__errno_location() = 0;
        let value = libc::strtod(start, &raw mut stop);
        (value, *libc::__errno_location() == libc::ERANGE, stop.offset_from(start))
    };
    Float { value, used: used.unsigned_abs(), out_of_range }
}

#[cfg(test)]
mod tests {
    use super::{Float, Value, printf, read_float};

    #[test]
    fn a_float_is_read_as_strtod_reads_it() {
        assert_eq!(read_float(b"1.5"), Float { value: 1.5, used: 3, out_of_range: false });
        assert_eq!(read_float(b" 0x10z"), Float { value: 16.0, used: 5, out_of_range: false });
        assert_eq!(read_float(b"abc").used, 0);
        assert_eq!(read_float(b"2\0junk").used, 1);
        let huge = read_float(b"1e5000");
        assert!(huge.out_of_range && huge.value.is_infinite() && huge.used == 6);
    }

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
            ("99999999999", 'd', Value::Signed(1)),
            ("5-", 'd', Value::Signed(1)),
        ] {
            assert!(printf(spec, letter, value).is_err(), "%{spec}{letter}");
        }
    }
}
