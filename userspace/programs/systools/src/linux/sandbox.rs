// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The sandbox of a Linux program (ADR 0622): carries out the plan of
//! `view.rs`, starts linuxd and the program, and waits for the program.
//! A Linux bundle's program (ADR 0625) runs in the same, with its bundle's
//! folder attached as it was checked, and is stopped as a bundle's is when
//! bundled says a permission it holds was taken away.
//!
//! It is a sibling of a bundle's sandbox (`bundles::run`), built the same
//! way: a view acting for the person, without the control right, attached
//! place by place; a process of its own, given the view as its root and
//! the terminal as descriptors 0 to 2. What differs is that linuxd runs
//! beside it, in the person's own root with no view: it holds the
//! connection to the kernel's `/devices/fuse` and the filesystem's root as
//! descriptors 3 and 4, and the receive end of the endpoint whose send end
//! is the program's SLOT_LINUX.
//!
//! The program waits for nothing of linuxd's before it starts, but the
//! view is built from linuxd's `proc` and `sys`, which linuxd answers, so
//! linuxd is started first. A run that stops here with "not_connected"
//! or "io" has a linuxd that ended at once: its own messages are on the
//! terminal.

use std::io::BufReader;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;

use super::view::View;
use crate::bundles::run::{Given, Sealed};

/// What the sandbox is built from.
#[cfg_attr(not(target_os = "radix"), allow(dead_code))]
pub struct Plan<'a> {
    /// What the program's view holds.
    pub view: &'a View,
    /// The Linux base, which the view's attaches of it open the parts of.
    pub base: Option<&'a Sealed>,
    /// The program, open.
    pub program: &'a OwnedFd,
    /// Its path, the same in the view.
    pub path: &'a str,
    pub argv: &'a [Vec<u8>],
    pub env: &'a [String],
    /// What linuxd's environment holds.
    pub daemon_env: &'a [String],
    /// The network it is given, and memory.jit.
    pub given: &'a Given,
    /// The bundle it is of, whose folder the view's attach of it is.
    pub bundle: Option<&'a Sealed>,
    /// What to do, the person is told, when the program ended badly after
    /// it was refused memory that is writable and executable.
    pub jit_hint: &'a str,
}

/// bundled's connection, followed while a bundle's program runs: `names`
/// are the permissions it was given of bundle `id`.
#[cfg_attr(not(target_os = "radix"), allow(dead_code))]
pub struct Follow {
    pub input: BufReader<UnixStream>,
    pub names: Vec<String>,
    pub id: String,
}

/// Built for another system, for tests: there is no sandbox to run in.
#[cfg(not(target_os = "radix"))]
pub fn run(_plan: &Plan<'_>, _follow: Option<Follow>) -> Result<i32, String> {
    Err("Linux programs run on Dukovo".into())
}

#[cfg(target_os = "radix")]
pub use radix::run;

#[cfg(target_os = "radix")]
mod radix {
    use std::fs::{self, File, OpenOptions};
    use std::os::fd::{AsRawFd, OwnedFd};
    use std::os::unix::process::ExitStatusExt;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use radix_abi::capability::{self, ObjectKind, Rights};
    use radix_abi::directory::{self, OpenFlags};
    use radix_abi::{jit, process, startup, syscalls};
    use sysapi::process as process_calls;
    use sysapi::signal;

    use super::{Follow, Plan};
    use crate::bundles::folder::Dir;
    use crate::bundles::run::Sealed;
    use crate::bundles::run::sandbox::{
        Followed, GIVEN, READ_ONLY, failed, give_jit, give_network,
    };
    use crate::linux::supervise::{self, Restarts};
    use crate::linux::view::{Attach, Source};
    use crate::signin::{self, Spares};
    use crate::sys::{self, SYS};
    use crate::util::strip_os_error;

    /// linuxd's program.
    const LINUXD: &str = "/system/commands/linuxd";
    /// The kernel's FUSE connections: each open of it is a new one.
    const FUSE: &str = "/devices/fuse";
    /// How long linuxd has to end after SIGTERM, before SIGKILL.
    const STOP_TIME: Duration = Duration::from_secs(2);
    /// How often it is looked at while it ends.
    const STOP_POLL: Duration = Duration::from_millis(20);

    /// The program's end of the endpoint: to send, and to pass the call's
    /// own capability, and to copy and pass on, as `fork` needs to give a
    /// forked process the same; never to receive.
    const PROGRAM_ENDPOINT: Rights = Rights(
        Rights::SEND.bits() | Rights::GRANT.bits() | Rights::COPY.bits() | Rights::TRANSFER.bits(),
    );

    /// Slots for the work, taken before anything is opened beneath them.
    struct Slots {
        /// The person's root, without control or the bypass right.
        base: u32,
        view: u32,
        given: u32,
        scratch: u32,
        /// The program's process.
        child: u32,
        network: u32,
        /// The endpoint, with every right: kept to share from, again and
        /// again, as linuxd is started.
        endpoint: u32,
        /// The root of the filesystem linuxd serves.
        root: u32,
        /// linuxd's process.
        daemon: u32,
    }

    /// What starting a process needs, held for the whole run.
    struct Ctx<'a> {
        plan: &'a Plan<'a>,
        s: Slots,
        /// The slot of the `/devices/fuse` connection.
        connection: u32,
        /// The slot of linuxd's program, open.
        server: u32,
        fallback: Fallback,
    }

    /// The info word of a reply with one word and no capabilities.
    const ANSWER_INFO: u64 = 1 << 32;

    /// What answers the calls the program forwards once linuxd is given
    /// up: with ENOSYS, as the program would be answered without linuxd.
    /// The endpoint is not closed while the program holds its send end, so
    /// without an answer a call would wait in it for good.
    ///
    /// Its thread is started before the spare slots are taken, as a
    /// thread may take a slot of its own, and waits for the endpoint's.
    struct Fallback {
        begin: Option<mpsc::Sender<u32>>,
    }

    impl Fallback {
        fn new() -> Fallback {
            let (begin, hear) = mpsc::channel();
            let started = std::thread::Builder::new().name("enosys".into()).spawn(move || {
                if let Ok(endpoint) = hear.recv() {
                    refuse_calls(endpoint);
                }
            });
            Fallback { begin: started.is_ok().then_some(begin) }
        }

        /// Answers the calls received on the endpoint in `slot` from now on.
        fn begin(&self, slot: u32) {
            if let Some(begin) = &self.begin {
                let _ = begin.send(slot);
            }
        }
    }

    /// Receives each call on the endpoint in `slot` and answers it with
    /// ENOSYS, until the endpoint is gone.
    fn refuse_calls(slot: u32) {
        let mut owed = false;
        loop {
            let mut words = [0u64; 8];
            words[0] = (-i64::from(libc::ENOSYS)) as u64;
            let received = if owed {
                syscalls::reply_recv(&SYS, slot, ANSWER_INFO, words).map(drop)
            } else {
                syscalls::recv(&SYS, slot, 0).map(drop)
            };
            owed = received.is_ok();
            // An interrupted wait is waited again; any other failure is the
            // endpoint's end.
            if let Err(status) = received
                && sys::errno_of(status) != libc::EINTR
            {
                return;
            }
        }
    }

    /// The linuxd that runs now, ended with the run.
    #[derive(Default)]
    struct Daemon {
        pid: Option<libc::pid_t>,
    }

    impl Daemon {
        /// SIGTERM, and SIGKILL if it has not ended `STOP_TIME` later.
        fn stop(&mut self) {
            let Some(pid) = self.pid.take() else { return };
            // linuxd may have ended already, which `gone` then sees.
            let _ = process_calls::kill(pid, libc::SIGTERM);
            let mut waited = Duration::ZERO;
            while waited < STOP_TIME {
                if gone(pid) {
                    return;
                }
                std::thread::sleep(STOP_POLL);
                waited += STOP_POLL;
            }
            // As above; and the wait fails only when it has been reaped.
            let _ = process_calls::kill(pid, libc::SIGKILL);
            let _ = process_calls::wait(pid);
        }
    }

    impl Drop for Daemon {
        fn drop(&mut self) {
            self.stop();
        }
    }

    /// Whether process `pid`, a child, has ended, which this takes note of.
    fn gone(pid: libc::pid_t) -> bool {
        !matches!(process_calls::try_wait(pid), Ok(None))
    }

    /// Builds the view, starts linuxd and the program in it, and waits for
    /// the program, with the program's exit status.
    pub fn run(plan: &Plan<'_>, follow: Option<Follow>) -> Result<i32, String> {
        // Everything that opens a descriptor comes before the spare slots:
        // an emptied slot is a free descriptor to the C library, and the
        // next open would take one of them.
        let open_failed = |what: &str, e: std::io::Error| format!("{what}: {}", strip_os_error(&e));
        let connection = OpenOptions::new()
            .read(true)
            .write(true)
            .open(FUSE)
            .map_err(|e| open_failed(FUSE, e))?;
        let server = File::open(LINUXD).map_err(|e| open_failed(LINUXD, e))?;
        let parts = base_parts(plan)?;
        let fallback = Fallback::new();
        let spares = Spares::new(9).map_err(|e| format!("no slots: {e}"))?;
        let [base, view, given, scratch, child, network, endpoint, root, daemon] =
            [0, 1, 2, 3, 4, 5, 6, 7, 8].map(|i| spares.slot(i));
        let ctx = Ctx {
            plan,
            s: Slots { base, view, given, scratch, child, network, endpoint, root, daemon },
            connection: sys::slot_of(connection.as_raw_fd()),
            server: sys::slot_of(server.as_raw_fd()),
            fallback,
        };
        let mut linuxd = Daemon::default();
        let program = start(&ctx, &parts, &mut linuxd)?;
        let followed = follow.map(|f| Followed::start(Some(f.input), program, f.names, f.id));
        let code = wait_for(&ctx, &mut linuxd, program);
        if let Some(followed) = followed {
            followed.ended();
        }
        linuxd.stop();
        if code.as_ref().is_ok_and(|&code| code != 0)
            && plan.given.jit.is_none()
            && refused_jit(ctx.s.child)
        {
            eprintln!(
                "linux: the program asked for memory that is writable and executable, as a JIT \
                 compiler does: {}",
                plan.jit_hint
            );
        }
        code
    }

    /// Whether the program, its process in `child`, was refused memory
    /// that is writable and executable for want of memory.jit (ADR 0624),
    /// as the jit capability the session holds tells.
    fn refused_jit(child: u32) -> bool {
        let held = capability::info(&SYS, startup::SLOT_JIT);
        held.is_ok_and(|info| info.kind == ObjectKind::Jit)
            && jit::state(&SYS, startup::SLOT_JIT, child).is_ok_and(|state| state.refused)
    }

    /// The pieces of the base the view attaches, open, in the order of its
    /// attaches: `None` for the attaches that are not of the base.
    fn base_parts(plan: &Plan<'_>) -> Result<Vec<Option<OwnedFd>>, String> {
        plan.view
            .attaches
            .iter()
            .map(|attach| match (&attach.source, plan.base) {
                (Source::Base { path, folder }, Some(sealed)) => {
                    open_part(sealed, path, *folder).map(Some)
                }
                (Source::Base { .. }, None) => Err("the view names a base that is not open".into()),
                _ => Ok(None),
            })
            .collect()
    }

    /// Opens `path` of the sealed base, one part at a time, never through
    /// a link.
    fn open_part(sealed: &Sealed, path: &str, folder: bool) -> Result<OwnedFd, String> {
        let shown = format!("{}/{path}", sealed.path);
        let mut parts: Vec<&str> = path.split('/').collect();
        let name = parts.pop().ok_or_else(|| format!("{shown}: no name"))?;
        let mut dirs: Vec<Dir> = Vec::new();
        for part in parts {
            let at = dirs.last().unwrap_or(&sealed.folder);
            let next = at.dir_at(part).map_err(|e| format!("{shown}: {}", strip_os_error(&e)))?;
            dirs.push(next);
        }
        let at = dirs.last().unwrap_or(&sealed.folder);
        at.open_at(name, folder).map_err(|e| format!("{shown}: {}", strip_os_error(&e)))
    }

    /// Makes the endpoint and the filesystem, starts linuxd, builds the
    /// view and starts the program in it: its process number.
    fn start(ctx: &Ctx<'_>, parts: &[Option<OwnedFd>], linuxd: &mut Daemon) -> Result<u64, String> {
        let s = &ctx.s;
        // Without the bypass right or control, whatever this program's
        // root has: the view acts for whoever runs it, as the owners and
        // modes of its files let them. linuxd has the same root.
        let rights =
            capability::info(&SYS, startup::SLOT_ROOT).map_err(failed("the root".into()))?.rights;
        let narrowed = Rights(rights.bits() & GIVEN.bits());
        capability::derive(&SYS, startup::SLOT_ROOT, s.base, narrowed, 0)
            .map_err(failed("narrowing the root".into()))?;
        process::create(&SYS, startup::SLOT_PROCESS, ObjectKind::Endpoint, 0, s.endpoint)
            .map_err(failed("making the endpoint".into()))?;
        directory::serve_fuse(&SYS, startup::SLOT_ROOT, ctx.connection, s.root)
            .map_err(failed("serving the filesystem".into()))?;
        linuxd.pid = Some(start_linuxd(ctx, false)?);
        build_and_start(ctx, parts)
    }

    /// Starts linuxd, as `serve --again` if it is a restart: its process
    /// ID.
    fn start_linuxd(ctx: &Ctx<'_>, again: bool) -> Result<libc::pid_t, String> {
        let s = &ctx.s;
        // The last one's process, ended and waited for, makes room.
        let _ = capability::delete(&SYS, s.daemon);
        process::create(&SYS, startup::SLOT_PROCESS, ObjectKind::Process, signin::SLOTS, s.daemon)
            .map_err(failed("making linuxd's process".into()))?;
        // As the person, in their own root, with the connection and the
        // filesystem's root where linuxd looks for them (descriptors 3 and
        // 4), and the endpoint to receive on.
        let mut shares = vec![
            (s.base, startup::SLOT_ROOT, Rights::ALL),
            (startup::SLOT_RANDOM, startup::SLOT_RANDOM, Rights::ALL),
            (startup::SLOT_LOG, startup::SLOT_LOG, Rights::ALL),
            (ctx.connection, startup::SLOT_FD_BASE + 3, Rights::ALL),
            (s.root, startup::SLOT_FD_BASE + 4, Rights::ALL),
            (s.endpoint, startup::SLOT_LINUX, Rights::RECEIVE),
        ];
        shares.extend(terminal_slots().map(|(from, to)| (from, to, Rights::ALL)));
        for (from, to, rights) in shares {
            process::share(&SYS, s.daemon, from, to, rights, 0)
                .map_err(failed("giving linuxd its capabilities".into()))?;
        }
        let mut args: Vec<&[u8]> = vec![b"linuxd", b"serve"];
        if again {
            args.push(b"--again");
        }
        let block = signin::exec_block_in(b"/", LINUXD.as_bytes(), &args, ctx.plan.daemon_env);
        process::exec(&SYS, s.daemon, ctx.server, &block)
            .map_err(failed("starting linuxd".into()))?;
        let number =
            process::info(&SYS, s.daemon).map_err(failed("linuxd's number".into()))?.number;
        let pid = libc::pid_t::try_from(number).map_err(|_| "linuxd's number".to_string())?;
        // In a group of its own, so that the terminal's Ctrl+C, which goes
        // to the foreground group, does not end it with the program. It
        // may not be allowed, and nothing is lost: linuxd is started again.
        let _ = process_calls::set_process_group(pid, pid);
        Ok(pid)
    }

    /// The descriptors 0 to 2 that are there, as slots, each with the slot
    /// it has in a process started from this one.
    fn terminal_slots() -> impl Iterator<Item = (u32, u32)> {
        (0..3).filter_map(|fd| {
            let slot = sys::slot_of(fd);
            capability::info(&SYS, slot).ok().map(|_| (slot, startup::SLOT_FD_BASE + fd as u32))
        })
    }

    fn build_and_start(ctx: &Ctx<'_>, parts: &[Option<OwnedFd>]) -> Result<u64, String> {
        let (plan, s) = (ctx.plan, &ctx.s);
        directory::make_view(&SYS, s.base, s.view).map_err(failed("making its view".into()))?;
        for (attach, part) in plan.view.attaches.iter().zip(parts) {
            attach_one(ctx, attach, part.as_ref())?;
        }
        for link in &plan.view.links {
            let at = link.at.trim_start_matches('/');
            directory::attach_link(&SYS, s.view, at.as_bytes(), link.target.as_bytes())
                .map_err(failed(format!("linking {}", link.at)))?;
        }
        // Without control: nothing more is attached to it.
        capability::derive(&SYS, s.view, s.given, GIVEN, 0)
            .map_err(failed("narrowing its view".into()))?;

        process::create(&SYS, startup::SLOT_PROCESS, ObjectKind::Process, signin::SLOTS, s.child)
            .map_err(failed("making its process".into()))?;
        let mut shares = vec![
            (s.given, startup::SLOT_ROOT, Rights::ALL),
            (startup::SLOT_RANDOM, startup::SLOT_RANDOM, Rights::ALL),
            (startup::SLOT_LOG, startup::SLOT_LOG, Rights::ALL),
            (s.endpoint, startup::SLOT_LINUX, PROGRAM_ENDPOINT),
        ];
        shares.extend(terminal_slots().map(|(from, to)| (from, to, Rights::ALL)));
        for (from, to, rights) in shares {
            process::share(&SYS, s.child, from, to, rights, 0)
                .map_err(failed("giving it its capabilities".into()))?;
        }
        give_network(plan.given, s.child, s.network)?;
        if let Some(required) = plan.given.jit {
            give_jit(s.child, required)?;
        }
        let argv: Vec<&[u8]> = plan.argv.iter().map(Vec::as_slice).collect();
        let block = signin::exec_block_in(
            plan.view.working.as_bytes(),
            plan.path.as_bytes(),
            &argv,
            plan.env,
        );
        let program = sys::slot_of(plan.program.as_raw_fd());
        process::exec(&SYS, s.child, program, &block).map_err(failed("starting it".into()))?;
        let number = process::info(&SYS, s.child).map_err(failed("its number".into()))?.number;
        Ok(number)
    }

    /// Attaches one place of the plan to the view, in `s.scratch`'s
    /// slot on the way. `part` is the open piece of the base, if it is one.
    fn attach_one(ctx: &Ctx<'_>, attach: &Attach, part: Option<&OwnedFd>) -> Result<(), String> {
        let s = &ctx.s;
        let at = attach.at.trim_start_matches('/');
        let rights = if attach.write { GIVEN } else { READ_ONLY };
        let folder = OpenFlags(OpenFlags::DIRECTORY.bits() | OpenFlags::NO_FOLLOW.bits());
        let opened = match &attach.source {
            Source::Base { .. } => {
                let part = part.ok_or_else(|| format!("{}: not open", attach.at))?;
                capability::derive(&SYS, sys::slot_of(part.as_raw_fd()), s.scratch, READ_ONLY, 0)
            }
            // The very folder checked, not one opened again by its path.
            Source::Bundle => {
                let sealed = ctx.plan.bundle.ok_or_else(|| format!("{}: not open", attach.at))?;
                let slot = sysapi::kernel::slot_of(&sealed.folder);
                capability::derive(&SYS, slot, s.scratch, READ_ONLY, 0)
            }
            Source::Person(path) => {
                let is_dir = fs::symlink_metadata(path)
                    .map_err(|e| format!("{path}: {}", strip_os_error(&e)))?
                    .is_dir();
                let flags = if is_dir { folder } else { OpenFlags(OpenFlags::NO_FOLLOW.bits()) };
                let from = path.trim_start_matches('/');
                directory::open(&SYS, s.base, from.as_bytes(), flags, rights, s.scratch, 0)
                    .map(drop)
            }
            Source::Linuxd(name) => {
                directory::open(&SYS, s.root, name.as_bytes(), folder, rights, s.scratch, 0)
                    .map(drop)
            }
            Source::Memory => directory::memory_filesystem(&SYS, startup::SLOT_ROOT, s.scratch),
            Source::Names { folder: path, .. } => {
                let from = path.trim_start_matches('/');
                directory::open(&SYS, s.base, from.as_bytes(), folder, rights, s.scratch, 0)
                    .map(drop)
            }
        };
        opened.map_err(failed(format!("opening what {} leads to", attach.at)))?;
        let attached = match &attach.source {
            Source::Names { names, .. } => {
                let mut list = Vec::new();
                for name in names {
                    list.extend_from_slice(name.as_bytes());
                    list.push(0);
                }
                directory::attach_names(&SYS, s.view, at.as_bytes(), s.scratch, &list)
            }
            _ => directory::attach(&SYS, s.view, at.as_bytes(), s.scratch),
        };
        let _ = capability::delete(&SYS, s.scratch);
        attached.map_err(failed(format!("attaching {}", attach.at)))
    }

    /// Waits for the program `number`, and starts linuxd again when it
    /// ends, at most `MOST` times in a minute; relays a stop of the
    /// program, as a shell asks for. The program's exit status.
    fn wait_for(ctx: &Ctx<'_>, linuxd: &mut Daemon, number: u64) -> Result<i32, String> {
        let program = libc::pid_t::try_from(number).map_err(|_| "its number".to_string())?;
        // As system(3): the terminal's Ctrl+C is the program's to take.
        let old = [libc::SIGINT, libc::SIGQUIT].map(signal::save);
        for number in [libc::SIGINT, libc::SIGQUIT] {
            // sigaction fails only for a number that is no signal.
            let _ = signal::ignore(number);
        }
        let begun = Instant::now();
        let mut restarts = Restarts::default();
        let code = loop {
            let (pid, status) = match wait_any() {
                Ok(done) => done,
                Err(why) => {
                    // The program may have ended already.
                    let _ = process_calls::kill(program, libc::SIGKILL);
                    break Err(why);
                }
            };
            if pid == program {
                if let Some(code) = supervise::exit_status(status) {
                    break Ok(code);
                }
                if supervise::stopped(status) {
                    relay_stop(number, program);
                }
            } else if Some(pid) == linuxd.pid {
                if supervise::stopped(status) {
                    // linuxd may have ended since, which the next wait sees.
                    let _ = process_calls::kill(pid, libc::SIGCONT);
                } else if supervise::exit_status(status).is_some() {
                    linuxd.pid = None;
                    restart(ctx, linuxd, &mut restarts, begun.elapsed());
                }
            }
        };
        for saved in old.into_iter().flatten() {
            // sigaction fails only for a number that is no signal.
            let _ = saved.restore();
        }
        code
    }

    /// Starts linuxd again if it may be, and says what it does.
    fn restart(ctx: &Ctx<'_>, linuxd: &mut Daemon, restarts: &mut Restarts, now: Duration) {
        if !restarts.allow(now) {
            eprintln!(
                "linux: linuxd ended {} times in a minute; the program goes on without it, and \
                 what it asks of linuxd fails",
                supervise::MOST + 1
            );
            ctx.fallback.begin(ctx.s.endpoint);
            return;
        }
        eprintln!("linux: linuxd ended; starting it again");
        match start_linuxd(ctx, true) {
            Ok(pid) => linuxd.pid = Some(pid),
            Err(why) => {
                eprintln!("linux: {why}; the program goes on without linuxd");
                ctx.fallback.begin(ctx.s.endpoint);
            }
        }
    }

    /// The next process of ours that ended or stopped, and its status.
    fn wait_any() -> Result<(libc::pid_t, i32), String> {
        loop {
            match process_calls::wait_with(-1, libc::WUNTRACED) {
                Ok(Some((pid, status))) => return Ok((pid, status.into_raw())),
                Ok(None) => {}
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(format!("waiting for it: {}", strip_os_error(&error))),
            }
        }
    }

    /// The program stopped: `linux` stops too, so that the shell that ran
    /// it has its terminal back, and has the program go on when it does.
    /// A stop that the terminal's Ctrl+Z gave `linux` as well is reported
    /// after `linux` has gone on, when the program is running again, and is
    /// not a stop to relay.
    fn relay_stop(number: u64, program: libc::pid_t) {
        let stopped = crate::proc::process(number).map_or(true, |process| process.run == "stopped");
        if !stopped {
            return;
        }
        // A stop of this process cannot fail, and the program may have
        // ended meanwhile, which the next wait sees.
        let _ = signal::raise(libc::SIGSTOP);
        let _ = process_calls::kill(program, libc::SIGCONT);
    }
}
