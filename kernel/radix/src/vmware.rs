// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! VMware's guest tools, in the kernel (Phase 14, ADR 0424).
//!
//! On a machine whose hypervisor names itself VMware's, or whose firmware
//! says VMware made it, and whose backdoor answers, the `vmware` thread
//! runs the toolbox of `radix-vmware` each second: the host then knows the
//! guest's tools run, which its Shut Down Guest and Restart Guest need,
//! and shows how long the machine has been up and its IPv4 address. Shut
//! Down Guest reaches Spark as a press of the power button, Restart Guest
//! as a reboot(8) ([`crate::user::power`]).
//!
//! The toolbox speaks VMware's guest RPC through the backdoor's ports,
//! which the architecture reaches ([`radix_hal::Vmware`]). A host that
//! answers the backdoor but gives no guest RPC, as QEMU's `vmport` does,
//! is tried again each 15 seconds and logged once.

use radix_hal::Vmware;
use radix_klog::{info, warn};
use radix_vmware::{Asked, Backdoor, Event, Guest, Regs, Toolbox};

use crate::Arch;
use crate::sched::{self, Options, Policy};

/// What SMBIOS names VMware's machines' maker.
const MAKER: &[u8] = b"VMware, Inc.";

/// The backdoor, through the architecture.
struct Door;

impl Backdoor for Door {
    fn call(&mut self, regs: &mut Regs) {
        Arch::vmware_call(regs);
    }

    fn send(&mut self, regs: &mut Regs, bytes: &[u8]) {
        Arch::vmware_send(regs, bytes);
    }

    fn receive(&mut self, regs: &mut Regs, bytes: &mut [u8]) {
        Arch::vmware_receive(regs, bytes);
    }
}

/// The machine, as the toolbox tells the host of it.
struct Machine;

impl Guest for Machine {
    fn uptime(&self) -> u64 {
        sched::now_ns() / 10_000_000
    }

    fn address(&self) -> Option<[u8; 4]> {
        crate::net::first_ipv4()
    }
}

/// Starts the toolbox's thread on VMware's hypervisors. After the power
/// thread, which the host's buttons wake.
pub fn start() {
    if !Arch::vmware_signature() && crate::dmi::sys_vendor() != Some(MAKER) {
        return;
    }
    let Some(version) = radix_vmware::present(&mut Door) else {
        info!("VMware: the hypervisor answers no backdoor; the host's buttons are not heard");
        return;
    };
    // Said first: the thread may log before the spawn returns.
    info!("VMware: backdoor version {version}; the toolbox runs");
    if let Err(err) =
        sched::spawn("vmware", run, 0, Options::new().policy(Policy::Fair { nice: 0 }))
    {
        warn!("VMware: could not start the toolbox's thread: {err}");
    }
}

/// The toolbox's thread: a tick, then a sleep as long as it asks.
fn run(_: usize) {
    let mut toolbox = Toolbox::new();
    // Each trouble is logged once, until the channel opens again.
    let (mut no_channel, mut failing) = (false, false);
    loop {
        let tick = toolbox.tick(&mut Door, &Machine);
        match tick.event {
            Event::Nothing | Event::Unknown => {}
            Event::Opened => {
                info!("VMware: the host hears the toolbox");
                (no_channel, failing) = (false, false);
            }
            Event::NoChannel(err) => {
                if !no_channel {
                    info!("VMware: {err}: the host gives no guest RPC; trying again each 15 s");
                    no_channel = true;
                }
            }
            Event::Registered => {
                info!("VMware: the host registered the toolbox: its Shut Down and Restart work");
            }
            Event::Asked(Asked::PowerOff) => crate::user::power::host_asked(),
            Event::Asked(Asked::Restart) => crate::user::power::host_asked_restart(),
            Event::Failed(err) => {
                if !failing {
                    warn!("VMware: the toolbox's channel failed: {err}; opening it again");
                    failing = true;
                }
            }
        }
        if tick.again_ms > 0 {
            sched::sleep_ns(u64::from(tick.again_ms) * 1_000_000);
        }
    }
}
