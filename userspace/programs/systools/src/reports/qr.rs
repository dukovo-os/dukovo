// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! QR codes on a terminal (ADR 0532): the address of a one-time download
//! ([`super::save`]) and the failure notice's support page (login(1)), as
//! lines of half blocks, two modules to a character: `▀`, `▄`, `█` and a
//! space.
//!
//! A phone reads a code as dark modules on a light ground, and a console
//! is dark. So the light modules and the quiet zone are the blocks, which
//! the console draws in its light foreground colour, and the dark modules
//! are the console's own background, left empty. Error correction is
//! medium (M), which a phone's camera needs for a code shown on a screen.
//! A code wants a quiet zone of four modules around it; two still read, and
//! a terminal that is too narrow or too low for the code with two gets the
//! address alone.
//!
//! When a phone will not read a code, try a console font that draws the
//! half blocks without a gap between the lines, or the address as text.

use qrcodegen::{QrCode, QrCodeEcc};

/// The quiet zone a code is drawn with when the terminal is wide enough.
const QUIET_ZONE: i32 = 4;

/// The narrowest quiet zone a phone reads.
const NARROW_QUIET_ZONE: i32 = 2;

/// The QR code of `text`, with medium error correction; none if the text
/// is too long for one.
pub fn encode(text: &str) -> Option<QrCode> {
    QrCode::encode_text(text, QrCodeEcc::Medium).ok()
}

/// How many columns and rows of characters `code` takes with a quiet zone
/// of `quiet` modules: two modules to a row.
fn extent(code: &QrCode, quiet: i32) -> (usize, usize) {
    let modules = usize::try_from(code.size() + 2 * quiet).unwrap_or(usize::MAX);
    (modules, modules.div_ceil(2))
}

/// Whether `code` with a quiet zone of `quiet` modules fits a terminal that
/// has `columns` and `rows` for it.
fn fits_with(code: &QrCode, quiet: i32, columns: usize, rows: usize) -> bool {
    let (wide, high) = extent(code, quiet);
    wide <= columns && high <= rows
}

/// Whether `code` fits a terminal that has `columns` and `rows` for it,
/// with its narrowest quiet zone.
pub fn fits(code: &QrCode, columns: usize, rows: usize) -> bool {
    fits_with(code, NARROW_QUIET_ZONE, columns, rows)
}

/// The lines that draw `code` on a terminal that has `columns` and `rows`
/// for it, with the widest quiet zone that fits, if it fits at all.
pub fn lines(code: &QrCode, columns: usize, rows: usize) -> Option<Vec<String>> {
    let quiet = if fits_with(code, QUIET_ZONE, columns, rows) {
        QUIET_ZONE
    } else if fits(code, columns, rows) {
        NARROW_QUIET_ZONE
    } else {
        return None;
    };
    Some(render(code.size(), quiet, |x, y| code.get_module(x, y)))
}

/// The lines of a square of `size` modules, `dark` saying whether the one
/// at a column and row is, inside a quiet zone of `quiet` modules. The two
/// modules of a character are one above the other; a code of an odd number
/// of rows ends in the quiet zone's.
fn render(size: i32, quiet: i32, dark: impl Fn(i32, i32) -> bool) -> Vec<String> {
    let range = -quiet..size + quiet;
    range
        .clone()
        .step_by(2)
        .map(|y| {
            range
                .clone()
                .map(|x| match (dark(x, y), dark(x, y + 1)) {
                    (false, false) => '█',
                    (false, true) => '▀',
                    (true, false) => '▄',
                    (true, true) => ' ',
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_modules_to_a_character_with_the_light_ones_drawn() {
        // The top two rows of four columns: light over light, light over
        // dark, dark over light, and dark over dark.
        let lines = render(4, 0, |x, y| matches!((x, y), (1 | 3, 1) | (2 | 3, 0)));
        assert_eq!(lines, ["█▀▄ ", "████"]);
    }

    #[test]
    fn a_quiet_zone_is_light_all_round_and_an_odd_row_count_ends_in_it() {
        // One dark module in a quiet zone of one: three rows, so two lines.
        let lines = render(1, 1, |x, y| (x, y) == (0, 0));
        assert_eq!(lines, ["█▀█", "███"]);
    }

    #[test]
    fn the_finder_pattern_of_a_version_one_code_is_drawn_dark_on_light() {
        let code = encode("HELLO").unwrap();
        assert_eq!(code.size(), 21);
        let lines = render(code.size(), 0, |x, y| code.get_module(x, y));
        let corner: Vec<String> = lines[..4].iter().map(|l| l.chars().take(7).collect()).collect();
        assert_eq!(corner, [" ▄▄▄▄▄ ", " █   █ ", " █▄▄▄█ ", "▄▄▄▄▄▄▄"]);
        assert_eq!(lines.len(), 11);
    }

    #[test]
    fn the_widest_quiet_zone_that_fits_is_used() {
        let code = encode("HELLO").unwrap();
        // 21 modules: 29 columns and 15 rows with four of quiet zone, 25
        // and 13 with two.
        for (columns, rows, drawn) in [
            (80, 50, Some((29, 15))),
            (29, 15, Some((29, 15))),
            (28, 15, Some((25, 13))),
            (29, 14, Some((25, 13))),
            (25, 13, Some((25, 13))),
            (24, 13, None),
            (25, 12, None),
        ] {
            let lines = lines(&code, columns, rows);
            let shape = lines.map(|l| (l[0].chars().count(), l.len()));
            assert_eq!(shape, drawn, "{columns} columns, {rows} rows");
        }
        assert!(fits(&code, 25, 13) && !fits(&code, 24, 13) && !fits(&code, 25, 12));
    }

    #[test]
    fn a_support_page_address_makes_a_small_code() {
        let address = "https://dukovo.org/support/0.16.0/errors/NET-132.html";
        let code = encode(address).unwrap();
        assert!(code.size() <= 37, "{}", code.size());
        assert!(fits(&code, 60, 30));
        assert_eq!(encode(&"x".repeat(5000)).map(|c| c.size()), None);
    }
}
