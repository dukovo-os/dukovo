// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Host tests: each module's values against the Linux interface's, each
//! rule written down in the modules' docs, and property tests that no input
//! makes a decoder or a writer panic.

use std::collections::HashSet;

use proptest::prelude::*;
use radix_abi::Status;
use radix_abi::capability::Rights;
use radix_abi::directory::{self, FileKind, OpenFlags, PathFlags};
use radix_abi::errors::{File, Kernel};
use radix_abi::file::{LockKind, TIME_NOW, TIME_OMIT};
use radix_abi::process::PipeFlags;
use radix_abi::signal::{self, ActionFlags, MaskHow, StackFlags};
use radix_abi::terminal::{self, Termios, WindowSize};

use crate::clone::{self, CloneArgs, Kind, classify};
use crate::errno::*;
use crate::flags::*;
use crate::records::*;
use crate::signals::*;
use crate::{futex, nr};

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn i64_at(bytes: &[u8], at: usize) -> i64 {
    i64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

/// Whether every byte of `bytes[from..to]` is 0.
fn zero(bytes: &[u8], from: usize, to: usize) -> bool {
    bytes[from..to].iter().all(|&byte| byte == 0)
}

#[test]
fn call_numbers_are_the_x86_64_ones() {
    let known = [
        (nr::READ, 0),
        (nr::WRITE, 1),
        (nr::OPEN, 2),
        (nr::CLOSE, 3),
        (nr::STAT, 4),
        (nr::MMAP, 9),
        (nr::BRK, 12),
        (nr::RT_SIGACTION, 13),
        (nr::RT_SIGRETURN, 15),
        (nr::IOCTL, 16),
        (nr::PREAD64, 17),
        (nr::PIPE, 22),
        (nr::DUP2, 33),
        (nr::GETPID, 39),
        (nr::SOCKET, 41),
        (nr::CLONE, 56),
        (nr::FORK, 57),
        (nr::VFORK, 58),
        (nr::EXECVE, 59),
        (nr::EXIT, 60),
        (nr::WAIT4, 61),
        (nr::UNAME, 63),
        (nr::FCNTL, 72),
        (nr::GETDENTS, 78),
        (nr::GETCWD, 79),
        (nr::CREAT, 85),
        (nr::GETTIMEOFDAY, 96),
        (nr::SYSINFO, 99),
        (nr::PTRACE, 101),
        (nr::SIGALTSTACK, 131),
        (nr::PIVOT_ROOT, 155),
        (nr::SYSCTL, 156),
        (nr::PRCTL, 157),
        (nr::ARCH_PRCTL, 158),
        (nr::SETRLIMIT, 160),
        (nr::SETTIMEOFDAY, 164),
        (nr::MOUNT, 165),
        (nr::UMOUNT2, 166),
        (nr::SWAPON, 167),
        (nr::SWAPOFF, 168),
        (nr::REBOOT, 169),
        (nr::SETHOSTNAME, 170),
        (nr::SETDOMAINNAME, 171),
        (nr::INIT_MODULE, 175),
        (nr::GETTID, 186),
        (nr::TKILL, 200),
        (nr::FUTEX, 202),
        (nr::GETDENTS64, 217),
        (nr::SET_TID_ADDRESS, 218),
        (nr::CLOCK_SETTIME, 227),
        (nr::CLOCK_GETTIME, 228),
        (nr::EXIT_GROUP, 231),
        (nr::TGKILL, 234),
        (nr::KEXEC_LOAD, 246),
        (nr::WAITID, 247),
        (nr::OPENAT, 257),
        (nr::NEWFSTATAT, 262),
        (nr::FACCESSAT, 269),
        (nr::PSELECT6, 270),
        (nr::PPOLL, 271),
        (nr::SET_ROBUST_LIST, 273),
        (nr::UTIMENSAT, 280),
        (nr::FALLOCATE, 285),
        (nr::ACCEPT4, 288),
        (nr::DUP3, 292),
        (nr::PIPE2, 293),
        (nr::PREADV, 295),
        (nr::PWRITEV, 296),
        (nr::PERF_EVENT_OPEN, 298),
        (nr::RECVMMSG, 299),
        (nr::PRLIMIT64, 302),
        (nr::SENDMMSG, 307),
        (nr::FINIT_MODULE, 313),
        (nr::RENAMEAT2, 316),
        (nr::GETRANDOM, 318),
        (nr::KEXEC_FILE_LOAD, 320),
        (nr::BPF, 321),
        (nr::EXECVEAT, 322),
        (nr::MEMBARRIER, 324),
        (nr::STATX, 332),
        (nr::RSEQ, 334),
        (nr::IO_URING_SETUP, 425),
        (nr::IO_URING_ENTER, 426),
        (nr::IO_URING_REGISTER, 427),
        (nr::PIDFD_OPEN, 434),
        (nr::CLONE3, 435),
        (nr::CLOSE_RANGE, 436),
        (nr::FACCESSAT2, 439),
    ];
    for (constant, number) in known {
        assert_eq!(constant, number);
    }
}

#[test]
fn every_number_up_to_rseq_has_a_name() {
    for number in 0..=nr::RSEQ {
        assert!(nr::name(number).is_some(), "call {number} has no name");
    }
}

#[test]
fn a_name_is_its_constants_in_upper_case() {
    assert_eq!(nr::name(nr::READ), Some("READ"));
    assert_eq!(nr::name(nr::OPENAT), Some("OPENAT"));
    assert_eq!(nr::name(nr::SYSCTL), Some("SYSCTL"));
    assert_eq!(nr::name(nr::UMOUNT2), Some("UMOUNT2"));
    assert_eq!(nr::name(nr::FACCESSAT2), Some("FACCESSAT2"));
}

#[test]
fn no_two_calls_share_a_name() {
    let names: Vec<&str> = (0..=nr::FACCESSAT2).filter_map(nr::name).collect();
    let distinct: HashSet<&str> = names.iter().copied().collect();
    assert_eq!(names.len(), distinct.len());
}

#[test]
fn numbers_between_the_two_ranges_and_past_the_last_have_no_name() {
    for number in [335, 400, 423, 440, 1000, u64::MAX] {
        assert_eq!(nr::name(number), None, "call {number}");
    }
}

#[test]
fn errno_values_are_linuxs() {
    let known = [
        (EPERM, 1),
        (ENOENT, 2),
        (ESRCH, 3),
        (EINTR, 4),
        (EIO, 5),
        (ENXIO, 6),
        (E2BIG, 7),
        (ENOEXEC, 8),
        (EBADF, 9),
        (ECHILD, 10),
        (EAGAIN, 11),
        (ENOMEM, 12),
        (EACCES, 13),
        (EFAULT, 14),
        (ENOTBLK, 15),
        (EBUSY, 16),
        (EEXIST, 17),
        (EXDEV, 18),
        (ENODEV, 19),
        (ENOTDIR, 20),
        (EISDIR, 21),
        (EINVAL, 22),
        (ENFILE, 23),
        (EMFILE, 24),
        (ENOTTY, 25),
        (ETXTBSY, 26),
        (EFBIG, 27),
        (ENOSPC, 28),
        (ESPIPE, 29),
        (EROFS, 30),
        (EMLINK, 31),
        (EPIPE, 32),
        (EDOM, 33),
        (ERANGE, 34),
        (EDEADLK, 35),
        (ENAMETOOLONG, 36),
        (ENOLCK, 37),
        (ENOSYS, 38),
        (ENOTEMPTY, 39),
        (ELOOP, 40),
        (ENOMSG, 42),
        (EIDRM, 43),
        (ENODATA, 61),
        (ETIME, 62),
        (ENONET, 64),
        (EPROTO, 71),
        (EOVERFLOW, 75),
        (EBADFD, 77),
        (EILSEQ, 84),
        (ENOTSOCK, 88),
        (EDESTADDRREQ, 89),
        (EMSGSIZE, 90),
        (EPROTOTYPE, 91),
        (ENOPROTOOPT, 92),
        (EPROTONOSUPPORT, 93),
        (ESOCKTNOSUPPORT, 94),
        (EOPNOTSUPP, 95),
        (EAFNOSUPPORT, 97),
        (EADDRINUSE, 98),
        (EADDRNOTAVAIL, 99),
        (ENETDOWN, 100),
        (ENETUNREACH, 101),
        (ECONNABORTED, 103),
        (ECONNRESET, 104),
        (ENOBUFS, 105),
        (EISCONN, 106),
        (ENOTCONN, 107),
        (ESHUTDOWN, 108),
        (ETOOMANYREFS, 109),
        (ETIMEDOUT, 110),
        (ECONNREFUSED, 111),
        (EHOSTDOWN, 112),
        (EHOSTUNREACH, 113),
        (EALREADY, 114),
        (EINPROGRESS, 115),
        (ESTALE, 116),
        (EUCLEAN, 117),
        (EDQUOT, 122),
        (ECANCELED, 125),
        (EOWNERDEAD, 130),
        (ENOTRECOVERABLE, 131),
    ];
    for (constant, number) in known {
        assert_eq!(constant, number);
    }
}

#[test]
fn restart_codes_are_kernel_internal_numbers() {
    assert_eq!(ERESTARTSYS, 512);
    assert_eq!(ERESTARTNOINTR, 513);
    assert_eq!(ERESTARTNOHAND, 514);
    assert_eq!(ERESTART_RESTARTBLOCK, 516);
}

/// What each native kernel error maps to, written out from `abi/errors.ridl`.
fn kernel_errnos() -> Vec<(Kernel, i32)> {
    vec![
        (Kernel::NoSuchSyscall, ENOSYS),
        (Kernel::NoSuchSlot, EBADF),
        (Kernel::EmptySlot, EBADF),
        (Kernel::WrongKind, EBADF),
        (Kernel::MissingRights, EACCES),
        (Kernel::Sealed, EPERM),
        (Kernel::Occupied, EEXIST),
        (Kernel::InvalidArgument, EINVAL),
        (Kernel::NoMemory, ENOMEM),
        (Kernel::BadAddress, EFAULT),
        (Kernel::WouldBlock, EAGAIN),
        (Kernel::Busy, EBUSY),
        (Kernel::NoRoom, ENOSPC),
        (Kernel::Gone, EPIPE),
        (Kernel::Interrupted, EINTR),
        (Kernel::NoCaller, ENOTCONN),
        (Kernel::NoSuchMethod, EOPNOTSUPP),
        (Kernel::Revoking, EBUSY),
        (Kernel::TooDeep, EMLINK),
        (Kernel::Exhausted, ENOMEM),
        (Kernel::OutOfRange, ERANGE),
        (Kernel::AlreadyMapped, EEXIST),
        (Kernel::NotMapped, ENXIO),
        (Kernel::TimedOut, ETIMEDOUT),
        (Kernel::WrongKey, EPERM),
        (Kernel::BadBadge, EINVAL),
        (Kernel::NoSuchProcess, ESRCH),
        (Kernel::NoChild, ECHILD),
        (Kernel::NotPermitted, EPERM),
        (Kernel::NotTerminal, ENOTTY),
        (Kernel::NotExecutable, ENOEXEC),
        (Kernel::ArgumentsTooLong, E2BIG),
        (Kernel::NotSeekable, ESPIPE),
        (Kernel::Execed, EACCES),
        (Kernel::OverLimit, EACCES),
        (Kernel::NotSocket, ENOTSOCK),
        (Kernel::Refused, ECONNREFUSED),
        (Kernel::Connected, EISCONN),
        (Kernel::NotConnected, ENOTCONN),
        (Kernel::NotSupported, EOPNOTSUPP),
        (Kernel::TooMany, ETOOMANYREFS),
        (Kernel::AddressInUse, EADDRINUSE),
        (Kernel::AddressNotAvailable, EADDRNOTAVAIL),
        (Kernel::Reset, ECONNRESET),
        (Kernel::Unreachable, ENETUNREACH),
        (Kernel::InProgress, EINPROGRESS),
        (Kernel::Already, EALREADY),
        (Kernel::MessageTooLong, EMSGSIZE),
        (Kernel::NoBuffers, ENOBUFS),
        (Kernel::Broadcast, EACCES),
        (Kernel::HostUnreachable, EHOSTUNREACH),
        (Kernel::ProtocolError, EPROTO),
        (Kernel::HostDown, EHOSTDOWN),
        (Kernel::NoNetwork, ENONET),
        (Kernel::ProtocolUnreachable, ENOPROTOOPT),
        (Kernel::Prohibited, EACCES),
        (Kernel::TooManyThreads, EAGAIN),
        (Kernel::WrongType, EPROTOTYPE),
        (Kernel::Cancelled, ECANCELED),
    ]
}

/// The same for the file errors.
fn file_errnos() -> Vec<(File, i32)> {
    vec![
        (File::NotFound, ENOENT),
        (File::Exists, EEXIST),
        (File::NotDirectory, ENOTDIR),
        (File::IsDirectory, EISDIR),
        (File::NotEmpty, ENOTEMPTY),
        (File::ReadOnly, EROFS),
        (File::NoSpace, ENOSPC),
        (File::NameTooLong, ENAMETOOLONG),
        (File::BadName, EINVAL),
        (File::Outside, EXDEV),
        (File::CrossMount, EXDEV),
        (File::InUse, EBUSY),
        (File::Io, EIO),
        (File::Corrupt, EUCLEAN),
        (File::TooBig, EFBIG),
        (File::UnknownFilesystem, ENODEV),
        (File::NotBlockDevice, ENOTBLK),
        (File::NoDevice, ENXIO),
        (File::Deadlock, EDEADLK),
        (File::TooManyLinks, ELOOP),
        (File::NotLink, EINVAL),
        (File::TooManyNames, EMLINK),
        (File::QuotaExceeded, EDQUOT),
        (File::Denied, EACCES),
        (File::NoAttribute, ENODATA),
        (File::Unmounted, ENODEV),
        (File::Removed, EIDRM),
        (File::TextBusy, ETXTBSY),
    ]
}

#[test]
fn each_kernel_error_maps_as_the_c_library_maps_it() {
    // An error in abi/errors.ridl that the list above lacks makes the
    // lengths differ, so that it gets the errno its entry names here.
    assert_eq!(kernel_errnos().len(), Kernel::ALL.len());
    for (error, errno) in kernel_errnos() {
        assert_eq!(from_status(error.status(0)), errno, "{error:?}");
    }
}

#[test]
fn each_file_error_maps_as_the_c_library_maps_it() {
    assert_eq!(file_errnos().len(), File::ALL.len());
    for (error, errno) in file_errnos() {
        assert_eq!(from_status(error.status(0)), errno, "{error:?}");
    }
}

#[test]
fn the_detail_of_a_status_does_not_change_its_errno() {
    let detail = 0xdead_beef;
    assert_eq!(from_status(Kernel::MissingRights.status(detail)), EACCES);
    assert_eq!(from_status(File::NotFound.status(detail)), ENOENT);
}

#[test]
fn success_cancellation_and_unknown_errors_map_as_the_c_library_maps_them() {
    assert_eq!(from_status(Status::OK), 0);
    assert_eq!(from_status(Status(u64::MAX)), ECANCELED);
    // A code past the last of its domain, and a domain no one defines.
    assert_eq!(from_status(Status::new(Kernel::DOMAIN, 60, 0)), EIO);
    assert_eq!(from_status(Status::new(Kernel::DOMAIN, 0, 0)), EIO);
    assert_eq!(from_status(Status::new(File::DOMAIN, 29, 0)), EIO);
    assert_eq!(from_status(Status::new(0, 5, 0)), EIO);
    assert_eq!(from_status(Status::new(3, 1, 0)), EIO);
}

proptest! {
    #[test]
    fn any_error_status_has_an_errno_in_the_linux_range(raw in 1u64..) {
        let errno = from_status(Status(raw));
        prop_assert!((1..=4095).contains(&errno), "{raw:#x} gave {errno}");
    }

    #[test]
    fn any_domain_and_code_has_an_errno_in_the_linux_range(
        domain in 0u16..6,
        code in 0u16..70,
        detail in any::<u32>(),
    ) {
        prop_assume!(domain != 0 || code != 0 || detail != 0);
        let errno = from_status(Status::new(domain, code, detail));
        prop_assert!((1..=4095).contains(&errno));
    }
}

#[test]
fn open_flag_values_are_linuxs_octal_ones() {
    assert_eq!(O_WRONLY, 1);
    assert_eq!(O_RDWR, 2);
    assert_eq!(O_CREAT, 0o100);
    assert_eq!(O_EXCL, 0o200);
    assert_eq!(O_NOCTTY, 0o400);
    assert_eq!(O_TRUNC, 0o1000);
    assert_eq!(O_APPEND, 0o2000);
    assert_eq!(O_NONBLOCK, 0o4000);
    assert_eq!(O_DSYNC, 0o10000);
    assert_eq!(O_ASYNC, 0o20000);
    assert_eq!(O_DIRECT, 0o40000);
    assert_eq!(O_LARGEFILE, 0o100000);
    assert_eq!(O_DIRECTORY, 0o200000);
    assert_eq!(O_NOFOLLOW, 0o400000);
    assert_eq!(O_NOATIME, 0o1000000);
    assert_eq!(O_CLOEXEC, 0o2000000);
    assert_eq!(O_SYNC, 0o4010000);
    assert_eq!(O_PATH, 0o10000000);
    assert_eq!(O_TMPFILE, 0o20200000);
}

#[test]
fn at_flag_values_are_linuxs() {
    assert_eq!(AT_FDCWD, -100);
    assert_eq!(AT_SYMLINK_NOFOLLOW, 0x100);
    assert_eq!(AT_REMOVEDIR, 0x200);
    assert_eq!(AT_SYMLINK_FOLLOW, 0x400);
    assert_eq!(AT_NO_AUTOMOUNT, 0x800);
    assert_eq!(AT_EMPTY_PATH, 0x1000);
    assert_eq!(AT_EACCESS, 0x200);
    assert_eq!(AT_STATX_SYNC_TYPE, 0x6000);
}

/// The native flags every open has, as the C library passes them.
const BASE: OpenFlags = OpenFlags::ABSOLUTE_LINKS_LEAVE;

#[test]
fn a_plain_open_reads_and_asks_for_nothing_else() {
    let opened = open(0).unwrap();
    assert_eq!(opened.flags, BASE);
    assert_eq!(opened.rights, Rights::READ);
    assert!(!opened.close_on_exec);
}

#[test]
fn the_access_mode_picks_the_rights() {
    assert_eq!(open(O_WRONLY).unwrap().rights, Rights::WRITE);
    assert_eq!(open(O_RDWR).unwrap().rights, Rights::READ.union(Rights::WRITE));
    assert_eq!(open(O_ACCMODE), Err(EINVAL));
    assert_eq!(open(O_ACCMODE | O_CREAT), Err(EINVAL));
}

#[test]
fn each_open_flag_with_a_native_twin_maps_to_it() {
    let twins = [
        (O_CREAT, OpenFlags::CREATE),
        (O_EXCL, OpenFlags::EXCLUSIVE),
        (O_TRUNC, OpenFlags::TRUNCATE),
        (O_DIRECTORY, OpenFlags::DIRECTORY),
        (O_APPEND, OpenFlags::APPEND),
        (O_NONBLOCK, OpenFlags::NON_BLOCKING),
        (O_NOCTTY, OpenFlags::NO_TERMINAL),
        (O_NOFOLLOW, OpenFlags::NO_FOLLOW),
        (O_CLOEXEC, OpenFlags::CLOSE_ON_EXEC),
    ];
    for (linux, native) in twins {
        // O_CREAT cannot come with O_DIRECTORY, so each is tried alone.
        let opened = open(linux).unwrap();
        assert_eq!(opened.flags, BASE.union(native), "{linux:#o}");
    }
}

#[test]
fn close_on_exec_is_in_the_flags_and_on_its_own() {
    let opened = open(O_RDWR | O_CLOEXEC).unwrap();
    assert!(opened.close_on_exec);
    assert!(opened.flags.contains(OpenFlags::CLOSE_ON_EXEC));
}

#[test]
fn flags_without_a_native_twin_change_nothing() {
    let ignored = O_DSYNC | O_SYNC | O_ASYNC | O_DIRECT | O_LARGEFILE | O_NOATIME;
    assert_eq!(open(O_WRONLY | ignored), open(O_WRONLY));
    // Bits Linux does not define are ignored too, as `open` ignores them.
    assert_eq!(open(O_RDWR | (1 << 24) | (1 << 31)), open(O_RDWR));
}

#[test]
fn a_path_open_reads_and_keeps_only_three_flags() {
    let named = open(O_PATH).unwrap();
    assert_eq!(named.rights, Rights::READ);
    assert_eq!(named.flags, BASE);
    let everything = O_PATH | O_WRONLY | O_CREAT | O_EXCL | O_TRUNC | O_APPEND | O_NONBLOCK;
    assert_eq!(open(everything), open(O_PATH));
    let kept = open(O_PATH | O_RDWR | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC).unwrap();
    assert_eq!(kept.rights, Rights::READ);
    let wanted = BASE.union(OpenFlags::DIRECTORY).union(OpenFlags::NO_FOLLOW);
    assert_eq!(kept.flags, wanted.union(OpenFlags::CLOSE_ON_EXEC));
    assert!(kept.close_on_exec);
}

#[test]
fn a_path_open_is_not_refused_for_flags_it_drops() {
    // O_CREAT with O_DIRECTORY and O_TMPFILE would both be refused without
    // O_PATH; with it they are dropped before the checks.
    assert!(open(O_PATH | O_CREAT | O_DIRECTORY).is_ok());
    assert_eq!(open(O_PATH | O_TMPFILE), open(O_PATH | O_DIRECTORY));
}

#[test]
fn creating_a_directory_is_refused() {
    assert_eq!(open(O_CREAT | O_DIRECTORY), Err(EINVAL));
    assert_eq!(open(O_WRONLY | O_CREAT | O_DIRECTORY | O_EXCL), Err(EINVAL));
}

#[test]
fn an_unnamed_file_is_not_supported_and_its_misuse_is_invalid() {
    assert_eq!(open(O_TMPFILE | O_WRONLY), Err(EOPNOTSUPP));
    assert_eq!(open(O_TMPFILE | O_RDWR), Err(EOPNOTSUPP));
    // No write access, `O_CREAT`, and O_TMPFILE's own bit without
    // O_DIRECTORY, which an old kernel would take for something else.
    assert_eq!(open(O_TMPFILE), Err(EINVAL));
    assert_eq!(open(O_TMPFILE | O_WRONLY | O_CREAT), Err(EINVAL));
    assert_eq!(open((O_TMPFILE & !O_DIRECTORY) | O_WRONLY), Err(EINVAL));
    assert_eq!(open(O_TMPFILE | O_ACCMODE), Err(EINVAL));
}

proptest! {
    #[test]
    fn any_flag_word_opens_or_is_refused_with_a_known_errno(linux in any::<u32>()) {
        match open(linux) {
            Ok(opened) => {
                prop_assert!(opened.flags.contains(OpenFlags::ABSOLUTE_LINKS_LEAVE));
                prop_assert!(OpenFlags::from_bits(opened.flags.bits()).is_some());
                prop_assert!(!opened.rights.is_empty());
                prop_assert_eq!(
                    opened.close_on_exec,
                    opened.flags.contains(OpenFlags::CLOSE_ON_EXEC)
                );
            }
            Err(errno) => prop_assert!(errno == EINVAL || errno == EOPNOTSUPP),
        }
    }
}

#[test]
fn only_nofollow_changes_the_path_flags() {
    assert_eq!(path_flags(0), PathFlags::NONE);
    assert_eq!(path_flags(AT_SYMLINK_NOFOLLOW), PathFlags::NO_FOLLOW);
    assert_eq!(path_flags(AT_EMPTY_PATH | AT_EACCESS), PathFlags::NONE);
    assert_eq!(path_flags(AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW), PathFlags::NO_FOLLOW);
}

#[test]
fn access_modes_map_to_the_native_ones_and_equal_them() {
    assert_eq!((F_OK, R_OK, W_OK, X_OK), (0, 4, 2, 1));
    assert_eq!(
        (R_OK, W_OK, X_OK),
        (directory::READ_OK, directory::WRITE_OK, directory::EXECUTE_OK)
    );
    assert_eq!(access(F_OK), Ok(0));
    assert_eq!(access(R_OK), Ok(directory::READ_OK));
    assert_eq!(access(R_OK | W_OK | X_OK), Ok(7));
    assert_eq!(access(8), Err(EINVAL));
    assert_eq!(access(R_OK | 0x100), Err(EINVAL));
}

fn sample_stat() -> Stat {
    Stat {
        dev: makedev(8, 17),
        ino: 0x1112_1314_1516_1718,
        mode: S_IFREG | 0o640,
        nlink: 3,
        uid: 1000,
        gid: 1001,
        rdev: makedev(1, 3),
        size: 123_456,
        blksize: 4096,
        blocks: 248,
        atime: Time { sec: 1_700_000_001, nsec: 1 },
        mtime: Time { sec: 1_700_000_002, nsec: 2 },
        ctime: Time { sec: 1_700_000_003, nsec: 3 },
        btime: Some(Time { sec: 1_600_000_004, nsec: 4 }),
    }
}

#[test]
fn stat_has_the_x86_64_layout() {
    let mut out = [0xaa; STAT_SIZE];
    put_stat(&sample_stat(), &mut out);
    assert_eq!(u64_at(&out, 0), 0x811, "st_dev");
    assert_eq!(u64_at(&out, 8), 0x1112_1314_1516_1718, "st_ino");
    assert_eq!(u64_at(&out, 16), 3, "st_nlink");
    assert_eq!(u32_at(&out, 24), S_IFREG | 0o640, "st_mode");
    assert_eq!(u32_at(&out, 28), 1000, "st_uid");
    assert_eq!(u32_at(&out, 32), 1001, "st_gid");
    assert!(zero(&out, 36, 40), "the pad after st_gid");
    assert_eq!(u64_at(&out, 40), 0x103, "st_rdev");
    assert_eq!(i64_at(&out, 48), 123_456, "st_size");
    assert_eq!(i64_at(&out, 56), 4096, "st_blksize");
    assert_eq!(i64_at(&out, 64), 248, "st_blocks");
    assert_eq!((i64_at(&out, 72), i64_at(&out, 80)), (1_700_000_001, 1), "st_atim");
    assert_eq!((i64_at(&out, 88), i64_at(&out, 96)), (1_700_000_002, 2), "st_mtim");
    assert_eq!((i64_at(&out, 104), i64_at(&out, 112)), (1_700_000_003, 3), "st_ctim");
    assert!(zero(&out, 120, 144), "the three reserved words");
}

#[test]
fn statfs_has_the_x86_64_layout() {
    let info = FsInfo {
        kind: 7,
        read_only: true,
        block_size: 4096,
        blocks: 1000,
        free: 400,
        files: 0,
        free_files: 0,
    };
    let mut out = [0xaa; STATFS_SIZE];
    put_statfs(&info, &mut out);
    assert_eq!(u64_at(&out, 0), 0x9123_683e, "f_type: Btrfs's");
    assert_eq!(u64_at(&out, 8), 4096, "f_bsize");
    assert_eq!((u64_at(&out, 16), u64_at(&out, 24), u64_at(&out, 32)), (1000, 400, 400));
    assert!(zero(&out, 40, 64), "f_files, f_ffree and f_fsid");
    assert_eq!(u64_at(&out, 64), 255, "f_namelen");
    assert_eq!(u64_at(&out, 72), 4096, "f_frsize");
    assert_eq!(u64_at(&out, 80), 0x21, "f_flags: ST_VALID and ST_RDONLY");
    assert!(zero(&out, 88, STATFS_SIZE), "f_spare");
    assert_eq!(filesystem_magic(9), 0xef53, "ext4");
    assert_eq!(filesystem_magic(0), 0x0102_1994, "tmpfs");
    assert_eq!(filesystem_magic(14), 0x0102_1994, "shm, a tmpfs on Linux");
}

#[test]
fn stat_keeps_sizes_past_what_a_signed_field_holds_at_its_largest() {
    let stat = Stat { size: u64::MAX, blocks: u64::MAX, ..sample_stat() };
    let mut out = [0; STAT_SIZE];
    put_stat(&stat, &mut out);
    assert_eq!(i64_at(&out, 48), i64::MAX);
    assert_eq!(i64_at(&out, 64), i64::MAX);
}

#[test]
fn statx_has_its_layout() {
    let mut out = [0xaa; STATX_SIZE];
    put_statx(&sample_stat(), STATX_BASIC_STATS | STATX_BTIME, &mut out);
    assert_eq!(u32_at(&out, 0), 0xfff, "stx_mask");
    assert_eq!(u32_at(&out, 4), 4096, "stx_blksize");
    assert_eq!(u64_at(&out, 8), 0, "stx_attributes");
    assert_eq!(u32_at(&out, 16), 3, "stx_nlink");
    assert_eq!(u32_at(&out, 20), 1000, "stx_uid");
    assert_eq!(u32_at(&out, 24), 1001, "stx_gid");
    assert_eq!(u16_at(&out, 28), 0o100_640, "stx_mode");
    assert_eq!(u16_at(&out, 30), 0, "the spare after stx_mode");
    assert_eq!(u64_at(&out, 32), 0x1112_1314_1516_1718, "stx_ino");
    assert_eq!(u64_at(&out, 40), 123_456, "stx_size");
    assert_eq!(u64_at(&out, 48), 248, "stx_blocks");
    assert_eq!(u64_at(&out, 56), 0, "stx_attributes_mask");
    let timestamps = [
        (64, 1_700_000_001, 1, "stx_atime"),
        (80, 1_600_000_004, 4, "stx_btime"),
        (96, 1_700_000_003, 3, "stx_ctime"),
        (112, 1_700_000_002, 2, "stx_mtime"),
    ];
    for (at, sec, nsec, name) in timestamps {
        assert_eq!(i64_at(&out, at), sec, "{name} seconds");
        assert_eq!(u32_at(&out, at + 8), nsec, "{name} nanoseconds");
        assert_eq!(u32_at(&out, at + 12), 0, "{name} pad");
    }
    assert_eq!((u32_at(&out, 128), u32_at(&out, 132)), (1, 3), "stx_rdev");
    assert_eq!((u32_at(&out, 136), u32_at(&out, 140)), (8, 17), "stx_dev");
    assert!(zero(&out, 144, 256), "everything past the device numbers");
}

#[test]
fn statx_mask_offers_the_birth_time_only_when_asked_for_and_kept() {
    let mut out = [0; STATX_SIZE];
    let with = sample_stat();
    let without = Stat { btime: None, ..sample_stat() };
    put_statx(&with, 0, &mut out);
    assert_eq!(u32_at(&out, 0), STATX_BASIC_STATS);
    assert!(zero(&out, 80, 96), "an unasked birth time is not written");
    put_statx(&with, STATX_BTIME, &mut out);
    assert_eq!(u32_at(&out, 0), STATX_BASIC_STATS | STATX_BTIME);
    assert_eq!(i64_at(&out, 80), 1_600_000_004);
    put_statx(&without, STATX_BASIC_STATS | STATX_BTIME, &mut out);
    assert_eq!(u32_at(&out, 0), STATX_BASIC_STATS);
    assert!(zero(&out, 80, 96));
}

#[test]
fn statx_link_count_saturates() {
    let stat = Stat { nlink: u64::from(u32::MAX) + 5, ..sample_stat() };
    let mut out = [0; STATX_SIZE];
    put_statx(&stat, 0, &mut out);
    assert_eq!(u32_at(&out, 16), u32::MAX);
}

#[test]
fn device_numbers_split_and_join_as_the_c_library_does() {
    assert_eq!(makedev(1, 3), 0x103);
    assert_eq!(makedev(8, 1), 0x801);
    assert_eq!(makedev(0x1234, 0x1234_5678), 0x1123_4562_3478);
    assert_eq!((major(0x103), minor(0x103)), (1, 3));
    assert_eq!((major(0x1123_4562_3478), minor(0x1123_4562_3478)), (0x1234, 0x1234_5678));
    assert_eq!((major(0), minor(0)), (0, 0));
}

proptest! {
    #[test]
    fn a_device_number_keeps_its_major_and_minor(major_in in any::<u32>(), minor_in in any::<u32>()) {
        let dev = makedev(major_in, minor_in);
        prop_assert_eq!((major(dev), minor(dev)), (major_in, minor_in));
    }

    #[test]
    fn writing_a_stat_never_panics(
        ids in any::<(u64, u64, u64, u64)>(),
        sizes in any::<(u32, u32, u64, u64, u32)>(),
        sec in any::<i64>(),
        nsec in any::<u32>(),
        has_btime in any::<bool>(),
        wanted in any::<u32>(),
    ) {
        let time = Time { sec, nsec };
        let stat = Stat {
            dev: ids.0,
            ino: ids.1,
            nlink: ids.2,
            rdev: ids.3,
            mode: sizes.0,
            uid: sizes.1,
            gid: sizes.4,
            size: sizes.2,
            blksize: sizes.4,
            blocks: sizes.3,
            atime: time,
            mtime: time,
            ctime: time,
            btime: has_btime.then_some(time),
        };
        put_stat(&stat, &mut [0; STAT_SIZE]);
        put_statx(&stat, wanted, &mut [0; STATX_SIZE]);
    }
}

#[test]
fn a_dirent64_is_header_name_nul_and_padding_to_eight() {
    let mut out = [0xff; 64];
    let used = put_dirent64(&mut out, 7, 9, DT_REG, b"abc").unwrap();
    assert_eq!(used, 24);
    assert_eq!(u64_at(&out, 0), 7, "d_ino");
    assert_eq!(i64_at(&out, 8), 9, "d_off");
    assert_eq!(u16_at(&out, 16), 24, "d_reclen");
    assert_eq!(out[18], DT_REG, "d_type");
    assert_eq!(&out[19..22], b"abc");
    assert!(zero(&out, 22, 24), "the NUL and the padding");
    assert_eq!(out[24], 0xff, "nothing is written past the record");
}

#[test]
fn a_dirent64_lengths_round_up_to_eight() {
    let mut out = [0; 512];
    // 19 bytes of header, the name and a NUL.
    for (name_len, record) in [(0, 24), (4, 24), (5, 32), (12, 32), (13, 40), (255, 280)] {
        let name = vec![b'x'; name_len];
        assert_eq!(put_dirent64(&mut out, 1, 1, DT_DIR, &name), Some(record), "{name_len}");
    }
}

#[test]
fn a_dirent64_that_does_not_fit_is_not_written() {
    let mut out = [0xee; 23];
    assert_eq!(put_dirent64(&mut out, 1, 1, DT_REG, b"abc"), None);
    assert!(out.iter().all(|&byte| byte == 0xee));
    assert_eq!(put_dirent64(&mut [], 1, 1, DT_REG, b""), None);
}

#[test]
fn a_dirent64_too_long_for_its_length_field_is_refused() {
    let mut out = vec![0; 70_000];
    // The longest record is the largest multiple of 8 a u16 holds, 65,528.
    assert_eq!(put_dirent64(&mut out, 1, 1, DT_REG, &vec![b'a'; 65_508]), Some(65_528));
    assert_eq!(u16_at(&out, 16), 65_528);
    assert_eq!(put_dirent64(&mut out, 1, 1, DT_REG, &vec![b'a'; 65_509]), None);
}

#[test]
fn directory_entry_types_are_linuxs() {
    assert_eq!(
        [DT_UNKNOWN, DT_FIFO, DT_CHR, DT_DIR, DT_BLK, DT_REG, DT_LNK, DT_SOCK],
        [0, 1, 2, 4, 6, 8, 10, 12]
    );
    assert_eq!(
        [S_IFMT, S_IFSOCK, S_IFLNK, S_IFREG, S_IFBLK, S_IFDIR, S_IFCHR, S_IFIFO],
        [0o170_000, 0o140_000, 0o120_000, 0o100_000, 0o060_000, 0o040_000, 0o020_000, 0o010_000]
    );
}

proptest! {
    #[test]
    fn a_dirent64_never_panics_and_fits_what_it_says(
        space in 0usize..400,
        name in proptest::collection::vec(any::<u8>(), 0..300),
        ino in any::<u64>(),
        offset in any::<i64>(),
        kind in any::<u8>(),
    ) {
        let mut out = vec![0x5a; space];
        match put_dirent64(&mut out, ino, offset, kind, &name) {
            Some(used) => {
                prop_assert!(used <= space);
                prop_assert_eq!(used % 8, 0);
                prop_assert_eq!(usize::from(u16_at(&out, 16)), used);
                prop_assert_eq!(&out[19..19 + name.len()], &name[..]);
                prop_assert!(zero(&out, 19 + name.len(), used));
                prop_assert!(out[used..].iter().all(|&byte| byte == 0x5a));
            }
            None => {
                prop_assert!((19 + name.len() + 1).next_multiple_of(8) > space);
                prop_assert!(out.iter().all(|&byte| byte == 0x5a));
            }
        }
    }
}

#[test]
fn a_sigaction_has_the_kernels_field_order() {
    let action = SigAction {
        handler: 0x1000,
        flags: SA_SIGINFO | SA_RESTORER,
        restorer: 0x2000,
        mask: 0x8000_0000_0000_0001,
    };
    let mut out = [0xaa; SIGACTION_SIZE];
    action.put(&mut out);
    assert_eq!(u64_at(&out, 0), 0x1000, "sa_handler");
    assert_eq!(u64_at(&out, 8), SA_SIGINFO | SA_RESTORER, "sa_flags");
    assert_eq!(u64_at(&out, 16), 0x2000, "sa_restorer");
    assert_eq!(u64_at(&out, 24), 0x8000_0000_0000_0001, "sa_mask");
    assert_eq!(SigAction::read(&out), action);
}

proptest! {
    #[test]
    fn a_sigaction_survives_a_round_trip(bytes in any::<[u8; SIGACTION_SIZE]>()) {
        let mut out = [0; SIGACTION_SIZE];
        SigAction::read(&bytes).put(&mut out);
        prop_assert_eq!(out, bytes);
    }
}

fn uts_field(out: &[u8; UTSNAME_SIZE], index: usize) -> &[u8] {
    &out[index * 65..(index + 1) * 65]
}

#[test]
fn utsname_has_six_nul_padded_fields() {
    let mut out = [0xaa; UTSNAME_SIZE];
    utsname(b"dukovo-pc", b"6.1.0-dukovo", b"#1 SMP Dukovo 0.17", &mut out);
    let expected: [&[u8]; 6] =
        [b"Linux", b"dukovo-pc", b"6.1.0-dukovo", b"#1 SMP Dukovo 0.17", b"x86_64", b"(none)"];
    for (index, text) in expected.into_iter().enumerate() {
        let field = uts_field(&out, index);
        assert_eq!(&field[..text.len()], text, "field {index}");
        assert!(field[text.len()..].iter().all(|&byte| byte == 0), "field {index}'s padding");
    }
}

#[test]
fn utsname_cuts_a_long_name_to_leave_its_nul() {
    let mut out = [0; UTSNAME_SIZE];
    let long = [b'n'; 100];
    utsname(&long, &long, &long, &mut out);
    for index in 1..=3 {
        let field = uts_field(&out, index);
        assert!(field[..64].iter().all(|&byte| byte == b'n'));
        assert_eq!(field[64], 0);
    }
    assert_eq!(uts_field(&out, 4)[..6], *b"x86_64");
}

proptest! {
    #[test]
    fn utsname_never_overruns_a_field(
        a in proptest::collection::vec(any::<u8>(), 0..200),
        b in proptest::collection::vec(any::<u8>(), 0..200),
        c in proptest::collection::vec(any::<u8>(), 0..200),
    ) {
        let mut out = [0; UTSNAME_SIZE];
        utsname(&a, &b, &c, &mut out);
        prop_assert_eq!(&uts_field(&out, 0)[..5], b"Linux");
        prop_assert_eq!(&uts_field(&out, 4)[..6], b"x86_64");
        prop_assert_eq!(&uts_field(&out, 5)[..6], b"(none)");
        for index in 0..6 {
            prop_assert_eq!(uts_field(&out, index)[64], 0);
        }
    }
}

fn timespec_bytes(sec: i64, fraction: i64) -> [u8; TIMESPEC_SIZE] {
    let mut bytes = [0; TIMESPEC_SIZE];
    bytes[..8].copy_from_slice(&sec.to_le_bytes());
    bytes[8..].copy_from_slice(&fraction.to_le_bytes());
    bytes
}

#[test]
fn a_timespec_is_read_when_it_is_valid() {
    assert_eq!(timespec(&timespec_bytes(5, 999_999_999)), Ok(Time { sec: 5, nsec: 999_999_999 }));
    assert_eq!(timespec(&timespec_bytes(0, 0)), Ok(Time { sec: 0, nsec: 0 }));
    assert_eq!(timespec(&timespec_bytes(i64::MAX, 0)), Ok(Time { sec: i64::MAX, nsec: 0 }));
}

#[test]
fn a_timespec_that_is_not_valid_is_einval() {
    for (sec, nsec) in
        [(-1, 0), (i64::MIN, 0), (0, -1), (0, 1_000_000_000), (0, i64::MAX), (1, i64::MIN)]
    {
        assert_eq!(timespec(&timespec_bytes(sec, nsec)), Err(EINVAL), "{sec} {nsec}");
    }
}

#[test]
fn a_timeval_is_read_in_microseconds() {
    assert_eq!(timeval(&timespec_bytes(5, 999_999)), Ok(Time { sec: 5, nsec: 999_999_000 }));
    assert_eq!(timeval(&timespec_bytes(0, 0)), Ok(Time { sec: 0, nsec: 0 }));
    for (sec, usec) in [(-1, 0), (0, -1), (0, 1_000_000), (0, i64::MAX)] {
        assert_eq!(timeval(&timespec_bytes(sec, usec)), Err(EINVAL), "{sec} {usec}");
    }
}

#[test]
fn a_time_is_written_as_timespec_and_timeval() {
    let time = Time { sec: 7, nsec: 1_234_567 };
    let mut out = [0xaa; TIMESPEC_SIZE];
    put_timespec(time, &mut out);
    assert_eq!((i64_at(&out, 0), i64_at(&out, 8)), (7, 1_234_567));
    put_timeval(time, &mut out);
    assert_eq!((i64_at(&out, 0), i64_at(&out, 8)), (7, 1_234));
}

fn itimerval_bytes(interval: (i64, i64), value: (i64, i64)) -> [u8; ITIMERVAL_SIZE] {
    let mut bytes = [0; ITIMERVAL_SIZE];
    bytes[..16].copy_from_slice(&timespec_bytes(interval.0, interval.1));
    bytes[16..].copy_from_slice(&timespec_bytes(value.0, value.1));
    bytes
}

#[test]
fn an_itimerval_is_read_as_value_then_interval() {
    // The interval comes first in the record, the value second.
    let bytes = itimerval_bytes((0, 250_000), (2, 500_000));
    assert_eq!(itimerval(&bytes), Ok((2_500_000_000, 250_000_000)));
    assert_eq!(itimerval(&[0; ITIMERVAL_SIZE]), Ok((0, 0)));
    for (interval, value) in [((0, 0), (-1, 0)), ((0, 1_000_000), (1, 0)), ((-1, 0), (0, 0))] {
        assert_eq!(itimerval(&itimerval_bytes(interval, value)), Err(EINVAL), "{interval:?}");
    }
}

#[test]
fn an_itimerval_is_written_with_a_set_timer_never_0() {
    let mut out = [0xaa; ITIMERVAL_SIZE];
    put_itimerval(1_500_000_000, 10_000_000, &mut out);
    assert_eq!(
        [i64_at(&out, 0), i64_at(&out, 8), i64_at(&out, 16), i64_at(&out, 24)],
        [0, 10_000, 1, 500_000]
    );
    // Under a microsecond left reads as one; nothing left reads as 0.
    put_itimerval(999, 0, &mut out);
    assert_eq!((i64_at(&out, 16), i64_at(&out, 24)), (0, 1));
    put_itimerval(0, 0, &mut out);
    assert_eq!(out, [0; ITIMERVAL_SIZE]);
}

#[test]
fn alarm_rounds_what_is_left_to_the_nearest_second_never_0() {
    assert_eq!(alarm_left(0), 0);
    assert_eq!(alarm_left(1), 1);
    assert_eq!(alarm_left(499_999_999), 1);
    assert_eq!(alarm_left(1_499_999_999), 1);
    assert_eq!(alarm_left(1_500_000_000), 2);
    assert_eq!(alarm_left(3_000_000_000), 3);
    // Cut to an unsigned int, as Linux returns it.
    assert_eq!(alarm_left((1 << 32) * 1_000_000_000 + 5), 0);
}

#[test]
fn a_time_counts_nanoseconds_since_1970_and_saturates() {
    assert_eq!(Time { sec: 2, nsec: 5 }.as_ns(), 2_000_000_005);
    assert_eq!(Time { sec: -1, nsec: 999 }.as_ns(), 0);
    assert_eq!(Time { sec: i64::MAX, nsec: 999_999_999 }.as_ns(), u64::MAX);
    assert_eq!(Time::from_ns(2_000_000_005), Time { sec: 2, nsec: 5 });
    assert_eq!(Time::from_ns(u64::MAX).as_ns(), u64::MAX);
}

proptest! {
    #[test]
    fn a_valid_timespec_survives_a_round_trip(sec in 0i64.., nsec in 0u32..1_000_000_000) {
        let mut bytes = [0; TIMESPEC_SIZE];
        put_timespec(Time { sec, nsec }, &mut bytes);
        prop_assert_eq!(timespec(&bytes), Ok(Time { sec, nsec }));
    }

    #[test]
    fn a_valid_timeval_survives_a_round_trip(sec in 0i64.., usec in 0u32..1_000_000) {
        let time = Time { sec, nsec: usec * 1000 };
        let mut bytes = [0; TIMESPEC_SIZE];
        put_timeval(time, &mut bytes);
        prop_assert_eq!(timeval(&bytes), Ok(time));
    }

    #[test]
    fn reading_any_bytes_as_a_time_never_panics(bytes in any::<[u8; TIMESPEC_SIZE]>()) {
        let _ = timespec(&bytes);
        let _ = timeval(&bytes);
    }

    #[test]
    fn nanoseconds_survive_being_a_time(ns in any::<u64>()) {
        let time = Time::from_ns(ns);
        prop_assert!(time.nsec < 1_000_000_000);
        prop_assert_eq!(time.as_ns(), ns);
    }
}

#[test]
fn rusage_has_its_times_and_largest_resident_set() {
    let mut out = [0xaa; RUSAGE_SIZE];
    put_rusage(3_000_004_000, 5_000_000_000, 777, &mut out);
    assert_eq!((i64_at(&out, 0), i64_at(&out, 8)), (3, 4), "ru_utime");
    assert_eq!((i64_at(&out, 16), i64_at(&out, 24)), (5, 0), "ru_stime");
    assert_eq!(i64_at(&out, 32), 777, "ru_maxrss");
    assert!(zero(&out, 40, 144), "the other thirteen counts");
}

#[test]
fn sysinfo_has_its_layout() {
    let mut out = [0xaa; SYSINFO_SIZE];
    put_sysinfo(86_400, 8 << 30, 3 << 30, 42, &mut out);
    assert_eq!(i64_at(&out, 0), 86_400, "uptime");
    assert!(zero(&out, 8, 32), "loads");
    assert_eq!(u64_at(&out, 32), 8 << 30, "totalram");
    assert_eq!(u64_at(&out, 40), 3 << 30, "freeram");
    assert!(zero(&out, 48, 80), "sharedram, bufferram, totalswap and freeswap");
    assert_eq!(u16_at(&out, 80), 42, "procs");
    assert!(zero(&out, 82, 104), "the pad, totalhigh and freehigh");
    assert_eq!(u32_at(&out, 104), 1, "mem_unit");
    assert!(zero(&out, 108, 112), "the tail pad");
}

/// The flags of a thread as C libraries make one, with SYSVSEM too.
const THREAD: u64 = clone::CLONE_VM
    | clone::CLONE_FS
    | clone::CLONE_FILES
    | clone::CLONE_SIGHAND
    | clone::CLONE_THREAD
    | clone::CLONE_SYSVSEM
    | clone::CLONE_SETTLS
    | clone::CLONE_PARENT_SETTID
    | clone::CLONE_CHILD_CLEARTID;

fn request(flags: u64) -> CloneArgs {
    CloneArgs { flags, ..CloneArgs::default() }
}

#[test]
fn clone_flag_values_are_linuxs() {
    use clone::*;
    let known = [
        (CLONE_VM, 0x100),
        (CLONE_FS, 0x200),
        (CLONE_FILES, 0x400),
        (CLONE_SIGHAND, 0x800),
        (CLONE_PIDFD, 0x1000),
        (CLONE_PTRACE, 0x2000),
        (CLONE_VFORK, 0x4000),
        (CLONE_PARENT, 0x8000),
        (CLONE_THREAD, 0x10000),
        (CLONE_NEWNS, 0x20000),
        (CLONE_SYSVSEM, 0x40000),
        (CLONE_SETTLS, 0x80000),
        (CLONE_PARENT_SETTID, 0x100000),
        (CLONE_CHILD_CLEARTID, 0x200000),
        (CLONE_DETACHED, 0x400000),
        (CLONE_UNTRACED, 0x800000),
        (CLONE_CHILD_SETTID, 0x1000000),
        (CLONE_NEWCGROUP, 0x2000000),
        (CLONE_NEWUTS, 0x4000000),
        (CLONE_NEWIPC, 0x8000000),
        (CLONE_NEWUSER, 0x10000000),
        (CLONE_NEWPID, 0x20000000),
        (CLONE_NEWNET, 0x40000000),
        (CLONE_IO, 0x80000000),
        (CLONE_CLEAR_SIGHAND, 0x100000000),
        (CLONE_INTO_CGROUP, 0x200000000),
        (CLONE_NEWTIME, 0x80),
    ];
    for (flag, value) in known {
        assert_eq!(flag, value);
    }
}

#[test]
fn the_raw_clone_takes_its_arguments_in_the_x86_64_order() {
    let args = CloneArgs::from_clone(THREAD, 0x7000, 0x100, 0x200, 0x300);
    assert_eq!(args.flags, THREAD);
    assert_eq!(args.stack, 0x7000);
    assert_eq!(args.parent_tid, 0x100);
    assert_eq!(args.child_tid, 0x200);
    assert_eq!(args.tls, 0x300);
    assert_eq!((args.exit_signal, args.stack_size), (0, 0));
    assert_eq!(args.child_stack(), 0x7000);
}

#[test]
fn the_raw_clone_splits_the_exit_signal_from_the_flags_and_drops_high_bits() {
    let args = CloneArgs::from_clone(clone::CLONE_VFORK | 0x11, 0, 0, 0, 0);
    assert_eq!((args.flags, args.exit_signal), (clone::CLONE_VFORK, 17));
    // clone takes 32 bits: a flag above them, which only clone3 has, is lost.
    let args = CloneArgs::from_clone(
        clone::CLONE_CLEAR_SIGHAND | clone::CLONE_INTO_CGROUP | 0x11,
        0,
        0,
        0,
        0,
    );
    assert_eq!((args.flags, args.exit_signal), (0, 17));
}

/// A `struct clone_args` of `size` bytes with the fields set.
fn clone3_bytes(size: usize, words: &[(usize, u64)]) -> Vec<u8> {
    let mut bytes = vec![0; size];
    for &(at, value) in words {
        bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

#[test]
fn clone3_reads_its_fields_in_order() {
    let words = [
        (0, clone::CLONE_VM | clone::CLONE_VFORK),
        (8, 0x11),
        (16, 0x22),
        (24, 0x33),
        (32, 17),
        (40, 0x10_0000),
        (48, 0x1000),
        (56, 0x44),
    ];
    for size in [clone::CLONE_ARGS_SIZE_VER0, 80, clone::CLONE_ARGS_SIZE_VER2] {
        let args = CloneArgs::read(&clone3_bytes(size, &words)).unwrap();
        let expected = CloneArgs {
            flags: clone::CLONE_VM | clone::CLONE_VFORK,
            pidfd: 0x11,
            child_tid: 0x22,
            parent_tid: 0x33,
            exit_signal: 17,
            stack: 0x10_0000,
            stack_size: 0x1000,
            tls: 0x44,
        };
        assert_eq!(args, expected, "size {size}");
        assert_eq!(args.child_stack(), 0x10_1000);
    }
}

#[test]
fn clone3_sizes_are_checked_as_linux_checks_them() {
    assert_eq!(CloneArgs::read(&[0; 0]), Err(EINVAL));
    assert_eq!(CloneArgs::read(&[0; 63]), Err(EINVAL));
    assert!(CloneArgs::read(&[0; 64]).is_ok());
    assert!(CloneArgs::read(&[0; 88]).is_ok());
    // Past the version this module knows, zeroes are fine and anything
    // else is E2BIG.
    assert!(CloneArgs::read(&[0; clone::CLONE_ARGS_SIZE_MAX]).is_ok());
    assert_eq!(CloneArgs::read(&[0; clone::CLONE_ARGS_SIZE_MAX + 1]), Err(E2BIG));
    let mut bytes = vec![0; 96];
    bytes[88] = 1;
    assert_eq!(CloneArgs::read(&bytes), Err(E2BIG));
    let mut bytes = vec![0; clone::CLONE_ARGS_SIZE_MAX];
    bytes[clone::CLONE_ARGS_SIZE_MAX - 1] = 1;
    assert_eq!(CloneArgs::read(&bytes), Err(E2BIG));
    // The cgroup field is read but only matters with CLONE_INTO_CGROUP, which
    // `classify` refuses.
    let mut bytes = vec![0; 88];
    bytes[87] = 1;
    assert!(CloneArgs::read(&bytes).is_ok());
}

#[test]
fn clone3_refuses_to_pick_the_new_tasks_id() {
    assert_eq!(CloneArgs::read(&clone3_bytes(88, &[(64, 0x1000)])), Err(EINVAL));
    assert_eq!(CloneArgs::read(&clone3_bytes(88, &[(72, 1)])), Err(EINVAL));
    assert_eq!(CloneArgs::read(&clone3_bytes(80, &[(64, 0x1000), (72, 1)])), Err(EINVAL));
}

#[test]
fn clone3_refuses_the_flags_it_leaves_free() {
    let read = |flags| CloneArgs::read(&clone3_bytes(64, &[(0, flags)]));
    assert_eq!(read(clone::CLONE_DETACHED), Err(EINVAL));
    // The exit signal's byte, which clone3 has a field for.
    assert_eq!(read(17), Err(EINVAL));
    assert_eq!(read(1), Err(EINVAL));
    // A bit no flag has.
    assert_eq!(read(1 << 34), Err(EINVAL));
    assert_eq!(read(1 << 63), Err(EINVAL));
    // The one flag in that byte that is a flag, and the two above 32 bits.
    assert!(read(clone::CLONE_NEWTIME).is_ok());
    assert!(read(clone::CLONE_CLEAR_SIGHAND).is_ok());
    assert!(read(clone::CLONE_INTO_CGROUP).is_ok());
}

#[test]
fn clone3_refuses_what_its_fields_contradict() {
    let both = clone::CLONE_VM | clone::CLONE_SIGHAND | clone::CLONE_CLEAR_SIGHAND;
    assert_eq!(CloneArgs::read(&clone3_bytes(64, &[(0, both)])), Err(EINVAL));
    // An exit signal with a thread or CLONE_PARENT, which have none.
    let thread = clone3_bytes(64, &[(0, THREAD), (32, 17)]);
    assert_eq!(CloneArgs::read(&thread), Err(EINVAL));
    let parent = clone3_bytes(64, &[(0, clone::CLONE_PARENT), (32, 17)]);
    assert_eq!(CloneArgs::read(&parent), Err(EINVAL));
    assert!(CloneArgs::read(&clone3_bytes(64, &[(0, THREAD)])).is_ok());
}

#[test]
fn clone3_stacks_are_an_address_and_a_size_together() {
    assert_eq!(CloneArgs::read(&clone3_bytes(64, &[(40, 0x1000)])), Err(EINVAL));
    assert_eq!(CloneArgs::read(&clone3_bytes(64, &[(48, 0x1000)])), Err(EINVAL));
    assert_eq!(CloneArgs::read(&clone3_bytes(64, &[(40, u64::MAX), (48, 2)])), Err(EINVAL));
    let top = CloneArgs::read(&clone3_bytes(64, &[(40, u64::MAX - 7), (48, 7)])).unwrap();
    assert_eq!(top.child_stack(), u64::MAX);
}

#[test]
fn a_thread_shares_everything_and_needs_every_share_flag() {
    assert_eq!(classify(&request(THREAD)), Ok(Kind::Thread));
    let minimal = clone::CLONE_VM
        | clone::CLONE_FS
        | clone::CLONE_FILES
        | clone::CLONE_SIGHAND
        | clone::CLONE_THREAD;
    assert_eq!(classify(&request(minimal)), Ok(Kind::Thread));
    for missing in [clone::CLONE_FS, clone::CLONE_FILES] {
        assert_eq!(classify(&request(minimal & !missing)), Err(EINVAL), "{missing:#x}");
    }
    // Linux's own rules: handlers with the thread, and memory with them.
    assert_eq!(classify(&request(minimal & !clone::CLONE_SIGHAND)), Err(EINVAL));
    assert_eq!(classify(&request(minimal & !clone::CLONE_VM)), Err(EINVAL));
    assert_eq!(classify(&request(clone::CLONE_THREAD)), Err(EINVAL));
}

#[test]
fn a_thread_may_not_wait_for_an_exec_it_does_not_make() {
    assert_eq!(classify(&request(THREAD | clone::CLONE_VFORK)), Err(EINVAL));
}

#[test]
fn flags_with_no_meaning_here_are_accepted() {
    let ignored = clone::CLONE_PTRACE
        | clone::CLONE_UNTRACED
        | clone::CLONE_IO
        | clone::CLONE_DETACHED
        | clone::CLONE_SYSVSEM;
    assert_eq!(classify(&request(THREAD | ignored)), Ok(Kind::Thread));
    assert_eq!(classify(&request(ignored)), Ok(Kind::Process { vfork: false }));
}

#[test]
fn a_process_shares_nothing_and_a_vfork_waits() {
    assert_eq!(classify(&request(0)), Ok(Kind::Process { vfork: false }));
    let settid =
        clone::CLONE_PARENT_SETTID | clone::CLONE_CHILD_SETTID | clone::CLONE_CHILD_CLEARTID;
    assert_eq!(classify(&request(settid)), Ok(Kind::Process { vfork: false }));
    assert_eq!(classify(&request(clone::CLONE_VFORK)), Ok(Kind::Process { vfork: true }));
    // posix_spawn and vfork: C libraries ask for the memory too.
    let spawn = clone::CLONE_VM | clone::CLONE_VFORK;
    assert_eq!(classify(&request(spawn)), Ok(Kind::Process { vfork: true }));
}

#[test]
fn processes_that_share_state_are_not_supported() {
    for shared in [
        clone::CLONE_VM,
        clone::CLONE_FS,
        clone::CLONE_FILES,
        clone::CLONE_VM | clone::CLONE_SIGHAND,
        clone::CLONE_VM | clone::CLONE_VFORK | clone::CLONE_SIGHAND,
        clone::CLONE_VM | clone::CLONE_VFORK | clone::CLONE_FILES,
        clone::CLONE_VFORK | clone::CLONE_FS,
    ] {
        assert_eq!(classify(&request(shared)), Err(EINVAL), "{shared:#x}");
    }
    // Handlers shared with no memory shared is Linux's own EINVAL.
    assert_eq!(classify(&request(clone::CLONE_SIGHAND)), Err(EINVAL));
}

#[test]
fn clearing_the_handlers_is_accepted_without_sharing_them() {
    assert_eq!(classify(&request(clone::CLONE_CLEAR_SIGHAND)), Ok(Kind::Process { vfork: false }));
    let shared = clone::CLONE_CLEAR_SIGHAND | clone::CLONE_SIGHAND | clone::CLONE_VM;
    assert_eq!(classify(&request(shared)), Err(EINVAL));
    assert_eq!(classify(&request(THREAD | clone::CLONE_CLEAR_SIGHAND)), Err(EINVAL));
}

#[test]
fn every_namespace_flag_is_eperm() {
    for namespace in [
        clone::CLONE_NEWNS,
        clone::CLONE_NEWCGROUP,
        clone::CLONE_NEWUTS,
        clone::CLONE_NEWIPC,
        clone::CLONE_NEWUSER,
        clone::CLONE_NEWPID,
        clone::CLONE_NEWNET,
        clone::CLONE_NEWTIME,
    ] {
        assert_eq!(classify(&request(namespace)), Err(EPERM), "{namespace:#x}");
        assert_eq!(classify(&request(namespace | clone::CLONE_VFORK)), Err(EPERM));
    }
}

#[test]
fn linuxs_rules_come_before_the_namespace_refusal() {
    assert_eq!(classify(&request(clone::CLONE_NEWNS | clone::CLONE_FS)), Err(EINVAL));
    assert_eq!(classify(&request(clone::CLONE_NEWUSER | clone::CLONE_FS)), Err(EINVAL));
    assert_eq!(classify(&request(THREAD | clone::CLONE_NEWPID)), Err(EINVAL));
    assert_eq!(classify(&request(THREAD | clone::CLONE_NEWUSER)), Err(EINVAL));
    // A thread in a namespace of the other kinds is Dukovo's refusal.
    assert_eq!(classify(&request(THREAD | clone::CLONE_NEWNET)), Err(EPERM));
}

#[test]
fn the_parent_and_cgroups_come_later() {
    for later in [clone::CLONE_PARENT, clone::CLONE_INTO_CGROUP] {
        assert_eq!(classify(&request(later)), Err(EINVAL), "{later:#x}");
        assert_eq!(classify(&request(THREAD | later)), Err(EINVAL), "{later:#x} in a thread");
    }
}

#[test]
fn a_process_may_ask_for_a_descriptor_for_its_child() {
    assert_eq!(classify(&request(clone::CLONE_PIDFD)), Ok(Kind::Process { vfork: false }));
    let vfork = clone::CLONE_PIDFD | clone::CLONE_VM | clone::CLONE_VFORK;
    assert_eq!(classify(&request(vfork)), Ok(Kind::Process { vfork: true }));
}

#[test]
fn a_descriptor_for_a_thread_or_with_the_detached_flag_is_invalid() {
    assert_eq!(classify(&request(THREAD | clone::CLONE_PIDFD)), Err(EINVAL));
    assert_eq!(classify(&request(clone::CLONE_PIDFD | clone::CLONE_DETACHED)), Err(EINVAL));
}

#[test]
fn the_descriptor_and_the_parents_thread_number_may_not_share_a_word() {
    let both = clone::CLONE_PIDFD | clone::CLONE_PARENT_SETTID;
    // `clone` has one word for both.
    let raw = CloneArgs::from_clone(both, 0, 0x7000, 0, 0);
    assert_eq!(classify(&raw), Err(EINVAL));
    // `clone3` has two, which differ.
    let apart =
        CloneArgs { flags: both, pidfd: 0x7000, parent_tid: 0x7008, ..CloneArgs::default() };
    assert_eq!(classify(&apart), Ok(Kind::Process { vfork: false }));
    let same = CloneArgs { flags: both, pidfd: 0x7000, parent_tid: 0x7000, ..CloneArgs::default() };
    assert_eq!(classify(&same), Err(EINVAL));
}

#[test]
fn a_request_with_a_bit_no_flag_has_is_invalid() {
    assert_eq!(classify(&request(1 << 40)), Err(EINVAL));
    assert_eq!(classify(&request(0x7f)), Err(EINVAL));
}

#[test]
fn an_exit_signal_must_be_a_signal() {
    let with = |exit_signal| CloneArgs { exit_signal, ..request(0) };
    assert_eq!(classify(&with(17)), Ok(Kind::Process { vfork: false }));
    assert_eq!(classify(&with(64)), Ok(Kind::Process { vfork: false }));
    assert_eq!(classify(&with(65)), Err(EINVAL));
    assert_eq!(classify(&with(u64::MAX)), Err(EINVAL));
}

proptest! {
    #[test]
    fn classifying_never_panics_and_what_it_accepts_shares_what_it_says(
        flags in any::<u64>(),
        exit_signal in any::<u64>(),
    ) {
        use clone::*;
        let args = CloneArgs { flags, exit_signal, ..CloneArgs::default() };
        let result = classify(&args);
        match result {
            Ok(Kind::Thread) => {
                let all = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD;
                prop_assert_eq!(flags & all, all);
                prop_assert_eq!(flags & CLONE_VFORK, 0);
            }
            Ok(Kind::Process { vfork }) => {
                prop_assert_eq!(vfork, flags & CLONE_VFORK != 0);
                prop_assert_eq!(flags & (CLONE_THREAD | CLONE_FS | CLONE_FILES | CLONE_SIGHAND), 0);
                prop_assert!(flags & CLONE_VM == 0 || vfork);
            }
            Err(errno) => prop_assert!(errno == EINVAL || errno == EPERM),
        }
        if result.is_ok() {
            let later = CLONE_NEWNS | CLONE_NEWCGROUP | CLONE_NEWUTS | CLONE_NEWIPC
                | CLONE_NEWUSER | CLONE_NEWPID | CLONE_NEWNET | CLONE_NEWTIME
                | CLONE_PARENT | CLONE_INTO_CGROUP;
            prop_assert_eq!(flags & later, 0);
            prop_assert!(flags & CLONE_PIDFD == 0 || flags & CLONE_THREAD == 0);
            prop_assert!(exit_signal <= 64);
        }
    }

    #[test]
    fn reading_any_clone3_bytes_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..5000)) {
        if let Ok(args) = CloneArgs::read(&bytes) {
            let _ = classify(&args);
            let _ = args.child_stack();
        }
    }

    #[test]
    fn the_raw_clone_never_panics(
        flags in any::<u64>(),
        stack in any::<u64>(),
        parent in any::<u64>(),
        child in any::<u64>(),
        tls in any::<u64>(),
    ) {
        let args = CloneArgs::from_clone(flags, stack, parent, child, tls);
        prop_assert!(args.exit_signal <= 0xff);
        let _ = classify(&args);
    }
}

#[test]
fn the_futex_op_splits_into_command_and_flags() {
    let plain = futex::op(0);
    assert_eq!(plain.command, futex::Command::Wait);
    assert!(!plain.private && !plain.realtime);
    let wake = futex::op(futex::FUTEX_WAKE | futex::FUTEX_PRIVATE_FLAG);
    assert_eq!(wake.command, futex::Command::Wake);
    assert!(wake.private && !wake.realtime);
    let wait = futex::op(
        futex::FUTEX_WAIT_BITSET | futex::FUTEX_PRIVATE_FLAG | futex::FUTEX_CLOCK_REALTIME,
    );
    assert_eq!(wait.command, futex::Command::WaitBitset);
    assert!(wait.private && wait.realtime);
    assert_eq!(futex::op(10).command, futex::Command::WakeBitset);
}

#[test]
fn futex_constants_are_linuxs() {
    assert_eq!(futex::FUTEX_PRIVATE_FLAG, 128);
    assert_eq!(futex::FUTEX_CLOCK_REALTIME, 256);
    assert_eq!(
        [
            futex::FUTEX_WAIT,
            futex::FUTEX_WAKE,
            futex::FUTEX_REQUEUE,
            futex::FUTEX_CMP_REQUEUE,
            futex::FUTEX_WAKE_OP,
            futex::FUTEX_WAIT_BITSET,
            futex::FUTEX_WAKE_BITSET
        ],
        [0, 1, 3, 4, 5, 9, 10]
    );
    assert_eq!(futex::FUTEX_BITSET_MATCH_ANY, 0xffff_ffff);
}

#[test]
fn other_futex_commands_keep_their_number_without_the_flags() {
    let both = futex::FUTEX_PRIVATE_FLAG | futex::FUTEX_CLOCK_REALTIME;
    for (number, flags) in
        [(2, 0), (6, both), (7, 0), (11, futex::FUTEX_PRIVATE_FLAG), (13, 0), (0x8000_0001, 0)]
    {
        let raw = number | flags;
        assert_eq!(futex::op(raw).command, futex::Command::Other(number), "{raw:#x}");
    }
}

proptest! {
    #[test]
    fn any_futex_op_has_a_command_and_its_flags(raw in any::<u32>()) {
        let op = futex::op(raw);
        prop_assert_eq!(op.private, raw & 128 != 0);
        prop_assert_eq!(op.realtime, raw & 256 != 0);
    }
}

/// Linux's signal numbers on x86-64, by name, from `asm-generic/signal.h`.
const LINUX_SIGNALS: [(u32, u32); 31] = [
    (signal::SIGHUP, 1),
    (signal::SIGINT, 2),
    (signal::SIGQUIT, 3),
    (signal::SIGILL, 4),
    (signal::SIGTRAP, 5),
    (signal::SIGABRT, 6),
    (signal::SIGBUS, 7),
    (signal::SIGFPE, 8),
    (signal::SIGKILL, 9),
    (signal::SIGUSR1, 10),
    (signal::SIGSEGV, 11),
    (signal::SIGUSR2, 12),
    (signal::SIGPIPE, 13),
    (signal::SIGALRM, 14),
    (signal::SIGTERM, 15),
    (signal::SIGSTKFLT, 16),
    (signal::SIGCHLD, 17),
    (signal::SIGCONT, 18),
    (signal::SIGSTOP, 19),
    (signal::SIGTSTP, 20),
    (signal::SIGTTIN, 21),
    (signal::SIGTTOU, 22),
    (signal::SIGURG, 23),
    (signal::SIGXCPU, 24),
    (signal::SIGXFSZ, 25),
    (signal::SIGVTALRM, 26),
    (signal::SIGPROF, 27),
    (signal::SIGWINCH, 28),
    (signal::SIGIO, 29),
    (signal::SIGPWR, 30),
    (signal::SIGSYS, 31),
];

#[test]
fn radix_numbers_its_signals_as_linux_does() {
    for (native, linux) in LINUX_SIGNALS {
        assert_eq!(native, linux);
    }
    assert_eq!(signal::SIGRTMIN, 32);
    assert_eq!(signal::SIGRTMAX, 64);
    assert_eq!(signal::SIGNALS, 64);
    assert_eq!(SIGSET_SIZE * 8, u64::from(signal::SIGNALS));
}

#[test]
fn radix_takes_linuxs_sigaction_flags_unchanged() {
    let flags = [
        (SA_NOCLDSTOP, ActionFlags::NO_CHILD_STOP, 1),
        (SA_NOCLDWAIT, ActionFlags::NO_CHILD_WAIT, 2),
        (SA_SIGINFO, ActionFlags::INFO, 4),
        (SA_RESTORER, ActionFlags::RESTORER, 0x0400_0000),
        (SA_ONSTACK, ActionFlags::ON_STACK, 0x0800_0000),
        (SA_RESTART, ActionFlags::RESTART, 0x1000_0000),
        (SA_NODEFER, ActionFlags::NO_DEFER, 0x4000_0000),
        (SA_RESETHAND, ActionFlags::RESET_HAND, 0x8000_0000),
    ];
    let mut all = 0;
    for (linux, native, value) in flags {
        assert_eq!(linux, value);
        assert_eq!(linux, native.bits());
        all |= linux;
    }
    assert_eq!(all, ActionFlags::ALL.bits(), "every native flag has a Linux name here");
}

#[test]
fn radix_takes_linuxs_handler_values_mask_operations_and_stack_flags_unchanged() {
    assert_eq!(SIG_DFL, signal::DEFAULT);
    assert_eq!(SIG_IGN, signal::IGNORE);
    assert_eq!(SIG_BLOCK, MaskHow::Block.raw());
    assert_eq!(SIG_UNBLOCK, MaskHow::Unblock.raw());
    assert_eq!(SIG_SETMASK, MaskHow::Set.raw());
    assert_eq!(SS_ONSTACK, StackFlags::ON_STACK.bits());
    assert_eq!(SS_DISABLE, StackFlags::DISABLED.bits());
    // The one native operation Linux has no number for.
    assert_eq!(MaskHow::Query.raw(), 3);
}

#[test]
fn the_descriptor_and_name_calls_have_their_x86_64_numbers() {
    let known = [
        (nr::LSEEK, 8),
        (nr::IOCTL, 16),
        (nr::READV, 19),
        (nr::WRITEV, 20),
        (nr::ACCESS, 21),
        (nr::PIPE, 22),
        (nr::DUP, 32),
        (nr::DUP2, 33),
        (nr::FCNTL, 72),
        (nr::FLOCK, 73),
        (nr::FSYNC, 74),
        (nr::FDATASYNC, 75),
        (nr::TRUNCATE, 76),
        (nr::FTRUNCATE, 77),
        (nr::RENAME, 82),
        (nr::MKDIR, 83),
        (nr::RMDIR, 84),
        (nr::LINK, 86),
        (nr::UNLINK, 87),
        (nr::SYMLINK, 88),
        (nr::READLINK, 89),
        (nr::CHMOD, 90),
        (nr::FCHMOD, 91),
        (nr::CHOWN, 92),
        (nr::FCHOWN, 93),
        (nr::LCHOWN, 94),
        (nr::UMASK, 95),
        (nr::MKNOD, 133),
        (nr::UTIMES, 235),
        (nr::FCHOWNAT, 260),
        (nr::FUTIMESAT, 261),
        (nr::MKDIRAT, 258),
        (nr::MKNODAT, 259),
        (nr::UNLINKAT, 263),
        (nr::RENAMEAT, 264),
        (nr::LINKAT, 265),
        (nr::SYMLINKAT, 266),
        (nr::READLINKAT, 267),
        (nr::FCHMODAT, 268),
    ];
    for (constant, number) in known {
        assert_eq!(constant, number);
    }
}

#[test]
fn fcntl_commands_are_linuxs() {
    let known = [
        (F_DUPFD, 0),
        (F_GETFD, 1),
        (F_SETFD, 2),
        (F_GETFL, 3),
        (F_SETFL, 4),
        (F_GETLK, 5),
        (F_SETLK, 6),
        (F_SETLKW, 7),
        (F_OFD_GETLK, 36),
        (F_OFD_SETLK, 37),
        (F_OFD_SETLKW, 38),
        (F_DUPFD_CLOEXEC, 1030),
        (F_SETPIPE_SZ, 1031),
        (F_GETPIPE_SZ, 1032),
    ];
    for (constant, number) in known {
        assert_eq!(constant, number);
    }
    assert_eq!(FD_CLOEXEC, 1);
}

#[test]
fn lock_and_close_range_flags_are_linuxs() {
    assert_eq!((LOCK_SH, LOCK_EX, LOCK_NB, LOCK_UN), (1, 2, 4, 8));
    assert_eq!((F_RDLCK, F_WRLCK, F_UNLCK), (0, 1, 2));
    assert_eq!(CLOSE_RANGE_UNSHARE, 2);
    assert_eq!(CLOSE_RANGE_CLOEXEC, 4);
}

#[test]
fn ioctl_requests_the_table_answers_itself_are_linuxs() {
    assert_eq!(FIONBIO, 0x5421);
    assert_eq!(FIOCLEX, 0x5451);
    assert_eq!(FIONCLEX, 0x5450);
    assert_eq!(TCSBRK, 0x5409);
}

#[test]
fn whence_has_five_values() {
    let known = [Seek::Set, Seek::Current, Seek::End, Seek::Data, Seek::Hole];
    for (raw, seek) in known.into_iter().enumerate() {
        assert_eq!(Seek::from_raw(raw as u32), Some(seek));
    }
    assert_eq!(Seek::from_raw(5), None);
    assert_eq!(Seek::from_raw(u32::MAX), None);
}

#[test]
fn a_lock_type_picks_the_native_kind() {
    assert_eq!(lock_kind(F_RDLCK, false), Ok(LockKind::Shared));
    assert_eq!(lock_kind(F_WRLCK, false), Ok(LockKind::Exclusive));
    assert_eq!(lock_kind(F_UNLCK, false), Ok(LockKind::None));
    assert_eq!(lock_kind(F_RDLCK, true), Ok(LockKind::Shared));
    assert_eq!(lock_kind(F_WRLCK, true), Ok(LockKind::Exclusive));
}

#[test]
fn looking_for_a_lock_cannot_ask_for_none() {
    assert_eq!(lock_kind(F_UNLCK, true), Err(EINVAL));
    for l_type in [-1, 3, i16::MAX, i16::MIN] {
        assert_eq!(lock_kind(l_type, false), Err(EINVAL), "{l_type}");
        assert_eq!(lock_kind(l_type, true), Err(EINVAL), "{l_type}");
    }
}

#[test]
fn a_native_lock_kind_has_its_lock_type_back() {
    for kind in [LockKind::None, LockKind::Shared, LockKind::Exclusive] {
        assert_eq!(lock_kind(lock_type(kind), false), Ok(kind));
    }
}

#[test]
fn flock_takes_one_operation_and_may_not_wait() {
    assert_eq!(flock_operation(LOCK_SH), Ok((LockKind::Shared, true)));
    assert_eq!(flock_operation(LOCK_EX), Ok((LockKind::Exclusive, true)));
    assert_eq!(flock_operation(LOCK_UN), Ok((LockKind::None, true)));
    assert_eq!(flock_operation(LOCK_SH | LOCK_NB), Ok((LockKind::Shared, false)));
    assert_eq!(flock_operation(LOCK_EX | LOCK_NB), Ok((LockKind::Exclusive, false)));
    assert_eq!(flock_operation(LOCK_UN | LOCK_NB), Ok((LockKind::None, false)));
}

#[test]
fn flock_refuses_no_operation_and_two_at_once() {
    for operation in [0, LOCK_NB, LOCK_SH | LOCK_EX, LOCK_SH | LOCK_UN, 16, u32::MAX] {
        assert_eq!(flock_operation(operation), Err(EINVAL), "{operation}");
    }
}

#[test]
fn pipe2_flags_are_non_blocking_and_close_on_exec() {
    assert_eq!(pipe(0), Ok(PipeFlags::NONE));
    assert_eq!(pipe(O_NONBLOCK), Ok(PipeFlags::NON_BLOCKING));
    assert_eq!(pipe(O_CLOEXEC), Ok(PipeFlags::CLOSE_ON_EXEC));
    assert_eq!(
        pipe(O_NONBLOCK | O_CLOEXEC),
        Ok(PipeFlags::NON_BLOCKING.union(PipeFlags::CLOSE_ON_EXEC))
    );
}

#[test]
fn pipe2_refuses_packet_mode_and_every_other_flag() {
    for flags in [O_DIRECT, O_NONBLOCK | O_DIRECT, O_APPEND, O_CREAT, 1, u32::MAX] {
        assert_eq!(pipe(flags), Err(EINVAL), "{flags:o}");
    }
}

#[test]
fn mknod_makes_files_pipes_and_socket_names() {
    assert_eq!(node_kind(0o644), Ok(FileKind::File));
    assert_eq!(node_kind(S_IFREG | 0o644), Ok(FileKind::File));
    assert_eq!(node_kind(S_IFIFO | 0o600), Ok(FileKind::Pipe));
    assert_eq!(node_kind(S_IFSOCK | 0o600), Ok(FileKind::Socket));
}

#[test]
fn mknod_leaves_devices_and_directories_to_those_with_the_privilege() {
    for mode in [S_IFCHR, S_IFBLK, S_IFDIR] {
        assert_eq!(node_kind(mode | 0o600), Err(EPERM), "{mode:o}");
    }
}

#[test]
fn mknod_refuses_a_type_it_does_not_know() {
    assert_eq!(node_kind(S_IFLNK), Err(EINVAL));
    assert_eq!(node_kind(0o030_000), Err(EINVAL));
}

#[test]
fn the_kernels_termios_is_the_first_36_bytes_of_the_native_one() {
    // Four flag words, the line, and the control characters that Linux
    // numbers: the native record starts the same way and goes on with
    // more control characters and the speeds.
    assert_eq!(TERMIOS_SIZE, Termios::OFFSET_CC + 19);
    assert_eq!(Termios::OFFSET_CC, 17);
    const { assert!(TERMIOS_SIZE < Termios::SIZE) };
}

#[test]
fn terminal_requests_move_the_bytes_linux_moves() {
    for request in [
        terminal::GET_ATTRIBUTES,
        terminal::SET_ATTRIBUTES,
        terminal::SET_ATTRIBUTES_DRAIN,
        terminal::SET_ATTRIBUTES_FLUSH,
    ] {
        assert_eq!(terminal_request_size(request), Some(TERMIOS_SIZE), "{request:#x}");
    }
    for request in [terminal::GET_WINDOW_SIZE, terminal::SET_WINDOW_SIZE] {
        assert_eq!(terminal_request_size(request), Some(WindowSize::SIZE), "{request:#x}");
    }
    for request in [
        terminal::GET_FOREGROUND,
        terminal::SET_FOREGROUND,
        terminal::GET_SESSION,
        terminal::READABLE,
        terminal::GET_PTY_NUMBER,
        terminal::SET_PTY_LOCK,
    ] {
        assert_eq!(terminal_request_size(request), Some(4), "{request:#x}");
    }
    for request in [terminal::FLOW, terminal::FLUSH, terminal::SET_CONTROLLING, terminal::DETACH] {
        assert_eq!(terminal_request_size(request), Some(0), "{request:#x}");
    }
}

#[test]
fn a_request_no_native_terminal_takes_has_no_size() {
    // TCSBRK, FIONBIO, the native line name request and an arbitrary one.
    for request in [TCSBRK, FIONBIO, terminal::GET_LINE_NAME, 0x1234_5678, 0] {
        assert_eq!(terminal_request_size(request), None, "{request:#x}");
    }
}

fn flock_bytes(kind: i16, whence: i16, start: i64, len: i64, pid: i32) -> [u8; FLOCK_SIZE] {
    let mut bytes = [0xaa; FLOCK_SIZE];
    Flock { kind, whence, start, len, pid }.put(&mut bytes);
    bytes
}

#[test]
fn a_flock_has_the_x86_64_layout() {
    let bytes = flock_bytes(F_WRLCK, 2, -5, 0x1_0000_0001, 77);
    assert_eq!(FLOCK_SIZE, 32);
    assert_eq!(u16_at(&bytes, 0), 1);
    assert_eq!(u16_at(&bytes, 2), 2);
    assert!(zero(&bytes, 4, 8), "the padding after l_whence");
    assert_eq!(i64_at(&bytes, 8), -5);
    assert_eq!(i64_at(&bytes, 16), 0x1_0000_0001);
    assert_eq!(u32_at(&bytes, 24), 77);
    assert!(zero(&bytes, 28, 32), "the padding after l_pid");
}

proptest! {
    #[test]
    fn a_flock_survives_a_round_trip(
        kind in any::<i16>(),
        whence in any::<i16>(),
        start in any::<i64>(),
        len in any::<i64>(),
        pid in any::<i32>(),
    ) {
        let lock = Flock { kind, whence, start, len, pid };
        let mut bytes = [0xaa; FLOCK_SIZE];
        lock.put(&mut bytes);
        prop_assert_eq!(Flock::read(&bytes), lock);
    }
}

#[test]
fn a_lock_range_counts_from_where_whence_says() {
    assert_eq!(lock_range(0, 0, 0), Ok((0, 0)));
    assert_eq!(lock_range(0, 10, 5), Ok((10, 5)));
    assert_eq!(lock_range(100, 10, 5), Ok((110, 5)));
    assert_eq!(lock_range(100, -100, 0), Ok((0, 0)));
}

#[test]
fn a_negative_length_is_the_bytes_before_the_start() {
    assert_eq!(lock_range(0, 10, -4), Ok((6, 4)));
    assert_eq!(lock_range(50, 0, -50), Ok((0, 50)));
}

#[test]
fn a_lock_range_may_not_start_before_the_file() {
    assert_eq!(lock_range(0, -1, 4), Err(EINVAL));
    assert_eq!(lock_range(5, 0, -6), Err(EINVAL));
    assert_eq!(lock_range(0, 3, i64::MIN), Err(EINVAL));
}

#[test]
fn a_lock_range_past_what_an_offset_holds_is_eoverflow() {
    assert_eq!(lock_range(i64::MAX, 1, 0), Err(EOVERFLOW));
    assert_eq!(lock_range(0, i64::MAX, 2), Err(EOVERFLOW));
    // The last byte is the largest offset: it fits.
    assert_eq!(lock_range(0, i64::MAX - 1, 2), Ok((i64::MAX as u64 - 1, 2)));
}

proptest! {
    #[test]
    fn a_lock_range_never_panics_and_ends_in_an_offset(
        base in any::<i64>(),
        start in any::<i64>(),
        len in any::<i64>(),
    ) {
        if let Ok((first, length)) = lock_range(base, start, len) {
            prop_assert!(first <= i64::MAX as u64);
            prop_assert!(length == 0 || first + (length - 1) <= i64::MAX as u64);
        }
    }
}

#[test]
fn an_iovec_is_a_base_and_a_length() {
    let mut bytes = [0u8; IOVEC_SIZE];
    bytes[..8].copy_from_slice(&0x1000u64.to_le_bytes());
    bytes[8..].copy_from_slice(&0x20u64.to_le_bytes());
    assert_eq!(Iovec::read(&bytes), Iovec { base: 0x1000, len: 0x20 });
}

#[test]
fn an_iovec_read_from_a_short_slice_is_zero_where_it_is_short() {
    assert_eq!(Iovec::read(&[]), Iovec::default());
    assert_eq!(Iovec::read(&[1, 0, 0, 0, 0, 0, 0, 0]), Iovec { base: 1, len: 0 });
}

fn time_bytes(sec: i64, fraction: i64) -> [u8; TIMESPEC_SIZE] {
    timespec_bytes(sec, fraction)
}

#[test]
fn utimensat_times_are_now_omit_or_a_time() {
    assert_eq!(UTIME_NOW, 0x3fff_ffff);
    assert_eq!(UTIME_OMIT, 0x3fff_fffe);
    assert_eq!(utimespec(&time_bytes(0, UTIME_NOW)), Ok(SetTime::Now));
    assert_eq!(utimespec(&time_bytes(0, UTIME_OMIT)), Ok(SetTime::Omit));
    assert_eq!(utimespec(&time_bytes(9, 8)), Ok(SetTime::At(Time { sec: 9, nsec: 8 })));
}

#[test]
fn now_and_omit_do_not_look_at_the_seconds() {
    assert_eq!(utimespec(&time_bytes(-1, UTIME_NOW)), Ok(SetTime::Now));
    assert_eq!(utimespec(&time_bytes(i64::MIN, UTIME_OMIT)), Ok(SetTime::Omit));
}

#[test]
fn an_invalid_utimensat_time_is_einval() {
    for (sec, nsec) in [(-1, 0), (0, -1), (0, 1_000_000_000), (0, UTIME_OMIT - 1)] {
        assert_eq!(utimespec(&time_bytes(sec, nsec)), Err(EINVAL), "{sec} {nsec}");
    }
}

#[test]
fn a_time_is_native_nanoseconds_and_now_and_omit_are_the_natives() {
    assert_eq!(SetTime::Now.native(), Ok(TIME_NOW));
    assert_eq!(SetTime::Omit.native(), Ok(TIME_OMIT));
    assert_eq!(SetTime::At(Time { sec: 3, nsec: 4 }).native(), Ok(3_000_000_004));
    assert_eq!(SetTime::At(Time { sec: 0, nsec: 0 }).native(), Ok(0));
}

#[test]
fn a_time_the_native_interface_cannot_hold_is_einval() {
    // The year 2554 is where a u64 of nanoseconds ends; the last two
    // values are the native "now" and "leave it".
    let last = Time::from_ns(TIME_NOW - 1);
    assert_eq!(SetTime::At(last).native(), Ok(TIME_NOW - 1));
    assert_eq!(SetTime::At(Time::from_ns(TIME_NOW)).native(), Err(EINVAL));
    assert_eq!(SetTime::At(Time { sec: i64::MAX, nsec: 0 }).native(), Err(EINVAL));
}

proptest! {
    #[test]
    fn a_utimensat_time_never_panics(bytes in any::<[u8; TIMESPEC_SIZE]>()) {
        if let Ok(time) = utimespec(&bytes) {
            let _ = time.native();
        }
    }
}

#[test]
fn an_ioctl_moves_the_bytes_its_number_says_unless_it_is_a_terminal_request() {
    // BLKGETSIZE64 is _IOR(0x12, 114, size_t), FICLONE _IOW(0x94, 9, int).
    assert_eq!(ioctl_size(0x8008_1272), 8);
    assert_eq!(ioctl_size(0x4004_9409), 4);
    // BLKSSZGET is _IO(0x12, 104): it carries no size.
    assert_eq!(ioctl_size(0x1268), 0);
    assert_eq!(ioctl_size(terminal::GET_WINDOW_SIZE), WindowSize::SIZE);
    assert_eq!(ioctl_size(terminal::GET_ATTRIBUTES), TERMIOS_SIZE);
}

proptest! {
    #[test]
    fn an_ioctl_never_moves_more_than_the_number_can_say(request in any::<u32>()) {
        prop_assert!(ioctl_size(request) <= 0x3fff);
    }
}

#[test]
fn a_filter_takes_the_flags_dukovo_has_and_refuses_a_listener() {
    use crate::seccomp::{self, FilterFlags};
    assert_eq!(seccomp::filter_flags(0), Ok(FilterFlags::default()));
    let all = seccomp::FLAG_TSYNC
        | seccomp::FLAG_LOG
        | seccomp::FLAG_SPEC_ALLOW
        | seccomp::FLAG_TSYNC_ESRCH;
    assert_eq!(
        seccomp::filter_flags(all),
        Ok(FilterFlags { tsync: true, log: true, tsync_esrch: true })
    );
    for refused in [seccomp::FLAG_NEW_LISTENER, seccomp::FLAG_WAIT_KILLABLE_RECV, 1 << 6, 1 << 31] {
        assert_eq!(seccomp::filter_flags(refused), Err(EINVAL), "{refused:#x}");
    }
}

#[test]
fn a_sock_fprog_reads_as_x86_64_lays_it_out() {
    use crate::seccomp::{SOCK_FPROG_SIZE, SockFprog};
    let mut bytes = [0xeeu8; SOCK_FPROG_SIZE];
    bytes[..2].copy_from_slice(&27u16.to_le_bytes());
    bytes[8..].copy_from_slice(&0x7fff_1234_5678u64.to_le_bytes());
    assert_eq!(SockFprog::read(&bytes), SockFprog { len: 27, filter: 0x7fff_1234_5678 });
}
