// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `display-test input`: the input that follows the screens (ADRs 0706
//! to 0708), on a machine whose consoles consoled shows.
//!
//! A boot test starts the program with the machine and tells it what to
//! do, a word a line, on a serial port of its own, COM2, which no key
//! and no screen has a part in; the keys the test presses meanwhile and
//! the pointers it moves are the machine's. The program keeps
//! descriptions of the input files: tied to a holder of the screens,
//! tied to none, and handed out by inputd(8). At `heard` it says what
//! each of them read since it was last asked, a line each, so that the
//! test sees who heard a key and who did not.
//!
//! | Word | What the program does |
//! |---|---|
//! | `borrow` | borrows the screens from consoled as `ours`, and ties to it a description of every device's file, of _/devices/kbd_ and of _/devices/mouse_ |
//! | `lend` | lends the screens on to a second holder, `theirs`, with descriptions of the two merged files tied to it |
//! | `back` | has `theirs` give the screens back, and closes what was tied to it |
//! | `end` | ends `theirs` while it has the screens; what was tied to it stays open |
//! | `give` | gives the screens back to consoled, and closes what was tied to `ours` and what inputd handed out |
//! | `inputd` | asks inputd for the devices, as `ours` |
//! | `strangers` | asks inputd as three programs it refuses |
//! | `down` | says which keys each device of `ours` says are down |
//! | `heard` | says what each description read |
//! | `quit` | ends |
//!
//! A line of `heard` names the description and then what it read, the
//! oldest first: `+30` and `-30` for key 30 going down and up, a
//! device's `rel0:-5` and `abs1:32768` for an axis by its code, its
//! `paused`, `resumed` and `dropped` marks, and a motion of
//! _/devices/mouse_ as its eight numbers. A key's repeats and the ends
//! of reports are left out. The holder `ours` being passed over and
//! coming to the front again is said as it happens.

use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use display::abi::{EVENT_BACK, EVENT_BORROWER_ENDED, EVENT_FRONT};
use input::abi::{
    EV_ABS, EV_KEY, EV_REL, EV_SYN, InputEvent, PRESSED, RELEASED, SYN_DROPPED, SYN_PAUSED,
    SYN_REPORT, SYN_RESUMED,
};
use input::kind::codes;
use input::service::{DEVICE, GONE, Inbox, READY, REFUSED, Record, SOCKET, VERSION};
use input::{Control, Device, wire};
use radix_abi::capability::Rights;
use sysapi::poll::{self, PollFd};
use sysapi::{kernel, net};

use crate::handover::borrow_seat;
use crate::rig::{Outcome, Seat};

/// Where the devices are listed, and the folder of their files.
const CONTROL: &str = "/devices/input/control";
const DEVICES: &str = "/devices/input";

/// How long an event of the display layer, or inputd's answer, may take.
const PATIENCE: Duration = Duration::from_secs(10);

/// How long nothing must come before `heard` answers: what the test did
/// last has reached every description by then.
const QUIET: Duration = Duration::from_millis(500);

/// How a request reaches a file of the kernel's.
struct Raw<'a>(&'a File);

impl Control for Raw<'_> {
    fn control(&self, request: u32, argument: u64, buffer: &mut [u8]) -> io::Result<u64> {
        kernel::control(self.0, request, argument, buffer).map_err(kernel::io_error)
    }
}

/// Which file a description is of, which says how its records read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sort {
    /// A device's own file.
    Device,
    /// _/devices/kbd_.
    Keys,
    /// _/devices/mouse_.
    Motions,
}

/// How one event of a device's file reads in a line of `heard`; nothing
/// for the end of a report and for a key's repeat.
fn event(event: &InputEvent) -> Option<String> {
    Some(match (event.kind, event.code, event.value) {
        (EV_SYN, SYN_REPORT, _) => return None,
        (EV_SYN, SYN_PAUSED, _) => "paused".into(),
        (EV_SYN, SYN_RESUMED, _) => "resumed".into(),
        (EV_SYN, SYN_DROPPED, _) => "dropped".into(),
        (EV_KEY, code, 0) => format!("-{code}"),
        (EV_KEY, code, 1) => format!("+{code}"),
        (EV_KEY, _, _) => return None,
        (EV_REL, code, by) => format!("rel{code}:{by}"),
        (EV_ABS, code, at) => format!("abs{code}:{at}"),
        (kind, code, value) => format!("{kind}/{code}:{value}"),
    })
}

/// How the records `bytes` of a file of `sort` read in a line of
/// `heard`.
fn said(sort: Sort, bytes: &[u8]) -> Vec<String> {
    match sort {
        Sort::Device => wire::events(bytes).filter_map(|read| event(&read)).collect(),
        Sort::Keys => wire::keys(bytes)
            .filter_map(|key| match key.value {
                PRESSED => Some(format!("+{}", key.code)),
                RELEASED => Some(format!("-{}", key.code)),
                _ => None,
            })
            .collect(),
        Sort::Motions => wire::motions(bytes)
            .map(|m| {
                let moved = [m.dx, m.dy, m.wheel, m.pan];
                let rest = [m.x, m.y, m.buttons, m.flags];
                let numbers =
                    moved.iter().map(i32::to_string).chain(rest.iter().map(u32::to_string));
                numbers.collect::<Vec<_>>().join(",")
            })
            .collect(),
    }
}

/// One description of an input file, and what it read since `heard` was
/// last asked.
struct Reader {
    name: String,
    sort: Sort,
    file: File,
    heard: Vec<String>,
    /// Its device has gone: it reads nothing more.
    gone: bool,
}

impl Reader {
    /// The description `file`, of a file of `sort`.
    fn of(name: String, sort: Sort, file: File) -> Reader {
        Reader { name, sort, file, heard: Vec::new(), gone: false }
    }

    /// A new description of the file at `path`, whose reads never wait.
    fn open(name: String, sort: Sort, path: &str) -> Outcome<Reader> {
        let file = File::options().read(true).custom_flags(libc::O_NONBLOCK).open(path);
        Ok(Reader::of(name, sort, file.map_err(|err| format!("{path}: {err}"))?))
    }

    /// Ties it to `seat`'s holder.
    fn tied(self, seat: &Seat) -> Outcome<Reader> {
        let tied = Device::new(Raw(&self.file)).tie(seat.slot());
        tied.map_err(|err| format!("tying {} to a holder: {err}", self.name))?;
        Ok(self)
    }

    /// Takes in what there is to read. Returns whether there was any.
    fn read(&mut self) -> Outcome<bool> {
        let mut any = false;
        let mut bytes = [0u8; 4096];
        while !self.gone {
            match (&self.file).read(&mut bytes) {
                Ok(0) => break,
                Ok(read) => self.heard.extend(said(self.sort, &bytes[..read])),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                // A device that has gone, or records one of the merged
                // files lost (radix-input(2)).
                Err(err) if err.raw_os_error() == Some(libc::EPIPE) => {
                    self.gone = self.sort == Sort::Device;
                    self.heard.push(if self.gone { "gone" } else { "lost" }.into());
                }
                Err(err) => return Err(format!("reading {}: {err}", self.name)),
            }
            any = true;
        }
        Ok(any)
    }

    /// Says what it read, and forgets it.
    fn tell(&mut self) {
        let heard = std::mem::take(&mut self.heard);
        let heard = if heard.is_empty() { "nothing".into() } else { heard.join(" ") };
        println!("input: {} heard: {heard}", self.name);
    }
}

/// The numbers of the devices there are.
fn listed() -> Outcome<Vec<u32>> {
    let control = File::open(CONTROL).map_err(|err| format!("{CONTROL}: {err}"))?;
    input::devices(&Raw(&control)).map_err(|err| format!("listing the input devices: {err}"))
}

/// A new description of the file of the device that calls itself
/// `name`, tied to nobody, whose reads wait.
pub fn open_named(name: &str) -> Outcome<File> {
    for number in listed()? {
        let Ok(file) = File::open(format!("{DEVICES}/{number}")) else { continue };
        if name_of(&file) == name {
            return Ok(file);
        }
    }
    Err(format!("no input device calls itself {name}"))
}

/// What the device `file` is of calls itself.
fn name_of(file: &File) -> String {
    Device::new(Raw(file)).name().unwrap_or_else(|err| format!("a device with no name ({err})"))
}

/// A holder of the screens, and the descriptions tied to it.
struct Party {
    who: &'static str,
    seat: Seat,
    readers: Vec<Reader>,
}

impl Party {
    /// `seat`'s holder, known as `who`, with a description of each of
    /// the merged files tied to it.
    fn of(who: &'static str, seat: Seat) -> Outcome<Party> {
        let mut readers = Vec::new();
        for (file, sort) in [("kbd", Sort::Keys), ("mouse", Sort::Motions)] {
            let name = format!("{file}, tied to {who}");
            let reader = Reader::open(name, sort, &format!("/devices/{file}"))?;
            readers.push(reader.tied(&seat)?);
        }
        Ok(Party { who, seat, readers })
    }

    /// Ties a description of every device's file to the holder, and
    /// says what each device is.
    fn take_devices(&mut self) -> Outcome {
        for number in listed()? {
            let path = format!("{DEVICES}/{number}");
            let reader = Reader::open(String::new(), Sort::Device, &path)?.tied(&self.seat)?;
            let device = Device::new(Raw(&reader.file));
            let kinds = device.kinds().map_err(|err| format!("{path}: {err}"))?;
            let place = device.place().map_err(|err| format!("{path}: {err}"))?;
            let name = name_of(&reader.file);
            println!("input: device {name} is {kinds}, at {place}");
            self.readers.push(Reader { name: format!("{name}, tied to {}", self.who), ..reader });
        }
        Ok(())
    }

    /// Waits for the holder's event of `kind`, named `what`.
    fn told(&mut self, kind: u32, what: &str) -> Outcome {
        self.seat.event(what, PATIENCE, |event| event.kind == kind).map(|_| ())
    }
}

/// A connection to inputd, and what it handed out.
struct Handed {
    stream: UnixStream,
    inbox: Inbox<OwnedFd>,
    /// Each device's number, and the description that came for it.
    readers: Vec<(u32, Reader)>,
}

/// A copy of `seat`'s description that names its holder and can do
/// nothing but be passed on: what a program sends inputd.
fn named(seat: &Seat) -> Outcome<OwnedFd> {
    kernel::narrowed(seat.file(), Rights::COPY)
        .map_err(|status| format!("a copy of a holder: {}", kernel::io_error(status)))
}

/// Connects to inputd and says `first`, with `holder` if there is one.
fn ask(first: Record, holder: Option<&OwnedFd>) -> Outcome<Handed> {
    let stream = UnixStream::connect(SOCKET).map_err(|err| format!("{SOCKET}: {err}"))?;
    let holder = holder.map(AsFd::as_fd);
    net::send_with_fds(&stream, &first.to_bytes(), holder.as_slice())
        .map_err(|err| format!("sending inputd a holder: {err}"))?;
    Ok(Handed { stream, inbox: Inbox::new(), readers: Vec::new() })
}

impl Handed {
    /// Takes in what inputd sent, waiting up to `patience` for it.
    /// Returns each record's meaning: READY's or REFUSED's number is the
    /// end of an answer.
    fn read(&mut self, patience: Duration) -> Outcome<Vec<Record>> {
        if !poll::wait_readable(&self.stream, patience).unwrap_or(false) {
            return Ok(Vec::new());
        }
        let mut bytes = [0u8; 4 * Record::SIZE];
        let mut files = Vec::new();
        let read = net::receive_with_fds(&self.stream, &mut bytes, &mut files, 4)
            .map_err(|err| format!("reading from inputd: {err}"))?;
        if read == 0 {
            return Err("inputd closed the connection".into());
        }
        self.inbox.take(&bytes[..read], files);
        let mut records = Vec::new();
        while let Some(next) = self.inbox.next() {
            let (record, file) = next.map_err(|lost| format!("{:?} came with no file", lost.0))?;
            self.took(record, file);
            records.push(record);
        }
        Ok(records)
    }

    /// Says what `record` told, and keeps the description that came
    /// with a device.
    fn took(&mut self, record: Record, file: Option<OwnedFd>) {
        match (record.what, file) {
            (DEVICE, Some(file)) => {
                let file = File::from(file);
                let name = name_of(&file);
                println!("input: inputd handed out {name}, {}", record.kinds());
                let reader = Reader::of(format!("{name}, from inputd"), Sort::Device, file);
                self.readers.push((record.number, reader));
            }
            (GONE, _) => {
                let gone = self.readers.iter().find(|(number, _)| *number == record.number);
                let name = gone.map_or("a device it never handed out", |(_, r)| r.name.as_str());
                println!("input: inputd says that {name} went");
            }
            (READY, _) => println!("input: inputd handed out {} devices", record.number),
            (REFUSED, _) => println!("input: inputd refused, for reason {}", record.number),
            (what, _) => println!("input: inputd said {what}, which is nothing it says"),
        }
    }

    /// Reads until inputd's answer has ended, with READY or REFUSED.
    fn answer(&mut self) -> Outcome<Record> {
        let until = Instant::now() + PATIENCE;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let records = self.read(left)?;
            if let Some(end) = records.iter().find(|r| matches!(r.what, READY | REFUSED)) {
                return Ok(*end);
            }
            if left.is_zero() {
                return Err("inputd did not answer".into());
            }
        }
    }
}

/// What the program has, between words.
#[derive(Default)]
struct Rig {
    /// Descriptions of the merged files tied to no holder.
    loose: Vec<Reader>,
    /// The holder that borrowed the screens.
    ours: Option<Party>,
    /// The holder it lent them on to.
    theirs: Option<Party>,
    /// What was tied to a holder that ended.
    left: Vec<Reader>,
    /// What inputd handed out.
    handed: Option<Handed>,
}

impl Rig {
    fn ours(&mut self) -> Outcome<&mut Party> {
        self.ours.as_mut().ok_or_else(|| "the screens are not borrowed".into())
    }

    /// Every description there is, in the order `heard` tells of them.
    fn readers(&mut self) -> impl Iterator<Item = &mut Reader> {
        let parties = self.ours.iter_mut().chain(self.theirs.iter_mut());
        let handed = self.handed.iter_mut().flat_map(|handed| handed.readers.iter_mut());
        (self.loose.iter_mut())
            .chain(parties.flat_map(|party| party.readers.iter_mut()))
            .chain(self.left.iter_mut())
            .chain(handed.map(|(_, reader)| reader))
    }

    /// Takes in what every description has to read. Returns whether
    /// there was any.
    fn read(&mut self) -> Outcome<bool> {
        let mut any = false;
        for reader in self.readers() {
            any |= reader.read()?;
        }
        Ok(any)
    }

    /// Says what happened to `ours` at the front, and what inputd said,
    /// since this was last looked at.
    fn news(&mut self) -> Outcome {
        if let Some(ours) = &mut self.ours {
            while let Some(event) = ours.seat.next(Duration::ZERO)? {
                match event.kind {
                    EVENT_BACK => println!("input: ours is passed over"),
                    EVENT_FRONT => println!("input: ours is in front"),
                    _ => {}
                }
            }
        }
        if let Some(handed) = &mut self.handed {
            handed.read(Duration::ZERO)?;
        }
        Ok(())
    }

    /// Does what `word` says.
    fn obey(&mut self, word: &str) -> Outcome {
        match word {
            "borrow" => {
                let mut ours = Party::of("ours", borrow_seat()?)?;
                ours.take_devices()?;
                self.ours = Some(ours);
                println!("input: ours has the screens");
            }
            "lend" => {
                let theirs = Party::of("theirs", Seat::open()?)?;
                let ours = self.ours()?;
                ours.seat.holder.lend(theirs.seat.slot()).map_err(|err| format!("LEND: {err}"))?;
                ours.told(EVENT_BACK, "EVENT_BACK for the lender")?;
                let theirs = self.theirs.insert(theirs);
                theirs.told(EVENT_FRONT, "EVENT_FRONT for the borrower")?;
                println!("input: theirs has the screens");
            }
            "back" => {
                let theirs = self.theirs.take().ok_or("the screens are not lent on")?;
                theirs.seat.holder.give_back().map_err(|err| format!("RETURN: {err}"))?;
                self.ours()?.told(EVENT_FRONT, "EVENT_FRONT for the lender")?;
                println!("input: ours has the screens again");
            }
            "end" => {
                let theirs = self.theirs.take().ok_or("the screens are not lent on")?;
                self.left.extend(theirs.readers);
                drop(theirs.seat);
                self.ours()?.told(EVENT_BORROWER_ENDED, "EVENT_BORROWER_ENDED")?;
                self.ours()?.told(EVENT_FRONT, "EVENT_FRONT after the borrower ended")?;
                println!("input: theirs ended, and ours has the screens again");
            }
            "give" => {
                let ours = self.ours.take().ok_or("the screens are not borrowed")?;
                ours.seat.holder.give_back().map_err(|err| format!("RETURN to consoled: {err}"))?;
                self.handed = None;
                println!("input: the consoles have the screens");
            }
            "inputd" => {
                let ours = self.ours()?;
                let mut handed = ask(Record::holder(), Some(&named(&ours.seat)?))?;
                handed.answer()?;
                self.handed = Some(handed);
            }
            "strangers" => strangers()?,
            "down" => self.down()?,
            "heard" => {
                // Until nothing has come for a while.
                loop {
                    std::thread::sleep(QUIET);
                    if !self.read()? {
                        break;
                    }
                }
                self.news()?;
                self.readers().for_each(Reader::tell);
                println!("input: that is all");
            }
            word => return Err(format!("`{word}` is no word of the input test's")),
        }
        Ok(())
    }

    /// Says which keys each device of `ours` says are down.
    fn down(&mut self) -> Outcome {
        for reader in self.ours()?.readers.iter().filter(|reader| reader.sort == Sort::Device) {
            let keys = Device::new(Raw(&reader.file)).state(EV_KEY);
            let keys = keys.map_err(|err| format!("the keys of {}: {err}", reader.name))?;
            let down: Vec<String> = codes(&keys).map(|code| code.to_string()).collect();
            let down = if down.is_empty() { "nothing".into() } else { down.join(" ") };
            println!("input: {} has down: {down}", reader.name);
        }
        println!("input: that is all");
        Ok(())
    }

    /// Waits until the test said something, or something happened to
    /// `ours` or came from inputd.
    fn wait(&mut self, input: &File) -> Outcome {
        let seat = self.ours.as_ref().map(|ours| ours.seat.file().as_fd());
        let inputd = self.handed.as_ref().map(|handed| handed.stream.as_fd());
        let mut fds = vec![
            PollFd::new(input.as_fd(), libc::POLLIN),
            PollFd::optional(seat, libc::POLLIN),
            PollFd::optional(inputd, libc::POLLIN),
        ];
        poll::poll(&mut fds, None).map(|_| ()).map_err(|err| format!("waiting: {err}"))
    }
}

/// Asks inputd as three programs it refuses: one that sends no holder,
/// one that sends a file that is no holder of the screens, and a holder
/// that speaks a version inputd does not know.
fn strangers() -> Outcome {
    let nothing = File::open("/devices/null").map_err(|err| format!("/devices/null: {err}"))?;
    let nothing = OwnedFd::from(nothing);
    let holder = named(&Seat::open()?)?;
    let later = Record { number: VERSION + 1, ..Record::holder() };
    for (first, sent) in
        [(Record::holder(), None), (Record::holder(), Some(&nothing)), (later, Some(&holder))]
    {
        let mut asked = ask(first, sent)?;
        let end = asked.answer()?;
        if end.what != REFUSED || !asked.readers.is_empty() {
            return Err(format!("inputd answered a stranger {end:?}"));
        }
    }
    Ok(())
}

/// Runs the test: does what each line that comes on the serial port
/// `line` says, until `quit`.
pub fn run(line: &str) -> Outcome {
    let mut input = File::open(line).map_err(|err| format!("{line}: {err}"))?;
    let mut rig = Rig::default();
    for (file, sort) in [("kbd", Sort::Keys), ("mouse", Sort::Motions)] {
        let path = format!("/devices/{file}");
        rig.loose.push(Reader::open(format!("{file}, tied to nobody"), sort, &path)?);
    }
    println!("input: ready");
    let mut typed = String::new();
    loop {
        rig.wait(&input)?;
        rig.news()?;
        if !poll::wait_readable(&input, Duration::ZERO).unwrap_or(false) {
            continue;
        }
        let mut bytes = [0u8; 256];
        let read = input.read(&mut bytes).map_err(|err| format!("{line}: {err}"))?;
        if read == 0 {
            return Err(format!("{line} ended before `quit`"));
        }
        typed.push_str(&String::from_utf8_lossy(&bytes[..read]));
        while let Some(end) = typed.find('\n') {
            let word: String = typed.drain(..=end).collect();
            match word.trim() {
                "" => {}
                "quit" => return Ok(()),
                word => rig.obey(word)?,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use input::abi::{ABS_Y, BTN_LEFT, Event, KEY_A, MOTION_ABSOLUTE, Motion, REL_X, REPEATED};

    use super::*;

    fn le(event: &InputEvent) -> Vec<u8> {
        let mut bytes = event.time_ns.to_le_bytes().to_vec();
        bytes.extend(event.kind.to_le_bytes());
        bytes.extend(event.code.to_le_bytes());
        bytes.extend(event.value.to_le_bytes());
        bytes
    }

    #[test]
    fn a_device_s_events_read_as_keys_axes_and_marks_without_repeats_and_report_ends() {
        let event = |kind, code, value| InputEvent { time_ns: 1, kind, code, value };
        let events = [
            event(EV_KEY, KEY_A, 1),
            event(EV_SYN, SYN_REPORT, 0),
            event(EV_KEY, KEY_A, 2),
            event(EV_KEY, KEY_A, 0),
            event(EV_KEY, BTN_LEFT, 1),
            event(EV_REL, REL_X, -5),
            event(EV_ABS, ABS_Y, 32768),
            event(EV_SYN, SYN_PAUSED, 0),
            event(EV_SYN, SYN_RESUMED, 0),
            event(EV_SYN, SYN_DROPPED, 0),
            event(5, 0, 1),
        ];
        let bytes: Vec<u8> = events.iter().flat_map(le).collect();
        assert_eq!(
            said(Sort::Device, &bytes).join(" "),
            "+30 -30 +272 rel0:-5 abs1:32768 paused resumed dropped 5/0:1"
        );
    }

    #[test]
    fn the_merged_files_read_as_keys_without_repeats_and_as_motions() {
        let key = |value| Event { time_ns: 2, code: KEY_A, value, reserved: 0 };
        let mut bytes = Vec::new();
        for key in [key(PRESSED), key(REPEATED), key(RELEASED)] {
            bytes.extend(key.time_ns.to_le_bytes());
            bytes.extend(key.code.to_le_bytes());
            bytes.extend(key.value.to_le_bytes());
            bytes.extend(0u32.to_le_bytes());
        }
        assert_eq!(said(Sort::Keys, &bytes), ["+30", "-30"]);

        let motion = Motion {
            time_ns: 3,
            dx: 10,
            dy: -5,
            x: 65535,
            flags: MOTION_ABSOLUTE,
            ..Motion::default()
        };
        let mut bytes = motion.time_ns.to_le_bytes().to_vec();
        for moved in [motion.dx, motion.dy, motion.wheel, motion.pan] {
            bytes.extend(moved.to_le_bytes());
        }
        for rest in [motion.x, motion.y, motion.buttons, motion.flags] {
            bytes.extend(rest.to_le_bytes());
        }
        assert_eq!(said(Sort::Motions, &bytes), ["10,-5,0,0,65535,0,0,1"]);
    }
}
