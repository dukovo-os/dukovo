// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The records against the kernel's own writers of them
//! (`radix_input::event`), the requests against a device as the kernel
//! keeps it (`radix_input::device`), what each kind of device is called,
//! and inputd's exchange cut every way a socket may cut it.

use std::cell::Cell;
use std::io;

use radix_abi::input::{
    ABS_X, ABS_Y, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, BTN_TOOL_PEN, BTN_TOUCH, BUS_I8042, BUS_USB,
    BUS_VIRTUAL, DEVICE_ADDED, DEVICE_REMOVED, EV_ABS, EV_KEY, EV_LED, EV_REL, EV_SW, EV_SYN,
    INPUT_AXIS, INPUT_BITS, INPUT_INFO, INPUT_LIST, INPUT_NAME, INPUT_PLACE, INPUT_SERIAL,
    INPUT_STATE, INPUT_TIE, InputAxis, InputChange, InputEvent, InputInfo, KEY_A, KEY_KP1,
    KEY_MUTE, KEY_POWER, KEY_VOLUMEDOWN, KEY_VOLUMEUP, POSITION_MAX, REL_WHEEL_HI_RES, REL_X,
    REL_Y, SW_LID, SYN_REPORT,
};
use radix_input::device::{About, Axis, Device as Kept, Few, Keys, padded};
use radix_input::event::{Record as Written, Said, mark};

use super::kind::{Has, Kinds, codes, has};
use super::service::{
    DEVICE, GONE, HOLDER, Inbox, NoFile, READY, REFUSED, REFUSED_FULL, Record, VERSION,
};
use super::*;

/// The bytes the kernel writes for `record`.
fn written<R: Written>(record: &R) -> Vec<u8> {
    let mut out = vec![0xaa; R::SIZE];
    record.encode(&mut out);
    out
}

#[test]
fn events_are_read_as_the_kernel_writes_them_and_a_cut_one_is_left_out() {
    let said = [
        Said::moved(REL_X, -3).event(1_000),
        Said::key(BTN_LEFT, true).event(1_000),
        mark(1_000, SYN_REPORT),
        Said::at(ABS_X, i32::MAX).event(u64::MAX),
    ];
    let mut bytes: Vec<u8> = said.iter().flat_map(written).collect();
    let read: Vec<InputEvent> = wire::events(&bytes).collect();
    assert_eq!(read, said);
    assert_eq!((read[0].kind, read[0].code, read[0].value), (EV_REL, REL_X, -3));
    assert_eq!((read[2].kind, read[2].code), (EV_SYN, SYN_REPORT));
    // Whatever a read returned, only whole events come of it.
    for cut in 0..bytes.len() {
        assert_eq!(wire::events(&bytes[..cut]).count(), cut / InputEvent::SIZE);
    }
    bytes.truncate(InputEvent::SIZE + 5);
    assert_eq!(wire::events(&bytes).collect::<Vec<_>>(), said[..1]);
}

#[test]
fn the_devices_that_came_and_went_and_their_numbers_are_read() {
    let changes = [
        InputChange { time_ns: 7, device: 3, what: DEVICE_ADDED },
        InputChange { time_ns: 9, device: 1, what: DEVICE_REMOVED },
    ];
    let bytes: Vec<u8> = changes.iter().flat_map(written).collect();
    assert_eq!(wire::changes(&bytes).collect::<Vec<_>>(), changes);
    assert_eq!(wire::changes(&bytes[..bytes.len() - 1]).count(), 1);

    let numbers: Vec<u8> = [1u32, 2, 70_000].iter().flat_map(|n| n.to_le_bytes()).collect();
    assert_eq!(wire::numbers(&numbers).collect::<Vec<_>>(), [1, 2, 70_000]);
    assert_eq!(wire::numbers(&numbers[..11]).collect::<Vec<_>>(), [1, 2]);
}

#[test]
fn what_a_device_says_of_itself_is_read_and_too_few_bytes_are_nothing() {
    let info = InputInfo {
        device: 4,
        bus: BUS_USB,
        vendor: 0x046d,
        product: 0xc077,
        version: 0x111,
        rate_hz: 1000,
    };
    let bytes = written(&info);
    assert_eq!(wire::info(&bytes), Some(info));
    assert_eq!(wire::info(&bytes[..bytes.len() - 1]), None);

    let axis = InputAxis { value: -5, min: -32768, max: 32767, fuzz: 4, flat: 16, resolution: 40 };
    let bytes = written(&axis);
    assert_eq!(wire::axis(&bytes), Some(axis));
    assert_eq!(wire::axis(&bytes[..bytes.len() - 1]), None);

    assert_eq!(wire::text(b"USB Optical Mouse"), "USB Optical Mouse");
    assert_eq!(wire::text(b"usb-1-2\0\0\0"), "usb-1-2");
    assert_eq!(wire::text(&[0xff, 0xfe]), "");
    assert_eq!(wire::text(b""), "");
}

/// A stand-in for the kernel's side of a device's file, which answers
/// each request from the device as the kernel keeps it.
struct File {
    device: Kept,
    /// Whether the reader is heard: its holder is in front.
    heard: bool,
    /// The slot the description was tied to.
    tied: Cell<Option<u64>>,
}

impl File {
    fn of(number: u32, about: &About) -> File {
        File { device: Kept::new(number, about), heard: true, tied: Cell::new(None) }
    }
}

fn errno(number: i32) -> io::Error {
    io::Error::from_raw_os_error(number)
}

/// EINVAL and EBUSY, as the C library numbers them.
const EINVAL: i32 = 22;
const EBUSY: i32 = 16;

impl Control for File {
    fn control(&self, request: u32, argument: u64, buffer: &mut [u8]) -> io::Result<u64> {
        let code = u16::try_from(argument).map_err(|_| errno(EINVAL));
        let about = &self.device.about;
        let mut answer = [0u8; 128];
        let whole = match request {
            INPUT_INFO => {
                self.device.info().encode(&mut answer[..InputInfo::SIZE]);
                InputInfo::SIZE
            }
            INPUT_AXIS => {
                let axis = self.device.axis(code?, self.heard).ok_or_else(|| errno(EINVAL))?;
                axis.encode(&mut answer[..InputAxis::SIZE]);
                InputAxis::SIZE
            }
            INPUT_BITS => about.bits(code?, &mut answer).ok_or_else(|| errno(EINVAL))?,
            INPUT_STATE => {
                self.device.state(code?, self.heard, &mut answer).ok_or_else(|| errno(EINVAL))?
            }
            INPUT_NAME | INPUT_PLACE | INPUT_SERIAL => {
                let field: &[u8] = match request {
                    INPUT_NAME => &about.name,
                    INPUT_PLACE => &about.place,
                    _ => &about.serial,
                };
                let text = radix_input::device::text(field).as_bytes();
                answer[..text.len()].copy_from_slice(text);
                text.len()
            }
            INPUT_TIE if self.tied.get().is_some() => return Err(errno(EBUSY)),
            INPUT_TIE => {
                self.tied.set(Some(argument));
                0
            }
            _ => return Err(errno(EINVAL)),
        };
        let fits = whole.min(buffer.len());
        buffer[..fits].copy_from_slice(&answer[..fits]);
        Ok(whole as u64)
    }
}

/// A mouse with three buttons and a wheel, as its USB driver tells of
/// it.
fn mouse() -> About {
    let mut about = About::new(BUS_USB, "USB Optical Mouse", "usb-1-2");
    (about.vendor, about.product, about.version, about.rate_hz) = (0x046d, 0xc077, 0x111, 125);
    about.serial = padded("A1B2");
    about.keys = Keys::of(&[BTN_LEFT, BTN_RIGHT, BTN_MIDDLE]);
    about.moves = Few::of(&[REL_X, REL_Y, REL_WHEEL_HI_RES]);
    about
}

/// The pointer of a virtual machine: it says where it points.
fn tablet() -> About {
    let mut about = About::new(BUS_VIRTUAL, "QEMU Virtio Tablet", "pci-0000:00:05.0");
    about.keys = Keys::of(&[BTN_LEFT, BTN_RIGHT, BTN_MIDDLE]);
    for code in [ABS_X, ABS_Y] {
        assert!(about.add_axis(Axis { code, max: POSITION_MAX as i32, ..Axis::default() }));
    }
    about
}

#[test]
fn a_device_answers_what_it_is_and_what_it_has() {
    let device = Device::new(File::of(4, &mouse()));
    let info = device.info().unwrap();
    assert_eq!((info.device, info.bus, info.vendor, info.product), (4, BUS_USB, 0x046d, 0xc077));
    assert_eq!((info.version, info.rate_hz), (0x111, 125));
    assert_eq!(device.name().unwrap(), "USB Optical Mouse");
    assert_eq!(device.place().unwrap(), "usb-1-2");
    assert_eq!(device.serial().unwrap(), "A1B2");

    // Its codes come without the zero bytes after the last.
    let keys = device.bits(EV_KEY).unwrap();
    assert_eq!(keys.len(), usize::from(BTN_MIDDLE) / 8 + 1);
    assert_eq!(codes(&keys).collect::<Vec<_>>(), [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE]);
    let moves = device.bits(EV_REL).unwrap();
    assert_eq!(codes(&moves).collect::<Vec<_>>(), [REL_X, REL_Y, REL_WHEEL_HI_RES]);
    assert!(has(&moves, REL_Y) && !has(&moves, 2) && !has(&moves, 4000));
    assert!(device.bits(EV_ABS).unwrap().is_empty() && device.bits(EV_LED).unwrap().is_empty());
    assert_eq!(device.bits(0x1f).err().and_then(|err| err.raw_os_error()), Some(EINVAL));
    assert!(device.state(EV_KEY).unwrap().is_empty());
    assert_eq!(device.kinds().unwrap(), Kinds::POINTER);
    assert_eq!(device.axis(ABS_X).err().and_then(|err| err.raw_os_error()), Some(EINVAL));
}

#[test]
fn a_tablet_answers_its_axes_and_a_description_is_tied_once() {
    let device = Device::new(File::of(2, &tablet()));
    let axis = device.axis(ABS_Y).unwrap();
    assert_eq!((axis.min, axis.max, axis.value), (0, POSITION_MAX as i32, 0));
    assert_eq!(codes(&device.bits(EV_ABS).unwrap()).collect::<Vec<_>>(), [ABS_X, ABS_Y]);
    assert_eq!(device.kinds().unwrap(), Kinds::TABLET);
    assert_eq!(device.serial().unwrap(), "");

    device.tie(7).unwrap();
    assert_eq!(device.file().tied.get(), Some(7));
    assert_eq!(device.tie(8).err().and_then(|err| err.raw_os_error()), Some(EBUSY));
    assert_eq!(device.into_file().tied.get(), Some(7));
}

/// A stand-in for _/devices/input/control_ with `numbers` devices.
struct Listed {
    numbers: Vec<u32>,
    asked: Cell<usize>,
}

impl Control for Listed {
    fn control(&self, request: u32, _argument: u64, buffer: &mut [u8]) -> io::Result<u64> {
        assert_eq!(request, INPUT_LIST);
        self.asked.set(self.asked.get() + 1);
        for (number, out) in self.numbers.iter().zip(buffer.as_chunks_mut::<4>().0) {
            *out = number.to_le_bytes();
        }
        Ok(self.numbers.len() as u64)
    }
}

#[test]
fn the_devices_are_listed_however_many_there_are() {
    let few = Listed { numbers: vec![1, 2, 5], asked: Cell::new(0) };
    assert_eq!(devices(&few).unwrap(), [1, 2, 5]);
    assert_eq!(few.asked.get(), 1);
    // More than the first buffer holds: asked again, with room for all.
    let many = Listed { numbers: (1..=40).collect(), asked: Cell::new(0) };
    assert_eq!(devices(&many).unwrap(), (1..=40).collect::<Vec<_>>());
    assert_eq!(many.asked.get(), 2);
    let none = Listed { numbers: Vec::new(), asked: Cell::new(0) };
    assert!(devices(&none).unwrap().is_empty());
}

/// What the kernel answers INPUT_BITS with for each kind of `about`.
fn has_of(about: &About) -> Has {
    let bits = |kind| {
        let mut out = [0u8; BITS_MAX];
        let whole = about.bits(kind, &mut out).unwrap();
        out[..whole].to_vec()
    };
    Has { keys: bits(EV_KEY), moves: bits(EV_REL), places: bits(EV_ABS), switches: bits(EV_SW) }
}

/// `about`, which says where it points, with `keys`.
fn pointing(keys: &[u16]) -> About {
    let mut about = tablet();
    about.keys = Keys::of(keys);
    about
}

#[test]
fn each_device_is_called_what_it_is() {
    let named = |about: &About| has_of(about).kinds().to_string();
    // The keyboard of a PS/2 port has every letter, and media keys too.
    let mut keyboard = About::new(BUS_I8042, "AT keyboard", "ps2");
    keyboard.keys = radix_input::set1_keys();
    assert_eq!(named(&keyboard), "keyboard");
    // One letter short of the 26 is not a keyboard.
    keyboard.keys.set(KEY_A, false);
    assert_eq!(named(&keyboard), "keys");

    assert_eq!(named(&mouse()), "pointer");
    assert_eq!(named(&tablet()), "tablet");
    let mut media = About::new(BUS_USB, "USB Keyboard", "usb-1-3");
    media.keys = Keys::of(&[KEY_VOLUMEUP, KEY_VOLUMEDOWN, KEY_MUTE]);
    assert_eq!(named(&media), "keys");
    media.keys = Keys::of(&[KEY_POWER]);
    assert_eq!(named(&media), "keys");
    media.keys = Keys::of(&[KEY_KP1]);
    assert_eq!(named(&media), "keys");

    // A pen is a tablet's, with or without its tip's touch; a finger
    // alone is a touch device's, with or without a button under it.
    assert_eq!(named(&pointing(&[BTN_TOOL_PEN, BTN_TOUCH])), "tablet");
    assert_eq!(named(&pointing(&[BTN_TOOL_PEN])), "tablet");
    assert_eq!(named(&pointing(&[BTN_TOUCH])), "touch");
    assert_eq!(named(&pointing(&[BTN_TOUCH, BTN_LEFT])), "touch");
    // A place with nothing to press is none of them.
    assert_eq!(named(&pointing(&[])), "other");

    let mut lid = About::new(BUS_I8042, "Lid", "acpi");
    lid.switches = Few::of(&[SW_LID]);
    assert_eq!(named(&lid), "switch");
    assert_eq!(named(&About::new(BUS_USB, "Nothing", "usb-1-4")), "other");
    // Buttons alone, as a game controller's, are no keys.
    let mut buttons = About::new(BUS_USB, "Buttons", "usb-1-5");
    buttons.keys = Keys::of(&[0x130, 0x131, BTN_RIGHT]);
    assert_eq!(named(&buttons), "other");
}

#[test]
fn a_device_can_be_several_kinds_and_they_cross_a_wire_as_bits() {
    // A keyboard with a pointing stick in one interface.
    let mut both = mouse();
    both.keys.merge(&radix_input::set1_keys());
    let kinds = has_of(&both).kinds();
    assert_eq!(kinds.to_string(), "keyboard, pointer");
    assert!(kinds.is(Kinds::KEYBOARD) && kinds.is(Kinds::POINTER) && !kinds.is(Kinds::KEYS));
    assert!(!kinds.is(Kinds::OTHER) && !Kinds::OTHER.is(Kinds::OTHER));
    assert_eq!(kinds.bits(), Kinds::KEYBOARD.bits() | Kinds::POINTER.bits());
    assert_eq!(Kinds::from_bits(kinds.bits()), kinds);
    // A kind a later release adds is left out, not misread.
    assert_eq!(Kinds::from_bits(Kinds::TABLET.bits() | 1 << 20), Kinds::TABLET);
    assert_eq!(Kinds::from_bits(1 << 20).to_string(), "other");
    assert_eq!(Kinds::from_bits(63).names().count(), 6);
}

#[test]
fn a_record_of_inputd_is_sixteen_bytes_each_number_in_its_place() {
    let record = Record::device(0x0102_0304, Kinds::POINTER);
    let bytes = record.to_bytes();
    assert_eq!(bytes, [1, 0, 0, 0, 4, 3, 2, 1, 4, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(Record::from_bytes(&bytes), record);
    assert_eq!((record.what, record.kinds()), (DEVICE, Kinds::POINTER));
    assert_eq!(Record::holder(), Record { what: HOLDER, number: VERSION, kind: 0 });
    assert_eq!(Record::gone(9), Record { what: GONE, number: 9, kind: 0 });
    assert_eq!(Record::ready(2), Record { what: READY, number: 2, kind: 0 });
    assert_eq!(Record::refused(REFUSED_FULL).number, REFUSED_FULL);
    assert_eq!(Record::refused(REFUSED_FULL).what, REFUSED);
}

/// What inputd sends a program on a machine with a keyboard and a mouse,
/// and then as the mouse is pulled out and a tablet plugged in: each
/// record, and the descriptor that comes with a DEVICE.
fn sent() -> [(Record, Option<&'static str>); 5] {
    [
        (Record::device(1, Kinds::KEYBOARD), Some("keyboard")),
        (Record::device(2, Kinds::POINTER), Some("mouse")),
        (Record::ready(2), None),
        (Record::gone(2), None),
        (Record::device(3, Kinds::TABLET), Some("tablet")),
    ]
}

#[test]
fn what_inputd_sends_is_put_together_however_the_socket_cuts_it() {
    let records = sent();
    let bytes: Vec<u8> = records.iter().flat_map(|(record, _)| record.to_bytes()).collect();
    // A descriptor comes with the first byte of its record.
    let came_in = |bytes: core::ops::Range<usize>| {
        let first = records.iter().enumerate().map(|(at, (_, file))| (at * Record::SIZE, file));
        first.filter_map(move |(byte, file)| file.filter(|_| bytes.contains(&byte)))
    };
    // Every read size, from a byte at a time to all at once.
    for size in 1..=bytes.len() {
        let mut inbox = Inbox::new();
        let mut read = Vec::new();
        for (at, chunk) in bytes.chunks(size).enumerate() {
            inbox.take(chunk, came_in(at * size..at * size + chunk.len()));
            while let Some(next) = inbox.next() {
                read.push(next.unwrap());
            }
        }
        assert_eq!(read, records, "read {size} bytes at a time");
    }
}

#[test]
fn a_device_without_its_file_is_an_error_and_the_rest_is_still_read() {
    let mut inbox: Inbox<&str> = Inbox::default();
    let device = Record::device(4, Kinds::KEYS);
    inbox.take(&device.to_bytes(), []);
    inbox.take(&Record::gone(4).to_bytes()[..9], []);
    assert_eq!(inbox.next(), Some(Err(NoFile(device))));
    assert_eq!(NoFile(device).to_string(), "input device 4 came without its file");
    // The cut record waits for the rest of its bytes.
    assert_eq!(inbox.next(), None);
    inbox.take(&Record::gone(4).to_bytes()[9..], []);
    assert_eq!(inbox.next(), Some(Ok((Record::gone(4), None))));
    assert_eq!(inbox.next(), None);
}
