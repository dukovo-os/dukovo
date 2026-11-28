// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `linux-bench --only desktop`: the Linux desktop that `ob bench
//! --desktop` measures Dukovo's beside (ADR 0724), as process 1.
//!
//! The desktop is labwc, a stacking compositor that draws its windows'
//! frames itself, with wlroots' pixman renderer: both desktops draw on
//! the processor. `ob bench` puts it and what it needs in the initramfs
//! (Ubuntu's packages, pinned), with `/window-test`, the program Dukovo's
//! desktop runs too, built for Linux. Its `cost` mode shows one window
//! and goes through the steps the host times: it says each on the
//! standard output, which is the console, and the host sends the keys
//! and the mouse's motions.
//!
//! What a distribution's init and udev would have done first is done
//! here:
//!
//! - `/run`, `/tmp` and `/dev/shm` are tmpfs.
//! - libinput takes a device only if udev's database says what it is,
//!   and no udev runs. The database's files for the keyboards, the mice
//!   and the graphics cards are written as udev writes them
//!   ([`describe_devices`]).
//! - labwc runs as root on the kernel's console, where libseat's own
//!   backend opens the devices itself, with no seat daemon.
//!
//! labwc goes on when its program ends, so this program reads what the
//! two say, passes it on, and ends labwc once `window-test` has said
//! how it went.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The compositor, and the program it runs.
const LABWC: &str = "/usr/bin/labwc";
const PROGRAM: &str = "/window-test cost";

/// What `window-test` says last, passed or not.
const ENDED: [&str; 2] = ["window-test: passed", "window-test: FAILED"];

/// udev's database, a file for each device by its number.
const UDEV_DATA: &str = "/run/udev/data";

/// Where the programs of the desktop meet.
const RUNTIME: &str = "/run/user";

/// Where labwc's own messages go; shown if it fails.
const LABWC_LOG: &str = "/tmp/labwc.log";

/// How long the graphics card, a keyboard and a mouse may take to show
/// up after their modules load.
const DEVICES_WAIT: Duration = Duration::from_secs(10);

/// What a device is to libinput.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Keyboard,
    Mouse,
}

impl Kind {
    /// The kind of the input device the kernel names `name`: the virtual
    /// machine's are "QEMU Virtio Keyboard" and "QEMU Virtio Mouse".
    fn of(name: &str) -> Option<Kind> {
        let name = name.to_ascii_lowercase();
        if name.contains("keyboard") {
            Some(Kind::Keyboard)
        } else if name.contains("mouse") {
            Some(Kind::Mouse)
        } else {
            None
        }
    }

    /// udev's properties for a device of the kind, as its database has
    /// them.
    fn properties(self) -> &'static str {
        match self {
            Kind::Keyboard => "E:ID_INPUT=1\nE:ID_INPUT_KEY=1\nE:ID_INPUT_KEYBOARD=1\n",
            Kind::Mouse => "E:ID_INPUT=1\nE:ID_INPUT_MOUSE=1\n",
        }
    }
}

/// The folders of `class` in sysfs, a device each, whose names start
/// with `prefix`.
fn devices(class: &str, prefix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(class) else { return Vec::new() };
    entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| {
            path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(prefix))
        })
        .collect()
}

/// The file of udev's database for the character device at the sysfs
/// folder `device`, whose `dev` file has its number.
fn database_file(device: &Path) -> Option<PathBuf> {
    let number = std::fs::read_to_string(device.join("dev")).ok()?;
    Some(Path::new(UDEV_DATA).join(format!("c{}", number.trim())))
}

/// Writes udev's database for the machine's keyboards, mice and graphics
/// cards, and gives the kinds of the input devices it found.
fn describe_devices() -> Vec<Kind> {
    if let Err(err) = std::fs::create_dir_all(UDEV_DATA) {
        eprintln!("linux-bench: cannot make {UDEV_DATA}: {err}");
    }
    let write = |file: PathBuf, text: &str| {
        if let Err(err) = std::fs::write(&file, text) {
            eprintln!("linux-bench: cannot write {}: {err}", file.display());
        }
    };
    let mut kinds = Vec::new();
    for device in devices("/sys/class/input", "event") {
        let name = std::fs::read_to_string(device.join("device/name")).unwrap_or_default();
        if let (Some(kind), Some(file)) = (Kind::of(&name), database_file(&device)) {
            write(file, kind.properties());
            kinds.push(kind);
        }
    }
    // A card needs only to be known to udev.
    for file in devices("/sys/class/drm", "card").iter().filter_map(|card| database_file(card)) {
        write(file, "");
    }
    kinds
}

/// Whether the machine has a graphics card, a keyboard and a mouse yet.
fn devices_there() -> bool {
    let kinds: Vec<Kind> = devices("/sys/class/input", "event")
        .iter()
        .filter_map(|device| std::fs::read_to_string(device.join("device/name")).ok())
        .filter_map(|name| Kind::of(&name))
        .collect();
    Path::new("/dev/dri/card0").exists()
        && kinds.contains(&Kind::Keyboard)
        && kinds.contains(&Kind::Mouse)
}

/// Runs the desktop with `window-test cost` in it, to that program's
/// end. Says what could not be done; the host sees that the program
/// never passed.
pub fn run() {
    for at in ["/run", "/tmp", "/dev/shm"] {
        crate::mount("tmpfs", at);
    }
    let given = Instant::now();
    while !devices_there() && given.elapsed() < DEVICES_WAIT {
        std::thread::sleep(Duration::from_millis(50));
    }
    let kinds = describe_devices();
    if !(kinds.contains(&Kind::Keyboard) && kinds.contains(&Kind::Mouse)) {
        eprintln!("linux-bench: the machine has no virtio keyboard and mouse: found {kinds:?}");
    }
    {
        use std::os::unix::fs::PermissionsExt;
        let made = std::fs::create_dir_all(RUNTIME).and_then(|()| {
            std::fs::set_permissions(RUNTIME, std::fs::Permissions::from_mode(0o700))
        });
        if let Err(err) = made {
            eprintln!("linux-bench: cannot make {RUNTIME}: {err}");
        }
    }
    let log = match std::fs::File::create(LABWC_LOG) {
        Ok(log) => Stdio::from(log),
        Err(_) => Stdio::inherit(),
    };
    let started = Command::new(LABWC)
        .args(["-s", PROGRAM])
        .env_clear()
        .env("XDG_RUNTIME_DIR", RUNTIME)
        .env("HOME", "/root")
        .env("WLR_RENDERER", "pixman")
        .env("WLR_BACKENDS", "drm,libinput")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(log)
        .spawn();
    let mut labwc = match started {
        Ok(labwc) => labwc,
        Err(err) => {
            eprintln!("linux-bench: cannot run {LABWC}: {err}");
            return;
        }
    };
    // What labwc and the program in it say, a line at a time, until the
    // program has said how it went or labwc is gone.
    let mut passed_on = false;
    if let Some(said) = labwc.stdout.take() {
        for line in BufReader::new(said).lines().map_while(Result::ok) {
            println!("{line}");
            if ENDED.iter().any(|ended| line.starts_with(ended)) {
                passed_on = true;
                break;
            }
        }
    }
    if !passed_on {
        eprintln!("linux-bench: labwc ended before window-test said how it went:");
        let log = std::fs::read_to_string(LABWC_LOG).unwrap_or_default();
        let lines: Vec<&str> = log.lines().collect();
        for line in lines.iter().skip(lines.len().saturating_sub(20)) {
            eprintln!("linux-bench: labwc: {line}");
        }
    }
    let _ = labwc.kill();
    let _ = labwc.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_virtual_machine_s_keyboard_and_mouse_are_known_by_their_names() {
        assert_eq!(Kind::of("QEMU Virtio Keyboard"), Some(Kind::Keyboard));
        assert_eq!(Kind::of("QEMU Virtio Mouse"), Some(Kind::Mouse));
        assert_eq!(Kind::of("AT Translated Set 2 keyboard"), Some(Kind::Keyboard));
        assert_eq!(Kind::of("Power Button"), None);
        assert!(Kind::Keyboard.properties().contains("E:ID_INPUT_KEYBOARD=1\n"));
        assert!(Kind::Mouse.properties().ends_with("E:ID_INPUT_MOUSE=1\n"));
    }
}
