// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Tests of the front passing on: lent and given back between three
//! holders, left by a holder that ends, and waited for by the kernel's
//! log. What a holder at the back costs is measured in `tests/kept.rs`,
//! with an allocator that counts.

use radix_abi::display::{EVENT_SCREEN, MAY_BLANK};

use super::*;

/// Gives `holder` a picture tagged `tag` and a mode, both of `size`, on
/// the virtio-like card, and returns the entries that show them on the
/// card's first screen.
fn shows(rig: &mut Rig, holder: u32, tag: u32, size: (u32, u32)) -> [(u32, u32, u64); 5] {
    let shape =
        PictureShape { card: V_CARD, format: FORMAT_XRGB8888, width: size.0, height: size.1 };
    let (picture, _) = rig.picture(holder, shape, tag);
    let mode = rig.mode(holder, mode(size.0, size.1));
    feed(0, mode, picture)
}

#[test]
fn the_front_is_lent_and_given_back_between_three_holders() {
    let mut rig = Rig::firmware();
    let virtio = rig.add_virtio();
    // A text console, the desktop it lends to, and a game the desktop
    // lends to: the first two in one mode, the game in a smaller one.
    let (console, desktop, game) = (rig.holder(1), rig.holder(2), rig.holder(3));
    let of_console = shows(&mut rig, console, 1, (1280, 800));
    let of_desktop = shows(&mut rig, desktop, 2, (1280, 800));
    let of_game = shows(&mut rig, game, 3, (800, 600));
    assert_eq!(rig.change(console, NOW, 0, &of_console), DONE);
    virtio.asked();

    // The desktop's first frame is in the mode that shows: none is set.
    assert_eq!(rig.layer.lend(console, desktop), Ok(()));
    assert!(virtio.asked().is_empty(), "what the lender showed stays");
    assert_eq!(rig.change(desktop, NOW, 0, &of_desktop), DONE);
    let drawn = virtio.shown();
    assert_eq!((drawn.main, drawn.mode, drawn.whole), (Some(2), false, true));
    assert_eq!(rig.layer.lend(desktop, game), Ok(()));
    assert_eq!(rig.change(game, NOW | MAY_BLANK, 0, &of_game), DONE);
    let drawn = virtio.shown();
    assert_eq!((drawn.main, drawn.size, drawn.mode), (Some(3), (800, 600), true));

    // Those at the back keep what they asked for, and change nothing.
    let dot = [changed(V_MAINS[0], Rect::sized(1, 1))];
    assert_eq!(rig.change(desktop, NOW, 0, &dot), status(busy()));
    assert_eq!(rig.change(console, NOW, 0, &dot), status(busy()));
    assert_eq!(rig.layer.give_back(desktop), Err(busy()));
    assert!(virtio.asked().is_empty());

    // The game gives the front back: the desktop's state is made again
    // in one step, with the one mode set that differs.
    assert_eq!(rig.layer.give_back(game), Ok(()));
    let drawn = virtio.shown();
    assert_eq!(
        (drawn.main, drawn.size, drawn.mode, drawn.whole),
        (Some(2), (1280, 800), true, true)
    );
    // And the desktop to the console, whose mode shows already: one
    // whole frame, and no mode set.
    assert_eq!(rig.layer.give_back(desktop), Ok(()));
    let drawn = virtio.shown();
    assert_eq!((drawn.main, drawn.mode, drawn.whole), (Some(1), false, true));
    assert_eq!(rig.layer.front(), Some(console));

    // Each was told as the front came and went.
    let passes = |kinds: Vec<u32>| -> Vec<u32> {
        kinds.into_iter().filter(|&kind| kind != EVENT_SCREEN).collect()
    };
    assert_eq!(passes(rig.kinds(console)), [EVENT_FRONT, EVENT_BACK, EVENT_FRONT]);
    assert_eq!(passes(rig.kinds(desktop)), [EVENT_FRONT, EVENT_BACK, EVENT_FRONT, EVENT_BACK]);
    assert_eq!(passes(rig.kinds(game)), [EVENT_FRONT, EVENT_BACK]);
}

#[test]
fn a_borrower_that_ends_leaves_its_last_picture_until_its_lender_draws() {
    let mut rig = Rig::firmware();
    let lender = rig.in_front();
    let borrower = rig.holder(2);
    let own = u64::from(rig.frame(borrower, 2));
    assert_eq!(rig.layer.lend(lender, borrower), Ok(()));
    assert_eq!(rig.change(borrower, NOW, 0, &[(MAIN, PLANE_PICTURE, own)]), DONE);
    let last = rig.layer.holder(borrower).unwrap().picture_ref(1).unwrap().clone();
    rig.firmware.asked();
    rig.kinds(lender);

    // The borrower is gone, as one that was killed is: nothing is drawn,
    // and its lender is told so before it is told it is in front.
    rig.layer.end(borrower);
    assert!(rig.firmware.asked().is_empty(), "its last picture stays");
    assert_eq!(rig.kinds(lender), [EVENT_BORROWER_ENDED, EVENT_FRONT]);
    assert_eq!((rig.layer.front(), rig.layer.keeps_last()), (Some(lender), false));
    assert!(rig.layer.holder(borrower).is_none());
    // The plane that shows the picture has the one count left of it
    // besides the test's.
    assert_eq!(Rc::strong_count(&last), 2);
    // However long the lender leaves it, and whatever beats come.
    rig.layer.set_time(LOG_WAIT_NS + BEAT);
    rig.layer.beat(CARD);
    assert!(rig.firmware.asked().is_empty());
    assert_eq!(rig.layer.next_beat(CARD), None);

    // The lender's next change, even one that names nothing, makes its
    // state: all of the frame, in the mode that shows.
    assert_eq!(rig.change(lender, NOW, 0, &[]), DONE);
    let drawn = rig.firmware.shown();
    assert_eq!((drawn.main, drawn.whole, drawn.mode), (Some(1), true, false));
    assert_eq!(Rc::strong_count(&last), 1);
}

#[test]
fn a_new_borrower_takes_the_screens_from_the_last_picture_of_the_one_before() {
    let mut rig = Rig::firmware();
    let lender = rig.in_front();
    let borrower = rig.holder(2);
    let own = u64::from(rig.frame(borrower, 2));
    assert_eq!(rig.layer.lend(lender, borrower), Ok(()));
    assert_eq!(rig.change(borrower, NOW, 0, &[(MAIN, PLANE_PICTURE, own)]), DONE);
    rig.layer.end(borrower);
    rig.firmware.asked();

    // The lender lends again without drawing: the next borrower's first
    // frame is drawn over the last one of the borrower before, and the
    // lender's own never shows between them.
    let next = rig.holder(3);
    let own = u64::from(rig.frame(next, 3));
    assert_eq!(rig.layer.lend(lender, next), Ok(()));
    assert!(rig.firmware.asked().is_empty());
    assert_eq!(rig.change(next, NOW, 0, &[(MAIN, PLANE_PICTURE, own)]), DONE);
    let drawn = rig.firmware.shown();
    assert_eq!((drawn.main, drawn.whole, drawn.mode), (Some(3), true, false));
}

#[test]
fn the_log_waits_its_turn_behind_the_last_picture_of_a_holder_that_ended() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    let picture = rig.layer.holder(holder).unwrap().picture_ref(1).unwrap().clone();
    // Its holder has one count of the picture, the plane that shows it
    // another, and the test a third.
    assert_eq!(Rc::strong_count(&picture), 3);
    rig.layer.set_time(MS);
    let Ok(Made::Wait(seq)) = rig.change(holder, 0, 0, &[changed(MAIN, Rect::sized(9, 9))]) else {
        panic!("the change was to wait for its beat");
    };
    rig.firmware.asked();

    // The holder ends with none under it. Nothing is drawn; what it
    // waited to show is dropped; and the card is asked for its beat by
    // the end of the log's wait.
    rig.layer.end(holder);
    let due = MS + LOG_WAIT_NS;
    assert_eq!(rig.firmware.asked(), [Call::Wake(due)]);
    assert_eq!((rig.layer.front(), rig.layer.keeps_last()), (None, true));
    assert_eq!((rig.layer.waits(seq), rig.layer.next_beat(CARD)), (false, Some(due)));
    assert_eq!(Rc::strong_count(&picture), 2);
    rig.layer.set_time(due - 1);
    rig.layer.beat(CARD);
    assert!(rig.firmware.asked().is_empty() && rig.layer.keeps_last());

    // The wait over, the log draws, and the picture is let go.
    rig.layer.set_time(due);
    rig.layer.beat(CARD);
    assert_eq!(rig.firmware.asked(), [Call::LogTakes(0)]);
    assert_eq!((rig.layer.keeps_last(), rig.layer.next_beat(CARD)), (false, None));
    assert_eq!(Rc::strong_count(&picture), 1);
    // The pipe's beat went on all the while: the first was at time 0.
    assert_eq!(rig.layer.pipe_info(PIPE).unwrap().beat_count, due / BEAT + 1);
}

#[test]
fn what_starts_in_an_ended_holders_place_takes_the_screens_with_no_log_between() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    rig.layer.set_time(MS);
    rig.layer.end(holder);
    rig.firmware.asked();

    // Its service starts it again well inside the wait: the first frame
    // of the new one is drawn whole over the old one's last. The log
    // never drew, so it is not told to stop.
    rig.layer.set_time(200 * MS);
    let next = rig.holder(2);
    let own = u64::from(rig.frame(next, 2));
    assert_eq!(rig.change(next, 0, 0, &[(MAIN, PLANE_PICTURE, own)]), DONE);
    let asked = rig.firmware.asked();
    let [Call::Show(drawn)] = &asked[..] else { panic!("one frame was to be shown: {asked:?}") };
    assert_eq!((drawn.main, drawn.whole, drawn.mode), (Some(2), true, false));
    assert_eq!((rig.layer.front(), rig.layer.keeps_last()), (Some(next), false));
    assert_eq!(rig.kinds(next), [EVENT_FRONT]);

    // And the log's turn does not come when the wait would have ended.
    rig.layer.set_time(MS + LOG_WAIT_NS);
    rig.layer.beat(CARD);
    assert!(rig.firmware.asked().is_empty());
}

#[test]
fn an_urgent_message_of_the_wait_brings_the_log_at_once() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    // With a holder in front, the log draws nothing, however urgent.
    rig.layer.set_time(4 * MS);
    rig.layer.log_urgent(4 * MS);
    assert!(rig.firmware.asked().is_empty());

    // The holder ends. The message from before the wait began shows no
    // log; one that comes in the wait does.
    rig.layer.set_time(5 * MS);
    rig.layer.end(holder);
    rig.firmware.asked();
    rig.layer.set_time(6 * MS);
    rig.layer.log_urgent(4 * MS);
    assert!(rig.firmware.asked().is_empty() && rig.layer.keeps_last());
    rig.layer.log_urgent(6 * MS);
    assert_eq!(rig.firmware.asked(), [Call::LogTakes(0)]);
    assert_eq!((rig.layer.keeps_last(), rig.layer.next_beat(CARD)), (false, None));
    // Once the log is in front, another changes nothing.
    rig.layer.log_urgent(6 * MS);
    assert!(rig.firmware.asked().is_empty());
}

#[test]
fn a_screen_that_changes_in_the_wait_has_the_log_laid_out_on_it() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    rig.layer.end(holder);
    rig.firmware.asked();
    // The last picture was for the screen as it was.
    let found = Found { plugged: true, modes: &[mode(800, 600)], data: &[] };
    assert_eq!(rig.layer.screen_found(CARD, 0, &found), Ok(true));
    assert_eq!(rig.firmware.asked(), [Call::LogTakes(0)]);
    assert!(!rig.layer.keeps_last());
}
