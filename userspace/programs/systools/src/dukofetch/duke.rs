// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Duke, Dukovo's mascot, as dukofetch(1) draws him (ADR 0363): the
//! brand kit's colour mark, scaled to two sizes of terminal cells, with
//! each pixel a letter of [`COLOURS`].

/// The colours of Duke's pixels, by the letter that stands for each in
/// the pictures below: the brand's steel blues, Duke's tan, and the
/// shades between them at his edges.
pub const COLOURS: &[(u8, u32)] = &[
    (b'a', 0x2c5074),
    (b'b', 0x345471),
    (b'c', 0x3c566e),
    (b'd', 0x44596b),
    (b'e', 0x4d5c68),
    (b'f', 0x495f72),
    (b'g', 0x576064),
    (b'h', 0x5d6262),
    (b'i', 0x66655e),
    (b'j', 0x4d6579),
    (b'k', 0x6b675c),
    (b'l', 0x73695a),
    (b'm', 0x7c6d56),
    (b'n', 0x897151),
    (b'o', 0x8f734f),
    (b'p', 0x96764c),
    (b'q', 0x9c784a),
    (b'r', 0xa17948),
    (b's', 0xab7d45),
    (b't', 0xb07f43),
    (b'u', 0xb78140),
    (b'v', 0xbf843d),
    (b'w', 0xc5863b),
    (b'x', 0xca8839),
    (b'y', 0xd08a37),
    (b'z', 0xd78c35),
];
/// Duke 16 rows high, for a terminal 80 columns wide or more: 32 rows
/// of 24 pixels, two rows to a line of the terminal, a space where the
/// terminal's own background shows through.
pub const DUKE_16: [&str; 32] = [
    "                        ",
    "                        ",
    "   d         d          ",
    "   jd        j          ",
    "   ff       djd         ",
    "  dfdd    dddfd         ",
    "  dfdfd d dfddd         ",
    "  dfddddddddddd         ",
    "  ddddddddddddf         ",
    " dfdddddddddddfd        ",
    " dfddddddddddddfdd      ",
    " dfddddddddddddddfd     ",
    " dfdddccccccccddddfd    ",
    " dfddepqqqqpnkdcdddfd   ",
    " dfddewyyyyyyyukcdddf   ",
    " dfddevxwwwyyxxymcdddd  ",
    " dfddevxxxikqvyxxhcddf  ",
    " dfddevxwxsmhgluzpcddf  ",
    " dfddevxwxxlmmiuyudddfd ",
    " dfddevxwwylabkxxveddfd ",
    " dfddevxwxxwmkuyyveddfd ",
    " dfddevxwxwxyyynhldddfd ",
    " dfddevxwxxwwxweccdddf  ",
    " dfddevxwwwxwtwqdddddd  ",
    " dfddevxxxxxxomgddddfd  ",
    " dfddeuvvvvvtrkdddddd   ",
    " dfdddeeeeeedccddddf    ",
    " dfdddddddddddddddf     ",
    " dfdddddddddddddfd      ",
    " djfffffffffffdd        ",
    "  fdddddddddd           ",
    "                        ",
];
/// Duke 10 rows high, for a narrower terminal: 20 rows of 15 pixels.
pub const DUKE_10: [&str; 20] = [
    "               ",
    "               ",
    "  f     d      ",
    "  jd   ff      ",
    " dfddddff      ",
    " dddddddfd     ",
    " dddddddddd    ",
    " dddcccccdfd   ",
    " dchrqqpmecdd  ",
    " dcizyxzzvgcfd ",
    " dcixxomqxtddd ",
    " dcixxvigpyhcd ",
    " dcixwxlgvzicd ",
    " dciywxxyqiedd ",
    " dcixwwwvocdfd ",
    " dcizxxxqhddf  ",
    " ddeiiihgddfd  ",
    " dddccccdffd   ",
    " ddddddddd     ",
    "               ",
];
