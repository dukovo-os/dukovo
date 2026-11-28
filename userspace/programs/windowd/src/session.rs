// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A desktop from its start to its end: what windowd waits on, and what
//! it does when each speaks.
//!
//! windowd is one thread that waits in poll(2) for the display layer,
//! consoled, inputd, each device, each program and the next frame's
//! time. Nothing it does between two waits waits itself.

use std::fs::File;
use std::io::{self, Read};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitCode};
use std::time::Duration;

use display::abi::DisplayEvent;
use sysapi::poll::{PollFd, poll};
use sysapi::signal::Caught;
use sysapi::{process, time};

use crate::borrowed::Borrowed;
use crate::desktop::Desktop;
use crate::devices::Devices;
use crate::keys::Chord;
use crate::link::Card;
use crate::settings::{self, Settings};
use crate::socket::Listening;

/// SIGTERM, SIGHUP or SIGINT came: the desktop is to end.
pub static END: Caught = Caught::new();

/// SIGCHLD came: the command may have ended.
pub static CHILD: Caught = Caught::new();

/// SIGUSR1 came: the settings files changed, and are to be read again
/// (windowd.json(5)).
pub static RELOAD: Caught = Caught::new();

/// How many of the layer's events are read at once.
const EVENTS: usize = 32;

/// The exit status of a command a signal ended, as the shell gives it.
const SIGNALLED: u8 = 128;

/// What a wait ended for.
#[derive(Debug, Default)]
struct Woke {
    signal: bool,
    layer: bool,
    consoled: bool,
    inputd: bool,
    program_came: bool,
    /// The devices that wrote, as places in [`Devices::waited_on`].
    devices: Vec<usize>,
    /// The programs that wrote, as places in [`Desktop::waited_on`].
    programs: Vec<usize>,
}

/// A desktop that runs.
pub struct Session {
    /// The desktop.
    pub desktop: Desktop<Card>,
    /// The keyboards and pointers.
    pub devices: Devices,
    /// The loan of the screens.
    pub borrowed: Borrowed,
    /// The socket the programs connect to.
    pub listening: Listening,
    /// The command the desktop was started with, while it runs.
    pub command: Option<Child>,
    /// The read end of the pipe the signals wake the wait through.
    pub woken: File,
    /// Whether inputd is still there to say which devices come and go.
    pub inputd_there: bool,
    /// The person's settings file, if their home folder is known.
    pub personal: Option<PathBuf>,
}

impl Session {
    /// Runs the desktop to its end. Returns the exit status: the
    /// command's if the desktop ended because it did.
    ///
    /// # Errors
    ///
    /// Why the desktop cannot go on, for the person to read.
    pub fn run(&mut self) -> Result<ExitCode, String> {
        self.lay_settings()?;
        loop {
            for said in self.desktop.said() {
                eprintln!("desktop: {said}");
            }
            if let Some(lights) = self.desktop.lights() {
                self.devices.light(lights);
            }
            self.keep_choices();
            match self.desktop.asked() {
                Some(Chord::End) => return Ok(ExitCode::SUCCESS),
                Some(Chord::Console(console)) => self.leave_for(console)?,
                None => {}
            }
            let now = time::monotonic_ns();
            if self.desktop.due(now).is_some_and(|due| due <= now) {
                self.desktop.draw(now).map_err(|err| err.to_string())?;
                continue;
            }
            let until = self.desktop.due(now).map(|due| Duration::from_nanos(due - now));
            let woke = self.wait(until).map_err(|err| format!("poll: {err}"))?;
            if let Some(ended) = self.hear(woke)? {
                return Ok(ended);
            }
        }
    }

    /// Reads the settings files and has the desktop be as they say.
    /// What is wrong in them is said on the standard error.
    fn lay_settings(&mut self) -> Result<(), String> {
        let files = [Some(Path::new(settings::SYSTEM)), self.personal.as_deref()];
        let (settings, wrong) = Settings::read(files.into_iter().flatten());
        for wrong in wrong {
            eprintln!("desktop: {wrong}");
        }
        self.desktop.set(&settings).map_err(|err| err.to_string())
    }

    /// Keeps what the person chose from a window's menu in their
    /// settings file. A choice that cannot be kept holds until the
    /// settings are next read.
    fn keep_choices(&mut self) {
        for (app, lets) in self.desktop.chosen() {
            let kept = match &self.personal {
                Some(file) => settings::keep_app(file, &app, lets),
                None => Err("HOME is not set".to_owned()),
            };
            if let Err(why) = kept {
                eprintln!("desktop: the choice for {app} is not kept: {why}");
            }
        }
    }

    /// Gives the screens back so that `console` shows.
    fn leave_for(&mut self, console: u32) -> Result<(), String> {
        let leaves = self.borrowed.leaves_for(console);
        if leaves.map_err(|err| format!("consoled: {err}"))? {
            // A desktop that is not in front has nothing to give back.
            let _ = self.desktop.holder().give_back();
        }
        Ok(())
    }

    /// Waits until something windowd waits on speaks, or `until` passes.
    fn wait(&self, until: Option<Duration>) -> io::Result<Woke> {
        let heard = libc::POLLIN;
        let inputd = self.inputd_there.then(|| self.devices.inputd());
        let mut fds = vec![
            PollFd::new(self.woken.as_fd(), heard),
            PollFd::new(self.desktop.holder().file().as_fd(), heard),
            PollFd::optional(self.borrowed.consoled(), heard),
            PollFd::optional(inputd, heard),
            PollFd::new(self.listening.listener.as_fd(), heard),
        ];
        let fixed = fds.len();
        fds.extend(self.devices.waited_on().map(|fd| PollFd::new(fd, heard)));
        let devices = fds.len() - fixed;
        fds.extend(self.desktop.waited_on().map(|fd| PollFd::new(fd, heard)));
        match poll(&mut fds, until) {
            // A signal's byte is in the pipe, which the next wait finds.
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Ok(Woke::default()),
            polled => polled?,
        };
        // A closed end and an error are found by reading, as bytes are.
        let spoke = |fd: &PollFd<'_>| fd.revents() != 0;
        let places = |fds: &[PollFd<'_>]| {
            let spoke = fds.iter().enumerate().filter(|(_, fd)| spoke(fd));
            spoke.map(|(at, _)| at).collect::<Vec<usize>>()
        };
        Ok(Woke {
            signal: spoke(&fds[0]),
            layer: spoke(&fds[1]),
            consoled: spoke(&fds[2]),
            inputd: spoke(&fds[3]),
            program_came: spoke(&fds[4]),
            devices: places(&fds[fixed..fixed + devices]),
            programs: places(&fds[fixed + devices..]),
        })
    }

    /// Hears what `woke` says spoke. Returns the exit status if the
    /// desktop is to end.
    fn hear(&mut self, woke: Woke) -> Result<Option<ExitCode>, String> {
        if woke.signal {
            // The bytes only woke the wait: the flags say which came.
            let _ = (&self.woken).read(&mut [0u8; 64]);
        }
        if END.take() {
            return Ok(Some(ExitCode::SUCCESS));
        }
        if CHILD.take()
            && let Some(ended) = self.command_ended()
        {
            return Ok(Some(ended));
        }
        if RELOAD.take() {
            self.lay_settings()?;
        }
        if woke.layer {
            self.hear_layer()?;
        }
        if woke.consoled {
            self.borrowed.consoled_ended();
        }
        if woke.inputd
            && let Err(error) = self.devices.inputd_wrote()
        {
            // The devices it handed out are heard on. Those that come
            // from here on are not (docs/deferred.md).
            eprintln!("desktop: inputd: {error}");
            self.inputd_there = false;
        }
        if woke.program_came {
            self.take_in();
        }
        self.devices.devices_wrote(woke.devices, &mut self.desktop);
        self.desktop.programs_wrote(woke.programs);
        Ok(None)
    }

    /// The exit status the desktop ends with if the command ended.
    fn command_ended(&mut self) -> Option<ExitCode> {
        let status = self.command.as_mut()?.try_wait().ok()??;
        self.command = None;
        let signalled = || {
            use std::os::unix::process::ExitStatusExt;
            status.signal().map_or(1, |signal| SIGNALLED.saturating_add(signal as u8))
        };
        Some(ExitCode::from(status.code().map_or_else(signalled, |code| code as u8)))
    }

    /// Tells the desktop the events of the display layer that wait.
    fn hear_layer(&mut self) -> Result<(), String> {
        let mut bytes = [0u8; EVENTS * DisplayEvent::SIZE];
        loop {
            let read = self.desktop.holder().file().events(&mut bytes);
            let read = read.map_err(|err| format!("the display layer: {err}"))?;
            for event in display::wire::events(&bytes[..read]) {
                self.desktop.layer_said(&event).map_err(|err| err.to_string())?;
            }
            if read < bytes.len() {
                return Ok(());
            }
        }
    }

    /// Takes in the programs that connected.
    fn take_in(&mut self) {
        while let Ok((stream, _)) = self.listening.listener.accept() {
            if let Err(error) = self.desktop.take_in(stream) {
                eprintln!("desktop: a program that connected was not taken in: {error}");
            }
        }
    }

    /// Ends the desktop: gives the screens back, and asks the command,
    /// if it still runs, to end.
    pub fn end(&mut self) {
        // In the back there is nothing to give back.
        let _ = self.desktop.holder().give_back();
        if let Some(command) = self.command.take() {
            // A command that ended since is not there to be asked.
            let _ = process::kill(command.id() as sysapi::Pid, libc::SIGTERM);
        }
    }
}
