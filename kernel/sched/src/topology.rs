// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The CPU topology, and the decisions that depend on it: where a waking
//! thread should run, and which CPU an underloaded one should pull from.
//!
//! The kernel describes each CPU by its core (hardware threads on one core
//! share it), its package (whose cores share the last-level cache) and its
//! NUMA node, and gives the distances between nodes (ACPI's SLIT; 10 is
//! local). From that come the *levels* of each CPU: the CPUs sharing its
//! core, its package, its node, and the whole machine. Each level holds
//! the one below: a package that spans nodes (AMD's NPS modes, or a
//! virtual machine's made-up topology) counts as one package per node,
//! since its last-level caches do not span nodes either. Placement and
//! balancing walk the levels from the closest out, so a thread moves to a
//! CPU that shares its caches before one that does not, and to a near node
//! before a far one. Moving between nodes also waits longer between
//! attempts ([`interval_ticks`]) and prefers threads whose caches have gone
//! cold ([`is_cache_hot`]).
//!
//! Loads are counts of runnable threads; weights are left to the fair queues
//! on each CPU. Balancing moves only threads waiting in a queue: a thread
//! alone on its CPU stays where it is, but on a machine whose cores differ.
//!
//! On a machine whose cores differ in speed, as Intel's performance and
//! efficient cores do, the topology is *ranked* ([`Topology::set_ranked`]):
//! each CPU's load carries its rank, higher for a faster CPU, and the
//! kernel sets the ranks. A waking thread then goes to the fastest idle
//! CPU it may use, the nearest of them if several are as fast, and a
//! thread running alone on a slower CPU moves to a faster one that is idle
//! ([`Topology::faster_idle`]). Counts of threads still decide what
//! balancing moves. With every rank the same, a ranked topology chooses
//! as an unranked one does.
//!
//! Each CPU's span at each level, and which of its levels add CPUs to the
//! one below, are worked out once, when the topology is complete
//! ([`Topology::finish`]): every wake-up that looks for an idle CPU and
//! every balancing attempt reads them, and working them out costs a look
//! at every CPU, which on a server of a hundred CPUs is thousands of
//! instructions each time.

use alloc::vec::Vec;
use core::alloc::Allocator;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::cpuset::{CpuSet, MAX_CPUS};

/// Most NUMA nodes (as `radix_mm::MAX_NODES`).
pub const MAX_NODES: usize = 8;

/// A topology level, closest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// Hardware threads of one core.
    Core,
    /// Cores of one package, which share its last-level cache.
    Package,
    /// CPUs of one NUMA node.
    Node,
    /// Every CPU.
    Machine,
}

impl Level {
    /// Every level, closest first.
    pub const ALL: [Level; 4] = [Level::Core, Level::Package, Level::Node, Level::Machine];

    /// Its place in [`Level::ALL`].
    pub const fn index(self) -> usize {
        match self {
            Level::Core => 0,
            Level::Package => 1,
            Level::Node => 2,
            Level::Machine => 3,
        }
    }

    /// Lower-case name.
    pub const fn name(self) -> &'static str {
        match self {
            Level::Core => "core",
            Level::Package => "package",
            Level::Node => "node",
            Level::Machine => "machine",
        }
    }

    const fn lower(self) -> Option<Level> {
        match self {
            Level::Core => None,
            Level::Package => Some(Level::Core),
            Level::Node => Some(Level::Package),
            Level::Machine => Some(Level::Node),
        }
    }
}

/// Where one CPU sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Place {
    /// Its package.
    pub package: u32,
    /// Its core, unique within the package.
    pub core: u32,
    /// Its NUMA node.
    pub node: u8,
}

/// The CPUs' loads, by CPU number: a snapshot, as a slice, or read as
/// they are asked for, as the kernel does (a snapshot of every CPU would
/// be kilobytes on the stack of a waking thread).
pub trait Loads {
    /// How busy CPU `cpu` is, if there is such a CPU.
    fn load(&self, cpu: usize) -> Option<CpuLoad>;
}

impl Loads for [CpuLoad] {
    fn load(&self, cpu: usize) -> Option<CpuLoad> {
        self.get(cpu).copied()
    }
}

/// How busy one CPU is, as a snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CpuLoad {
    /// Runnable threads, the running one included; 0 when idle.
    pub running: u32,
    /// The real-time priority of the running thread, 0 when it is fair or
    /// the CPU is idle.
    pub rt_prio: u8,
    /// How fast the CPU is beside the others, higher for faster, which a
    /// ranked topology reads ([`Topology::set_ranked`]).
    pub rank: u8,
}

/// How many timer ticks pass between balancing attempts at `level` on a
/// busy CPU. An idle CPU tries every level at every tick.
pub const fn interval_ticks(level: Level) -> u64 {
    match level {
        Level::Core => 1,
        Level::Package => 2,
        Level::Node => 8,
        Level::Machine => 16,
    }
}

/// Whether a thread that last ran `since_ns` ago probably still has its
/// data in the caches that moving it across `level` would leave behind.
pub const fn is_cache_hot(since_ns: u64, level: Level) -> bool {
    let window = match level {
        Level::Core => 0,
        Level::Package => 500_000,
        Level::Node | Level::Machine => 5_000_000,
    };
    since_ns < window
}

/// Whether moving one thread from a group of `from_cpus` CPUs running
/// `from` threads to a group of `to_cpus` running `to` makes the load more
/// even. It never reverses the imbalance, so two groups cannot pass a
/// thread back and forth (argued in `docs/design/scheduler.md` and checked
/// by the host tests, as `proofs.rs` says).
pub const fn should_pull(from: u32, from_cpus: u32, to: u32, to_cpus: u32) -> bool {
    if from == 0 || from_cpus == 0 || to_cpus == 0 {
        return false;
    }
    (from as u64 - 1) * to_cpus as u64 >= (to as u64 + 1) * from_cpus as u64
}

/// What [`Topology::finish`] works out for one CPU.
#[derive(Clone, Copy)]
struct Known {
    /// Its span at each level, in [`Level::ALL`]'s order.
    spans: [CpuSet; 4],
    /// Bit `i` set when level `Level::ALL[i]` adds CPUs to the one below.
    grows: u8,
}

/// The machine's CPUs, packages and nodes.
pub struct Topology<A: Allocator> {
    cpus: usize,
    places: [Place; MAX_CPUS],
    distance: [[u8; MAX_NODES]; MAX_NODES],
    /// Each CPU's spans and levels, from [`Topology::finish`]. Empty before
    /// it, after a change of the CPUs or their places, or when there was
    /// no memory for them: they are then worked out each time.
    known: Vec<Known, A>,
    /// Whether the CPUs' ranks count ([`Topology::set_ranked`]). Set once
    /// every CPU has told the kernel what kind of core it is, which is
    /// after the topology is in use.
    ranked: AtomicBool,
}

impl<A: Allocator> Topology<A> {
    /// `cpus` CPUs, each its own core in one package on one node, keeping
    /// what [`Topology::finish`] works out in memory from `alloc`.
    pub fn flat_in(cpus: usize, alloc: A) -> Topology<A> {
        let cpus = cpus.min(MAX_CPUS);
        let mut places = [Place { package: 0, core: 0, node: 0 }; MAX_CPUS];
        for (i, place) in places.iter_mut().enumerate() {
            place.core = i as u32;
        }
        let mut distance = [[20u8; MAX_NODES]; MAX_NODES];
        for (n, row) in distance.iter_mut().enumerate() {
            row[n] = 10;
        }
        Topology {
            cpus,
            places,
            distance,
            known: Vec::new_in(alloc),
            ranked: AtomicBool::new(false),
        }
    }

    /// Works out each CPU's spans and levels once, for every later look:
    /// call it when the CPUs and their places are all set. Returns false
    /// when there is no memory for them, which leaves them to be worked
    /// out each time, as before.
    pub fn finish(&mut self) -> bool {
        self.known.clear();
        if self.known.try_reserve_exact(self.cpus).is_err() {
            return false;
        }
        for cpu in 0..self.cpus {
            let known = Known {
                spans: Level::ALL.map(|level| self.span_of(cpu, level)),
                grows: self.grows_of(cpu),
            };
            // Reserved just above.
            let _ = self.known.push_within_capacity(known);
        }
        true
    }

    /// Sets how many CPUs there are.
    pub fn set_cpus(&mut self, cpus: usize) {
        self.cpus = cpus.min(MAX_CPUS);
        self.known.clear();
    }

    /// Records where CPU `cpu` sits.
    pub fn set_place(&mut self, cpu: usize, place: Place) {
        if let Some(slot) = self.places.get_mut(cpu) {
            *slot = Place { node: place.node.min(MAX_NODES as u8 - 1), ..place };
            self.known.clear();
        }
    }

    /// Records the distance between two nodes (10 is local).
    pub fn set_distance(&mut self, from: usize, to: usize, distance: u8) {
        if from < MAX_NODES && to < MAX_NODES {
            self.distance[from][to] = distance;
        }
    }

    /// Makes placement read the CPUs' ranks ([`CpuLoad::rank`]), or stop
    /// reading them: for a machine whose cores differ in speed.
    pub fn set_ranked(&self, ranked: bool) {
        self.ranked.store(ranked, Ordering::Relaxed);
    }

    /// Whether placement reads the CPUs' ranks.
    pub fn is_ranked(&self) -> bool {
        self.ranked.load(Ordering::Relaxed)
    }

    /// How many CPUs there are.
    pub fn cpus(&self) -> usize {
        self.cpus
    }

    /// Where CPU `cpu` sits.
    pub fn place(&self, cpu: usize) -> Place {
        self.places.get(cpu).copied().unwrap_or_default()
    }

    /// The distance from CPU `a`'s node to CPU `b`'s.
    pub fn cpu_distance(&self, a: usize, b: usize) -> u8 {
        let (a, b) = (self.place(a).node as usize, self.place(b).node as usize);
        self.distance[a][b]
    }

    fn shares(&self, a: usize, b: usize, level: Level) -> bool {
        let (pa, pb) = (self.place(a), self.place(b));
        let package = pa.package == pb.package && pa.node == pb.node;
        match level {
            Level::Core => package && pa.core == pb.core,
            Level::Package => package,
            Level::Node => pa.node == pb.node,
            Level::Machine => true,
        }
    }

    /// The CPUs that share `level` with `cpu`.
    pub fn span(&self, cpu: usize, level: Level) -> CpuSet {
        match self.known.get(cpu) {
            Some(known) => known.spans[level.index()],
            None => self.span_of(cpu, level),
        }
    }

    /// [`Topology::span`], worked out from the places.
    fn span_of(&self, cpu: usize, level: Level) -> CpuSet {
        let mut set = CpuSet::EMPTY;
        for other in 0..self.cpus {
            if self.shares(cpu, other, level) {
                set.insert(other);
            }
        }
        set
    }

    /// The levels of `cpu` that add CPUs to the one below, closest first.
    pub fn levels(&self, cpu: usize) -> impl Iterator<Item = Level> + '_ {
        let grows = match self.known.get(cpu) {
            Some(known) => known.grows,
            None => self.grows_of(cpu),
        };
        Level::ALL.into_iter().filter(move |level| grows & (1 << level.index()) != 0)
    }

    /// [`Topology::levels`] as bits of [`Level::ALL`], worked out from the
    /// places.
    fn grows_of(&self, cpu: usize) -> u8 {
        let mut below = CpuSet::single(cpu);
        let mut grows = 0;
        for level in Level::ALL {
            let span = self.span_of(cpu, level).or(&below);
            if span.len() > below.len() {
                grows |= 1 << level.index();
            }
            below = span;
        }
        grows
    }

    /// The CPU an underloaded `this` should pull a thread from, comparing
    /// the groups that make up its span at `level`. `None` when the load
    /// there is even, or no CPU has a thread waiting.
    pub fn find_busiest(
        &self,
        this: usize,
        level: Level,
        loads: &(impl Loads + ?Sized),
    ) -> Option<usize> {
        let span = self.span(this, level);
        let group_of = |cpu: usize| match level.lower() {
            Some(lower) => self.span(cpu, lower).and(&span),
            None => CpuSet::single(cpu),
        };
        let sum = |group: &CpuSet| -> (u32, u32) {
            let running = group.iter().map(|c| loads.load(c).map_or(0, |l| l.running)).sum();
            (running, group.len() as u32)
        };
        let local = group_of(this);
        let (to, to_cpus) = sum(&local);
        let mut seen = local;
        let mut best: Option<(CpuSet, u32, u32, u8)> = None;
        for cpu in span.iter() {
            if seen.contains(cpu) {
                continue;
            }
            let group = group_of(cpu);
            seen = seen.or(&group);
            let (from, from_cpus) = sum(&group);
            let waiting = group.iter().any(|c| loads.load(c).is_some_and(|l| l.running >= 2));
            if !waiting || !should_pull(from, from_cpus, to, to_cpus) {
                continue;
            }
            let distance = self.cpu_distance(this, cpu);
            let better = match best {
                None => true,
                // Busier per CPU first, then nearer.
                Some((_, b, b_cpus, b_distance)) => {
                    let (mine, theirs) =
                        (u64::from(from) * u64::from(b_cpus), u64::from(b) * u64::from(from_cpus));
                    mine > theirs || (mine == theirs && distance < b_distance)
                }
            };
            if better {
                best = Some((group, from, from_cpus, distance));
            }
        }
        let (group, ..) = best?;
        group
            .iter()
            .filter(|&c| loads.load(c).is_some_and(|l| l.running >= 2))
            .max_by_key(|&c| (loads.load(c).map_or(0, |l| l.running), core::cmp::Reverse(c)))
    }

    /// Where a waking fair thread that last ran on `prev` should run: an
    /// idle CPU as close to `prev` as possible (the fastest idle one, when
    /// ranked), else `prev`, else the least busy CPU it may use. `allowed`
    /// must not be empty.
    pub fn select_cpu(
        &self,
        prev: usize,
        allowed: &CpuSet,
        loads: &(impl Loads + ?Sized),
    ) -> usize {
        if let Some(cpu) = self.idle_near(prev, allowed, loads) {
            return cpu;
        }
        if allowed.contains(prev) && prev < self.cpus {
            return prev;
        }
        self.least_busy(prev, allowed, loads)
    }

    /// Where a waking real-time thread of priority `prio` should run: an
    /// idle CPU near `prev`, else `prev` if it runs something less urgent,
    /// else the CPU running the least urgent thing below `prio`, else
    /// `prev`. `allowed` must not be empty.
    pub fn select_rt_cpu(
        &self,
        prev: usize,
        prio: u8,
        allowed: &CpuSet,
        loads: &(impl Loads + ?Sized),
    ) -> usize {
        if let Some(cpu) = self.idle_near(prev, allowed, loads) {
            return cpu;
        }
        let below = |cpu: usize| loads.load(cpu).map_or(u8::MAX, |l| l.rt_prio);
        if allowed.contains(prev) && below(prev) < prio {
            return prev;
        }
        let lowest = allowed
            .iter()
            .filter(|&c| c < self.cpus && below(c) < prio)
            .min_by_key(|&c| (below(c), self.cpu_distance(prev, c), c));
        match lowest {
            Some(cpu) => cpu,
            None if allowed.contains(prev) && prev < self.cpus => prev,
            None => self.least_busy(prev, allowed, loads),
        }
    }

    /// The idle CPU a thread that last ran on `prev` should wake on, if
    /// one it may use is idle: `prev` itself, else the nearest, by level
    /// and then by node distance. When ranked, only the fastest idle CPUs
    /// count.
    fn idle_near(
        &self,
        prev: usize,
        allowed: &CpuSet,
        loads: &(impl Loads + ?Sized),
    ) -> Option<usize> {
        let load = |cpu: usize| loads.load(cpu).filter(|l| allowed.contains(cpu) && l.running == 0);
        let rank = if self.is_ranked() { self.fastest(allowed, &load, 0)? } else { 0 };
        let idle = |cpu: usize| load(cpu).is_some_and(|l| l.rank >= rank);
        if idle(prev) {
            return Some(prev);
        }
        self.nearest(prev, allowed, idle)
    }

    /// Where the thread running alone on `cpu` should move, if the
    /// topology is ranked: the nearest of the fastest idle CPUs it may
    /// use, when they are faster than `cpu`. `None` on an unranked
    /// topology, or when no idle CPU it may use is faster.
    pub fn faster_idle(
        &self,
        cpu: usize,
        allowed: &CpuSet,
        loads: &(impl Loads + ?Sized),
    ) -> Option<usize> {
        if !self.is_ranked() {
            return None;
        }
        let here = loads.load(cpu)?.rank;
        let load = |c: usize| loads.load(c).filter(|l| allowed.contains(c) && l.running == 0);
        let rank = self.fastest(allowed, &load, here.checked_add(1)?)?;
        self.nearest(cpu, allowed, |c| load(c).is_some_and(|l| l.rank >= rank))
    }

    /// The highest rank, of at least `least`, of the CPUs in `allowed`
    /// for which `load` gives a load.
    fn fastest(
        &self,
        allowed: &CpuSet,
        load: &impl Fn(usize) -> Option<CpuLoad>,
        least: u8,
    ) -> Option<u8> {
        allowed
            .iter()
            .take_while(|&c| c < self.cpus)
            .filter_map(load)
            .map(|l| l.rank)
            .filter(|&rank| rank >= least)
            .max()
    }

    /// The CPU satisfying `want` that is closest to `cpu`, by level and
    /// then by node distance.
    fn nearest(&self, cpu: usize, allowed: &CpuSet, want: impl Fn(usize) -> bool) -> Option<usize> {
        for level in self.levels(cpu) {
            let found = self
                .span(cpu, level)
                .and(allowed)
                .iter()
                .filter(|&c| want(c))
                .min_by_key(|&c| (self.cpu_distance(cpu, c), c));
            if found.is_some() {
                return found;
            }
        }
        None
    }

    fn least_busy(&self, prev: usize, allowed: &CpuSet, loads: &(impl Loads + ?Sized)) -> usize {
        allowed
            .iter()
            .filter(|&c| c < self.cpus)
            .min_by_key(|&c| {
                (loads.load(c).map_or(u32::MAX, |l| l.running), self.cpu_distance(prev, c), c)
            })
            .or_else(|| allowed.lowest())
            .unwrap_or(prev)
    }
}

#[cfg(test)]
mod tests {
    use std::alloc::Global;

    use proptest::prelude::*;

    use super::*;

    fn flat(cpus: usize) -> Topology<Global> {
        Topology::flat_in(cpus, Global)
    }

    /// Two packages of two cores with two hardware threads each; one node
    /// per package; finished.
    fn machine() -> Topology<Global> {
        let mut topology = unfinished_machine();
        assert!(topology.finish());
        topology
    }

    /// [`machine`], with its spans worked out each time.
    fn unfinished_machine() -> Topology<Global> {
        let mut topology = flat(8);
        for cpu in 0..8 {
            topology.set_place(
                cpu,
                Place {
                    package: (cpu / 4) as u32,
                    core: ((cpu / 2) % 2) as u32,
                    node: (cpu / 4) as u8,
                },
            );
        }
        topology.set_distance(0, 1, 21);
        topology.set_distance(1, 0, 21);
        topology
    }

    fn loads(running: &[u32]) -> Vec<CpuLoad> {
        running.iter().map(|&r| CpuLoad { running: r, ..CpuLoad::default() }).collect()
    }

    #[test]
    fn levels_grow_outwards() {
        let topology = machine();
        assert_eq!(topology.span(0, Level::Core).to_string(), "0-1");
        assert_eq!(topology.span(5, Level::Package).to_string(), "4-7");
        assert_eq!(topology.span(5, Level::Node).to_string(), "4-7");
        assert_eq!(topology.span(5, Level::Machine).to_string(), "0-7");
        // The node adds nothing to the package here.
        assert_eq!(
            topology.levels(0).collect::<Vec<_>>(),
            [Level::Core, Level::Package, Level::Machine]
        );
        // One package over two nodes counts as one per node.
        let mut split = flat(4);
        for cpu in 0..4 {
            split.set_place(cpu, Place { package: 0, core: cpu as u32, node: (cpu / 2) as u8 });
        }
        assert_eq!(split.span(3, Level::Package).to_string(), "2-3");
        assert_eq!(split.levels(3).collect::<Vec<_>>(), [Level::Package, Level::Machine]);
        let four = flat(4);
        assert_eq!(four.levels(2).collect::<Vec<_>>(), [Level::Package]);
        assert_eq!(flat(1).levels(0).count(), 0);
    }

    #[test]
    fn a_change_of_place_forgets_what_finish_worked_out() {
        let mut topology = flat(4);
        assert!(topology.finish());
        assert_eq!(topology.span(3, Level::Package).to_string(), "0-3");
        topology.set_place(3, Place { package: 1, core: 0, node: 0 });
        assert_eq!(topology.span(3, Level::Package).to_string(), "3");
        assert_eq!(topology.span(0, Level::Package).to_string(), "0-2");
        assert!(topology.finish());
        assert_eq!(topology.span(0, Level::Package).to_string(), "0-2");
        assert_eq!(topology.levels(3).collect::<Vec<_>>(), [Level::Node]);
        topology.set_cpus(2);
        assert_eq!(topology.span(0, Level::Machine).to_string(), "0-1");
    }

    #[test]
    fn pulling_never_reverses_the_imbalance() {
        assert!(should_pull(2, 1, 0, 1));
        assert!(!should_pull(2, 1, 1, 1));
        assert!(should_pull(3, 2, 1, 2));
        assert!(!should_pull(3, 2, 2, 2));
        assert!(!should_pull(0, 1, 0, 1));
        // Every small case. The general case is two lines of algebra
        // (docs/design/scheduler.md): a pull needs (from - 1) t >= (to + 1) f,
        // the pull back would need to f >= from t, and the first gives
        // from t >= to f + f + t > to f.
        for from in 0..=48u32 {
            for to in 0..=48u32 {
                for from_cpus in 1..=12u32 {
                    for to_cpus in 1..=12u32 {
                        if should_pull(from, from_cpus, to, to_cpus) {
                            assert!(!should_pull(to + 1, to_cpus, from - 1, from_cpus));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn busiest_prefers_threads_that_are_waiting() {
        let topology = machine();
        // CPU 1, the core sibling, has a thread waiting.
        assert_eq!(
            topology.find_busiest(0, Level::Core, &*loads(&[0, 2, 0, 0, 0, 0, 0, 0])),
            Some(1)
        );
        // One thread each on CPUs 2 and 3: nothing is waiting, so nothing
        // moves, although the package is uneven.
        assert_eq!(
            topology.find_busiest(0, Level::Package, &*loads(&[0, 0, 1, 1, 0, 0, 0, 0])),
            None
        );
        assert_eq!(
            topology.find_busiest(0, Level::Package, &*loads(&[0, 0, 3, 1, 0, 0, 0, 0])),
            Some(2)
        );
        // The other package is busier than this one.
        let far = loads(&[1, 1, 1, 1, 3, 2, 2, 2]);
        assert_eq!(topology.find_busiest(0, Level::Machine, &*far), Some(4));
        assert_eq!(topology.find_busiest(0, Level::Package, &*far), None);
    }

    #[test]
    fn waking_threads_stay_close() {
        let topology = machine();
        let all = CpuSet::first(8);
        assert_eq!(topology.select_cpu(0, &all, &*loads(&[0; 8])), 0);
        assert_eq!(topology.select_cpu(0, &all, &*loads(&[1, 1, 1, 0, 0, 0, 0, 0])), 3);
        assert_eq!(topology.select_cpu(0, &all, &*loads(&[1, 1, 1, 1, 0, 1, 0, 1])), 4);
        assert_eq!(topology.select_cpu(0, &all, &*loads(&[1; 8])), 0);
        let only = CpuSet::single(6);
        assert_eq!(topology.select_cpu(0, &only, &*loads(&[0; 8])), 6);
    }

    #[test]
    fn real_time_threads_find_the_least_urgent_cpu() {
        let topology = machine();
        let all = CpuSet::first(4);
        let busy = |prios: [u8; 4]| {
            prios.iter().map(|&p| CpuLoad { running: 1, rt_prio: p, rank: 0 }).collect::<Vec<_>>()
        };
        assert_eq!(topology.select_rt_cpu(0, 50, &all, &*busy([0, 0, 0, 0])), 0);
        assert_eq!(topology.select_rt_cpu(0, 50, &all, &*busy([60, 10, 0, 70])), 2);
        assert_eq!(topology.select_rt_cpu(0, 50, &all, &*busy([60, 70, 80, 90])), 0);
        let mut mixed = busy([60, 70, 80, 90]);
        mixed[3].running = 0;
        assert_eq!(topology.select_rt_cpu(0, 50, &all, &*mixed), 3);
    }

    /// A machine like a 12th generation Core's, in small: CPUs 0 to 7 are
    /// four performance cores of two threads each, CPUs 8 to 11 four
    /// efficient cores of one, all in one package. Ranks as the kernel
    /// gives them: a performance core's first thread, then an efficient
    /// core, then a performance core's second thread.
    fn hybrid() -> Topology<Global> {
        let mut topology = flat(12);
        for cpu in 0..12 {
            let core = if cpu < 8 { cpu / 2 } else { cpu - 4 };
            topology.set_place(cpu, Place { package: 0, core: core as u32, node: 0 });
        }
        assert!(topology.finish());
        topology.set_ranked(true);
        topology
    }

    const HYBRID_RANKS: [u8; 12] = [3, 1, 3, 1, 3, 1, 3, 1, 2, 2, 2, 2];

    fn ranked(running: &[u32]) -> Vec<CpuLoad> {
        running
            .iter()
            .zip(HYBRID_RANKS)
            .map(|(&running, rank)| CpuLoad { running, rt_prio: 0, rank })
            .collect()
    }

    #[test]
    fn waking_threads_go_to_the_fastest_idle_cpu() {
        let topology = hybrid();
        let all = CpuSet::first(12);
        // Last on an efficient core, with a performance core idle.
        assert_eq!(topology.select_cpu(9, &all, &*ranked(&[0; 12])), 0);
        // The nearest idle performance core's first thread.
        assert_eq!(
            topology.select_cpu(9, &all, &*ranked(&[1, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0])),
            4
        );
        // Every performance core busy: an idle efficient core before a
        // performance core's second thread, and the one it ran on first.
        let first_threads_busy = [1, 0, 1, 0, 1, 0, 1, 0, 0, 0, 0, 0];
        assert_eq!(topology.select_cpu(9, &all, &*ranked(&first_threads_busy)), 9);
        assert_eq!(topology.select_cpu(0, &all, &*ranked(&first_threads_busy)), 8);
        // Only a second thread idle.
        let only_seconds = [1, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1];
        assert_eq!(topology.select_cpu(9, &all, &*ranked(&only_seconds)), 1);
        // Nothing idle: where it ran.
        assert_eq!(topology.select_cpu(9, &all, &*ranked(&[1; 12])), 9);
        // Affinity first.
        let efficient = CpuSet::from_words([0xf00, 0, 0, 0]);
        assert_eq!(topology.select_cpu(0, &efficient, &*ranked(&[0; 12])), 8);
        // Real-time threads look for idle CPUs the same way.
        assert_eq!(topology.select_rt_cpu(9, 50, &all, &*ranked(&[0; 12])), 0);
        // Unranked, the topology keeps a thread where it ran.
        topology.set_ranked(false);
        assert_eq!(topology.select_cpu(9, &all, &*ranked(&[0; 12])), 9);
    }

    #[test]
    fn a_lone_thread_moves_to_a_faster_idle_cpu() {
        let topology = hybrid();
        let all = CpuSet::first(12);
        let alone_on_9 = [0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0];
        assert_eq!(topology.faster_idle(9, &all, &*ranked(&alone_on_9)), Some(0));
        // Only performance cores' second threads idle: slower than an
        // efficient core, so it stays.
        let seconds_idle = [1, 0, 1, 0, 1, 0, 1, 0, 1, 1, 1, 1];
        assert_eq!(topology.faster_idle(9, &all, &*ranked(&seconds_idle)), None);
        // From a second thread, an idle efficient core is faster.
        let one_efficient_idle = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1];
        assert_eq!(topology.faster_idle(1, &all, &*ranked(&one_efficient_idle)), Some(10));
        // The fastest never move, nor threads that may use no faster CPU.
        let alone_on_0 = [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(topology.faster_idle(0, &all, &*ranked(&alone_on_0)), None);
        let efficient = CpuSet::from_words([0xf00, 0, 0, 0]);
        assert_eq!(topology.faster_idle(9, &efficient, &*ranked(&alone_on_9)), None);
        // Unranked, nothing moves.
        topology.set_ranked(false);
        assert_eq!(topology.faster_idle(9, &all, &*ranked(&alone_on_9)), None);
    }

    proptest! {
        /// With every rank the same, a ranked topology places threads as
        /// an unranked one does: a machine whose cores are alike is
        /// untouched by ranking.
        #[test]
        fn equal_ranks_place_as_unranked(
            prev in 0usize..12,
            words in any::<u64>(),
            running in prop::collection::vec(0u32..3, 12),
            rank in any::<u8>(),
        ) {
            let topology = hybrid();
            let allowed = CpuSet::from_words([words & 0xfff, 0, 0, 0]);
            prop_assume!(!allowed.is_empty());
            let loads: Vec<CpuLoad> =
                running.iter().map(|&running| CpuLoad { running, rt_prio: 0, rank }).collect();
            let ranked = (
                topology.select_cpu(prev, &allowed, &*loads),
                topology.select_rt_cpu(prev, 40, &allowed, &*loads),
                topology.faster_idle(prev, &allowed, &*loads),
            );
            topology.set_ranked(false);
            let unranked = (
                topology.select_cpu(prev, &allowed, &*loads),
                topology.select_rt_cpu(prev, 40, &allowed, &*loads),
                None,
            );
            prop_assert_eq!(ranked, unranked);
        }

        /// A ranked wake-up picks an idle CPU of the highest rank the
        /// thread may use, when one is idle, and a move is always to a
        /// faster idle CPU.
        #[test]
        fn ranked_choices_are_the_fastest(
            prev in 0usize..12,
            words in any::<u64>(),
            running in prop::collection::vec(0u32..3, 12),
        ) {
            let topology = hybrid();
            let allowed = CpuSet::from_words([words & 0xfff, 0, 0, 0]);
            prop_assume!(!allowed.is_empty());
            let loads = ranked(&running);
            let best = allowed.iter().filter(|&c| running[c] == 0).map(|c| HYBRID_RANKS[c]).max();
            let chosen = topology.select_cpu(prev, &allowed, &*loads);
            prop_assert!(allowed.contains(chosen));
            if let Some(best) = best {
                prop_assert_eq!(running[chosen], 0);
                prop_assert_eq!(HYBRID_RANKS[chosen], best);
            }
            if let Some(to) = topology.faster_idle(prev, &allowed, &*loads) {
                prop_assert!(allowed.contains(to));
                prop_assert_eq!(running[to], 0);
                prop_assert!(HYBRID_RANKS[to] > HYBRID_RANKS[prev]);
            }
        }

        /// What `finish` works out answers as the places do.
        #[test]
        fn finished_answers_as_worked_out(
            cpus in 1usize..16,
            places in prop::collection::vec((0u32..3, 0u32..4, 0u8..3), 16),
            running in prop::collection::vec(0u32..4, 16),
            words in any::<u64>(),
        ) {
            let mut worked_out = flat(cpus);
            for (cpu, &(package, core, node)) in places.iter().enumerate().take(cpus) {
                worked_out.set_place(cpu, Place { package, core, node });
            }
            worked_out.set_distance(0, 1, 21);
            let mut finished = flat(cpus);
            for cpu in 0..cpus {
                finished.set_place(cpu, worked_out.place(cpu));
            }
            finished.set_distance(0, 1, 21);
            prop_assert!(finished.finish());
            let loads = loads(&running);
            let allowed = CpuSet::from_words([words & 0xffff, 0, 0, 0]).and(&CpuSet::first(cpus));
            for cpu in 0..cpus {
                for level in Level::ALL {
                    prop_assert_eq!(finished.span(cpu, level), worked_out.span(cpu, level));
                    prop_assert_eq!(
                        finished.find_busiest(cpu, level, &*loads),
                        worked_out.find_busiest(cpu, level, &*loads)
                    );
                }
                prop_assert_eq!(
                    finished.levels(cpu).collect::<Vec<_>>(),
                    worked_out.levels(cpu).collect::<Vec<_>>()
                );
                if !allowed.is_empty() {
                    prop_assert_eq!(
                        finished.select_cpu(cpu, &allowed, &*loads),
                        worked_out.select_cpu(cpu, &allowed, &*loads)
                    );
                }
            }
        }

        #[test]
        fn placement_respects_affinity(prev in 0usize..8, words in any::<u64>(), running in prop::collection::vec(0u32..4, 8)) {
            let topology = machine();
            let allowed = CpuSet::from_words([words & 0xff, 0, 0, 0]);
            prop_assume!(!allowed.is_empty());
            let loads = loads(&running);
            prop_assert!(allowed.contains(topology.select_cpu(prev, &allowed, &*loads)));
            prop_assert!(allowed.contains(topology.select_rt_cpu(prev, 40, &allowed, &*loads)));
        }

        #[test]
        fn pulling_evens_without_reversing(from in 0u32..64, from_cpus in 1u32..8, to in 0u32..64, to_cpus in 1u32..8) {
            if should_pull(from, from_cpus, to, to_cpus) {
                // After the move the source is still at least as busy per
                // CPU as the destination.
                prop_assert!(u64::from(from - 1) * u64::from(to_cpus) >= u64::from(to + 1) * u64::from(from_cpus));
                prop_assert!(!should_pull(to + 1, to_cpus, from - 1, from_cpus));
            }
        }
    }
}
