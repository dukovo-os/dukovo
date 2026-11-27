// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What an input device is, from what it has.
//!
//! A device says which keys, moving axes, positions and switches it has
//! (radix-input(2)'s INPUT_BITS), and nothing of what it is for. The
//! rules here turn the one into the other, in one place, so that
//! inputd(8), `hardware inputs` and every program that reads a device
//! call it the same:
//!
//! - a **keyboard** has the keys of all 26 letters;
//! - **keys** are keys without the letters: a volume wheel, a row of
//!   media keys, a power button, a number pad;
//! - a **pointer** moves both ways and has a left button: a mouse, a
//!   trackball, and a touchpad that reports as a mouse;
//! - a **tablet** says where it points both ways, and has a pen, or a
//!   left button and no finger: a drawing tablet, and the pointer of a
//!   virtual machine;
//! - a **touch** device says where a finger is, and has no pen: a touch
//!   screen, or a touchpad that reports its fingers;
//! - a **switch** has a switch, as the lid of a laptop.
//!
//! A device can be several of them, and one that is none is **other**.

use core::fmt;

use radix_abi::input::{
    ABS_X, ABS_Y, BTN_LEFT, BTN_TOOL_PEN, BTN_TOUCH, KEY_A, KEY_B, KEY_C, KEY_D, KEY_E, KEY_F,
    KEY_G, KEY_H, KEY_I, KEY_J, KEY_K, KEY_L, KEY_M, KEY_N, KEY_O, KEY_P, KEY_Q, KEY_R, KEY_S,
    KEY_T, KEY_U, KEY_V, KEY_W, KEY_X, KEY_Y, KEY_Z, REL_X, REL_Y,
};

/// The keys of the letters.
const LETTERS: [u16; 26] = [
    KEY_A, KEY_B, KEY_C, KEY_D, KEY_E, KEY_F, KEY_G, KEY_H, KEY_I, KEY_J, KEY_K, KEY_L, KEY_M,
    KEY_N, KEY_O, KEY_P, KEY_Q, KEY_R, KEY_S, KEY_T, KEY_U, KEY_V, KEY_W, KEY_X, KEY_Y, KEY_Z,
];

/// Linux's codes from here up to [`BUTTONS_END`] are buttons, a mouse's,
/// a game controller's and a pen's among them; the codes on either side
/// are keys.
const BUTTONS: u16 = 0x100;
const BUTTONS_END: u16 = 0x160;

/// Whether `bits`, a bit for each code as INPUT_BITS answers, has
/// `code`: the code's eighth is the byte, and the rest the bit in it.
pub fn has(bits: &[u8], code: u16) -> bool {
    bits.get(usize::from(code / 8)).is_some_and(|byte| byte & (1 << (code % 8)) != 0)
}

/// The codes `bits` has, lowest first.
pub fn codes(bits: &[u8]) -> impl Iterator<Item = u16> + '_ {
    let all = 0..u16::try_from(bits.len() * 8).unwrap_or(u16::MAX);
    all.filter(|&code| has(bits, code))
}

/// What a device has: INPUT_BITS's answer for each kind of code, however
/// many of its bytes there are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Has {
    /// Its keys and buttons (EV_KEY).
    pub keys: Vec<u8>,
    /// The axes it moves along (EV_REL).
    pub moves: Vec<u8>,
    /// The axes it says its place on (EV_ABS).
    pub places: Vec<u8>,
    /// Its switches (EV_SW).
    pub switches: Vec<u8>,
}

impl Has {
    /// What a device that has these is.
    pub fn kinds(&self) -> Kinds {
        let key = |code| has(&self.keys, code);
        let places = has(&self.places, ABS_X) && has(&self.places, ABS_Y);
        let (pen, finger) = (key(BTN_TOOL_PEN), key(BTN_TOUCH));
        let keyboard = LETTERS.iter().all(|&letter| key(letter));
        let other_keys = codes(&self.keys).any(|code| !(BUTTONS..BUTTONS_END).contains(&code));
        let is = [
            (keyboard, Kinds::KEYBOARD),
            (other_keys && !keyboard, Kinds::KEYS),
            (has(&self.moves, REL_X) && has(&self.moves, REL_Y) && key(BTN_LEFT), Kinds::POINTER),
            (places && (pen || (key(BTN_LEFT) && !finger)), Kinds::TABLET),
            (places && finger && !pen, Kinds::TOUCH),
            (self.switches.iter().any(|&byte| byte != 0), Kinds::SWITCH),
        ];
        let bits = is.iter().filter(|(is, _)| *is).fold(0, |bits, (_, kind)| bits | kind.0);
        Kinds(bits)
    }
}

/// What a device is: any of a keyboard, keys, a pointer, a tablet, a
/// touch device and a switch, or none of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Kinds(u32);

impl Kinds {
    /// None of the kinds: a device whose use nothing here knows, as a
    /// game controller until it has a kind of its own.
    pub const OTHER: Kinds = Kinds(0);
    /// The keys of the 26 letters.
    pub const KEYBOARD: Kinds = Kinds(1);
    /// Keys, without the letters.
    pub const KEYS: Kinds = Kinds(2);
    /// Moves both ways, with a left button.
    pub const POINTER: Kinds = Kinds(4);
    /// Says where it points, with a pen, or a left button and no finger.
    pub const TABLET: Kinds = Kinds(8);
    /// Says where a finger is, with no pen.
    pub const TOUCH: Kinds = Kinds(16);
    /// Has a switch.
    pub const SWITCH: Kinds = Kinds(32);

    /// Each kind and its name, in the order they are named in.
    const NAMES: [(Kinds, &'static str); 6] = [
        (Kinds::KEYBOARD, "keyboard"),
        (Kinds::KEYS, "keys"),
        (Kinds::POINTER, "pointer"),
        (Kinds::TABLET, "tablet"),
        (Kinds::TOUCH, "touch"),
        (Kinds::SWITCH, "switch"),
    ];

    /// The kinds as a number, a bit for each, as inputd(8) sends them.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// The kinds `bits` names. A bit that names none, as one a later
    /// release added, is left out.
    pub fn from_bits(bits: u32) -> Kinds {
        let known = Kinds::NAMES.iter().fold(0, |known, (kind, _)| known | kind.0);
        Kinds(bits & known)
    }

    /// Whether the device is `kind`, or every one of them if `kind` is
    /// several.
    pub const fn is(self, kind: Kinds) -> bool {
        self.0 & kind.0 == kind.0 && kind.0 != 0
    }

    /// The names of the kinds the device is: `other` alone if it is
    /// none.
    pub fn names(self) -> impl Iterator<Item = &'static str> {
        let named = Kinds::NAMES.into_iter().filter(move |(kind, _)| self.is(*kind));
        let other = (self.0 == 0).then_some("other");
        named.map(|(_, name)| name).chain(other)
    }
}

impl fmt::Display for Kinds {
    /// The names, with a comma between: `keyboard`, `keys, pointer`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (at, name) in self.names().enumerate() {
            if at > 0 {
                f.write_str(", ")?;
            }
            f.write_str(name)?;
        }
        Ok(())
    }
}
