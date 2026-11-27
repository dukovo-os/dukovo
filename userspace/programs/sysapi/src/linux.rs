// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What only Linux has, for the lab's programs that run as process 1 of a
//! small Linux system: the Linux side of the benchmarks
//! (`tools/linux-bench`) and the disk writer (`tools/lab-writer`). They
//! mount what they need, load the kernel modules of the machine's disks
//! and cards, set a card's address, pin threads to CPUs, and stop the
//! machine when they are done. The calls the benchmarks time are inlined
//! into them, so that each costs what the system call costs, as in C.

use std::ffi::CStr;
use std::io;
use std::net::Ipv4Addr;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::sync::atomic::AtomicU32;

use crate::{c_path, c_string, checked};

/// finit_module(2)'s flag for a module file the kernel decompresses.
const MODULE_INIT_COMPRESSED_FILE: libc::c_uint = 4;

/// The ioctls of a block device that drop its cached pages and zero a
/// range of it (linux/fs.h).
const BLKFLSBUF: libc::c_ulong = 0x1261;
const BLKZEROOUT: libc::c_ulong = 0x127f;

/// The ioctl that sets a file's inode flags, as chattr(1) does
/// (linux/fs.h).
const FS_IOC_SETFLAGS: libc::c_ulong = 0x4008_6602;

/// How [`end`] stops the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// The machine is switched off.
    PowerOff,
    /// The machine starts again.
    Restart,
}

/// Stops the machine, as process 1 does once it is done; what it wrote is
/// to be on the disks first ([`sync`]). It returns only when it cannot,
/// with the reason.
pub fn end(ending: Ending) -> io::Error {
    let command = match ending {
        Ending::PowerOff => libc::LINUX_REBOOT_CMD_POWER_OFF,
        Ending::Restart => libc::LINUX_REBOOT_CMD_RESTART,
    };
    // SAFETY: reboot takes a number and reads no memory.
    unsafe { libc::reboot(command) };
    io::Error::last_os_error()
}

/// Has every filesystem write what it holds to its disk (sync(2)).
pub fn sync() {
    // SAFETY: sync takes nothing and cannot fail.
    unsafe { libc::sync() };
}

/// Mounts `source` on `at` as a filesystem of kind `kind`, with the
/// `MS_*` flags `flags` and the options `options` (none when empty).
///
/// # Errors
///
/// EPERM without the right to mount, ENOENT for a path that is not there,
/// and what the filesystem refuses.
pub fn mount(
    source: &str,
    at: &Path,
    kind: &str,
    flags: libc::c_ulong,
    options: &str,
) -> io::Result<()> {
    let (source, at, kind) =
        (c_string(source.as_bytes())?, c_path(at)?, c_string(kind.as_bytes())?);
    let options = (!options.is_empty()).then(|| c_string(options.as_bytes())).transpose()?;
    let data = options.as_ref().map_or(std::ptr::null(), |options| options.as_ptr().cast());
    // SAFETY: the strings are C strings, and the data a C string or null,
    // which the call only reads.
    checked(unsafe { libc::mount(source.as_ptr(), at.as_ptr(), kind.as_ptr(), flags, data) })?;
    Ok(())
}

/// Unmounts the filesystem on `at`.
///
/// # Errors
///
/// EINVAL when nothing is mounted there, EBUSY while it is in use.
pub fn unmount(at: &Path) -> io::Result<()> {
    let at = c_path(at)?;
    // SAFETY: the path is a C string, which the call only reads.
    checked(unsafe { libc::umount(at.as_ptr()) })?;
    Ok(())
}

/// Loads the kernel module in `file` (finit_module(2)), with no
/// parameters; `compressed` when the kernel is to decompress it.
///
/// # Errors
///
/// EEXIST when it is loaded or built in already, ENODEV when the machine
/// has none of what it drives, EPERM without the right to load modules.
pub fn load_module(file: impl AsFd, compressed: bool) -> io::Result<()> {
    let flags = if compressed { MODULE_INIT_COMPRESSED_FILE } else { 0 };
    // SAFETY: the descriptor is borrowed for the call, and the parameters
    // are an empty C string that lives through it.
    let result = unsafe {
        libc::syscall(libc::SYS_finit_module, file.as_fd().as_raw_fd(), c"".as_ptr(), flags)
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// A new eventfd(2) counter at 0, closed on exec.
///
/// # Errors
///
/// EMFILE when the process can open no more descriptors.
pub fn event_counter() -> io::Result<OwnedFd> {
    // SAFETY: eventfd takes numbers.
    let fd = checked(unsafe { libc::eventfd(0, libc::EFD_CLOEXEC) })?;
    // SAFETY: the descriptor is new, and nothing else holds it.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Waits while `word` holds `expected`, until a [`futex_wake`] on it or a
/// signal (FUTEX_WAIT of the process's own futexes); returns at once when
/// it holds something else.
#[inline]
pub fn futex_wait(word: &AtomicU32, expected: u32) {
    // SAFETY: the kernel reads the word, which is aligned and lives through
    // the call; the null timeout waits without a limit.
    unsafe {
        libc::syscall(
            libc::SYS_futex,
            word.as_ptr(),
            libc::FUTEX_WAIT | libc::FUTEX_PRIVATE_FLAG,
            expected,
            std::ptr::null::<libc::timespec>(),
        )
    };
}

/// Wakes up to `count` threads waiting on `word` in [`futex_wait`].
#[inline]
pub fn futex_wake(word: &AtomicU32, count: u32) {
    // SAFETY: as in `futex_wait`; FUTEX_WAKE reads only the word's address.
    unsafe {
        libc::syscall(
            libc::SYS_futex,
            word.as_ptr(),
            libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG,
            count,
        )
    };
}

/// The parent's process number, asked of the kernel through syscall(2)
/// itself, so that no C library answers from a cache: the cheapest system
/// call there is, for a benchmark of the cost of one.
#[must_use]
#[inline]
pub fn parent_through_system_call() -> i32 {
    // SAFETY: getppid takes nothing and cannot fail.
    let parent = unsafe { libc::syscall(libc::SYS_getppid) };
    i32::try_from(parent).unwrap_or(0)
}

/// The calling process's persona, asked of personality(2) with
/// 0xffffffff, which reads it and changes nothing: a call that Dukovo's
/// Linux table forwards to linuxd, for the benchmark of what forwarding
/// costs.
#[must_use]
#[inline]
pub fn persona() -> i32 {
    // SAFETY: personality takes a number, and 0xffffffff only reads.
    let persona = unsafe { libc::syscall(libc::SYS_personality, 0xffff_ffff_u64) };
    i32::try_from(persona).unwrap_or(-1)
}

/// Sets no_new_privs and installs `program`, a classic BPF program of
/// 8-byte instructions laid out as Linux's `struct sock_filter`, little
/// endian, as a seccomp filter of the calling thread
/// (seccomp(2), `SECCOMP_SET_MODE_FILTER`). The filter holds for the
/// thread's life and passes to the processes it makes.
///
/// # Errors
///
/// InvalidInput for a program that is empty, not whole instructions or
/// longer than a filter may be; EINVAL when Linux refuses the program;
/// EFAULT, ENOMEM or EACCES as seccomp(2) says.
pub fn install_seccomp_filter(program: &[u8]) -> io::Result<()> {
    let (whole, rest) = program.as_chunks::<8>();
    let instructions: Vec<libc::sock_filter> = whole
        .iter()
        .map(|bytes| libc::sock_filter {
            code: u16::from_le_bytes([bytes[0], bytes[1]]),
            jt: bytes[2],
            jf: bytes[3],
            k: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        })
        .collect();
    let len = u16::try_from(instructions.len())
        .ok()
        .filter(|&len| len > 0 && rest.is_empty())
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let filter = libc::sock_fprog { len, filter: instructions.as_ptr().cast_mut() };
    // SAFETY: PR_SET_NO_NEW_PRIVS takes a flag and changes nothing else.
    checked(unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) })?;
    // SAFETY: `filter` points at `len` instructions, which live through the
    // call; the kernel copies them and keeps no pointer.
    let result = unsafe {
        libc::syscall(libc::SYS_seccomp, libc::SECCOMP_SET_MODE_FILTER, 0, &raw const filter)
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The CPUs the calling thread may run on, lowest first
/// (sched_getaffinity(2)).
///
/// # Errors
///
/// What the kernel says, which it does not for the calling thread.
pub fn allowed_cpus() -> io::Result<Vec<usize>> {
    // SAFETY: cpu_set_t is a bit set, for which all zeros is the empty set.
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    // SAFETY: thread 0 is the caller, and `set` is a writable cpu_set_t of
    // the size given.
    checked(unsafe { libc::sched_getaffinity(0, size_of::<libc::cpu_set_t>(), &raw mut set) })?;
    let cpus = 0..libc::CPU_SETSIZE as usize;
    // SAFETY: CPU_ISSET reads the bit of a CPU below CPU_SETSIZE, in the set.
    Ok(cpus.filter(|&cpu| unsafe { libc::CPU_ISSET(cpu, &set) }).collect())
}

/// Lets the calling thread run only on `cpus` (sched_setaffinity(2)).
///
/// # Errors
///
/// EINVAL for a CPU at or above CPU_SETSIZE, or a list of none the
/// machine has.
pub fn set_allowed_cpus(cpus: &[usize]) -> io::Result<()> {
    // SAFETY: as in `allowed_cpus`.
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    for &cpu in cpus {
        if cpu >= libc::CPU_SETSIZE as usize {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        // SAFETY: the CPU is below CPU_SETSIZE, so its bit is in the set.
        unsafe { libc::CPU_SET(cpu, &mut set) };
    }
    // SAFETY: thread 0 is the caller, and `set` is a cpu_set_t of the size
    // given, which the call only reads.
    checked(unsafe { libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &raw const set) })?;
    Ok(())
}

/// Sets the inode flags of `file` to `flags`, as chattr(1) does
/// (`FS_COMPR_FL` to have Btrfs compress what is written to it).
///
/// # Errors
///
/// ENOTTY or EOPNOTSUPP where the filesystem keeps no such flags.
pub fn set_inode_flags(file: impl AsFd, flags: libc::c_long) -> io::Result<()> {
    // SAFETY: the ioctl reads a long, which lives through the call.
    checked(unsafe { libc::ioctl(file.as_fd().as_raw_fd(), FS_IOC_SETFLAGS, &raw const flags) })?;
    Ok(())
}

/// Drops the cached pages of the block device `disk` (BLKFLSBUF), so that
/// what is read next comes from the disk.
///
/// # Errors
///
/// ENOTTY for a file that is no block device.
pub fn drop_block_cache(disk: impl AsFd) -> io::Result<()> {
    // SAFETY: BLKFLSBUF takes no argument.
    checked(unsafe { libc::ioctl(disk.as_fd().as_raw_fd(), BLKFLSBUF, 0) })?;
    Ok(())
}

/// Zeroes `length` bytes at `offset` of the block device `disk`
/// (BLKZEROOUT), which the disk may do itself; both are multiples of 512.
///
/// # Errors
///
/// ENOTTY for a file that is no block device, EINVAL for a range that is
/// not whole sectors or goes past the end.
pub fn zero_range(disk: impl AsFd, offset: u64, length: u64) -> io::Result<()> {
    let range = [offset, length];
    // SAFETY: BLKZEROOUT reads two u64s, which live through the call.
    checked(unsafe { libc::ioctl(disk.as_fd().as_raw_fd(), BLKZEROOUT, &raw const range) })?;
    Ok(())
}

/// Which of a network card's IPv4 addresses [`set_card_address`] sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardAddress {
    /// Its own address (SIOCSIFADDR).
    Own,
    /// Its network's mask (SIOCSIFNETMASK).
    Netmask,
}

/// An `ifreq` naming the card `card`, the rest zero.
fn card_request(card: &str) -> io::Result<libc::ifreq> {
    // SAFETY: ifreq is a name and a union of numbers and addresses, for
    // all of which all zeros is a valid value.
    let mut request: libc::ifreq = unsafe { std::mem::zeroed() };
    if card.len() >= request.ifr_name.len() || card.as_bytes().contains(&0) {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    for (to, &from) in request.ifr_name.iter_mut().zip(card.as_bytes()) {
        *to = from as libc::c_char;
    }
    Ok(request)
}

/// An IPv4 socket, which is what the cards' ioctls are made on.
fn card_socket() -> io::Result<std::net::UdpSocket> {
    std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
}

/// Sets the IPv4 address `which` of the network card `card`.
///
/// # Errors
///
/// EINVAL for a name longer than the kernel's names, ENODEV for no such
/// card, EPERM without the right to configure it.
pub fn set_card_address(card: &str, which: CardAddress, address: Ipv4Addr) -> io::Result<()> {
    let mut request = card_request(card)?;
    let mut data = [0 as libc::c_char; 14];
    // A sockaddr_in in a sockaddr: the port's two bytes, then the address.
    for (to, from) in data[2..6].iter_mut().zip(address.octets()) {
        *to = from as libc::c_char;
    }
    request.ifr_ifru.ifru_addr =
        libc::sockaddr { sa_family: libc::AF_INET as libc::sa_family_t, sa_data: data };
    let ioctl = match which {
        CardAddress::Own => libc::SIOCSIFADDR,
        CardAddress::Netmask => libc::SIOCSIFNETMASK,
    };
    let socket = card_socket()?;
    // SAFETY: the request is a whole ifreq that lives through the call,
    // as these ioctls read it.
    checked(unsafe { libc::ioctl(socket.as_raw_fd(), ioctl, &raw mut request) })?;
    Ok(())
}

/// Brings the network card `card` up, keeping its other flags.
///
/// # Errors
///
/// As for [`set_card_address`].
pub fn bring_up(card: &str) -> io::Result<()> {
    let mut request = card_request(card)?;
    let socket = card_socket()?;
    // SAFETY: SIOCGIFFLAGS writes the flags into the ifreq, which lives
    // through the call.
    checked(unsafe { libc::ioctl(socket.as_raw_fd(), libc::SIOCGIFFLAGS, &raw mut request) })?;
    // SAFETY: SIOCGIFFLAGS filled the union's flags.
    let flags = unsafe { request.ifr_ifru.ifru_flags };
    request.ifr_ifru.ifru_flags = flags | libc::IFF_UP as libc::c_short;
    // SAFETY: SIOCSIFFLAGS reads the name and flags of the ifreq.
    checked(unsafe { libc::ioctl(socket.as_raw_fd(), libc::SIOCSIFFLAGS, &raw mut request) })?;
    Ok(())
}

/// Makes a process that runs `program` with `args` (the first its name)
/// and the same environment, and waits for it, as fork(2), execv(2) and
/// waitpid(2) do one after the other: the cost of starting a program, as
/// the benchmarks time it. The child runs only execv, and `_exit(127)`
/// when the program cannot start, so it is sound whatever the process's
/// other threads hold. The child's wait status.
///
/// # Errors
///
/// EAGAIN when the system has no room for another process.
pub fn fork_exec_wait(program: &CStr, args: &[&CStr]) -> io::Result<libc::c_int> {
    let argv: Vec<*const libc::c_char> =
        args.iter().map(|arg| arg.as_ptr()).chain(std::iter::once(std::ptr::null())).collect();
    // SAFETY: the child calls only execv and _exit, which a child of fork
    // may call; the array ends with a null pointer, and it and the strings
    // it points at live until the parent's wait returns.
    let child = checked(unsafe { libc::fork() })?;
    if child == 0 {
        // SAFETY: as above.
        unsafe {
            libc::execv(program.as_ptr(), argv.as_ptr());
            libc::_exit(127);
        }
    }
    let mut status = 0;
    loop {
        // SAFETY: waitpid writes the child's status into `status`.
        match checked(unsafe { libc::waitpid(child, &raw mut status, 0) }) {
            Ok(_) => return Ok(status),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

/// A new epoll(7) instance, closed on exec.
///
/// # Errors
///
/// EMFILE when the process can open no more descriptors.
pub fn epoll() -> io::Result<OwnedFd> {
    // SAFETY: epoll_create1 takes a number.
    let fd = checked(unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) })?;
    // SAFETY: the descriptor is new, and nothing else holds it.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Has `epoll` watch `fd` for `events` (EPOLLIN, EPOLLET), each reported
/// with `fd`'s number as its data (EPOLL_CTL_ADD).
///
/// # Errors
///
/// EEXIST when it watches `fd` already, EPERM for a file that cannot be
/// polled.
pub fn epoll_watch(epoll: impl AsFd, fd: impl AsFd, events: u32) -> io::Result<()> {
    let number = fd.as_fd().as_raw_fd();
    let mut watch = libc::epoll_event { events, u64: u64::from(number.unsigned_abs()) };
    // SAFETY: epoll_ctl reads the event, which lives through the call.
    checked(unsafe {
        libc::epoll_ctl(epoll.as_fd().as_raw_fd(), libc::EPOLL_CTL_ADD, number, &raw mut watch)
    })?;
    Ok(())
}

/// The events `epoll` has ready now, at most `ready.len()` of them, put
/// in `ready`, without waiting (epoll_wait(2) with a timeout of 0): how
/// many.
///
/// # Errors
///
/// EINTR when a signal came first.
#[inline]
pub fn epoll_ready_now(epoll: impl AsFd, ready: &mut [libc::epoll_event]) -> io::Result<usize> {
    let most = libc::c_int::try_from(ready.len()).unwrap_or(libc::c_int::MAX);
    // SAFETY: epoll_wait writes at most `most` events, which `ready` has
    // room for.
    let count = checked(unsafe {
        libc::epoll_wait(epoll.as_fd().as_raw_fd(), ready.as_mut_ptr(), most, 0)
    })?;
    Ok(count.unsigned_abs() as usize)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::sync::atomic::Ordering;

    use super::*;

    #[test]
    fn an_event_counter_adds_what_is_written() {
        let counter = std::fs::File::from(event_counter().unwrap());
        (&counter).write_all(&3u64.to_ne_bytes()).unwrap();
        (&counter).write_all(&4u64.to_ne_bytes()).unwrap();
        let mut bytes = [0u8; 8];
        (&counter).read_exact(&mut bytes).unwrap();
        assert_eq!(u64::from_ne_bytes(bytes), 7);
    }

    #[test]
    fn a_futex_wait_on_a_changed_word_returns_at_once() {
        let word = AtomicU32::new(5);
        futex_wait(&word, 4);
        futex_wake(&word, 1);
        assert_eq!(word.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn the_parent_is_the_one_std_names() {
        assert_eq!(parent_through_system_call(), std::os::unix::process::parent_id() as i32);
    }

    #[test]
    fn the_persona_is_read_without_change() {
        let persona = persona();
        assert!(persona >= 0, "{persona}");
        assert_eq!(super::persona(), persona);
    }

    #[test]
    fn a_seccomp_filter_is_installed_or_refused_with_why() {
        // On a thread of its own, as a filter holds for the thread that
        // installs it: `RET ALLOW` lets every call through.
        std::thread::spawn(|| {
            let allow = [0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0x7f];
            install_seccomp_filter(&allow).unwrap();
            assert_eq!(parent_through_system_call(), std::os::unix::process::parent_id() as i32);
            let kind = |program: &[u8]| install_seccomp_filter(program).unwrap_err().kind();
            assert_eq!(kind(&[]), io::ErrorKind::InvalidInput);
            assert_eq!(kind(&allow[..7]), io::ErrorKind::InvalidInput);
            // A jump past the program's end.
            let past = [0x05, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00];
            assert_eq!(
                install_seccomp_filter(&past).unwrap_err().raw_os_error(),
                Some(libc::EINVAL)
            );
        })
        .join()
        .unwrap();
    }

    #[test]
    fn a_started_program_is_waited_for_with_its_status() {
        let status = fork_exec_wait(c"/bin/sh", &[c"sh", c"-c", c"exit 3"]).unwrap();
        assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 3);
        let missing = fork_exec_wait(c"/no/such/program", &[c"program"]).unwrap();
        assert!(libc::WIFEXITED(missing) && libc::WEXITSTATUS(missing) == 127);
    }

    #[test]
    fn an_epoll_reports_the_pipe_that_was_written_to() {
        let epoll = epoll().unwrap();
        let (reader, mut writer) = std::io::pipe().unwrap();
        let (idle, _idle_writer) = std::io::pipe().unwrap();
        epoll_watch(&epoll, &reader, (libc::EPOLLIN | libc::EPOLLET).cast_unsigned()).unwrap();
        epoll_watch(&epoll, &idle, libc::EPOLLIN.cast_unsigned()).unwrap();
        let mut ready = [libc::epoll_event { events: 0, u64: 0 }; 4];
        assert_eq!(epoll_ready_now(&epoll, &mut ready).unwrap(), 0);
        writer.write_all(b"x").unwrap();
        assert_eq!(epoll_ready_now(&epoll, &mut ready).unwrap(), 1);
        let token = ready[0].u64;
        assert_eq!(token, u64::from(reader.as_raw_fd().unsigned_abs()));
        let again = epoll_watch(&epoll, &reader, libc::EPOLLIN.cast_unsigned());
        assert_eq!(again.unwrap_err().raw_os_error(), Some(libc::EEXIST));
    }

    #[test]
    fn the_allowed_cpus_can_be_set_again() {
        let cpus = allowed_cpus().unwrap();
        assert!(!cpus.is_empty());
        set_allowed_cpus(&cpus).unwrap();
        assert_eq!(allowed_cpus().unwrap(), cpus);
        assert!(set_allowed_cpus(&[libc::CPU_SETSIZE as usize]).is_err());
    }

    #[test]
    fn calls_on_what_is_not_there_fail_cleanly() {
        let file = std::fs::File::open("/dev/null").unwrap();
        assert!(drop_block_cache(&file).is_err());
        assert!(zero_range(&file, 0, 512).is_err());
        assert!(unmount(Path::new("/nonexistent-mount-point")).is_err());
        assert!(bring_up("no-such-card-0").is_err());
        assert!(bring_up("a-name-longer-than-the-kernels").is_err());
        let error = set_card_address("no-such-card-0", CardAddress::Own, Ipv4Addr::LOCALHOST);
        assert!(error.is_err());
    }
}
