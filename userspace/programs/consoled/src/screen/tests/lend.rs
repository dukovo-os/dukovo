// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The screens lent to a borrower, a stand-in for a desktop: given back,
//! left by a borrower that ended, and taken by the next one, each
//! through the kernel's own display layer.

use display::abi::PLANE_PICTURE;

use super::*;

/// The byte a borrower's picture is made of.
const THEIRS: u8 = 0x77;

/// A console of 640 by 400 on a card like the firmware's, with something
/// written on it and drawn.
fn drawn() -> (Wire, Rc<FakeCard>, Screen<Wire>, Terminal) {
    let (wire, card) = firmware(640, 400);
    let mut screen = Screen::on(wire.clone(), radix_fbcon::MIN_GRID).unwrap();
    let mut term = terminal(&screen);
    term.feed(b"before the loan");
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
    (wire, card, screen, term)
}

/// The borrower draws: a picture of its own goes on the first screen's
/// main plane, and its pixels cover the screen.
fn borrower_draws(borrower: &Wire, card: &FakeCard) {
    let holder = Holder::new(borrower.clone());
    let found = outputs::Found::read(&holder).unwrap();
    let output = outputs::outputs(&found)[0];
    let picture = holder.keep_picture(output.card, output.mode.width, output.mode.height).unwrap();
    let mut change = ChangeBytes::new(0, 0);
    change.set(output.plane, PLANE_PICTURE, u64::from(picture));
    holder.change(&mut change).unwrap();
    card.glass.borrow_mut()[0].1.fill(THEIRS);
}

/// Whether all of `card`'s first screen is the borrower's picture still.
fn shows_theirs(card: &FakeCard) -> bool {
    card.glass.borrow()[0].1.iter().all(|&byte| byte == THEIRS)
}

/// How many PUT and CHANGE requests the layer has had.
fn asked(wire: &Wire) -> (usize, usize) {
    let kernel = wire.0.borrow();
    (kernel.puts, kernel.changes)
}

#[test]
fn the_screens_are_lent_and_the_consoles_show_again_when_given_back() {
    let (wire, card, mut screen, mut term) = drawn();
    assert!(!screen.away());
    let borrower = screen.lend().unwrap();
    let (theirs, front) = (borrower.holder(), wire.0.borrow().layer.front());
    assert_eq!(front, Some(theirs));
    assert!(screen.away());
    borrower_draws(&borrower, &card);

    // While the borrower has them, consoled asks nothing of the layer,
    // whatever is written to a console, and lends to no other.
    let before = asked(&wire);
    term.feed(b", and while lent");
    assert!(!screen.events() && !screen.tick(Instant::now()));
    screen.draw(&term, None);
    assert_eq!(asked(&wire), before);
    assert!(shows_theirs(&card));
    assert_eq!(screen.lend().err().and_then(|err| err.raw_os_error()), Some(libc::EBUSY));

    // The borrower gives them back: the consoles show, with what was
    // written meanwhile, and there is nothing to wait for.
    Holder::new(borrower).give_back().unwrap();
    assert!(!screen.events() && !screen.tick(Instant::now()));
    assert!(!screen.away() && screen.deadline().is_none());
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
}

#[test]
fn a_borrower_that_ends_leaves_its_last_picture_for_five_seconds() {
    let (wire, card, mut screen, mut term) = drawn();
    let borrower = screen.lend().unwrap();
    borrower_draws(&borrower, &card);
    // The borrower is gone without a word, as one that was killed is.
    borrower.close();
    let before = asked(&wire);
    let now = Instant::now();
    assert!(!screen.events() && !screen.tick(now));
    assert_eq!(screen.deadline(), Some(now + BORROWER_WAIT));
    assert!(screen.away(), "no key is typed behind its picture");

    // Its picture stays, and consoled asks nothing of the layer, until
    // the five seconds are over.
    term.feed(b", and unseen");
    screen.draw(&term, None);
    assert!(!screen.tick(now + BORROWER_WAIT - Duration::from_millis(1)));
    screen.draw(&term, None);
    assert_eq!(asked(&wire), before);
    assert!(shows_theirs(&card));

    // No other borrower came: the consoles show again, by one change
    // that asks for their state as it was.
    assert!(!screen.tick(now + BORROWER_WAIT));
    assert!(!screen.away() && screen.deadline().is_none());
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
    assert_eq!(wire.0.borrow().changes, before.1 + 1);
}

#[test]
fn another_borrower_within_the_five_seconds_takes_the_screens_from_the_last_picture() {
    let (wire, card, mut screen, term) = drawn();
    let borrower = screen.lend().unwrap();
    borrower_draws(&borrower, &card);
    borrower.close();
    let now = Instant::now();
    assert!(!screen.events() && !screen.tick(now));
    let before = asked(&wire);

    // The desktop is started again and asks for the screens: it has
    // them with the picture of the one before still on them, and no
    // console was drawn between the two.
    let next = screen.lend().unwrap();
    assert_eq!(asked(&wire), before);
    assert!(shows_theirs(&card));
    assert!(screen.away() && screen.deadline().is_none());
    // The wait the first one began is over with the loan.
    assert!(!screen.events() && !screen.tick(now + BORROWER_WAIT));
    screen.draw(&term, None);
    assert_eq!(asked(&wire), before);

    // When this one gives the screens back, the consoles show.
    Holder::new(next).give_back().unwrap();
    assert!(!screen.events());
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
}

#[test]
fn a_loan_that_never_reached_its_borrower_leaves_nothing_to_wait_behind() {
    let (_wire, card, mut screen, mut term) = drawn();
    // The program that asked left before its answer: consoled closes
    // the description it opened for it.
    let borrower = screen.lend().unwrap();
    screen.unlent();
    borrower.close();
    assert!(!screen.events() && !screen.tick(Instant::now()));
    assert!(!screen.away() && screen.deadline().is_none());
    term.feed(b", and after it");
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
    // The next borrower's end is waited behind as any is.
    let borrower = screen.lend().unwrap();
    borrower.close();
    assert!(!screen.events() && !screen.tick(Instant::now()));
    assert!(screen.away() && screen.deadline().is_some());
}

#[test]
fn with_the_events_of_a_loan_lost_the_consoles_still_show_again() {
    let (wire, card, mut screen, term) = drawn();
    let borrower = screen.lend().unwrap();
    borrower_draws(&borrower, &card);
    borrower.close();
    // consoled never reads that the front went, that the borrower ended
    // and that the front came back: only that events were lost.
    let mut lost = [0u8; 16 * DisplayEvent::SIZE];
    assert!(wire.events(&mut lost).is_ok());
    screen.event(&DisplayEvent { kind: EVENT_OVERFLOW, ..DisplayEvent::default() });
    assert!(!screen.events() && !screen.tick(Instant::now()));
    assert!(!screen.away());
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
}

#[test]
fn screens_that_change_behind_a_last_picture_are_arranged_when_the_wait_is_over() {
    let mut kernel = FakeKernel::new();
    let card = kernel.add_card(false, &[Some((640, 400))]);
    let wire = Wire::of(kernel);
    let mut screen = Screen::on(wire.clone(), radix_fbcon::MIN_GRID).unwrap();
    let mut term = terminal(&screen);
    let borrower = screen.lend().unwrap();
    borrower_draws(&borrower, &card);
    borrower.close();
    let now = Instant::now();
    assert!(!screen.events() && !screen.tick(now));

    // The monitor's size changes in the wait. consoled leaves the
    // screens alone until the wait is over, and then lays the consoles
    // out for the size there is.
    wire.0.borrow_mut().found(&card, 0, Some((800, 600)));
    let before = asked(&wire);
    assert!(!screen.events());
    assert_eq!(asked(&wire), before);
    assert!(screen.tick(now + BORROWER_WAIT), "the grid changed");
    assert_eq!(screen.describe(), ["console of 100x37 cells with font 8x16 at (0, 4)"]);
    term.resize(screen.cols(), screen.rows());
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
}
