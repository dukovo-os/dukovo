// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The desktop whole: a Wayland program ([`window_test`]) on one end,
//! the kernel's own display layer on the other ([`rig`]), and the
//! desktop told of each thing as `main` tells it.

mod rig;

use std::cell::RefCell;
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::mpsc::{self, Sender, TryRecvError};
use std::time::Duration;

use display_pattern::pattern;
use input::abi::{BTN_LEFT, KEY_A, KEY_F2, KEY_LEFTALT, KEY_LEFTCTRL};
use radix_display::Rect;
use sysapi::poll::{PollFd, poll};
use window_test::{Client, Heard};

use crate::desktop::Desktop;
use crate::keys::Chord;
use crate::screens::BACKGROUND;
use rig::{FakeCard, FakeKernel, Showed, Wire};

/// How long a program waits for the desktop before a test fails.
const PATIENCE: Duration = Duration::from_secs(20);

/// What a test asks a program to do, in the program's thread.
type Job = Box<dyn FnOnce(&mut Client) + Send>;

/// A program: a thread with a connection to the desktop.
struct Program {
    jobs: Sender<Job>,
}

/// A desktop on a stand-in kernel.
struct World {
    desktop: Desktop<Wire>,
    kernel: Rc<RefCell<FakeKernel>>,
    card: Rc<FakeCard>,
}

impl World {
    /// A desktop on a card with `screens`, as [`rig::lent`] takes them.
    fn new(screens: &[Option<(u32, u32)>], shared: bool) -> World {
        let (holder, card) = rig::lent(screens, shared);
        let kernel = holder.file().0.clone();
        let desktop = Desktop::new(holder, "us").unwrap();
        let mut world = World { desktop, kernel, card };
        world.settle();
        world
    }

    /// A desktop on one screen of 1024 by 768.
    fn one_screen() -> World {
        World::new(&[Some((1024, 768))], false)
    }

    fn now(&self) -> u64 {
        self.kernel.borrow().now
    }

    /// Tells the desktop each event the layer has for it.
    fn hear_layer(&mut self) {
        loop {
            let event = self.kernel.borrow_mut().event();
            let Some(event) = event else { break };
            self.desktop.layer_said(&event).unwrap();
        }
    }

    /// One turn of windowd's loop: the programs that wrote are heard,
    /// the layer's events are, the frames that are due are drawn, and
    /// two milliseconds pass.
    fn turn(&mut self) {
        let wrote: Vec<usize> = {
            let waited_on = self.desktop.waited_on();
            let mut fds: Vec<PollFd<'_>> =
                waited_on.map(|fd| PollFd::new(fd, libc::POLLIN)).collect();
            poll(&mut fds, Some(Duration::from_millis(1))).unwrap();
            let wrote = fds.iter().enumerate().filter(|(_, fd)| fd.revents() != 0);
            wrote.map(|(at, _)| at).collect()
        };
        self.desktop.programs_wrote(wrote);
        self.hear_layer();
        let now = self.now();
        if self.desktop.due(now).is_some_and(|due| due <= now) {
            self.desktop.draw(now).unwrap();
        }
        self.kernel.borrow_mut().pass(2_000_000);
        self.hear_layer();
    }

    /// Turns until nothing has been drawn for two beats.
    fn settle(&mut self) {
        let mut quiet = 0;
        while quiet < 20 {
            let before = self.card.showed.borrow().len();
            self.turn();
            quiet = if self.card.showed.borrow().len() == before { quiet + 1 } else { 0 };
        }
    }

    /// Starts a program.
    fn program(&mut self) -> Program {
        let (ours, theirs) = UnixStream::pair().unwrap();
        self.desktop.take_in(ours).unwrap();
        let (jobs, todo) = mpsc::channel::<Job>();
        std::thread::spawn(move || {
            let mut client = Client::on(theirs).expect("the program could not start");
            for job in todo {
                job(&mut client);
            }
        });
        Program { jobs }
    }

    /// Has `program` do `job`, and runs the desktop until it has.
    fn ask<T: Send + 'static>(
        &mut self,
        program: &Program,
        job: impl FnOnce(&mut Client) -> T + Send + 'static,
    ) -> T {
        let (answer, answered) = mpsc::channel();
        let job = move |client: &mut Client| {
            // The test that asked may have failed meanwhile.
            let _ = answer.send(job(client));
        };
        program.jobs.send(Box::new(job)).unwrap();
        loop {
            match answered.try_recv() {
                Ok(answer) => return answer,
                Err(TryRecvError::Empty) => self.turn(),
                Err(TryRecvError::Disconnected) => panic!("the program ended"),
            }
        }
    }

    /// Has `program` show its window of `size` with the pattern of
    /// `seed`, and waits until it was told of the frame that shows it.
    fn show(&mut self, program: &Program, size: (u32, u32), seed: u32) {
        self.ask(program, move |client| {
            client.show(size, seed).unwrap();
            let frame = |heard: &Heard| matches!(heard, Heard::Frame(_)).then_some(());
            client.wait(PATIENCE, frame).unwrap();
        });
    }

    /// What `program` heard of the things `wanted` takes, up to the
    /// first that `last` takes.
    fn heard(
        &mut self,
        program: &Program,
        last: impl Fn(&Heard) -> bool + Send + 'static,
    ) -> Vec<Heard> {
        self.ask(program, move |client| {
            let mut heard = Vec::new();
            loop {
                heard.push(client.heard(PATIENCE).unwrap());
                if heard.last().is_some_and(&last) {
                    return heard;
                }
            }
        })
    }

    /// The colour the first screen shows at (`x`, `y`): red, green and
    /// blue.
    fn colour(&mut self, x: u32, y: u32) -> [u8; 3] {
        let (width, pixels) = self.desktop.shows(0);
        let at = ((y * width + x) * 4) as usize;
        [pixels[at + 2], pixels[at + 1], pixels[at]]
    }

    /// What the card was asked to show since `from` things before.
    fn showed_since(&self, from: usize) -> Vec<Showed> {
        self.card.showed.borrow()[from..].to_vec()
    }

    fn showed(&self) -> usize {
        self.card.showed.borrow().len()
    }

    /// Presses and lets go of the key of `code`.
    fn tap(&mut self, code: u16) {
        let now = self.now();
        self.desktop.key(now, code, true);
        self.desktop.key(now, code, false);
    }
}

/// Where a window of `size` first shows on a screen of 1024 by 768.
fn middle((width, height): (u32, u32)) -> (u32, u32) {
    ((1024 - width) / 2, (768 - height) / 2)
}

#[test]
fn a_program_s_window_shows_in_the_middle_of_the_screen_pixel_for_pixel() {
    let mut world = World::one_screen();
    assert_eq!(world.colour(0, 0), BACKGROUND);
    assert_eq!(world.colour(512, 384), BACKGROUND);
    let program = world.program();
    world.show(&program, (320, 200), 7);
    let (left, top) = middle((320, 200));
    for (x, y) in [(0, 0), (319, 0), (0, 199), (319, 199), (160, 100)] {
        assert_eq!(world.colour(left + x, top + y), pattern::colour(7, x, y), "({x}, {y})");
    }
    for (x, y) in [(left - 1, top), (left + 320, top), (left, top - 1), (left, top + 200)] {
        assert_eq!(world.colour(x, y), BACKGROUND, "({x}, {y})");
    }
    let told = world.ask(&program, |client| {
        (client.screens(), client.layout_bytes(), client.repeat(), client.has(), client.active())
    });
    let (screens, layout_bytes, repeat, has, active) = told;
    assert_eq!(screens.len(), 1);
    let screen = &screens[0];
    assert_eq!((screen.name.as_str(), screen.width, screen.height), ("virtio0/0", 1024, 768));
    assert!(layout_bytes.is_some_and(|bytes| bytes > 1000), "{layout_bytes:?}");
    assert_eq!(repeat, Some((250, 30)));
    assert_eq!(has, (true, true));
    assert!(active);
}

#[test]
fn a_frame_drawn_before_the_one_before_it_showed_takes_its_place() {
    let (holder, card) = rig::lent(&[Some((1024, 768))], false);
    let kernel = holder.file().0.clone();
    let desktop = Desktop::new(holder, "us").unwrap();
    let mut world = World { desktop, kernel, card };
    // The first frame is drawn before the layer's word that the desktop
    // is in front is read, as when both wait at the desktop's start. The
    // word has every screen drawn anew, before the first frame's beat.
    let now = world.now();
    world.desktop.draw(now).unwrap();
    world.hear_layer();
    assert_eq!(world.desktop.due(now), Some(now));
    world.desktop.draw(now).unwrap();
    assert!(world.desktop.front());
    world.settle();
    let program = world.program();
    world.show(&program, (320, 200), 7);
    let (left, top) = middle((320, 200));
    assert_eq!(world.colour(left, top), pattern::colour(7, 0, 0));
}

#[test]
fn a_still_desktop_draws_nothing_and_is_not_woken() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 7);
    world.settle();
    let before = world.showed();
    for _ in 0..100 {
        world.turn();
        assert_eq!(world.desktop.due(world.now()), None);
    }
    assert_eq!(world.showed(), before);
}

#[test]
fn only_what_changed_is_drawn_and_the_card_is_told_where() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 7);
    world.settle();
    let before = world.showed();
    world.show(&program, (320, 200), 8);
    world.settle();
    let (left, top) = middle((320, 200));
    let window = Rect { x: left as i32, y: top as i32, width: 320, height: 200 };
    let showed = world.showed_since(before);
    let drawn: Vec<&Showed> = showed.iter().filter(|showed| !showed.damage.is_empty()).collect();
    assert_eq!(drawn.len(), 1, "{showed:?}");
    assert_eq!(drawn[0].damage, [window]);
    assert!(!drawn[0].mode && !drawn[0].main);
    assert_eq!(world.colour(left, top), pattern::colour(8, 0, 0));
    assert_eq!(world.colour(left - 1, top), BACKGROUND);
}

#[test]
fn a_card_that_reads_the_pages_at_any_moment_gets_its_frames_in_two_pictures_in_turn() {
    let mut world = World::new(&[Some((1024, 768))], true);
    let program = world.program();
    let (left, top) = middle((320, 200));
    for seed in 7..11 {
        world.show(&program, (320, 200), seed);
        world.settle();
        // The picture that shows has the whole frame, whatever the one
        // before it had.
        assert_eq!(world.colour(left + 5, top + 5), pattern::colour(seed, 5, 5));
        assert_eq!(world.colour(3, 3), BACKGROUND);
    }
    let showed = world.showed_since(0);
    let frames: Vec<&Showed> = showed.iter().filter(|showed| !showed.damage.is_empty()).collect();
    assert!(frames.len() >= 5, "{frames:?}");
    // Every frame is in the picture the card is not reading.
    for pair in frames.windows(2) {
        assert!(pair[1].main && pair[0].picture != pair[1].picture, "{pair:?}");
    }
    assert_eq!(frames[0].picture, frames[2].picture);
    // The frame after a window's first redraws no more than that window.
    let window = Rect { x: left as i32, y: top as i32, width: 320, height: 200 };
    assert_eq!(frames.last().unwrap().damage, [window]);
}

#[test]
fn a_program_that_asks_for_a_frame_with_nothing_new_is_told_at_the_next_beat() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 7);
    world.settle();
    let before = world.showed();
    for _ in 0..3 {
        world.ask(&program, |client| client.ask_frame().unwrap());
        let heard = world.heard(&program, |heard| matches!(heard, Heard::Frame(_)));
        assert!(matches!(heard.last(), Some(Heard::Frame(_))));
    }
    // Nothing was shown for it: the beats alone were asked for.
    assert_eq!(world.showed_since(before), []);
}

#[test]
fn keys_reach_the_window_with_the_keyboard_and_windowd_s_chords_reach_none() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 7);
    world.tap(KEY_A);
    let now = world.now();
    world.desktop.key(now, KEY_LEFTCTRL, true);
    world.desktop.key(now, KEY_LEFTALT, true);
    assert_eq!(world.desktop.asked(), None);
    world.tap(KEY_F2);
    assert_eq!(world.desktop.asked(), Some(Chord::Console(2)));
    world.desktop.key(now, KEY_LEFTALT, false);
    world.desktop.key(now, KEY_LEFTCTRL, false);
    let last = Heard::Key { code: u32::from(KEY_LEFTCTRL), pressed: false };
    let ends = last.clone();
    let heard = world.heard(&program, move |heard| *heard == ends);
    let keys: Vec<Heard> =
        heard.into_iter().filter(|heard| matches!(heard, Heard::Key { .. })).collect();
    let key = |code: u16, pressed| Heard::Key { code: u32::from(code), pressed };
    assert_eq!(
        keys,
        [
            key(KEY_A, true),
            key(KEY_A, false),
            key(KEY_LEFTCTRL, true),
            key(KEY_LEFTALT, true),
            key(KEY_LEFTALT, false),
            last,
        ]
    );
}

#[test]
fn the_pointer_moves_at_once_and_the_window_under_it_is_told() {
    let mut world = World::one_screen();
    let program = world.program();
    // The pointer starts in the middle, where the window shows.
    world.show(&program, (320, 200), 7);
    let heard = world.heard(&program, |heard| matches!(heard, Heard::Enter(..)));
    assert_eq!(heard.last(), Some(&Heard::Enter(160.0, 100.0)));
    world.settle();
    let before = world.showed();
    let now = world.now();
    world.desktop.moved(now, 4, -2);
    // No frame was drawn or waited for.
    let showed = world.showed_since(before);
    assert_eq!(showed.len(), 1, "{showed:?}");
    assert!(showed[0].moved && showed[0].damage.is_empty());
    assert_eq!(showed[0].pointer_at, Some((516, 382)));
    let heard = world.heard(&program, |heard| matches!(heard, Heard::Motion(..)));
    assert_eq!(heard.last(), Some(&Heard::Motion(164.0, 98.0)));
    // Off the window, and against the screen's edge.
    world.desktop.moved(now + 20_000_000, -4000, -4000);
    let heard = world.heard(&program, |heard| *heard == Heard::Leave);
    assert_eq!(heard.last(), Some(&Heard::Leave));
    assert_eq!(world.showed_since(before).last().unwrap().pointer_at, Some((0, 0)));
    world.desktop.pointed(now, Some(1.0), Some(1.0));
    assert_eq!(world.showed_since(before).last().unwrap().pointer_at, Some((1023, 767)));
}

#[test]
fn a_click_gives_a_window_the_keyboard_and_brings_it_to_the_front() {
    let mut world = World::one_screen();
    let (first, second) = (world.program(), world.program());
    world.show(&first, (320, 200), 7);
    world.show(&second, (320, 200), 8);
    // The second shows a step down and to the right, in front.
    let (left, top) = middle((320, 200));
    assert_eq!(world.colour(left + 40, top + 40), pattern::colour(8, 8, 8));
    assert_eq!(world.colour(left + 5, top + 5), pattern::colour(7, 5, 5));
    assert!(!world.ask(&first, |client| {
        client.wait(PATIENCE, |heard| (*heard == Heard::Active(false)).then_some(())).unwrap();
        client.active()
    }));
    // A click on the corner of the first that still shows.
    let now = world.now();
    world.desktop.pointed(
        now,
        Some(f64::from(left + 5) / 1023.0),
        Some(f64::from(top + 5) / 767.0),
    );
    world.desktop.button(now, BTN_LEFT, true);
    world.desktop.button(now, BTN_LEFT, false);
    let pressed = Heard::Button { code: u32::from(BTN_LEFT), pressed: true };
    let ends = pressed.clone();
    let heard = world.heard(&first, move |heard| *heard == ends);
    assert!(heard.contains(&Heard::Keyboard(true)) && heard.contains(&Heard::Active(true)));
    world.settle();
    assert_eq!(world.colour(left + 40, top + 40), pattern::colour(7, 40, 40));
    world.tap(KEY_A);
    let heard = world.heard(&first, |heard| matches!(heard, Heard::Key { pressed: false, .. }));
    assert!(heard.contains(&Heard::Key { code: u32::from(KEY_A), pressed: true }));
    let heard = world.heard(&second, |heard| *heard == Heard::Keyboard(false));
    assert!(!heard.iter().any(|heard| matches!(heard, Heard::Key { .. })));
}

#[test]
fn a_program_that_goes_takes_its_window_and_the_one_behind_has_the_keyboard() {
    let mut world = World::one_screen();
    let (first, second) = (world.program(), world.program());
    world.show(&first, (320, 200), 7);
    world.show(&second, (320, 200), 8);
    let (left, top) = middle((320, 200));
    drop(second);
    let heard = world.heard(&first, |heard| *heard == Heard::Keyboard(true));
    assert!(heard.contains(&Heard::Active(true)));
    world.settle();
    assert_eq!(world.colour(left + 40, top + 40), pattern::colour(7, 40, 40));
    assert_eq!(world.colour(left + 330, top + 210), BACKGROUND);
}

#[test]
fn a_program_whose_pixels_are_cut_short_is_let_go_and_the_desktop_draws_on() {
    let mut world = World::one_screen();
    let (first, broken) = (world.program(), world.program());
    world.show(&first, (320, 200), 7);
    world.ask(&broken, |client| client.cut_short((320, 200))).unwrap();
    // The window that stays is drawn anew, and nothing shows of the
    // other, which was a step right and down of it.
    world.show(&first, (320, 200), 9);
    world.settle();
    let (left, top) = middle((320, 200));
    assert_eq!(world.colour(left + 40, top + 40), pattern::colour(9, 40, 40));
    assert_eq!(world.colour(left + 330, top + 210), BACKGROUND);
    let has_keyboard = world.ask(&first, |client| {
        let keyboard = |heard: &Heard| match heard {
            Heard::Keyboard(has) => Some(*has),
            _ => None,
        };
        client.take_heard().iter().rev().find_map(keyboard)
    });
    assert_ne!(has_keyboard, Some(false));
}

#[test]
fn a_program_s_own_pointer_shows_over_its_window_with_no_frame_drawn() {
    assert_eq!(BACKGROUND, window_test::DESKTOP);
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 7);
    world.heard(&program, |heard| matches!(heard, Heard::Enter(..)));
    world.settle();
    let before = world.showed();
    world.ask(&program, |client| client.point_with(16, 9)).unwrap();
    world.settle();
    let showed = world.showed_since(before);
    assert!(showed.iter().any(|showed| showed.pointer), "{showed:?}");
    assert!(showed.iter().all(|showed| !showed.main && showed.damage.is_empty()), "{showed:?}");
    assert_eq!(showed.last().unwrap().pointer_at, Some((512, 384)));
}

#[test]
fn nothing_is_drawn_in_the_back_and_the_keys_held_are_let_go() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 7);
    world.settle();
    let now = world.now();
    world.desktop.key(now, KEY_A, true);
    world.desktop.button(now, BTN_LEFT, true);
    // The person chose a console: the lender has the screens.
    world.kernel.borrow_mut().lend(false);
    world.hear_layer();
    assert!(!world.desktop.front());
    world.desktop.let_go(now);
    let heard =
        world.heard(&program, |heard| matches!(heard, Heard::Button { pressed: false, .. }));
    assert!(heard.contains(&Heard::Key { code: u32::from(KEY_A), pressed: false }));
    let before = world.showed();
    world.ask(&program, |client| client.show((320, 200), 9).unwrap());
    for _ in 0..50 {
        world.turn();
    }
    assert_eq!(world.desktop.due(world.now()), None);
    // And back: the frame the program drew meanwhile shows.
    world.kernel.borrow_mut().lend(true);
    world.settle();
    let (left, top) = middle((320, 200));
    assert_eq!(world.colour(left, top), pattern::colour(9, 0, 0));
    let showed = world.showed_since(before);
    assert!(showed.iter().any(|showed| !showed.damage.is_empty()), "{showed:?}");
}

#[test]
fn a_screen_that_comes_is_shown_on_beside_the_first_and_the_window_stays() {
    let mut world = World::new(&[Some((1024, 768)), None], false);
    let program = world.program();
    world.show(&program, (320, 200), 7);
    world.kernel.borrow_mut().found(1, Some((800, 600)));
    world.settle();
    let screens = world.ask(&program, |client| {
        // The window server's news of the screen is read with a frame.
        client.ask_frame().unwrap();
        client.wait(PATIENCE, |heard| matches!(heard, Heard::Frame(_)).then_some(())).unwrap();
        client.screens()
    });
    assert_eq!(screens.len(), 2, "{screens:?}");
    let second = screens.iter().find(|screen| screen.name == "virtio0/1").unwrap();
    assert_eq!((second.width, second.height, second.x, second.y), (800, 600, 1024, 0));
    let (left, top) = middle((320, 200));
    assert_eq!(world.colour(left, top), pattern::colour(7, 0, 0));
    let (width, pixels) = world.desktop.shows(1);
    assert_eq!(width, 800);
    assert_eq!(pixels[..3], [BACKGROUND[2], BACKGROUND[1], BACKGROUND[0]]);
    // The pointer crosses to it, and shows there.
    let before = world.showed();
    let now = world.now();
    world.desktop.moved(now, 400, 0);
    world.settle();
    let on_second = world.showed_since(before);
    let on_second: Vec<&Showed> = on_second.iter().filter(|showed| showed.pipe == 1).collect();
    assert!(on_second.iter().any(|showed| showed.pointer_at.is_some()), "{on_second:?}");
}
