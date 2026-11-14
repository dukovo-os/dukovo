// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A try of `changes bisect` (ADR 0475): one start of a slot with given
//! boot options, which the boot menu starts once in place of the start
//! the slot state names.
//!
//! The try is a GRUB environment block ([`crate::envblock`]) of its own,
//! `boot/grub/trial` on each EFI system partition, beside the slot state,
//! and never a change to the slot state: its number, its slot, the
//! options after the menu's own words, a check of those three, and
//! whether the menu has taken it. The menu's Dukovo entry takes a try
//! when the block is whole, no update is being tried, its slot is the
//! default or the previous version, and it is not yet taken: it writes
//! `trial_taken` as the try's number, reads the block back, and starts
//! the try only if it reads back taken. So a try starts at most once, and
//! a hang, a panic or a power cut during it leaves the next start the
//! usual one. The menu marks the start as it marks one chosen by hand
//! ([`crate::State::chose`]), and changes nothing else in the slot state.
//!
//! The menu starts the try's slot with `trial=N` first on the kernel's
//! command line ([`crate::cmdline`]), then its own words (the root, the
//! slot and `panic=10`), then the try's options in place of the slot's
//! own: the console, the slot's flags and the machine's. A block torn by
//! a power cut, or one the system cannot read, is no try. GRUB's side is
//! `trial_read` and `trial_take` in `image/grub/menu.cfg`, and
//! `tla/SlotState.tla` models the try with the slot state.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::envblock::{self, BLOCK};
use crate::state::Slot;

/// Where the block is on each EFI system partition, beside the slot
/// state.
pub const PATH: &str = "boot/grub/trial";

/// The block's variables, in the order it holds them. The menu writes
/// only the last, so a write of it cut short spoils no other.
pub const VARIABLES: [&str; 5] =
    ["trial_number", "trial_slot", "trial_options", "trial_check", "trial_taken"];

/// The highest number a try can have: a search of up to 64 changes takes
/// at most 6 tries, each tried at most twice, with the confirming one.
pub const MAX_NUMBER: u8 = 99;

/// The most bytes of options a try can have: the block holds them twice,
/// in `trial_options` and in its check.
pub const MAX_OPTIONS: usize = 400;

/// The words the menu writes itself before the try's options, which the
/// options never hold: a second would name another root or slot, another
/// try, or a fallback, whose settings Spark would put back. A second
/// `panic=` is the machine's own, and the kernel takes the last.
const MENU_WORDS: [&str; 4] = ["root=", "slot=", "trial=", "fallback="];

/// A try: a start of `slot` with `options`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trial {
    /// Its number, from 1 to [`MAX_NUMBER`]: the menu takes a try whose
    /// number it has not taken.
    pub number: u8,
    /// The slot it starts.
    pub slot: Slot,
    /// The words after the menu's own on the kernel's command line, in
    /// place of the slot's own: the console, the slot's flags and the
    /// machine's, as a start's record holds them ([`options_of`]).
    pub options: String,
}

/// Why a try cannot be written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Its number is 0 or above [`MAX_NUMBER`].
    Number(u8),
    /// Its options are longer than [`MAX_OPTIONS`] bytes.
    TooLong(usize),
    /// A word of its options holds a character the menu could take for
    /// part of its own script.
    Character(String),
    /// A word of its options is one the menu writes itself.
    MenuWord(String),
}

impl core::fmt::Display for Refused {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Refused::Number(number) => {
                write!(f, "a try is numbered 1 to {MAX_NUMBER}, not {number}")
            }
            Refused::TooLong(length) => write!(
                f,
                "the boot options are {length} bytes, and a try holds at most {MAX_OPTIONS}"
            ),
            Refused::Character(word) => write!(
                f,
                "the boot option `{word}` holds a character the boot menu cannot pass on as it is"
            ),
            Refused::MenuWord(word) => {
                write!(f, "the boot option `{word}` is one the boot menu writes itself")
            }
        }
    }
}

/// Whether the menu passes `byte` on as it is in an unquoted word: the
/// characters of kernel flags, with nothing GRUB's script reads.
const fn plain(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(byte, b'-' | b'_' | b'.' | b',' | b':' | b'=' | b'/' | b'+' | b'@' | b'%')
}

impl Trial {
    /// A try of `slot` numbered `number` with `options`, whose words are
    /// kept as they are, one space apart.
    ///
    /// # Errors
    ///
    /// If the number is not one a try can have, or the options do not fit
    /// the block or hold a word the menu could not pass on as it is.
    pub fn new(number: u8, slot: Slot, options: &str) -> Result<Trial, Refused> {
        if number == 0 || number > MAX_NUMBER {
            return Err(Refused::Number(number));
        }
        let words: Vec<&str> = options.split_ascii_whitespace().collect();
        for word in &words {
            if !word.bytes().all(plain) {
                return Err(Refused::Character(String::from(*word)));
            }
            if MENU_WORDS.iter().any(|menu| word.starts_with(menu)) {
                return Err(Refused::MenuWord(String::from(*word)));
            }
        }
        let options = words.join(" ");
        if options.len() > MAX_OPTIONS {
            return Err(Refused::TooLong(options.len()));
        }
        Ok(Trial { number, slot, options })
    }

    /// The block's check: the try's fields again, in one value, which a
    /// block a write left half done does not match.
    #[must_use]
    pub fn check(&self) -> String {
        format!("{}:{}:{}", self.number, self.slot.letter(), self.options)
    }

    /// The block of this try, not yet taken.
    ///
    /// # Panics
    ///
    /// Never: [`Trial::new`] keeps the options short enough to fit.
    #[must_use]
    pub fn encode(&self) -> [u8; BLOCK] {
        let number = format!("{}", self.number);
        let check = self.check();
        envblock::write(
            &["# A try of changes bisect (ADR 0475): written by it and by GRUB, not by hand."],
            &[
                (VARIABLES[0], &number),
                (VARIABLES[1], self.slot.letter()),
                (VARIABLES[2], &self.options),
                (VARIABLES[3], &check),
                (VARIABLES[4], ""),
            ],
        )
        .expect("a try fits a block")
    }

    /// The try a block holds, if it is whole, and whether the menu has
    /// taken it, as the menu's `trial_read` decides: GRUB's signature, a
    /// number and a slot the try can have, and the check matching them.
    #[must_use]
    pub fn decode(block: &[u8]) -> Option<(Trial, bool)> {
        let variables = envblock::parse(block)?;
        let field = |name| envblock::get(&variables, name);
        let number_text = field(VARIABLES[0])?;
        let number = number_text
            .bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| number_text.parse::<u8>().ok())
            .flatten()
            .filter(|&n| n != 0 && n <= MAX_NUMBER)?;
        let slot = Slot::named(field(VARIABLES[1])?)?;
        let trial = Trial { number, slot, options: String::from(field(VARIABLES[2])?) };
        // The check holds the number as the system writes it, with no
        // leading 0, which the menu compares as text.
        if field(VARIABLES[3])? != trial.check() {
            return None;
        }
        let taken = field(VARIABLES[4]) == Some(number_text);
        Some((trial, taken))
    }
}

/// The block with no try in it, as `changes bisect` leaves it when a
/// search ends or stops: no number, so the menu reads no try.
///
/// # Panics
///
/// Never: empty variables fit a block.
#[must_use]
pub fn cleared() -> [u8; BLOCK] {
    envblock::write(
        &["# No try of changes bisect waits (ADR 0475): written by it and by GRUB, not by hand."],
        &VARIABLES.map(|name| (name, "")),
    )
    .expect("an empty block fits")
}

/// The options a start's record holds (`options`, the kernel's command
/// line less `slot=` and `fallback=`), as a try of that start passes them:
/// the words after the menu's own, `root=` and the `panic=` after it, so
/// that a word an entry put first, such as `essentials`, is left out as
/// well. A line without them, from a start GRUB's menu did not make, keeps
/// every word but those the menu writes.
#[must_use]
pub fn options_of(recorded: &str) -> String {
    let words: Vec<&str> = recorded.split_ascii_whitespace().collect();
    let after = words.iter().position(|word| word.starts_with("root=")).map(|root| {
        match words.get(root + 1) {
            Some(word) if word.starts_with("panic=") => root + 2,
            _ => root + 1,
        }
    });
    words[after.unwrap_or(0)..]
        .iter()
        .filter(|word| !MENU_WORDS.iter().any(|menu| word.starts_with(menu)))
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trial(options: &str) -> Trial {
        Trial::new(3, Slot::B, options).unwrap()
    }

    #[test]
    fn a_try_reads_back_whole_and_not_taken() {
        let block = trial("quiet console=ttyS0,115200n8 loglevel=info").encode();
        assert_eq!(block.len(), BLOCK);
        let (read, taken) = Trial::decode(&block).unwrap();
        assert_eq!(read, trial("quiet  console=ttyS0,115200n8 loglevel=info"));
        assert_eq!(read.check(), "3:b:quiet console=ttyS0,115200n8 loglevel=info");
        assert!(!taken);
        // Options of no words: a start with none of the slot's own.
        assert_eq!(Trial::decode(&trial("").encode()).unwrap().0.options, "");
    }

    /// The block as the menu's `save_env` leaves it once it has set
    /// `trial_taken` to `taken`: the value put in its place, the rest
    /// moved along over the padding.
    fn taken_as(trial: &Trial, taken: &str) -> [u8; BLOCK] {
        let block = trial.encode();
        let text = core::str::from_utf8(&block).unwrap();
        let at = text.find("\ntrial_taken=\n").unwrap() + "\ntrial_taken=".len();
        let mut out = [b'#'; BLOCK];
        out[..at].copy_from_slice(&block[..at]);
        out[at..at + taken.len()].copy_from_slice(taken.as_bytes());
        out[at + taken.len()..].copy_from_slice(&block[at..BLOCK - taken.len()]);
        out
    }

    #[test]
    fn a_cleared_block_holds_no_try() {
        let block = cleared();
        assert_eq!(block.len(), BLOCK);
        assert_eq!(Trial::decode(&block), None);
        let variables = envblock::parse(&block).unwrap();
        for name in VARIABLES {
            assert_eq!(envblock::get(&variables, name), Some(""), "{name}");
        }
    }

    #[test]
    fn a_try_is_taken_only_by_its_own_number() {
        let try3 = trial("quiet");
        assert!(Trial::decode(&taken_as(&try3, "3")).unwrap().1);
        assert!(!Trial::decode(&taken_as(&try3, "2")).unwrap().1);
        assert!(!Trial::decode(&taken_as(&try3, "03")).unwrap().1);
        assert!(!Trial::decode(&taken_as(&try3, "")).unwrap().1);
    }

    #[test]
    fn a_block_cut_short_or_changed_is_no_try() {
        let block = trial("quiet console=ttyS0").encode();
        // Cut at every byte: the start of the block written and the rest
        // as the partition held it, GRUB's padding or zeros.
        for cut in 0..BLOCK {
            for rest in [b'#', 0] {
                let mut torn = [rest; BLOCK];
                torn[..cut].copy_from_slice(&block[..cut]);
                if let Some((read, taken)) = Trial::decode(&torn) {
                    assert_eq!(read, trial("quiet console=ttyS0"), "cut at {cut}");
                    assert!(!taken);
                }
            }
        }
        // The fields changed without the check.
        let text = core::str::from_utf8(&block).unwrap();
        for (from, to) in [("trial_slot=b", "trial_slot=a"), ("trial_number=3", "trial_number=4")] {
            let changed = text.replacen(from, to, 1);
            assert!(Trial::decode(changed.as_bytes()).is_none(), "{to}");
        }
        assert!(Trial::decode(b"not a block").is_none());
        assert!(Trial::decode(&envblock::write(&[], &[]).unwrap()).is_none());
        // A number with a leading 0, or 0, which the system never writes.
        for (number, check) in [("03", "03:b:"), ("0", "0:b:")] {
            let odd = envblock::write(
                &[],
                &[
                    (VARIABLES[0], number),
                    (VARIABLES[1], "b"),
                    (VARIABLES[2], ""),
                    (VARIABLES[3], check),
                    (VARIABLES[4], ""),
                ],
            )
            .unwrap();
            assert!(Trial::decode(&odd).is_none(), "{number}");
        }
    }

    #[test]
    fn the_menus_take_cut_short_leaves_the_same_try() {
        // GRUB writes the block's two sectors over the partition's; a cut
        // between them leaves one sector of each write.
        for options in ["", "quiet", &"x".repeat(200), &"y".repeat(MAX_OPTIONS)] {
            let try_it = Trial::new(MAX_NUMBER, Slot::A, options).unwrap();
            let before = try_it.encode();
            let after = taken_as(&try_it, "99");
            for (first, second) in [(&before, &after), (&after, &before)] {
                let mut torn = *first;
                torn[512..].copy_from_slice(&second[512..]);
                if let Some((read, _)) = Trial::decode(&torn) {
                    assert_eq!(read, try_it, "{} bytes of options", options.len());
                }
            }
        }
    }

    #[test]
    fn refuses_what_the_menu_could_not_start_as_it_is() {
        assert_eq!(Trial::new(0, Slot::A, ""), Err(Refused::Number(0)));
        assert_eq!(Trial::new(100, Slot::A, ""), Err(Refused::Number(100)));
        assert!(Trial::new(MAX_NUMBER, Slot::A, "").is_ok());
        for word in ["a$b", "x;y", "\"q\"", "{x}", "a*", "back\\slash", "#c", "é"] {
            assert_eq!(
                Trial::new(1, Slot::A, word),
                Err(Refused::Character(String::from(word))),
                "{word}"
            );
        }
        for word in ["root=UUID=x", "slot=a", "trial=2", "fallback=b"] {
            assert_eq!(
                Trial::new(1, Slot::A, word),
                Err(Refused::MenuWord(String::from(word))),
                "{word}"
            );
        }
        let longest = "x".repeat(MAX_OPTIONS);
        let try_it = Trial::new(MAX_NUMBER, Slot::B, &longest).unwrap();
        assert_eq!(Trial::decode(&try_it.encode()).unwrap().0, try_it);
        assert_eq!(
            Trial::new(1, Slot::A, &"x".repeat(MAX_OPTIONS + 1)),
            Err(Refused::TooLong(MAX_OPTIONS + 1))
        );
        // The longest try still fits the block once the menu has taken it.
        assert!(Trial::decode(&taken_as(&try_it, "99")).unwrap().1);
    }

    #[test]
    fn options_of_a_record_are_the_words_after_the_menus() {
        // The Dukovo entry's start, as the record keeps it.
        assert_eq!(
            options_of("root=UUID=1234 panic=10 quiet console=ttyS0 loglevel=info"),
            "quiet console=ttyS0 loglevel=info"
        );
        // A word an entry puts first, and a try's own number.
        assert_eq!(options_of("essentials root=UUID=1234 panic=10 quiet"), "quiet");
        assert_eq!(options_of("trial=2 root=UUID=1234 panic=10"), "");
        // The machine's own panic= after the menu's is kept.
        assert_eq!(options_of("root=UUID=1 panic=10 panic=0"), "panic=0");
        assert!(Trial::new(1, Slot::A, "panic=0").is_ok());
        assert_eq!(options_of("root=UUID=1 quiet panic=0"), "quiet panic=0");
        // A start the menu did not make.
        assert_eq!(options_of("quiet slot=a trial=1 kdb"), "quiet kdb");
        assert_eq!(options_of(""), "");
    }
}
