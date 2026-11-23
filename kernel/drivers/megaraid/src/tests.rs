// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Tests of the transports against the simulated controller ([`crate::sim`]).
//!
//! Each test brings a [`MegaRaid`] up on a [`Sim`] of the family it
//! exercises, drives it through the public API and checks both what it
//! answers and what the simulated firmware saw: the bytes of the frames,
//! the doorbell, the reply index. The files in `tests/` group them:
//! bring-up, commands, the queues and faults.

use std::sync::Arc;
use std::vec::Vec;

use radix_hw::Dma;

use crate::sim::{Inner, Sim};
use crate::{Answer, Direction, Error, Family, MegaRaid, Target};

mod bring_up;
mod commands;
mod faults;
mod fusion_frames;
mod mfi_frames;
mod queues;
mod report;

/// The MFI families, each with a PCI device ID.
pub const MFI_FAMILIES: [(Family, u16); 3] =
    [(Family::Mfi1078, 0x0060), (Family::MfiGen2, 0x0079), (Family::MfiSkinny, 0x0073)];

/// The fusion families, each with a PCI device ID.
pub const FUSION_FAMILIES: [(Family, u16); 4] = [
    (Family::Thunderbolt, 0x005b),
    (Family::Invader, 0x005d),
    (Family::Ventura, 0x0016),
    (Family::Aero, 0x10e1),
];

/// Every family.
pub fn all_families() -> Vec<(Family, u16)> {
    MFI_FAMILIES.iter().chain(FUSION_FAMILIES.iter()).copied().collect()
}

/// The device ID the tests use for `family`.
pub fn device(family: Family) -> u16 {
    all_families().into_iter().find(|&(f, _)| f == family).unwrap().1
}

/// Brings a driver up on `sim`, which the test may have set up first.
pub fn try_new(sim: &Arc<Sim>, msix: bool) -> Result<MegaRaid, Error> {
    let mut alloc = |pages: usize| sim.alloc(pages);
    let mut wait = sim.waiter();
    MegaRaid::new(sim.regs(), device(sim.family()), msix, &mut alloc, &mut wait)
}

/// A controller of `family` with two volumes (targets 0 and 2) and a
/// pass-through disk (ID 5), each of 64 blocks, and a driver on it with
/// its interrupts enabled.
pub fn up(family: Family) -> (Arc<Sim>, MegaRaid) {
    up_with(family, |_| {})
}

/// As [`up`], with `setup` run on the controller before the driver starts.
pub fn up_with(family: Family, setup: impl FnOnce(&mut Inner)) -> (Arc<Sim>, MegaRaid) {
    let sim = Sim::new(family);
    {
        let mut inner = sim.lock();
        inner.add_volume(0, 64);
        inner.add_volume(2, 64);
        inner.add_disk(5, 64);
        setup(&mut inner);
    }
    let raid = try_new(&sim, false).expect("the driver comes up");
    raid.enable_interrupts();
    (sim, raid)
}

/// Memory for data: `len` zeroed bytes.
pub fn buffer(len: usize) -> Dma {
    Dma::fake(len)
}

/// Reads `len` bytes of `memory`.
pub fn bytes_of(memory: &Dma, len: usize) -> Vec<u8> {
    let mut out = std::vec![0u8; len];
    memory.read(0, &mut out);
    out
}

/// Sends a command and takes its answer, which must come at once.
pub fn exec(
    raid: &mut MegaRaid,
    target: Target,
    cdb: &[u8],
    runs: &[(u64, u32)],
    direction: Direction,
) -> Answer {
    let context = raid.submit(target, 0, cdb, runs, direction).expect("the command goes");
    let (done, answer) = raid.complete().expect("the command ended");
    assert_eq!(done, context);
    answer
}

/// READ(10).
pub fn read10(lba: u32, blocks: u16) -> [u8; 10] {
    let mut cdb = [0x28, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    cdb[2..6].copy_from_slice(&lba.to_be_bytes());
    cdb[7..9].copy_from_slice(&blocks.to_be_bytes());
    cdb
}

/// WRITE(10).
pub fn write10(lba: u32, blocks: u16) -> [u8; 10] {
    let mut cdb = read10(lba, blocks);
    cdb[0] = 0x2a;
    cdb
}

/// READ(16).
pub fn read16(lba: u64, blocks: u32) -> [u8; 16] {
    let mut cdb = [0u8; 16];
    cdb[0] = 0x88;
    cdb[2..10].copy_from_slice(&lba.to_be_bytes());
    cdb[10..14].copy_from_slice(&blocks.to_be_bytes());
    cdb
}

/// WRITE(16).
pub fn write16(lba: u64, blocks: u32) -> [u8; 16] {
    let mut cdb = read16(lba, blocks);
    cdb[0] = 0x8a;
    cdb
}

/// INQUIRY for `len` bytes.
pub fn inquiry(len: u16) -> [u8; 6] {
    let mut cdb = [0x12, 0, 0, 0, 0, 0];
    cdb[3..5].copy_from_slice(&len.to_be_bytes());
    cdb
}

/// READ CAPACITY(10).
pub const READ_CAPACITY_10: [u8; 10] = [0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// READ CAPACITY(16) for `len` bytes.
pub fn read_capacity16(len: u32) -> [u8; 16] {
    let mut cdb = [0u8; 16];
    cdb[0] = 0x9e;
    cdb[1] = 0x10;
    cdb[10..14].copy_from_slice(&len.to_be_bytes());
    cdb
}

/// SYNCHRONIZE CACHE(10).
pub const SYNC_CACHE: [u8; 10] = [0x35, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// Memory holding blocks of 512 bytes, block `n` filled with `fills[n]`,
/// and the runs that name it, one run for each block, for any family.
pub fn block_runs(_family: Family, fills: &[u8]) -> (Vec<Dma>, Vec<(u64, u32)>) {
    let memories: Vec<Dma> = fills
        .iter()
        .map(|&fill| {
            let memory = buffer(4096);
            memory.write(0, &[fill; 512]);
            memory
        })
        .collect();
    let runs = memories.iter().map(|m| (m.phys(), 512)).collect();
    (memories, runs)
}
