// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The nodes of the filesystem, named by what they are, and the numbers
//! the kernel knows them by.
//!
//! A node's number is a function of its path alone (the process, the
//! thread and the file; the CPU; the interface; the socket), and the
//! path can be read back from it, so a linuxd started again answers for
//! the numbers the kernel remembers from the one before. The numbers
//! are below [`NODE_BASE`], where the sockets' begin. Their layout, from
//! the top bit down, is a class of two bits and then:
//!
//! - class 0, the fixed nodes and the files of a process: the process
//!   number in bits 8 to 38 and the entry in bits 0 to 7; process
//!   number 0, which no process has, stands for the fixed nodes, whose
//!   entry is their place in [`Fixed::ALL`] and one;
//! - class 1, the descriptors: the process number in bits 24 to 54 and
//!   the descriptor in bits 0 to 23;
//! - class 2, the threads' files: the process number in bits 32 to 59,
//!   the thread number in bits 4 to 31 and the entry in bits 0 to 3;
//! - class 3, the CPUs and the interfaces: bit 56 tells which, the
//!   number in bits 8 to 31 and the entry in bits 0 to 7.
//!
//! A process or thread number that does not fit its field has no node:
//! process numbers go to 2^31 (a Linux `pid_t`), those of a thread and
//! its process to 2^28.

use std::time::Duration;

use super::text::decimal;
use crate::netlink::{self, NODE_BASE};

/// What a node is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A directory.
    Directory,
    /// A file of text.
    File,
    /// A symbolic link.
    Link,
}

/// What a fixed node holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    /// A directory.
    Directory,
    /// A file with this text.
    Constant(&'static str),
    /// A file that is a copy of the kernel's file at this path.
    Copy(&'static str),
    /// A file whose text is made when it is opened.
    Generated(Generated),
    /// A symbolic link to where this names.
    Link(Target),
}

/// Where a link of a fixed node goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The path it holds, relative to its directory.
    Path(&'static str),
    /// The number of the process that asks (`self`).
    Requester,
    /// `<number>/task/<number>`, of the process that asks (`thread-self`).
    RequesterThread,
}

/// The files of the fixed tree that are made when they are opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Generated {
    /// `/proc/cpuinfo`.
    Cpuinfo,
    /// `/proc/meminfo`.
    Meminfo,
    /// `/proc/stat`.
    Stat,
    /// `/proc/loadavg`.
    Loadavg,
    /// `/proc/uptime`.
    Uptime,
    /// `/proc/version`.
    Version,
    /// `/proc/sys/kernel/ostype`.
    Ostype,
    /// `/proc/sys/kernel/osrelease`.
    Osrelease,
    /// `/proc/sys/kernel/version`.
    KernelVersion,
    /// `/proc/filesystems`.
    Filesystems,
    /// `/proc/sys/kernel/hostname`.
    Hostname,
    /// `/proc/sys/kernel/threads-max`.
    ThreadsMax,
    /// `/proc/sys/kernel/random/boot_id`.
    BootId,
    /// `/proc/sys/kernel/random/uuid`.
    Uuid,
    /// `/proc/sys/kernel/ngroups_max`.
    NgroupsMax,
    /// `/proc/sys/net/core/somaxconn`.
    Somaxconn,
    /// `/proc/sys/net/ipv4/ip_local_port_range`.
    PortRange,
    /// `/sys/fs/cgroup/memory.current`.
    MemoryCurrent,
}

/// Declares the fixed nodes: each with its name, the node it is in and
/// what it holds. The nodes are numbered by their order, which therefore
/// never changes: a new node goes last.
macro_rules! fixed_nodes {
    ($($variant:ident: $name:literal in $parent:ident is $what:expr;)*) => {
        /// A node of the tree that is always there: the directories above
        /// the processes, and the files that are the machine's.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Fixed {
            $(
                #[doc = concat!("`", $name, "`.")]
                $variant,
            )*
        }

        impl Fixed {
            /// Every fixed node, in the order of their numbers.
            pub const ALL: &'static [Fixed] = &[$(Fixed::$variant,)*];

            /// The name it has in its directory.
            pub fn name(self) -> &'static str {
                match self { $(Fixed::$variant => $name,)* }
            }

            /// The directory it is in; the root's is the root.
            pub fn parent(self) -> Fixed {
                match self { $(Fixed::$variant => Fixed::$parent,)* }
            }

            /// What it holds.
            pub fn what(self) -> What {
                match self { $(Fixed::$variant => $what,)* }
            }
        }
    };
}

fixed_nodes! {
    Root: "" in Root is What::Directory;
    Proc: "proc" in Root is What::Directory;
    Sys: "sys" in Root is What::Directory;
    Netlink: ".netlink" in Root is What::Directory;

    ProcSelf: "self" in Proc is What::Link(Target::Requester);
    ProcThreadSelf: "thread-self" in Proc is What::Link(Target::RequesterThread);
    ProcCpuinfo: "cpuinfo" in Proc is What::Generated(Generated::Cpuinfo);
    ProcMeminfo: "meminfo" in Proc is What::Generated(Generated::Meminfo);
    ProcStat: "stat" in Proc is What::Generated(Generated::Stat);
    ProcLoadavg: "loadavg" in Proc is What::Generated(Generated::Loadavg);
    ProcUptime: "uptime" in Proc is What::Generated(Generated::Uptime);
    ProcVersion: "version" in Proc is What::Generated(Generated::Version);
    ProcFilesystems: "filesystems" in Proc is What::Generated(Generated::Filesystems);
    ProcMounts: "mounts" in Proc is What::Link(Target::Path("self/mounts"));
    ProcSwaps: "swaps" in Proc is What::Constant(super::system::SWAPS);
    ProcNet: "net" in Proc is What::Link(Target::Path("self/net"));
    ProcSys: "sys" in Proc is What::Directory;

    SysctlKernel: "kernel" in ProcSys is What::Directory;
    SysctlVm: "vm" in ProcSys is What::Directory;
    SysctlFs: "fs" in ProcSys is What::Directory;
    SysctlNet: "net" in ProcSys is What::Directory;

    KernelOstype: "ostype" in SysctlKernel is What::Generated(Generated::Ostype);
    KernelOsrelease: "osrelease" in SysctlKernel is What::Generated(Generated::Osrelease);
    KernelVersion: "version" in SysctlKernel is What::Generated(Generated::KernelVersion);
    KernelHostname: "hostname" in SysctlKernel is What::Generated(Generated::Hostname);
    KernelDomainname: "domainname" in SysctlKernel is What::Constant("(none)\n");
    KernelPidMax: "pid_max" in SysctlKernel is What::Constant("4194304\n");
    KernelThreadsMax: "threads-max" in SysctlKernel is What::Generated(Generated::ThreadsMax);
    KernelRandom: "random" in SysctlKernel is What::Directory;
    KernelNgroupsMax: "ngroups_max" in SysctlKernel is What::Generated(Generated::NgroupsMax);
    RandomBootId: "boot_id" in KernelRandom is What::Generated(Generated::BootId);
    RandomUuid: "uuid" in KernelRandom is What::Generated(Generated::Uuid);

    VmOvercommitMemory: "overcommit_memory" in SysctlVm is What::Constant("0\n");
    VmMaxMapCount: "max_map_count" in SysctlVm is What::Constant("65530\n");
    FsFileMax: "file-max" in SysctlFs is What::Constant("9223372036854775807\n");
    FsNrOpen: "nr_open" in SysctlFs is What::Constant("1048576\n");
    NetCore: "core" in SysctlNet is What::Directory;
    NetIpv4: "ipv4" in SysctlNet is What::Directory;
    CoreSomaxconn: "somaxconn" in NetCore is What::Generated(Generated::Somaxconn);
    Ipv4PortRange: "ip_local_port_range" in NetIpv4 is What::Generated(Generated::PortRange);

    SysDevices: "devices" in Sys is What::Directory;
    SysDevicesSystem: "system" in SysDevices is What::Directory;
    SysCpu: "cpu" in SysDevicesSystem is What::Directory;
    CpuOnline: "online" in SysCpu is What::Copy("/kernel/devices/system/cpu/online");
    CpuPossible: "possible" in SysCpu is What::Copy("/kernel/devices/system/cpu/possible");
    CpuPresent: "present" in SysCpu is What::Copy("/kernel/devices/system/cpu/present");
    CpuKernelMax: "kernel_max" in SysCpu is What::Constant("255\n");
    SysClass: "class" in Sys is What::Directory;
    SysClassNet: "net" in SysClass is What::Directory;
    SysFs: "fs" in Sys is What::Directory;
    SysFsCgroup: "cgroup" in SysFs is What::Directory;
    CgroupControllers: "cgroup.controllers" in SysFsCgroup is What::Constant("cpu memory pids\n");
    CgroupCpuMax: "cpu.max" in SysFsCgroup is What::Constant("max 100000\n");
    CgroupMemoryMax: "memory.max" in SysFsCgroup is What::Constant("max\n");
    CgroupPidsMax: "pids.max" in SysFsCgroup is What::Constant("max\n");
    CgroupMemoryCurrent: "memory.current" in SysFsCgroup
        is What::Generated(Generated::MemoryCurrent);
    SysKernel: "kernel" in Sys is What::Directory;
    SysKernelMm: "mm" in SysKernel is What::Directory;
    SysKernelThp: "transparent_hugepage" in SysKernelMm is What::Directory;
    ThpEnabled: "enabled" in SysKernelThp is What::Constant("always [madvise] never\n");
}

impl Fixed {
    /// The fixed nodes in this directory.
    pub fn children(self) -> impl Iterator<Item = Fixed> {
        Fixed::ALL
            .iter()
            .copied()
            .filter(move |child| *child != Fixed::Root && child.parent() == self)
    }
}

/// The files and directories of a process, and of each of its threads.
/// The numbers are the entries' own in a node's number: the first 16 are
/// those a thread has too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Entry {
    /// The directory itself: `/proc/<n>`, or `task/<n>`.
    Dir = 0,
    /// `cmdline`.
    Cmdline = 1,
    /// `comm`.
    Comm = 2,
    /// `environ`.
    Environ = 3,
    /// `status`.
    Status = 4,
    /// `stat`.
    Stat = 5,
    /// `statm`.
    Statm = 6,
    /// `maps`.
    Maps = 7,
    /// `mounts`.
    Mounts = 8,
    /// `mountinfo`.
    Mountinfo = 9,
    /// `cgroup`.
    Cgroup = 10,
    /// `limits`.
    Limits = 11,
    /// `oom_score_adj`.
    OomScoreAdj = 12,
    /// `exe`, a link.
    Exe = 13,
    /// `cwd`, a link.
    Cwd = 14,
    /// `root`, a link.
    Root = 15,
    /// `fd`, a directory of links.
    Fd = 16,
    /// `task`, a directory of threads.
    Task = 17,
    /// `net`, a directory.
    Net = 18,
    /// `net/dev`.
    NetDev = 19,
    /// `net/route`.
    NetRoute = 20,
    /// `net/tcp`.
    NetTcp = 21,
    /// `net/udp`.
    NetUdp = 22,
    /// `net/tcp6`.
    NetTcp6 = 23,
    /// `net/udp6`.
    NetUdp6 = 24,
    /// `net/if_inet6`.
    NetIfInet6 = 25,
    /// `net/ipv6_route`.
    NetIpv6Route = 26,
}

impl Entry {
    /// Every entry, in the order of their numbers.
    const ALL: [Entry; 27] = [
        Entry::Dir,
        Entry::Cmdline,
        Entry::Comm,
        Entry::Environ,
        Entry::Status,
        Entry::Stat,
        Entry::Statm,
        Entry::Maps,
        Entry::Mounts,
        Entry::Mountinfo,
        Entry::Cgroup,
        Entry::Limits,
        Entry::OomScoreAdj,
        Entry::Exe,
        Entry::Cwd,
        Entry::Root,
        Entry::Fd,
        Entry::Task,
        Entry::Net,
        Entry::NetDev,
        Entry::NetRoute,
        Entry::NetTcp,
        Entry::NetUdp,
        Entry::NetTcp6,
        Entry::NetUdp6,
        Entry::NetIfInet6,
        Entry::NetIpv6Route,
    ];

    /// How many entries a thread's directory has: all but its own.
    const THREAD_ENTRIES: usize = 16;

    /// The entry numbered `number`.
    fn of(number: u64) -> Option<Entry> {
        Entry::ALL.get(usize::try_from(number).ok()?).copied()
    }

    /// The files and links of a thread's directory, which are a
    /// process's too.
    pub fn thread_files() -> &'static [Entry] {
        &Entry::ALL[1..Entry::THREAD_ENTRIES]
    }

    /// What a process's directory holds, in the order it is listed.
    pub fn process_children() -> &'static [Entry] {
        &Entry::ALL[1..=Entry::Net as usize]
    }

    /// The files of `net`.
    pub fn net_files() -> &'static [Entry] {
        &Entry::ALL[Entry::NetDev as usize..]
    }

    /// The name in its directory.
    pub fn name(self) -> &'static str {
        match self {
            Entry::Dir => "",
            Entry::Cmdline => "cmdline",
            Entry::Comm => "comm",
            Entry::Environ => "environ",
            Entry::Status => "status",
            Entry::Stat => "stat",
            Entry::Statm => "statm",
            Entry::Maps => "maps",
            Entry::Mounts => "mounts",
            Entry::Mountinfo => "mountinfo",
            Entry::Cgroup => "cgroup",
            Entry::Limits => "limits",
            Entry::OomScoreAdj => "oom_score_adj",
            Entry::Exe => "exe",
            Entry::Cwd => "cwd",
            Entry::Root => "root",
            Entry::Fd => "fd",
            Entry::Task => "task",
            Entry::Net => "net",
            Entry::NetDev => "dev",
            Entry::NetRoute => "route",
            Entry::NetTcp => "tcp",
            Entry::NetUdp => "udp",
            Entry::NetTcp6 => "tcp6",
            Entry::NetUdp6 => "udp6",
            Entry::NetIfInet6 => "if_inet6",
            Entry::NetIpv6Route => "ipv6_route",
        }
    }

    /// What it is.
    pub fn kind(self) -> Kind {
        match self {
            Entry::Dir | Entry::Fd | Entry::Task | Entry::Net => Kind::Directory,
            Entry::Exe | Entry::Cwd | Entry::Root => Kind::Link,
            _ => Kind::File,
        }
    }

    /// Whether a thread's directory has it.
    fn of_thread(self) -> bool {
        (self as usize) < Entry::THREAD_ENTRIES
    }

    /// The kernel's file a net entry copies.
    pub fn net_path(self) -> Option<&'static str> {
        Some(match self {
            Entry::NetDev => "/processes/net/dev",
            Entry::NetRoute => "/processes/net/route",
            Entry::NetTcp => "/processes/net/tcp",
            Entry::NetUdp => "/processes/net/udp",
            Entry::NetTcp6 => "/processes/net/tcp6",
            Entry::NetUdp6 => "/processes/net/udp6",
            Entry::NetIfInet6 => "/processes/net/if_inet6",
            Entry::NetIpv6Route => "/processes/net/ipv6_route",
            _ => return None,
        })
    }
}

/// The directory and files of a CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CpuEntry {
    /// `cpu<N>`.
    Dir = 0,
    /// `topology`.
    Topology = 1,
    /// `topology/core_id`.
    CoreId = 2,
    /// `topology/physical_package_id`.
    PhysicalPackageId = 3,
    /// `topology/thread_siblings_list`.
    ThreadSiblingsList = 4,
    /// `topology/core_siblings_list`.
    CoreSiblingsList = 5,
}

impl CpuEntry {
    const ALL: [CpuEntry; 6] = [
        CpuEntry::Dir,
        CpuEntry::Topology,
        CpuEntry::CoreId,
        CpuEntry::PhysicalPackageId,
        CpuEntry::ThreadSiblingsList,
        CpuEntry::CoreSiblingsList,
    ];

    /// The files of `topology`.
    pub fn topology_files() -> &'static [CpuEntry] {
        &CpuEntry::ALL[2..]
    }

    /// The name in its directory.
    pub fn name(self) -> &'static str {
        match self {
            CpuEntry::Dir => "",
            CpuEntry::Topology => "topology",
            CpuEntry::CoreId => "core_id",
            CpuEntry::PhysicalPackageId => "physical_package_id",
            CpuEntry::ThreadSiblingsList => "thread_siblings_list",
            CpuEntry::CoreSiblingsList => "core_siblings_list",
        }
    }
}

/// The directory and files of a network interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum InterfaceEntry {
    /// The interface's directory.
    Dir = 0,
    /// `address`.
    Address = 1,
    /// `mtu`.
    Mtu = 2,
    /// `operstate`.
    Operstate = 3,
    /// `ifindex`.
    Ifindex = 4,
    /// `flags`.
    Flags = 5,
    /// `type`.
    Type = 6,
    /// `carrier`.
    Carrier = 7,
}

impl InterfaceEntry {
    const ALL: [InterfaceEntry; 8] = [
        InterfaceEntry::Dir,
        InterfaceEntry::Address,
        InterfaceEntry::Mtu,
        InterfaceEntry::Operstate,
        InterfaceEntry::Ifindex,
        InterfaceEntry::Flags,
        InterfaceEntry::Type,
        InterfaceEntry::Carrier,
    ];

    /// The files of an interface's directory.
    pub fn files() -> &'static [InterfaceEntry] {
        &InterfaceEntry::ALL[1..]
    }

    /// The name in its directory, which is the name of the kernel's file
    /// it copies.
    pub fn name(self) -> &'static str {
        match self {
            InterfaceEntry::Dir => "",
            InterfaceEntry::Address => "address",
            InterfaceEntry::Mtu => "mtu",
            InterfaceEntry::Operstate => "operstate",
            InterfaceEntry::Ifindex => "ifindex",
            InterfaceEntry::Flags => "flags",
            InterfaceEntry::Type => "type",
            InterfaceEntry::Carrier => "carrier",
        }
    }
}

/// A node, named by what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Node {
    /// One that is always there.
    Fixed(Fixed),
    /// A file or directory of process `pid`.
    Process {
        /// The process's number.
        pid: u32,
        /// Which of its files.
        entry: Entry,
    },
    /// The link `fd/<fd>` of process `pid`.
    Descriptor {
        /// The process's number.
        pid: u32,
        /// The descriptor.
        fd: u32,
    },
    /// A file or directory of thread `tid` of process `pid`.
    Thread {
        /// The process's number.
        pid: u32,
        /// The thread's number.
        tid: u32,
        /// Which of its files.
        entry: Entry,
    },
    /// A file or directory of CPU `cpu`.
    Cpu {
        /// The CPU's number.
        cpu: u32,
        /// Which of its files.
        entry: CpuEntry,
    },
    /// A file or directory of the network interface numbered `index`.
    Interface {
        /// The interface's number, which Linux's `ifindex` gives.
        index: u32,
        /// Which of its files.
        entry: InterfaceEntry,
    },
    /// The file of netlink socket `id`.
    Socket(u64),
}

/// The class of a node number, in its top two bits below the sockets'.
const CLASS_SHIFT: u32 = 60;
/// Descriptors' class, threads' and CPUs' and interfaces'.
const CLASS_DESCRIPTOR: u64 = 1 << CLASS_SHIFT;
const CLASS_THREAD: u64 = 2 << CLASS_SHIFT;
const CLASS_OTHER: u64 = 3 << CLASS_SHIFT;
/// Bit 56 of the CPUs' and interfaces' class says which.
const INTERFACE_BIT: u64 = 1 << 56;
const _: () = assert!(NODE_BASE == 1 << (CLASS_SHIFT + 2));
const _: () = assert!(Fixed::ALL.len() < 256);

/// Process numbers below this have a node, as a Linux `pid_t` is.
const PROCESS_LIMIT: u32 = 1 << 31;
/// A process or thread number in a thread's node is below this.
const THREAD_LIMIT: u32 = 1 << 28;
/// Descriptors, CPUs and interfaces are numbered below this.
const INDEX_LIMIT: u32 = 1 << 24;

/// `number` as a `u32` in `first..limit`.
fn within(number: u64, first: u32, limit: u32) -> Option<u32> {
    u32::try_from(number).ok().filter(|number| (first..limit).contains(number))
}

impl Node {
    /// A file or directory of process `pid`, if the process has a node.
    pub fn process(pid: u64, entry: Entry) -> Option<Node> {
        Some(Node::Process { pid: within(pid, 1, PROCESS_LIMIT)?, entry })
    }

    /// The descriptor `fd` of process `pid`, if it has a node.
    pub fn descriptor(pid: u64, fd: u64) -> Option<Node> {
        Some(Node::Descriptor {
            pid: within(pid, 1, PROCESS_LIMIT)?,
            fd: within(fd, 0, INDEX_LIMIT)?,
        })
    }

    /// A file or directory of thread `tid` of process `pid`, if the
    /// thread has a node. `entry` is one a thread has.
    pub fn thread(pid: u64, tid: u64, entry: Entry) -> Option<Node> {
        if !entry.of_thread() {
            return None;
        }
        let (pid, tid) = (within(pid, 1, THREAD_LIMIT)?, within(tid, 1, THREAD_LIMIT)?);
        Some(Node::Thread { pid, tid, entry })
    }

    /// A file or directory of CPU `cpu`, if it has a node.
    pub fn cpu(cpu: u64, entry: CpuEntry) -> Option<Node> {
        Some(Node::Cpu { cpu: within(cpu, 0, INDEX_LIMIT)?, entry })
    }

    /// A file or directory of the interface numbered `index`, if it has a
    /// node.
    pub fn interface(index: u64, entry: InterfaceEntry) -> Option<Node> {
        Some(Node::Interface { index: within(index, 1, INDEX_LIMIT)?, entry })
    }

    /// The node's number.
    pub fn number(self) -> u64 {
        match self {
            Node::Fixed(fixed) => fixed as u64 + 1,
            Node::Process { pid, entry } => u64::from(pid) << 8 | entry as u64,
            Node::Descriptor { pid, fd } => CLASS_DESCRIPTOR | u64::from(pid) << 24 | u64::from(fd),
            Node::Thread { pid, tid, entry } => {
                CLASS_THREAD | u64::from(pid) << 32 | u64::from(tid) << 4 | entry as u64
            }
            Node::Cpu { cpu, entry } => CLASS_OTHER | u64::from(cpu) << 8 | entry as u64,
            Node::Interface { index, entry } => {
                CLASS_OTHER | INTERFACE_BIT | u64::from(index) << 8 | entry as u64
            }
            Node::Socket(id) => netlink::node_of(id),
        }
    }

    /// The node that has number `number`, if any node does.
    pub fn of_number(number: u64) -> Option<Node> {
        if let Some(id) = netlink::id_of(number) {
            return Some(Node::Socket(id));
        }
        let low = |bits: u32| number & ((1 << bits) - 1);
        let field = |first: u32, bits: u32| (number >> first) & ((1 << bits) - 1);
        let node = match number >> CLASS_SHIFT {
            0 if number < 256 => {
                let index = usize::try_from(number).ok()?.checked_sub(1)?;
                Fixed::ALL.get(index).copied().map(Node::Fixed)
            }
            0 => Node::process(number >> 8, Entry::of(low(8))?),
            1 => Node::descriptor(low(CLASS_SHIFT) >> 24, low(24)),
            2 => Node::thread(low(CLASS_SHIFT) >> 32, field(4, 28), Entry::of(low(4))?),
            _ if number & INTERFACE_BIT != 0 => {
                let entry = InterfaceEntry::ALL.get(usize::try_from(low(8)).ok()?).copied()?;
                Node::interface(field(8, 24), entry)
            }
            _ => {
                let entry = CpuEntry::ALL.get(usize::try_from(low(8)).ok()?).copied()?;
                Node::cpu(field(8, 24), entry)
            }
        };
        // Bits no field holds are zero in a number that is a node's.
        node.filter(|node| node.number() == number)
    }

    /// What the node is.
    pub fn kind(self) -> Kind {
        match self {
            Node::Fixed(fixed) => match fixed.what() {
                What::Directory => Kind::Directory,
                What::Link(_) => Kind::Link,
                _ => Kind::File,
            },
            Node::Process { entry, .. } | Node::Thread { entry, .. } => entry.kind(),
            Node::Descriptor { .. } => Kind::Link,
            Node::Cpu { entry: CpuEntry::Dir | CpuEntry::Topology, .. } => Kind::Directory,
            Node::Interface { entry: InterfaceEntry::Dir, .. } => Kind::Directory,
            Node::Cpu { .. } | Node::Interface { .. } | Node::Socket(_) => Kind::File,
        }
    }

    /// The directory the node is in; the root's is the root.
    pub fn parent(self) -> Node {
        match self {
            Node::Fixed(fixed) => Node::Fixed(fixed.parent()),
            Node::Process { entry: Entry::Dir, .. } => Node::Fixed(Fixed::Proc),
            Node::Process { pid, entry } if entry.net_path().is_some() => {
                Node::Process { pid, entry: Entry::Net }
            }
            Node::Process { pid, .. } => Node::Process { pid, entry: Entry::Dir },
            Node::Descriptor { pid, .. } => Node::Process { pid, entry: Entry::Fd },
            Node::Thread { pid, entry: Entry::Dir, .. } => {
                Node::Process { pid, entry: Entry::Task }
            }
            Node::Thread { pid, tid, .. } => Node::Thread { pid, tid, entry: Entry::Dir },
            Node::Cpu { entry: CpuEntry::Dir, .. } => Node::Fixed(Fixed::SysCpu),
            Node::Cpu { cpu, entry: CpuEntry::Topology } => Node::Cpu { cpu, entry: CpuEntry::Dir },
            Node::Cpu { cpu, .. } => Node::Cpu { cpu, entry: CpuEntry::Topology },
            Node::Interface { entry: InterfaceEntry::Dir, .. } => Node::Fixed(Fixed::SysClassNet),
            Node::Interface { index, .. } => Node::Interface { index, entry: InterfaceEntry::Dir },
            Node::Socket(_) => Node::Fixed(Fixed::Netlink),
        }
    }

    /// How long the kernel may keep what it is told of the node: not at
    /// all for `self`, `thread-self` and what is under a process's
    /// directory, where the answer changes with who asks or when, and a
    /// second for the rest.
    pub fn timeout(self) -> Duration {
        match self {
            Node::Fixed(Fixed::ProcSelf | Fixed::ProcThreadSelf)
            | Node::Process { .. }
            | Node::Descriptor { .. }
            | Node::Thread { .. } => Duration::ZERO,
            _ => Duration::from_secs(1),
        }
    }

    /// The node named `name` in this directory, if the name is one a
    /// directory of this kind has: a process's number in `/proc`, `cpu3`
    /// in a CPU's directory, and the fixed names of the rest. Whether the
    /// process, thread or CPU is there is for the caller to find out; an
    /// interface's name is the kernel's, so its directory is not looked
    /// in here.
    pub fn child(self, name: &str) -> Option<Node> {
        let fixed_child =
            |parent: Fixed| parent.children().find(|child| child.name() == name).map(Node::Fixed);
        match self {
            Node::Fixed(Fixed::Proc) => match decimal(name) {
                Some(pid) => Node::process(pid, Entry::Dir),
                None => fixed_child(Fixed::Proc),
            },
            Node::Fixed(Fixed::SysCpu) => match name.strip_prefix("cpu").and_then(decimal) {
                Some(cpu) => Node::cpu(cpu, CpuEntry::Dir),
                None => fixed_child(Fixed::SysCpu),
            },
            Node::Fixed(Fixed::Netlink) => Some(Node::Socket(decimal(name)?)),
            Node::Fixed(fixed) => fixed_child(fixed),
            Node::Process { pid, entry: Entry::Dir } => Entry::process_children()
                .iter()
                .find(|entry| entry.name() == name)
                .and_then(|&entry| Node::process(u64::from(pid), entry)),
            Node::Process { pid, entry: Entry::Fd } => {
                Node::descriptor(u64::from(pid), decimal(name)?)
            }
            Node::Process { pid, entry: Entry::Task } => {
                Node::thread(u64::from(pid), decimal(name)?, Entry::Dir)
            }
            Node::Process { pid, entry: Entry::Net } => Entry::net_files()
                .iter()
                .find(|entry| entry.name() == name)
                .and_then(|&entry| Node::process(u64::from(pid), entry)),
            Node::Thread { pid, tid, entry: Entry::Dir } => Entry::thread_files()
                .iter()
                .find(|entry| entry.name() == name)
                .and_then(|&entry| Node::thread(u64::from(pid), u64::from(tid), entry)),
            Node::Cpu { cpu, entry: CpuEntry::Dir } => (name == CpuEntry::Topology.name())
                .then_some(Node::Cpu { cpu, entry: CpuEntry::Topology }),
            Node::Cpu { cpu, entry: CpuEntry::Topology } => CpuEntry::topology_files()
                .iter()
                .find(|entry| entry.name() == name)
                .map(|&entry| Node::Cpu { cpu, entry }),
            Node::Interface { index, entry: InterfaceEntry::Dir } => InterfaceEntry::files()
                .iter()
                .find(|entry| entry.name() == name)
                .map(|&entry| Node::Interface { index, entry }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    /// Numbers that reach each field's ends.
    const NUMBERS: [u64; 6] = [1, 2, 255, 256, 65_535, (1 << 28) - 1];

    /// A node of every kind and entry, for each of a few numbers.
    fn samples() -> Vec<Node> {
        let mut nodes: Vec<Node> = Fixed::ALL.iter().copied().map(Node::Fixed).collect();
        for &pid in &NUMBERS {
            nodes.extend(Entry::ALL.iter().filter_map(|&entry| Node::process(pid, entry)));
            nodes.extend(
                [0, 1, 4095, (1 << 24) - 1].iter().filter_map(|&fd| Node::descriptor(pid, fd)),
            );
            for &tid in &NUMBERS {
                nodes.extend(Entry::ALL.iter().filter_map(|&entry| Node::thread(pid, tid, entry)));
            }
            nodes.extend(CpuEntry::ALL.iter().filter_map(|&entry| Node::cpu(pid - 1, entry)));
            nodes.extend(
                InterfaceEntry::ALL.iter().filter_map(|&entry| Node::interface(pid, entry)),
            );
        }
        nodes.extend([Node::Process { pid: (1 << 31) - 1, entry: Entry::Status }]);
        nodes.extend([0, 1, 1 << 40].map(Node::Socket));
        nodes
    }

    #[test]
    fn every_node_has_a_number_of_its_own_that_names_it_again() {
        let nodes = samples();
        let mut seen = HashSet::new();
        for node in &nodes {
            let number = node.number();
            assert!(seen.insert(number), "{node:?} shares {number}");
            if !matches!(node, Node::Socket(_)) {
                assert!(number < NODE_BASE, "{node:?} is {number:#x}");
            }
            assert_eq!(Node::of_number(number), Some(*node), "{number:#x}");
        }
        assert!(nodes.len() > 500);
    }

    #[test]
    fn the_numbers_the_kernel_remembers_stay_what_they_are() {
        assert_eq!(Node::Fixed(Fixed::Root).number(), 1);
        assert_eq!(Node::Fixed(Fixed::Proc).number(), 2);
        assert_eq!(Node::process(1, Entry::Dir).map(Node::number), Some(0x100));
        assert_eq!(Node::process(4242, Entry::Status).map(Node::number), Some(4242 << 8 | 4));
        assert_eq!(Node::descriptor(7, 3).map(Node::number), Some(1 << 60 | 7 << 24 | 3));
        assert_eq!(
            Node::thread(7, 9, Entry::Comm).map(Node::number),
            Some(2 << 60 | 7 << 32 | 9 << 4 | 2)
        );
        assert_eq!(Node::cpu(3, CpuEntry::CoreId).map(Node::number), Some(3 << 60 | 3 << 8 | 2));
        assert_eq!(
            Node::interface(2, InterfaceEntry::Mtu).map(Node::number),
            Some(3 << 60 | 1 << 56 | 2 << 8 | 2)
        );
        assert_eq!(Node::Socket(5).number(), NODE_BASE + 5);
    }

    #[test]
    fn numbers_that_name_nothing_are_refused() {
        let past_fixed = Fixed::ALL.len() as u64 + 1;
        for number in [
            0,
            past_fixed,
            255,
            0x1c8,
            0x3000_0100_0000_0000,
            0x2000_0000_0000_0010,
            0x1000_0000_0000_0000,
        ] {
            assert_eq!(Node::of_number(number), None, "{number:#x}");
        }
    }

    #[test]
    fn a_number_too_large_for_its_field_has_no_node() {
        assert_eq!(Node::process(0, Entry::Dir), None);
        assert_eq!(Node::process(1 << 31, Entry::Dir), None);
        assert_eq!(Node::thread(1 << 28, 1, Entry::Dir), None);
        assert_eq!(Node::thread(1, 1 << 28, Entry::Dir), None);
        assert_eq!(Node::thread(1, 1, Entry::Fd), None);
        assert_eq!(Node::descriptor(1, 1 << 24), None);
        assert_eq!(Node::interface(0, InterfaceEntry::Dir), None);
    }

    #[test]
    fn names_lead_down_and_parents_lead_back() {
        let proc = Node::Fixed(Fixed::Proc);
        let process = proc.child("4242").unwrap();
        assert_eq!(process, Node::process(4242, Entry::Dir).unwrap());
        assert_eq!(proc.child("007"), None);
        assert_eq!(proc.child("cpuinfo"), Some(Node::Fixed(Fixed::ProcCpuinfo)));
        let status = process.child("status").unwrap();
        assert_eq!(status.parent(), process);
        let fd = process.child("fd").unwrap().child("12").unwrap();
        assert_eq!(fd, Node::descriptor(4242, 12).unwrap());
        assert_eq!(fd.parent().parent(), process);
        let thread = process.child("task").unwrap().child("4250").unwrap();
        assert_eq!(thread.parent().parent(), process);
        let thread_comm = thread.child("comm").unwrap();
        assert_eq!(thread_comm.parent(), thread);
        assert_eq!(thread.child("task"), None);
        let route = process.child("net").unwrap().child("route").unwrap();
        assert_eq!(route.parent().parent(), process);
        let cpu = Node::Fixed(Fixed::SysCpu).child("cpu12").unwrap();
        let siblings = cpu.child("topology").unwrap().child("core_siblings_list").unwrap();
        assert_eq!(siblings.parent().parent(), cpu);
        assert_eq!(cpu.parent(), Node::Fixed(Fixed::SysCpu));
        assert_eq!(Node::Fixed(Fixed::SysCpu).child("online"), Some(Node::Fixed(Fixed::CpuOnline)));
        assert_eq!(Node::Fixed(Fixed::Netlink).child("17"), Some(Node::Socket(17)));
        assert_eq!(Node::Socket(17).parent(), Node::Fixed(Fixed::Netlink));
    }

    #[test]
    fn a_fixed_node_is_found_by_its_name_in_its_directory() {
        for &fixed in Fixed::ALL.iter().filter(|fixed| **fixed != Fixed::Root) {
            let parent = Node::Fixed(fixed.parent());
            let found = parent.child(fixed.name());
            assert_eq!(found, Some(Node::Fixed(fixed)), "{}", fixed.name());
            assert_eq!(Node::Fixed(fixed).parent(), parent);
        }
    }

    #[test]
    fn what_a_node_is_follows_its_entry() {
        assert_eq!(Node::process(1, Entry::Dir).unwrap().kind(), Kind::Directory);
        assert_eq!(Node::process(1, Entry::Exe).unwrap().kind(), Kind::Link);
        assert_eq!(Node::process(1, Entry::Maps).unwrap().kind(), Kind::File);
        assert_eq!(Node::descriptor(1, 0).unwrap().kind(), Kind::Link);
        assert_eq!(Node::Fixed(Fixed::ProcSelf).kind(), Kind::Link);
        assert_eq!(Node::Fixed(Fixed::ProcMounts).kind(), Kind::Link);
        assert_eq!(Node::Fixed(Fixed::ProcMeminfo).kind(), Kind::File);
        assert_eq!(Node::cpu(0, CpuEntry::Topology).unwrap().kind(), Kind::Directory);
        assert_eq!(Node::Socket(1).kind(), Kind::File);
    }

    #[test]
    fn what_changes_with_who_asks_is_never_kept() {
        assert_eq!(Node::Fixed(Fixed::ProcSelf).timeout(), Duration::ZERO);
        assert_eq!(Node::Fixed(Fixed::ProcThreadSelf).timeout(), Duration::ZERO);
        assert_eq!(Node::process(3, Entry::Status).unwrap().timeout(), Duration::ZERO);
        assert_eq!(Node::process(3, Entry::Dir).unwrap().timeout(), Duration::ZERO);
        assert_eq!(Node::descriptor(3, 1).unwrap().timeout(), Duration::ZERO);
        assert_eq!(Node::thread(3, 3, Entry::Stat).unwrap().timeout(), Duration::ZERO);
        assert_eq!(Node::Fixed(Fixed::ProcMeminfo).timeout(), Duration::from_secs(1));
        assert_eq!(Node::Fixed(Fixed::Proc).timeout(), Duration::from_secs(1));
        assert_eq!(Node::cpu(0, CpuEntry::Dir).unwrap().timeout(), Duration::from_secs(1));
    }
}
