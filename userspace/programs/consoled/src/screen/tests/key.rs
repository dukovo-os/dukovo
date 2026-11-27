// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The secure key (ADR 0708): the kernel's own display layer raises
//! consoled, the holder of the attention file, over a borrower or over
//! nobody, and passes the front back when consoled says it is done.

use super::lend::{asked, borrower_draws, drawn, shows_theirs};
use super::*;

/// Ctrl+Alt+Delete, as the kernel's input hub tells the layer of it.
fn press(wire: &Wire) {
    let raised = wire.0.borrow_mut().layer.secure_key();
    assert_eq!(raised, Some(wire.holder()));
}

/// The holders in front and raised, as the layer has them.
fn front(wire: &Wire) -> (Option<u32>, Option<u32>) {
    let kernel = wire.0.borrow();
    (kernel.layer.front(), kernel.layer.raised())
}

#[test]
fn the_secure_key_puts_the_consoles_in_front_of_a_borrower_until_they_are_done() {
    let (wire, card, mut screen, mut term) = drawn();
    let borrower = screen.lend().unwrap();
    borrower_draws(&borrower, &card);
    assert!(screen.away() && !screen.take_key());

    // The key: consoled is told once, is in front over the borrower,
    // and the keys are the consoles'. All of the console is drawn.
    press(&wire);
    assert!(!screen.events());
    assert!(screen.take_key() && !screen.take_key());
    assert!(screen.raised() && screen.over() && !screen.away());
    term.feed(b", and the menu");
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
    // Nobody is lent the screens meanwhile.
    assert_eq!(screen.lend().err().and_then(|err| err.raw_os_error()), Some(libc::EBUSY));

    // Done: the borrower has the screens back at once, with no event
    // still to read, and consoled asks nothing of the layer.
    screen.lower();
    assert_eq!(front(&wire), (Some(borrower.holder()), None));
    assert!(!screen.raised() && !screen.over() && screen.away());
    borrower_draws(&borrower, &card);
    let before = asked(&wire);
    term.feed(b", and behind again");
    assert!(!screen.events() && !screen.tick(Instant::now()));
    screen.draw(&term, None);
    assert_eq!(asked(&wire), before);
    assert!(shows_theirs(&card));

    // The borrower gives the screens back as before the key.
    Holder::new(borrower).give_back().unwrap();
    assert!(!screen.events() && !screen.away());
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
}

#[test]
fn the_secure_key_with_the_consoles_in_front_passes_no_front() {
    let (wire, card, mut screen, mut term) = drawn();
    let before = asked(&wire).1;
    press(&wire);
    assert!(!screen.events());
    assert!(screen.take_key() && screen.raised() && !screen.over() && !screen.away());
    // What shows is the consoles' still: only the cells that change
    // are sent, and no state is asked for again.
    term.feed(b", and the menu");
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
    assert_eq!(asked(&wire).1, before);

    screen.lower();
    assert_eq!(front(&wire), (Some(wire.holder()), None));
    assert!(!screen.raised() && !screen.away());
    term.feed(b", and after it");
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
    // Done a second time is nothing, and the screens can be lent.
    screen.lower();
    assert!(screen.lend().is_ok());
}

#[test]
fn a_borrower_that_ends_under_the_raised_consoles_is_not_waited_behind() {
    let (wire, card, mut screen, mut term) = drawn();
    let borrower = screen.lend().unwrap();
    borrower_draws(&borrower, &card);
    press(&wire);
    assert!(!screen.events() && screen.take_key() && screen.over());
    screen.draw(&term, None);

    // The borrower ends while the menu shows: its picture is not what
    // shows, so there is nothing to leave on the screens.
    borrower.close();
    assert!(!screen.events() && !screen.tick(Instant::now()));
    assert!(screen.raised() && !screen.over());
    assert!(!screen.away() && screen.deadline().is_none());
    term.feed(b", with the borrower gone");
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);

    // Done: the consoles are simply in front.
    screen.lower();
    assert_eq!(front(&wire), (Some(wire.holder()), None));
    assert!(!screen.events() && !screen.away());
    term.feed(b", and done");
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
}

#[test]
fn the_secure_key_ends_the_wait_behind_the_last_picture_of_a_borrower_that_ended() {
    let (wire, card, mut screen, mut term) = drawn();
    let borrower = screen.lend().unwrap();
    borrower_draws(&borrower, &card);
    borrower.close();
    let now = Instant::now();
    assert!(!screen.events() && !screen.tick(now));
    assert!(screen.away() && screen.deadline().is_some());
    assert!(shows_theirs(&card));

    // The person asks for consoled: the consoles show now, not when the
    // five seconds are over, and nothing is under them.
    press(&wire);
    assert!(!screen.events() && !screen.tick(now));
    assert!(screen.take_key() && screen.raised() && !screen.over());
    assert!(!screen.away() && screen.deadline().is_none());
    term.feed(b", and the menu");
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
    screen.lower();
    assert!(!screen.tick(now + BORROWER_WAIT) && !screen.away());
}

#[test]
fn a_secure_key_among_lost_events_does_not_keep_its_hold() {
    let (wire, card, mut screen, term) = drawn();
    press(&wire);
    // consoled never reads of the key: only that events were lost.
    let mut lost = [0u8; 16 * DisplayEvent::SIZE];
    assert!(wire.events(&mut lost).is_ok());
    screen.event(&DisplayEvent { kind: EVENT_OVERFLOW, ..DisplayEvent::default() });
    assert!(!screen.events());
    assert!(screen.raised() && !screen.take_key());
    // It says it is done, as it does whenever nothing of the key shows.
    screen.lower();
    assert_eq!(front(&wire), (Some(wire.holder()), None));
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
}
