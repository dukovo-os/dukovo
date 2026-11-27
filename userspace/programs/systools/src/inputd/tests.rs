// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! inputd's broker, on the build machine: devices that are numbers, and
//! programs that keep what they are sent.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use input::service::{DEVICE, GONE};

use super::*;

/// A description of a device's file: the device's number, and which
/// opening of a file this was, from 1.
type Opened = (u32, u32);

/// Devices that are numbers, and holders of the screens that are too.
#[derive(Default)]
struct Fake {
    /// The devices there are, and what each is.
    there: RefCell<BTreeMap<u32, Kinds>>,
    /// Devices that are listed and gone when their file is opened.
    went: RefCell<BTreeSet<u32>>,
    /// Devices whose files cannot be tied.
    stuck: RefCell<BTreeSet<u32>>,
    /// The holders of the screens there are.
    holders: BTreeSet<u32>,
    /// How many files were opened.
    opened: Cell<u32>,
    /// Each tie made: the description, and the holder.
    ties: RefCell<Vec<(Opened, u32)>>,
}

impl Fake {
    /// A machine with a keyboard, device 0, and a mouse, device 1, whose
    /// screens the holders 7 and 8 hold.
    fn machine() -> Fake {
        let fake = Fake { holders: BTreeSet::from([7, 8]), ..Fake::default() };
        fake.plug(0, Kinds::KEYBOARD);
        fake.plug(1, Kinds::POINTER);
        fake
    }

    fn plug(&self, number: u32, kinds: Kinds) {
        self.there.borrow_mut().insert(number, kinds);
    }

    fn pull(&self, number: u32) {
        self.there.borrow_mut().remove(&number);
    }
}

impl Devices for Fake {
    type File = Opened;
    type Holder = u32;

    fn list(&self) -> io::Result<Vec<u32>> {
        Ok(self.there.borrow().keys().copied().collect())
    }

    fn open(&self, number: u32) -> io::Result<Opened> {
        if !self.there.borrow().contains_key(&number) || self.went.borrow().contains(&number) {
            return Err(io::ErrorKind::NotFound.into());
        }
        self.opened.set(self.opened.get() + 1);
        Ok((number, self.opened.get()))
    }

    fn kinds(&self, file: &Opened) -> io::Result<Kinds> {
        self.there.borrow().get(&file.0).copied().ok_or_else(|| io::ErrorKind::NotFound.into())
    }

    fn tie(&self, file: &Opened, holder: &u32) -> io::Result<()> {
        if !self.holders.contains(holder) || self.stuck.borrow().contains(&file.0) {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        self.ties.borrow_mut().push((*file, *holder));
        Ok(())
    }

    fn holds(&self, holder: &u32) -> io::Result<bool> {
        Ok(self.holders.contains(holder))
    }
}

/// What a program was sent: each record, and the description with it.
type Sent = Rc<RefCell<Vec<(Record, Option<Opened>)>>>;

/// A program that keeps what it is sent, until it is `full`.
#[derive(Clone, Default)]
struct Program {
    sent: Sent,
    full: Rc<Cell<bool>>,
}

impl Program {
    /// What it was sent since this was last asked.
    fn took(&self) -> Vec<(Record, Option<Opened>)> {
        std::mem::take(&mut self.sent.borrow_mut())
    }

    /// The records it was sent since this was last asked, without what
    /// came with them.
    fn heard(&self) -> Vec<Record> {
        self.took().into_iter().map(|(record, _)| record).collect()
    }
}

impl Peer<Opened> for Program {
    fn send(&mut self, record: Record, file: Option<&Opened>) -> io::Result<()> {
        if self.full.get() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        self.sent.borrow_mut().push((record, file.copied()));
        Ok(())
    }
}

/// A broker of the machine's devices, and a program that holds `holder`
/// and was handed them.
fn served(holder: u32) -> (Broker<Fake, Program>, Program) {
    let mut broker = Broker::new(Fake::machine());
    let program = Program::default();
    assert!(broker.join(program.clone(), Record::holder(), Some(holder)).is_some());
    (broker, program)
}

#[test]
fn a_holder_of_the_screens_is_handed_every_device_tied_to_it_and_told_that_is_all() {
    let (broker, program) = served(7);
    assert_eq!(
        program.took(),
        [
            (Record::device(0, Kinds::KEYBOARD), Some((0, 1))),
            (Record::device(1, Kinds::POINTER), Some((1, 2))),
            (Record::ready(2), None),
        ]
    );
    assert_eq!(*broker.devices().ties.borrow(), [((0, 1), 7), ((1, 2), 7)]);
}

#[test]
fn each_program_gets_descriptions_of_its_own_tied_to_its_own_holder() {
    let (mut broker, first) = served(7);
    let second = Program::default();
    assert!(broker.join(second.clone(), Record::holder(), Some(8)).is_some());
    let files = |program: &Program| -> Vec<Opened> {
        program.took().into_iter().filter_map(|(_, file)| file).collect()
    };
    assert_eq!(files(&first), [(0, 1), (1, 2)]);
    assert_eq!(files(&second), [(0, 3), (1, 4)]);
    assert_eq!(broker.devices().ties.borrow()[2..], [((0, 3), 8), ((1, 4), 8)]);
}

#[test]
fn a_program_that_holds_no_screen_is_refused_and_handed_nothing() {
    let mut broker = Broker::new(Fake::machine());
    for holder in [None, Some(3)] {
        let program = Program::default();
        assert_eq!(broker.join(program.clone(), Record::holder(), holder), None);
        assert_eq!(program.took(), [(Record::refused(REFUSED_NOT_A_HOLDER), None)]);
    }
    assert_eq!(broker.devices().opened.get(), 0, "no device's file was opened for them");
    assert_eq!(broker.peers().count(), 0);
}

#[test]
fn a_first_record_that_is_not_a_holder_of_this_version_is_refused() {
    let mut broker = Broker::new(Fake::machine());
    let later = Record { number: VERSION + 1, ..Record::holder() };
    for first in [Record::ready(0), Record::gone(0), later, Record::default()] {
        let program = Program::default();
        assert_eq!(broker.join(program.clone(), first, Some(7)), None);
        assert_eq!(program.heard(), [Record::refused(REFUSED_VERSION)]);
    }
}

#[test]
fn a_device_that_comes_is_handed_to_every_program_and_one_that_goes_is_told() {
    let (mut broker, first) = served(7);
    let second = Program::default();
    broker.join(second.clone(), Record::holder(), Some(8));
    first.took();
    second.took();

    broker.devices().plug(4, Kinds::TABLET);
    broker.added(4);
    assert_eq!(first.took(), [(Record::device(4, Kinds::TABLET), Some((4, 5)))]);
    assert_eq!(second.took(), [(Record::device(4, Kinds::TABLET), Some((4, 6)))]);

    broker.devices().pull(1);
    broker.removed(1);
    assert_eq!(first.heard(), [Record::gone(1)]);
    assert_eq!(second.heard(), [Record::gone(1)]);
}

#[test]
fn a_device_is_handed_once_and_its_going_told_once() {
    let (mut broker, program) = served(7);
    program.took();
    broker.added(0);
    broker.removed(5);
    assert_eq!(program.took(), []);
    broker.devices().pull(0);
    broker.removed(0);
    broker.removed(0);
    assert_eq!(program.heard(), [Record::gone(0)]);
}

#[test]
fn a_number_that_is_used_again_is_a_new_device() {
    let (mut broker, program) = served(7);
    program.took();
    broker.devices().pull(1);
    broker.removed(1);
    broker.devices().plug(1, Kinds::TOUCH);
    broker.added(1);
    assert_eq!(
        program.took(),
        [(Record::gone(1), None), (Record::device(1, Kinds::TOUCH), Some((1, 3)))]
    );
}

#[test]
fn after_changes_were_lost_only_what_differs_is_sent() {
    let (mut broker, program) = served(7);
    program.took();
    broker.devices().pull(0);
    broker.devices().plug(2, Kinds::KEYS);
    broker.devices().plug(3, Kinds::SWITCH);
    broker.relist();
    let heard = program.heard();
    assert_eq!(heard.iter().filter(|record| record.what == GONE).count(), 1);
    assert_eq!(
        heard,
        [Record::gone(0), Record::device(2, Kinds::KEYS), Record::device(3, Kinds::SWITCH)]
    );
    broker.relist();
    assert_eq!(program.took(), [], "nothing changed since");
}

#[test]
fn a_program_that_does_not_take_what_it_is_sent_is_dropped_and_the_others_go_on() {
    let (mut broker, slow) = served(7);
    let other = Program::default();
    let id = broker.join(other.clone(), Record::holder(), Some(8));
    slow.took();
    other.took();

    slow.full.set(true);
    broker.devices().plug(2, Kinds::KEYS);
    broker.added(2);
    assert_eq!(other.heard(), [Record::device(2, Kinds::KEYS)]);
    assert_eq!(broker.peers().map(|(id, _)| Some(id)).collect::<Vec<_>>(), [id]);

    slow.full.set(false);
    broker.devices().pull(0);
    broker.removed(0);
    assert_eq!(slow.took(), [], "it is served no longer");
    assert_eq!(other.heard(), [Record::gone(0)]);
}

#[test]
fn a_program_that_cannot_be_told_a_device_went_is_dropped() {
    let (mut broker, program) = served(7);
    program.full.set(true);
    broker.devices().pull(0);
    broker.removed(0);
    assert_eq!(broker.peers().count(), 0);
}

#[test]
fn a_program_that_takes_nothing_from_the_start_is_not_served() {
    let mut broker = Broker::new(Fake::machine());
    let program = Program::default();
    program.full.set(true);
    assert_eq!(broker.join(program, Record::holder(), Some(7)), None);
    assert_eq!(broker.peers().count(), 0);
}

#[test]
fn a_device_that_went_before_its_file_was_opened_is_passed_over() {
    let fake = Fake::machine();
    fake.went.borrow_mut().insert(0);
    let mut broker = Broker::new(fake);
    let program = Program::default();
    assert!(broker.join(program.clone(), Record::holder(), Some(7)).is_some());
    assert_eq!(program.heard(), [Record::device(1, Kinds::POINTER), Record::ready(1)]);
    // Its going is told to nobody: nobody had it.
    broker.removed(0);
    assert_eq!(program.took(), []);
}

#[test]
fn a_device_whose_file_cannot_be_tied_is_not_handed_out() {
    let fake = Fake::machine();
    fake.stuck.borrow_mut().insert(1);
    let mut broker = Broker::new(fake);
    let program = Program::default();
    assert!(broker.join(program.clone(), Record::holder(), Some(7)).is_some());
    let took = program.took();
    assert_eq!(took.len(), 2);
    assert_eq!(took[0].0, Record::device(0, Kinds::KEYBOARD));
    assert_eq!(took[1].0, Record::ready(1));
    assert!(took.iter().all(|(record, file)| (record.what == DEVICE) == file.is_some()));
}

#[test]
fn a_machine_with_no_input_device_answers_all_the_same() {
    let fake = Fake { holders: BTreeSet::from([7]), ..Fake::default() };
    let mut broker = Broker::new(fake);
    let program = Program::default();
    assert!(broker.join(program.clone(), Record::holder(), Some(7)).is_some());
    assert_eq!(program.heard(), [Record::ready(0)]);
    let stranger = Program::default();
    assert_eq!(broker.join(stranger.clone(), Record::holder(), Some(1)), None);
    assert_eq!(stranger.heard(), [Record::refused(REFUSED_NOT_A_HOLDER)]);
}

#[test]
fn no_more_programs_are_served_than_the_most() {
    let mut broker = Broker::new(Fake::machine());
    for _ in 0..CLIENTS_MAX {
        assert!(broker.join(Program::default(), Record::holder(), Some(7)).is_some());
    }
    let late = Program::default();
    assert_eq!(broker.join(late.clone(), Record::holder(), Some(7)), None);
    assert_eq!(late.heard(), [Record::refused(REFUSED_FULL)]);
    let opened = broker.devices().opened.get();
    assert_eq!(opened, 2 * CLIENTS_MAX as u32, "no file was opened for the one refused");
}

#[test]
fn a_program_that_left_is_sent_nothing_more_and_makes_room() {
    let mut broker = Broker::new(Fake::machine());
    let programs: Vec<Program> = (0..CLIENTS_MAX).map(|_| Program::default()).collect();
    let ids: Vec<u64> = programs
        .iter()
        .map(|program| broker.join(program.clone(), Record::holder(), Some(8)).expect("is served"))
        .collect();
    programs[3].took();
    broker.leave(ids[3]);
    broker.devices().plug(9, Kinds::KEYS);
    broker.added(9);
    assert_eq!(programs[3].took(), []);
    assert_eq!(broker.peers().count(), CLIENTS_MAX - 1);
    let next = broker.join(Program::default(), Record::holder(), Some(7)).expect("there is room");
    assert!(!ids.contains(&next), "a program is never known by a number another was");
}
