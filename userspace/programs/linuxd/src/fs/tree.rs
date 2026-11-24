// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The tree: what is in each directory, what each node's attributes are,
//! and what a file or link holds, made from what [`Kernel`] reads of the
//! machine and what [`Netlinks`] keeps.
//!
//! [`super::LinuxFs`] answers the kernel's requests through it, and its
//! methods are free of the FUSE protocol so that the host's tests walk the
//! tree on a [`super::kernel::Fake`] machine. A process's files are the
//! kernel's `/processes/<n>` read afresh at each call: a process that has
//! ended is a node that fails with ENOENT, as Linux's does.
//!
//! When a path under `/proc` or `/sys` answers wrongly, find the node in
//! [`super::node`]'s table, then the generator in [`super::task`],
//! [`super::system`] or [`super::cpu`] it names, and read the kernel's
//! file it is made from (radix-proc(5), radix-sysfs(5)).

use std::sync::Arc;
use std::time::SystemTime;

use radix_abi::{file, names, network};

use super::cpu::Processor;
use super::kernel::{Kernel, errno_of};
use super::mounts;
use super::node::{CpuEntry, Entry, Fixed, Generated, InterfaceEntry, Kind, Node, Target, What};
use super::records::{Descriptor, Process, Thread};
use super::system::{self, Population};
use super::task::{self, Task};
use super::text::{decimal, number};
use crate::netlink::Netlinks;

/// What a failed call answers: an errno.
pub type Errno = i32;

/// Where a machine keeps its name.
const HOSTNAME_FILE: &str = "/system/settings/hostname";

/// Where the places in a listing of the names that come and go start: the
/// tree's fixed names come before.
const DYNAMIC_COOKIE: i64 = 1000;

/// What a node is, for `getattr` and `lookup`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attr {
    /// The node.
    pub node: Node,
    /// What it is.
    pub kind: Kind,
    /// The bytes of a link's target, and 0 for the rest: a file's size is
    /// known only when it is opened, as on Linux.
    pub size: u64,
    /// Its owner.
    pub user: u32,
    /// Its group.
    pub group: u32,
    /// Its permission bits.
    pub mode: u16,
}

/// A name in a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Child {
    /// Its place in the listing, which a `READDIR` that goes on from it
    /// asks after.
    pub cookie: i64,
    /// Its name.
    pub name: String,
    /// The node it names.
    pub node: Node,
}

/// The tree of `/proc`, `/sys` and `.netlink`.
pub struct Tree<K: Kernel> {
    kernel: K,
    netlinks: Arc<Netlinks>,
    /// The file the launcher wrote the sandbox's mounts to.
    mounts: Option<String>,
    /// The user and group of linuxd, who owns the socket files.
    owner: (u32, u32),
}

impl<K: Kernel> Tree<K> {
    /// The tree of a machine, whose sockets are `netlinks`' and whose
    /// mounts are in the file `mounts` names, if any; linuxd runs as
    /// `owner`, a user and group.
    pub fn new(
        kernel: K,
        netlinks: Arc<Netlinks>,
        mounts: Option<String>,
        owner: (u32, u32),
    ) -> Tree<K> {
        Tree { kernel, netlinks, mounts, owner }
    }

    /// The bytes of the kernel's file `path`.
    fn read(&self, path: &str) -> Result<Vec<u8>, Errno> {
        self.kernel.read(path).map_err(|error| errno_of(&error))
    }

    /// The names in the kernel's directory `path`.
    fn names(&self, path: &str) -> Result<Vec<String>, Errno> {
        self.kernel.names(path).map_err(|error| errno_of(&error))
    }

    /// When the kernel started, which every node's times say: the one
    /// time the kernel keeps for the machine.
    pub fn boot_time(&self) -> SystemTime {
        let nanoseconds =
            self.read("/processes/stat").ok().and_then(|text| number(&text, "boot_time_ns"));
        SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(nanoseconds.unwrap_or(0))
    }

    /// The process `pid`, as the kernel describes it: ENOENT once it has
    /// gone.
    fn process(&self, pid: u32) -> Result<Process, Errno> {
        self.read(&format!("/processes/{pid}/status")).map(|status| Process::parse(&status))
    }

    /// The threads of process `pid`.
    fn threads(&self, pid: u32) -> Result<Vec<Thread>, Errno> {
        self.read(&format!("/processes/{pid}/threads")).map(|text| Thread::parse_all(&text))
    }

    /// The file descriptors of process `pid`.
    fn descriptors(&self, pid: u32) -> Result<Vec<Descriptor>, Errno> {
        self.read(&format!("/processes/{pid}/files")).map(|text| Descriptor::parse_all(&text))
    }

    /// How many CPUs the kernel runs.
    fn cpus(&self) -> Result<u32, Errno> {
        let text = self.read("/kernel/cpus")?;
        let count = std::str::from_utf8(&text).ok().and_then(|text| text.trim().parse().ok());
        count.filter(|&count| count > 0).ok_or(libc::EIO)
    }

    /// The network interfaces, as the kernel numbers and names them,
    /// in the order of their numbers.
    fn interfaces(&self) -> Result<Vec<(u32, String)>, Errno> {
        let mut found = Vec::new();
        for name in self.names("/kernel/class/net")? {
            let text = self.read(&format!("/kernel/class/net/{name}/ifindex"))?;
            let index = std::str::from_utf8(&text).ok().and_then(|text| text.trim().parse().ok());
            if let Some(index) = index {
                found.push((index, name));
            }
        }
        found.sort();
        Ok(found)
    }

    /// The interface numbered `index`.
    fn interface_name(&self, index: u32) -> Result<String, Errno> {
        let interfaces = self.interfaces()?;
        let (_, name) =
            interfaces.into_iter().find(|(number, _)| *number == index).ok_or(libc::ENOENT)?;
        Ok(name)
    }

    /// The processor linuxd runs on.
    fn processor(&self) -> Processor {
        Processor::read(&|leaf, sub| self.kernel.cpuid(leaf, sub))
    }

    /// The text of the sandbox's mounts.
    fn mounts_text(&self) -> Result<Vec<u8>, Errno> {
        match &self.mounts {
            Some(path) => self.read(path),
            None => Ok(mounts::ROOT_ONLY.as_bytes().to_vec()),
        }
    }

    /// The node named `name` in directory `parent`, whether or not it is
    /// there.
    fn child_node(&self, parent: Node, name: &str) -> Result<Node, Errno> {
        if parent == Node::Fixed(Fixed::SysClassNet) {
            let interfaces = self.interfaces()?;
            let (index, _) =
                interfaces.into_iter().find(|(_, found)| found == name).ok_or(libc::ENOENT)?;
            return Node::interface(u64::from(index), InterfaceEntry::Dir).ok_or(libc::ENOENT);
        }
        parent.child(name).ok_or(libc::ENOENT)
    }

    /// The node named `name` in directory `parent`, and its attributes,
    /// for the process `asker`.
    pub fn lookup(&self, parent: Node, name: &str, asker: u64) -> Result<Attr, Errno> {
        if parent.kind() != Kind::Directory {
            return Err(libc::ENOTDIR);
        }
        let node = match name {
            "." => parent,
            ".." => parent.parent(),
            name => self.child_node(parent, name)?,
        };
        self.attr(node, asker)
    }

    /// The attributes of `node`, for the process `asker`; ENOENT if the
    /// process, thread, descriptor, CPU, interface or socket it belongs
    /// to is gone.
    pub fn attr(&self, node: Node, asker: u64) -> Result<Attr, Errno> {
        let kind = node.kind();
        let (user, group, size) = match node {
            Node::Fixed(fixed) => (0, 0, fixed_size(fixed, asker)),
            Node::Process { pid, entry } => {
                let process = self.process(pid)?;
                let size = process_link_size(&process, entry, kind);
                (process.user, process.group, size)
            }
            Node::Thread { pid, tid, entry } => {
                let process = self.process(pid)?;
                if !self.threads(pid)?.iter().any(|thread| thread.number == u64::from(tid)) {
                    return Err(libc::ENOENT);
                }
                let size = process_link_size(&process, entry, kind);
                (process.user, process.group, size)
            }
            Node::Descriptor { pid, fd } => {
                let process = self.process(pid)?;
                let target = self.descriptor_target(pid, fd)?;
                (process.user, process.group, target.len() as u64)
            }
            Node::Cpu { cpu, .. } => {
                if cpu >= self.cpus()? {
                    return Err(libc::ENOENT);
                }
                (0, 0, 0)
            }
            Node::Interface { index, .. } => {
                self.interface_name(index)?;
                (0, 0, 0)
            }
            Node::Socket(id) => {
                if !self.netlinks.exists(id) {
                    return Err(libc::ENOENT);
                }
                (self.owner.0, self.owner.1, 0)
            }
        };
        let (user, group) =
            if node == Node::Fixed(Fixed::Netlink) { self.owner } else { (user, group) };
        Ok(Attr { node, kind, size, user, group, mode: mode_of(node, kind) })
    }

    /// Where descriptor `fd` of process `pid` leads.
    fn descriptor_target(&self, pid: u32, fd: u32) -> Result<Vec<u8>, Errno> {
        let descriptors = self.descriptors(pid)?;
        let descriptor =
            descriptors.iter().find(|descriptor| descriptor.fd == fd).ok_or(libc::ENOENT)?;
        Ok(descriptor.target())
    }

    /// What the link `node` holds, for the process `asker`.
    pub fn readlink(&self, node: Node, asker: u64) -> Result<Vec<u8>, Errno> {
        match node {
            Node::Fixed(fixed) => match fixed.what() {
                What::Link(target) => Ok(target_of(target, asker)),
                _ => Err(libc::EINVAL),
            },
            Node::Process { pid, entry } | Node::Thread { pid, entry, .. }
                if entry.kind() == Kind::Link =>
            {
                process_link(&self.process(pid)?, entry)
            }
            Node::Descriptor { pid, fd } => self.descriptor_target(pid, fd),
            _ => Err(libc::EINVAL),
        }
    }

    /// The names in directory `dir`, in the order of their places.
    pub fn children(&self, dir: Node) -> Result<Vec<Child>, Errno> {
        if dir.kind() != Kind::Directory {
            return Err(libc::ENOTDIR);
        }
        let mut children = match dir {
            Node::Fixed(fixed) => self.fixed_children(fixed)?,
            Node::Process { pid, entry: Entry::Dir } => {
                self.process(pid)?;
                listed(
                    Entry::process_children()
                        .iter()
                        .map(|e| (e.name().to_string(), Node::process(u64::from(pid), *e))),
                )
            }
            Node::Process { pid, entry: Entry::Fd } => self.fd_children(pid)?,
            Node::Process { pid, entry: Entry::Task } => self.task_children(pid)?,
            Node::Process { pid, entry: Entry::Net } => listed(
                Entry::net_files()
                    .iter()
                    .map(|e| (e.name().to_string(), Node::process(u64::from(pid), *e))),
            ),
            Node::Thread { pid, tid, entry: Entry::Dir } => {
                listed(Entry::thread_files().iter().map(|e| {
                    (e.name().to_string(), Node::thread(u64::from(pid), u64::from(tid), *e))
                }))
            }
            Node::Cpu { cpu, entry: CpuEntry::Dir } => listed([(
                CpuEntry::Topology.name().to_string(),
                Node::cpu(u64::from(cpu), CpuEntry::Topology),
            )]),
            Node::Cpu { cpu, entry: CpuEntry::Topology } => listed(
                CpuEntry::topology_files()
                    .iter()
                    .map(|e| (e.name().to_string(), Node::cpu(u64::from(cpu), *e))),
            ),
            Node::Interface { index, entry: InterfaceEntry::Dir } => listed(
                InterfaceEntry::files()
                    .iter()
                    .map(|e| (e.name().to_string(), Node::interface(u64::from(index), *e))),
            ),
            // The rest of the directories are those of nodes that are
            // never listed, such as a socket's.
            _ => return Err(libc::ENOTDIR),
        };
        children.sort_by_key(|child| child.cookie);
        Ok(children)
    }

    /// The children of a fixed directory, and what the machine adds to
    /// it: the processes, the CPUs, the interfaces and the sockets.
    fn fixed_children(&self, dir: Fixed) -> Result<Vec<Child>, Errno> {
        let mut children = listed(
            dir.children().map(|fixed| (fixed.name().to_string(), Some(Node::Fixed(fixed)))),
        );
        match dir {
            Fixed::Proc => {
                for name in self.names("/processes")? {
                    let pid = decimal(&name);
                    let node = pid.and_then(|pid| Node::process(pid, Entry::Dir));
                    children.extend(node.map(|node| Child {
                        cookie: place(pid.unwrap_or(0)),
                        name,
                        node,
                    }));
                }
            }
            Fixed::SysCpu => {
                for cpu in 0..self.cpus()? {
                    let node = Node::cpu(u64::from(cpu), CpuEntry::Dir);
                    children.extend(node.map(|node| Child {
                        cookie: place(u64::from(cpu)),
                        name: format!("cpu{cpu}"),
                        node,
                    }));
                }
            }
            Fixed::SysClassNet => {
                for (index, name) in self.interfaces()? {
                    let node = Node::interface(u64::from(index), InterfaceEntry::Dir);
                    children.extend(node.map(|node| Child {
                        cookie: place(u64::from(index)),
                        name,
                        node,
                    }));
                }
            }
            Fixed::Netlink => {
                let mut ids = self.netlinks.ids();
                ids.sort_unstable();
                for id in ids {
                    children.push(Child {
                        cookie: place(id),
                        name: id.to_string(),
                        node: Node::Socket(id),
                    });
                }
            }
            _ => {}
        }
        Ok(children)
    }

    /// The descriptors of a process, as the links of `fd`.
    fn fd_children(&self, pid: u32) -> Result<Vec<Child>, Errno> {
        let mut children = Vec::new();
        for descriptor in self.descriptors(pid)? {
            let node = Node::descriptor(u64::from(pid), u64::from(descriptor.fd));
            children.extend(node.map(|node| Child {
                cookie: place(u64::from(descriptor.fd)),
                name: descriptor.fd.to_string(),
                node,
            }));
        }
        Ok(children)
    }

    /// The threads of a process, as the directories of `task`.
    fn task_children(&self, pid: u32) -> Result<Vec<Child>, Errno> {
        let mut children = Vec::new();
        for thread in self.threads(pid)? {
            let node = Node::thread(u64::from(pid), thread.number, Entry::Dir);
            children.extend(node.map(|node| Child {
                cookie: place(thread.number),
                name: thread.number.to_string(),
                node,
            }));
        }
        Ok(children)
    }

    /// The bytes of file `node` as they are now, which a program that
    /// opens it reads from: its snapshot.
    pub fn open(&self, node: Node) -> Result<Vec<u8>, Errno> {
        match node.kind() {
            Kind::Directory => return Err(libc::EISDIR),
            Kind::Link => return Err(libc::ELOOP),
            Kind::File => {}
        }
        match node {
            Node::Fixed(fixed) => match fixed.what() {
                What::Constant(text) => Ok(text.as_bytes().to_vec()),
                What::Copy(path) => self.read(path),
                What::Generated(generated) => self.generate(generated).map(String::into_bytes),
                What::Directory | What::Link(_) => Err(libc::EINVAL),
            },
            Node::Process { pid, entry } => self.process_file(pid, None, entry),
            Node::Thread { pid, tid, entry } => self.process_file(pid, Some(tid), entry),
            Node::Cpu { cpu, entry } => self.cpu_file(cpu, entry).map(String::into_bytes),
            Node::Interface { index, entry } => {
                let name = self.interface_name(index)?;
                self.read(&format!("/kernel/class/net/{name}/{}", entry.name()))
            }
            Node::Descriptor { .. } | Node::Socket(_) => Err(libc::EINVAL),
        }
    }

    /// A file of process `pid`, or of its thread `tid`.
    fn process_file(&self, pid: u32, tid: Option<u32>, entry: Entry) -> Result<Vec<u8>, Errno> {
        match entry {
            Entry::Cmdline => self.read(&format!("/processes/{pid}/cmdline")),
            // The kernel keeps no environment for a program once it has
            // started it, so there is none to show.
            Entry::Environ => Ok(Vec::new()),
            Entry::Maps => self.read(&format!("/processes/{pid}/maps")),
            Entry::Mounts => self.mounts_text(),
            Entry::Mountinfo => {
                let text = self.mounts_text()?;
                Ok(mounts::mountinfo(&String::from_utf8_lossy(&text)).into_bytes())
            }
            Entry::Cgroup => Ok(b"0::/\n".to_vec()),
            Entry::Limits => Ok(task::limits().into_bytes()),
            Entry::OomScoreAdj => Ok(b"0\n".to_vec()),
            Entry::Comm | Entry::Status | Entry::Stat | Entry::Statm => {
                self.task_file(pid, tid, entry).map(String::into_bytes)
            }
            entry => match entry.net_path() {
                Some(path) => self.read(path),
                None => Err(libc::EINVAL),
            },
        }
    }

    /// `comm`, `status`, `stat` or `statm` of a process or thread.
    fn task_file(&self, pid: u32, tid: Option<u32>, entry: Entry) -> Result<String, Errno> {
        let process = self.process(pid)?;
        let threads = self.threads(pid)?;
        let task = match tid {
            Some(tid) => {
                let thread = threads.iter().find(|thread| thread.number == u64::from(tid));
                Task::of_thread(&process, thread.ok_or(libc::ENOENT)?)
            }
            None => Task::of_process(&process, &threads),
        };
        match entry {
            Entry::Comm => Ok(task::comm(&task)),
            Entry::Status => Ok(task::status(&task, self.cpus()?)),
            Entry::Stat => Ok(task::stat(&task)),
            _ => Ok(task::statm(&task)),
        }
    }

    /// A file of a CPU's topology.
    fn cpu_file(&self, cpu: u32, entry: CpuEntry) -> Result<String, Errno> {
        let cpus = self.cpus()?;
        let topology = self.processor().topology_of(cpu, cpus);
        match entry {
            CpuEntry::CoreId => Ok(topology.core_id),
            CpuEntry::PhysicalPackageId => Ok(topology.physical_package_id),
            CpuEntry::ThreadSiblingsList => Ok(topology.thread_siblings_list),
            CpuEntry::CoreSiblingsList => Ok(topology.core_siblings_list),
            CpuEntry::Dir | CpuEntry::Topology => Err(libc::EISDIR),
        }
    }

    /// The text of a fixed file that is made when it is opened.
    fn generate(&self, generated: Generated) -> Result<String, Errno> {
        let processes = || self.names("/processes");
        Ok(match generated {
            Generated::Cpuinfo => self.processor().cpuinfo(self.cpus()?),
            Generated::Meminfo => system::meminfo(&self.read("/processes/meminfo")?),
            Generated::Stat => {
                let listed = processes()?.iter().filter(|name| decimal(name).is_some()).count();
                system::stat(&self.read("/processes/stat")?, listed)
            }
            Generated::Loadavg => system::loadavg(
                &self.read("/processes/loadavg")?,
                &self.read("/processes/stat")?,
                self.population()?,
            ),
            Generated::Uptime => {
                system::uptime(&self.read("/processes/uptime")?, &self.read("/processes/stat")?)
            }
            Generated::Version => system::version(),
            Generated::Ostype => format!("{}\n", system::OSTYPE),
            Generated::Osrelease => format!("{}\n", system::RELEASE),
            Generated::KernelVersion => format!("{}\n", system::VERSION),
            Generated::Filesystems => system::filesystems(),
            Generated::Hostname => {
                system::hostname(self.kernel.read(HOSTNAME_FILE).ok().as_deref())
            }
            Generated::ThreadsMax => system::threads_max(&self.read("/processes/meminfo")?),
            Generated::BootId => system::boot_id(&self.read("/processes/stat")?),
            Generated::Uuid => {
                let mut bytes = [0u8; 16];
                self.kernel.random(&mut bytes).map_err(|error| errno_of(&error))?;
                system::uuid(bytes)
            }
            Generated::NgroupsMax => format!("{}\n", names::GROUPS_MAX),
            Generated::Somaxconn => format!("{}\n", file::BACKLOG_MAX),
            Generated::PortRange => format!("{}\t65535\n", network::EPHEMERAL_FIRST),
            Generated::MemoryCurrent => system::memory_current(&self.read("/processes/meminfo")?),
        })
    }

    /// The threads of every process, and the newest process, for
    /// `loadavg`. A process that ends meanwhile is left out.
    fn population(&self) -> Result<Population, Errno> {
        let mut population = Population::default();
        for pid in self.names("/processes")?.iter().filter_map(|name| decimal(name)) {
            let Ok(status) = self.read(&format!("/processes/{pid}/status")) else { continue };
            population.threads += number(&status, "threads").unwrap_or(0);
            population.newest = population.newest.max(pid);
        }
        Ok(population)
    }
}

/// Names with their nodes as a listing, the places counted from 3: `.`
/// and `..` have 1 and 2. A name with no node is left out.
fn listed(names: impl IntoIterator<Item = (String, Option<Node>)>) -> Vec<Child> {
    let mut children = Vec::new();
    for (name, node) in names {
        let Some(node) = node else { continue };
        children.push(Child { cookie: 3 + children.len() as i64, name, node });
    }
    children
}

/// The place in a listing of the name of a process, thread, descriptor,
/// CPU, interface or socket: by its number, so that a listing that goes
/// on after a call finds the same place whatever has come and gone.
fn place(number: u64) -> i64 {
    DYNAMIC_COOKIE.saturating_add(i64::try_from(number).unwrap_or(i64::MAX))
}

/// The size of a fixed node: a link's target, which `self` makes of the
/// asker's number.
fn fixed_size(fixed: Fixed, asker: u64) -> u64 {
    match fixed.what() {
        What::Link(target) => target_of(target, asker).len() as u64,
        _ => 0,
    }
}

/// The size of a link of a process's, `exe`, `cwd` or `root`: the target's
/// length where it has one, else 0.
fn process_link_size(process: &Process, entry: Entry, kind: Kind) -> u64 {
    if kind != Kind::Link {
        return 0;
    }
    process_link(process, entry).map_or(0, |target| target.len() as u64)
}

/// The text of the link `target` for the process `asker`.
fn target_of(target: Target, asker: u64) -> Vec<u8> {
    match target {
        Target::Path(path) => path.as_bytes().to_vec(),
        Target::Requester => asker.to_string().into_bytes(),
        Target::RequesterThread => format!("{asker}/task/{asker}").into_bytes(),
    }
}

/// Where `exe`, `cwd` or `root` of `process` lead: ENOENT for a program
/// or directory the kernel does not know, as Linux's do for a process
/// with none.
fn process_link(process: &Process, entry: Entry) -> Result<Vec<u8>, Errno> {
    match entry {
        Entry::Exe => process.program.clone().ok_or(libc::ENOENT),
        Entry::Cwd => process.directory.clone().ok_or(libc::ENOENT),
        Entry::Root => Ok(b"/".to_vec()),
        _ => Err(libc::EINVAL),
    }
}

/// The permission bits of `node`: as Linux's: 0555 for directories, 0444
/// for files and 0777 for links, and 0600 for a socket's file, which only
/// linuxd uses. A process's environment would be 0400.
fn mode_of(node: Node, kind: Kind) -> u16 {
    match (node, kind) {
        (Node::Socket(_), _) => 0o600,
        (Node::Fixed(Fixed::Netlink), _) => 0o500,
        (
            Node::Process { entry: Entry::Environ, .. }
            | Node::Thread { entry: Entry::Environ, .. },
            _,
        ) => 0o400,
        (_, Kind::Directory) => 0o555,
        (_, Kind::File) => 0o444,
        (_, Kind::Link) => 0o777,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::kernel::Fake;
    use super::super::node::{Entry, Fixed};
    use super::*;
    use crate::netlink::NoFiles;

    const STAT: &str = "cpu0: user_ns=1000000000 system_ns=500000000 idle_ns=9000000000 \
        switches=7\ncpus: 1\nuser_ns: 1000000000\nsystem_ns: 500000000\n\
        idle_ns: 9000000000\nswitches: 7\nrunning: 1\ndisk_waiting: 0\n\
        boot_time_ns: 1700000000123456789\n";

    const STATUS: &str = "number: 4242\nname: shell\nstate: running\nthreads: 2\nuser: 1000\n\
        user_group: 100\nparent: 1\ngroup: 4242\nsession: 4242\nrun: sleeping\n\
        directory: /home/me\n";

    const FILES: &str = "fd=0 kind=char id=3 path=/dev/null\nfd=3 kind=pipe id=9\n\
        fd=4 kind=file id=11 path=/tmp/a\\040b\nfd=7 kind=timer id=12\n";

    const THREADS: &str = "number=4242 run=sleeping cpu=0 name=main\n\
        number=4250 run=running cpu=1 name=worker\n";

    fn machine() -> Fake {
        Fake::new()
            .with("/kernel/cpus", "4\n")
            .with("/kernel/devices/system/cpu/online", "0-3\n")
            .with("/processes/stat", STAT)
            .with("/processes/meminfo", "total: 4194304 KiB\nfree: 2097152 KiB\n")
            .with("/processes/loadavg", "load_1: 0.05\nload_5: 0.10\nload_15: 1.25\n")
            .with("/processes/uptime", "uptime_ns: 123456789012\n")
            .with("/processes/net/dev", "Inter-|   Receive\n")
            .with("/processes/net/route", "Iface\tDestination\n")
            .with("/processes/4242/status", STATUS)
            .with("/processes/4242/cmdline", "shell\0-l\0")
            .with("/processes/4242/maps", "00400000-00401000 r-xp 00000000 00:00 0 /bin/sh\n")
            .with("/processes/4242/files", FILES)
            .with("/processes/4242/threads", THREADS)
            .with("/processes/4300/status", "number: 4300\nname: other\nthreads: 1\nuser: 0\n")
            .with("/processes/4300/threads", "number=4300 run=sleeping cpu=0 name=other\n")
            .with("/kernel/class/net/net0/ifindex", "2\n")
            .with("/kernel/class/net/net0/address", "52:54:00:12:34:56\n")
            .with("/kernel/class/net/net0/mtu", "1500\n")
            .with("/kernel/class/net/lo/ifindex", "1\n")
            .with("/kernel/class/net/lo/mtu", "65536\n")
            .with("/system/settings/hostname", "workstation\n")
    }

    fn tree() -> Tree<Fake> {
        Tree::new(machine(), Arc::new(Netlinks::new(Box::new(NoFiles))), None, (1000, 100))
    }

    /// An Intel processor of 4 logical CPUs: 2 cores of 2 threads.
    fn cpuid(leaf: u32, sub: u32) -> [u32; 4] {
        match (leaf, sub) {
            (0, _) => [0xb, 0x756e_6547, 0x6c65_746e, 0x4965_6e69],
            (0xb, 0) => [1, 2, 0x100, 0],
            (0xb, 1) => [4, 4, 0x201, 0],
            _ => [0; 4],
        }
    }

    fn names(children: &[Child]) -> Vec<&str> {
        children.iter().map(|child| child.name.as_str()).collect()
    }

    fn text(tree: &Tree<Fake>, node: Node) -> String {
        String::from_utf8(tree.open(node).unwrap()).unwrap()
    }

    fn process(entry: Entry) -> Node {
        Node::process(4242, entry).unwrap()
    }

    #[test]
    fn the_root_holds_proc_sys_and_the_sockets() {
        let children = tree().children(Node::Fixed(Fixed::Root)).unwrap();
        assert_eq!(names(&children), ["proc", "sys", ".netlink"]);
        assert_eq!(children[2].node.number(), Node::Fixed(Fixed::Netlink).number());
    }

    #[test]
    fn proc_lists_its_files_then_the_processes_by_number() {
        let children = tree().children(Node::Fixed(Fixed::Proc)).unwrap();
        let names = names(&children);
        assert_eq!(
            &names[..13],
            [
                "self",
                "thread-self",
                "cpuinfo",
                "meminfo",
                "stat",
                "loadavg",
                "uptime",
                "version",
                "filesystems",
                "mounts",
                "swaps",
                "net",
                "sys"
            ]
        );
        assert_eq!(&names[13..], ["4242", "4300"]);
        let cookies: Vec<i64> = children.iter().map(|child| child.cookie).collect();
        assert!(cookies.windows(2).all(|pair| pair[0] < pair[1]), "{cookies:?}");
        assert!(cookies.iter().all(|&cookie| cookie > 2));
    }

    #[test]
    fn self_is_the_number_of_who_asks() {
        let tree = tree();
        let link = Node::Fixed(Fixed::ProcSelf);
        assert_eq!(tree.readlink(link, 77).unwrap(), b"77");
        assert_eq!(tree.attr(link, 1234).unwrap().size, 4);
        assert_eq!(tree.readlink(Node::Fixed(Fixed::ProcThreadSelf), 77).unwrap(), b"77/task/77");
        assert_eq!(tree.readlink(Node::Fixed(Fixed::ProcNet), 77).unwrap(), b"self/net");
        assert_eq!(tree.readlink(Node::Fixed(Fixed::ProcMounts), 77).unwrap(), b"self/mounts");
        assert_eq!(tree.readlink(Node::Fixed(Fixed::ProcMeminfo), 1), Err(libc::EINVAL));
    }

    #[test]
    fn a_process_directory_is_its_users() {
        let tree = tree();
        let found = tree.lookup(Node::Fixed(Fixed::Proc), "4242", 1).unwrap();
        assert_eq!(found.node, process(Entry::Dir));
        assert_eq!(
            (found.kind, found.user, found.group, found.mode),
            (Kind::Directory, 1000, 100, 0o555)
        );
        let status = tree.lookup(found.node, "status", 1).unwrap();
        assert_eq!((status.kind, status.user, status.mode), (Kind::File, 1000, 0o444));
        assert_eq!(tree.lookup(found.node, "environ", 1).unwrap().mode, 0o400);
        assert_eq!(tree.lookup(found.node, "exe", 1).unwrap().mode, 0o777);
    }

    #[test]
    fn dot_and_dot_dot_are_the_directory_and_the_one_above() {
        let tree = tree();
        let found = tree.lookup(process(Entry::Dir), ".", 1).unwrap();
        assert_eq!(found.node, process(Entry::Dir));
        let up = tree.lookup(process(Entry::Dir), "..", 1).unwrap();
        assert_eq!(up.node, Node::Fixed(Fixed::Proc));
        let root = tree.lookup(Node::Fixed(Fixed::Root), "..", 1).unwrap();
        assert_eq!(root.node, Node::Fixed(Fixed::Root));
    }

    #[test]
    fn what_is_not_there_is_not_found() {
        let tree = tree();
        let proc = Node::Fixed(Fixed::Proc);
        assert_eq!(tree.lookup(proc, "9999", 1), Err(libc::ENOENT));
        assert_eq!(tree.lookup(proc, "nothing", 1), Err(libc::ENOENT));
        assert_eq!(tree.lookup(proc, "0042", 1), Err(libc::ENOENT));
        assert_eq!(tree.lookup(Node::Fixed(Fixed::ProcMeminfo), "x", 1), Err(libc::ENOTDIR));
        assert_eq!(tree.attr(Node::process(9999, Entry::Dir).unwrap(), 1), Err(libc::ENOENT));
        assert_eq!(tree.lookup(Node::Fixed(Fixed::Netlink), "5", 1), Err(libc::ENOENT));
        assert_eq!(tree.lookup(Node::Fixed(Fixed::SysCpu), "cpu9", 1), Err(libc::ENOENT));
    }

    #[test]
    fn a_process_that_has_ended_is_gone_from_every_file() {
        let mut tree = tree();
        tree.kernel.remove("/processes/4242/status");
        assert_eq!(tree.attr(process(Entry::Dir), 1), Err(libc::ENOENT));
        assert_eq!(tree.open(process(Entry::Status)), Err(libc::ENOENT));
        assert_eq!(tree.readlink(process(Entry::Exe), 1), Err(libc::ENOENT));
    }

    #[test]
    fn a_process_directory_holds_the_files_of_proc_5() {
        let children = tree().children(process(Entry::Dir)).unwrap();
        assert_eq!(
            names(&children),
            [
                "cmdline",
                "comm",
                "environ",
                "status",
                "stat",
                "statm",
                "maps",
                "mounts",
                "mountinfo",
                "cgroup",
                "limits",
                "oom_score_adj",
                "exe",
                "cwd",
                "root",
                "fd",
                "task",
                "net"
            ]
        );
    }

    #[test]
    fn the_net_directory_copies_the_kernels() {
        let tree = tree();
        let children = tree.children(process(Entry::Net)).unwrap();
        assert_eq!(
            names(&children),
            ["dev", "route", "tcp", "udp", "tcp6", "udp6", "if_inet6", "ipv6_route"]
        );
        assert_eq!(text(&tree, process(Entry::NetDev)), "Inter-|   Receive\n");
        assert_eq!(text(&tree, process(Entry::NetRoute)), "Iface\tDestination\n");
        // A file the kernel has not is that error.
        assert_eq!(tree.open(process(Entry::NetTcp)), Err(libc::ENOENT));
    }

    #[test]
    fn files_of_a_process_are_the_kernels_or_fixed() {
        let tree = tree();
        assert_eq!(tree.open(process(Entry::Cmdline)).unwrap(), b"shell\0-l\0");
        assert_eq!(tree.open(process(Entry::Environ)).unwrap(), b"");
        assert_eq!(text(&tree, process(Entry::Cgroup)), "0::/\n");
        assert_eq!(text(&tree, process(Entry::OomScoreAdj)), "0\n");
        assert_eq!(
            text(&tree, process(Entry::Maps)),
            "00400000-00401000 r-xp 00000000 00:00 0 /bin/sh\n"
        );
        assert_eq!(text(&tree, process(Entry::Comm)), "shell\n");
        assert!(text(&tree, process(Entry::Limits)).starts_with("Limit "));
        assert!(text(&tree, process(Entry::Status)).starts_with("Name:\tshell\n"));
        assert!(text(&tree, process(Entry::Stat)).starts_with("4242 (shell) S 1 4242 4242 0 "));
        assert!(text(&tree, process(Entry::Statm)).ends_with(" 0\n"));
    }

    #[test]
    fn the_maps_of_a_process_the_kernel_will_not_show_are_that_error() {
        let tree = Tree::new(
            machine().with("/processes/4300/threads", ""),
            Arc::new(Netlinks::new(Box::new(NoFiles))),
            None,
            (0, 0),
        );
        let maps = Node::process(4300, Entry::Maps).unwrap();
        assert_eq!(tree.open(maps), Err(libc::ENOENT));
    }

    #[test]
    fn the_mounts_are_the_launchers_or_the_root_alone() {
        let alone = tree();
        assert_eq!(text(&alone, process(Entry::Mounts)), "rootfs / rootfs rw 0 0\n");
        assert_eq!(text(&alone, process(Entry::Mountinfo)), "1 0 0:1 / / rw - rootfs rootfs rw\n");
        let mounts = "rootfs / rootfs rw 0 0\nproc /proc proc ro,nosuid 0 0\n";
        let launched = Tree::new(
            machine().with("/run/mounts", mounts),
            Arc::new(Netlinks::new(Box::new(NoFiles))),
            Some("/run/mounts".into()),
            (0, 0),
        );
        assert_eq!(text(&launched, process(Entry::Mounts)), mounts);
        let info = text(&launched, process(Entry::Mountinfo));
        assert_eq!(info.lines().count(), 2);
        assert!(info.lines().nth(1).unwrap().starts_with("2 1 0:2 / /proc ro,nosuid"));
    }

    #[test]
    fn the_links_of_a_process_lead_where_it_works() {
        let tree = tree();
        assert_eq!(tree.readlink(process(Entry::Cwd), 1).unwrap(), b"/home/me");
        assert_eq!(tree.readlink(process(Entry::Root), 1).unwrap(), b"/");
        // Without a program path in its status, a process has no exe.
        assert_eq!(tree.readlink(process(Entry::Exe), 1), Err(libc::ENOENT));
        assert_eq!(tree.attr(process(Entry::Cwd), 1).unwrap().size, 8);
        let with_program = Tree::new(
            machine().with("/processes/4242/status", &format!("{STATUS}program: /bin/sh\n")),
            Arc::new(Netlinks::new(Box::new(NoFiles))),
            None,
            (0, 0),
        );
        assert_eq!(with_program.readlink(process(Entry::Exe), 1).unwrap(), b"/bin/sh");
    }

    #[test]
    fn the_descriptors_are_links_named_by_number() {
        let tree = tree();
        let children = tree.children(process(Entry::Fd)).unwrap();
        assert_eq!(names(&children), ["0", "3", "4", "7"]);
        let target = |fd: &str| {
            let found = tree.lookup(process(Entry::Fd), fd, 1).unwrap();
            (found.size, tree.readlink(found.node, 1).unwrap())
        };
        assert_eq!(target("0"), (9, b"/dev/null".to_vec()));
        assert_eq!(target("3"), (8, b"pipe:[9]".to_vec()));
        assert_eq!(target("4"), (8, b"/tmp/a b".to_vec()));
        assert_eq!(target("7"), (18, b"anon_inode:[timer]".to_vec()));
        assert_eq!(tree.lookup(process(Entry::Fd), "5", 1), Err(libc::ENOENT));
    }

    #[test]
    fn the_threads_are_directories_of_task() {
        let tree = tree();
        let children = tree.children(process(Entry::Task)).unwrap();
        assert_eq!(names(&children), ["4242", "4250"]);
        let worker = tree.lookup(process(Entry::Task), "4250", 1).unwrap();
        assert_eq!((worker.kind, worker.user, worker.mode), (Kind::Directory, 1000, 0o555));
        let files = tree.children(worker.node).unwrap();
        assert_eq!(files.len(), 15);
        assert_eq!(names(&files)[..3], ["cmdline", "comm", "environ"]);
        let comm = tree.lookup(worker.node, "comm", 1).unwrap();
        assert_eq!(text(&tree, comm.node), "worker\n");
        assert_eq!(tree.lookup(process(Entry::Task), "4299", 1), Err(libc::ENOENT));
        // A thread's directory has none of the process's directories.
        assert_eq!(tree.lookup(worker.node, "task", 1), Err(libc::ENOENT));
    }

    #[test]
    fn what_a_process_shows_is_never_kept_by_the_kernel() {
        let tree = tree();
        let status = tree.attr(process(Entry::Status), 1).unwrap();
        assert_eq!(status.node.timeout(), Duration::ZERO);
        let meminfo = tree.attr(Node::Fixed(Fixed::ProcMeminfo), 1).unwrap();
        assert_eq!(meminfo.node.timeout(), Duration::from_secs(1));
        assert_eq!((meminfo.user, meminfo.group, meminfo.mode), (0, 0, 0o444));
    }

    #[test]
    fn the_files_of_the_machine_are_made_from_the_kernels() {
        let tree = tree();
        assert!(
            text(&tree, Node::Fixed(Fixed::ProcMeminfo))
                .starts_with("MemTotal:        4194304 kB\n")
        );
        assert!(
            text(&tree, Node::Fixed(Fixed::ProcStat)).contains("\nbtime 1700000000\nprocesses 2\n")
        );
        assert_eq!(text(&tree, Node::Fixed(Fixed::ProcUptime)), "123.45 9.00\n");
        assert_eq!(text(&tree, Node::Fixed(Fixed::ProcLoadavg)), "0.05 0.10 1.25 1/3 4300\n");
        assert_eq!(text(&tree, Node::Fixed(Fixed::ProcSwaps)), system::SWAPS);
        assert_eq!(text(&tree, Node::Fixed(Fixed::KernelOstype)), "Linux\n");
        assert_eq!(text(&tree, Node::Fixed(Fixed::KernelHostname)), "workstation\n");
        assert_eq!(text(&tree, Node::Fixed(Fixed::KernelDomainname)), "(none)\n");
        assert_eq!(
            text(&tree, Node::Fixed(Fixed::RandomUuid)),
            "5a5a5a5a-5a5a-4a5a-9a5a-5a5a5a5a5a5a\n"
        );
        assert_eq!(text(&tree, Node::Fixed(Fixed::KernelNgroupsMax)), "32\n");
        assert_eq!(text(&tree, Node::Fixed(Fixed::CoreSomaxconn)), "4096\n");
        assert_eq!(text(&tree, Node::Fixed(Fixed::Ipv4PortRange)), "49152\t65535\n");
        assert_eq!(text(&tree, Node::Fixed(Fixed::CpuOnline)), "0-3\n");
        assert_eq!(
            text(&tree, Node::Fixed(Fixed::CgroupMemoryCurrent)),
            format!("{}\n", 2_097_152u64 * 1024)
        );
        assert_eq!(text(&tree, Node::Fixed(Fixed::ThpEnabled)), "always [madvise] never\n");
        assert_eq!(tree.open(Node::Fixed(Fixed::Proc)), Err(libc::EISDIR));
        assert_eq!(tree.open(Node::Fixed(Fixed::ProcSelf)), Err(libc::ELOOP));
    }

    #[test]
    fn the_cpus_are_directories_of_sys() {
        let tree = Tree::new(
            machine().with_cpuid(cpuid),
            Arc::new(Netlinks::new(Box::new(NoFiles))),
            None,
            (1000, 100),
        );
        let children = tree.children(Node::Fixed(Fixed::SysCpu)).unwrap();
        assert_eq!(
            names(&children),
            ["online", "possible", "present", "kernel_max", "cpu0", "cpu1", "cpu2", "cpu3"]
        );
        let cpu = tree.lookup(Node::Fixed(Fixed::SysCpu), "cpu3", 1).unwrap();
        let topology = tree.lookup(cpu.node, "topology", 1).unwrap();
        let files = tree.children(topology.node).unwrap();
        assert_eq!(
            names(&files),
            ["core_id", "physical_package_id", "thread_siblings_list", "core_siblings_list"]
        );
        let core = tree.lookup(topology.node, "core_id", 1).unwrap();
        assert_eq!(text(&tree, core.node), "1\n");
        let siblings = tree.lookup(topology.node, "thread_siblings_list", 1).unwrap();
        assert_eq!(text(&tree, siblings.node), "2-3\n");
        let package = tree.lookup(topology.node, "core_siblings_list", 1).unwrap();
        assert_eq!(text(&tree, package.node), "0-3\n");
        assert_eq!(tree.lookup(Node::Fixed(Fixed::SysCpu), "cpu4", 1), Err(libc::ENOENT));
    }

    #[test]
    fn the_interfaces_are_numbered_as_the_kernel_numbers_them() {
        let tree = tree();
        let class = Node::Fixed(Fixed::SysClassNet);
        let children = tree.children(class).unwrap();
        assert_eq!(names(&children), ["lo", "net0"]);
        let net0 = tree.lookup(class, "net0", 1).unwrap();
        assert_eq!(net0.node, Node::interface(2, InterfaceEntry::Dir).unwrap());
        assert_eq!(names(&tree.children(net0.node).unwrap()).len(), 7);
        let address = tree.lookup(net0.node, "address", 1).unwrap();
        assert_eq!(text(&tree, address.node), "52:54:00:12:34:56\n");
        let mtu = tree.lookup(net0.node, "mtu", 1).unwrap();
        assert_eq!(text(&tree, mtu.node), "1500\n");
        assert_eq!(tree.lookup(class, "eth9", 1), Err(libc::ENOENT));
        // An interface that goes takes its directory with it.
        let gone = Node::interface(5, InterfaceEntry::Dir).unwrap();
        assert_eq!(tree.attr(gone, 1), Err(libc::ENOENT));
    }

    #[test]
    fn the_socket_files_are_linuxds_and_private() {
        let tree = tree();
        let dir = tree.attr(Node::Fixed(Fixed::Netlink), 1).unwrap();
        assert_eq!((dir.user, dir.group, dir.mode), (1000, 100, 0o500));
        assert!(tree.children(Node::Fixed(Fixed::Netlink)).unwrap().is_empty());
        assert_eq!(tree.attr(Node::Socket(3), 1), Err(libc::ENOENT));
        assert_eq!(tree.open(Node::Socket(3)), Err(libc::EINVAL));
    }

    #[test]
    fn a_file_has_no_children() {
        let tree = tree();
        assert_eq!(tree.children(Node::Fixed(Fixed::ProcMeminfo)), Err(libc::ENOTDIR));
        assert_eq!(tree.children(Node::Socket(1)), Err(libc::ENOTDIR));
    }

    #[test]
    fn the_boot_time_is_the_kernels() {
        let boot = tree().boot_time().duration_since(SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!(boot.as_nanos(), 1_700_000_000_123_456_789);
    }
}
