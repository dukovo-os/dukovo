// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Giving each virtual CPU of a benchmark boot a host core of its own
//! (ADR 0554).
//!
//! The CI runners are virtual machines with 2 cores of 2 threads each, and
//! their kernel puts QEMU's vCPU threads wherever it likes: two vCPUs that
//! share a core move a cache line in tens of nanoseconds, two on different
//! cores in some 170. [`start`] boots a machine, then pins vCPU `i` to the
//! `i`th CPU of [`cpu_order`], so that every boot, of either system, gets
//! the placement real multi-core machines have.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::process;
use crate::qemu::Machine;
use crate::test::{Qmp, Vm};
use crate::{Ctx, Result};

/// A host CPU and the physical core it is a thread of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HostCpu {
    cpu: usize,
    package: u32,
    core: u32,
}

/// Boots `machine`, which must have a QMP socket, and pins its vCPUs.
/// Pinning is skipped, with a note on stderr, wherever it cannot be done:
/// the boot goes on unpinned.
pub fn start(
    ctx: &Ctx,
    machine: &Machine,
    dir: &Path,
    name: &str,
    timeout: Duration,
) -> Result<Vm> {
    if let Some(socket) = &machine.qmp {
        // QEMU leaves the socket of an earlier boot behind.
        let _ = std::fs::remove_file(socket);
    }
    let vm = Vm::start(ctx, machine, dir, name, timeout).map_err(crate::Error::new)?;
    if machine.kvm {
        match pin(ctx, machine) {
            Ok(line) => eprintln!("{line}"),
            Err(why) => eprintln!("note: vCPUs of the {name} boot are not pinned: {why}"),
        }
    }
    Ok(vm)
}

/// Pins each vCPU of the running `machine`; says where.
fn pin(ctx: &Ctx, machine: &Machine) -> std::result::Result<String, String> {
    if !cfg!(target_os = "linux") {
        return Err("the host is not Linux".into());
    }
    let vcpus = machine.cpus as usize;
    let socket = machine.qmp.as_deref().ok_or("the machine has no QMP socket")?;
    let order = host_cpu_order().ok_or("cannot read the host's CPU topology")?;
    let cores: std::collections::HashSet<_> = order.iter().map(|c| (c.package, c.core)).collect();
    if cores.len() < 2 {
        return Err("the host has fewer than 2 cores".into());
    }
    if order.len() < vcpus {
        return Err(format!("the host has {} CPUs for {} vCPUs", order.len(), vcpus));
    }
    let threads = vcpu_threads(socket)?;
    if threads.len() != vcpus {
        return Err(format!("QEMU reports {} vCPU threads, not {}", threads.len(), vcpus));
    }
    let chosen = &order[..vcpus];
    for (thread, host) in threads.iter().zip(chosen) {
        let mut cmd = Command::new("taskset");
        cmd.args(["-p", "-c", &host.cpu.to_string(), &thread.to_string()]);
        process::output(ctx, &mut cmd).map_err(|err| err.to_string())?;
    }
    let join = |f: fn(&HostCpu) -> u32| {
        chosen.iter().map(|c| f(c).to_string()).collect::<Vec<_>>().join(",")
    };
    Ok(format!(
        "pinned vCPUs {} to host CPUs {} (cores {})",
        (0..vcpus).map(|i| i.to_string()).collect::<Vec<_>>().join(","),
        chosen.iter().map(|c| c.cpu.to_string()).collect::<Vec<_>>().join(","),
        join(|c| c.core),
    ))
}

/// The host thread of each vCPU, in vCPU order, from QMP.
pub(crate) fn vcpu_threads(socket: &Path) -> std::result::Result<Vec<u64>, String> {
    let reply = Qmp::connect(socket)?.execute_reply(r#"{"execute":"query-cpus-fast"}"#)?;
    parse_vcpu_threads(&reply)
}

/// The `thread-id`s of a `query-cpus-fast` reply, by `cpu-index`.
fn parse_vcpu_threads(reply: &str) -> std::result::Result<Vec<u64>, String> {
    let value: serde_json::Value =
        serde_json::from_str(reply).map_err(|err| format!("reading QMP's reply: {err}"))?;
    let cpus = value["return"].as_array().ok_or("QMP's reply lists no CPUs")?;
    let mut threads = Vec::new();
    for cpu in cpus {
        let index = cpu["cpu-index"].as_u64().ok_or("a vCPU has no cpu-index")?;
        let thread = cpu["thread-id"].as_u64().ok_or("a vCPU has no thread-id")?;
        threads.push((index, thread));
    }
    threads.sort_unstable();
    Ok(threads.into_iter().map(|(_, thread)| thread).collect())
}

/// The CPUs this process may run on, ordered by [`order_cpus`]; `None`
/// where the host does not say.
fn host_cpu_order() -> Option<Vec<HostCpu>> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let allowed = status.lines().find_map(|l| l.strip_prefix("Cpus_allowed_list:"))?;
    let mut cpus = parse_cpu_list(allowed)?;
    // Offline CPUs may be in the mask; skip them when the host says which are.
    if let Some(online) = std::fs::read_to_string("/sys/devices/system/cpu/online")
        .ok()
        .and_then(|text| parse_cpu_list(&text))
    {
        cpus.retain(|cpu| online.contains(cpu));
    }
    let read = |cpu: usize, file: &str| -> Option<u32> {
        let path = format!("/sys/devices/system/cpu/cpu{cpu}/topology/{file}");
        std::fs::read_to_string(path).ok()?.trim().parse().ok()
    };
    let topology: Vec<HostCpu> = cpus
        .into_iter()
        .map(|cpu| {
            Some(HostCpu {
                cpu,
                package: read(cpu, "physical_package_id")?,
                core: read(cpu, "core_id")?,
            })
        })
        .collect::<Option<_>>()?;
    Some(order_cpus(&topology))
}

/// A kernel CPU list such as `0-3,6`.
fn parse_cpu_list(text: &str) -> Option<Vec<usize>> {
    let mut cpus = Vec::new();
    for part in text.trim().split(',').filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((from, to)) => cpus.extend(from.parse::<usize>().ok()?..=to.parse().ok()?),
            None => cpus.push(part.parse().ok()?),
        }
    }
    cpus.sort_unstable();
    cpus.dedup();
    (!cpus.is_empty()).then_some(cpus)
}

/// `cpus` with one thread of each physical core first, then each core's
/// second thread, and so on; lowest numbers first within each round.
fn order_cpus(cpus: &[HostCpu]) -> Vec<HostCpu> {
    let mut sorted = cpus.to_vec();
    sorted.sort_by_key(|c| c.cpu);
    let mut seen: std::collections::HashMap<(u32, u32), usize> = std::collections::HashMap::new();
    let mut ranked: Vec<(usize, HostCpu)> = sorted
        .into_iter()
        .map(|c| {
            let rank = seen.entry((c.package, c.core)).or_insert(0);
            *rank += 1;
            (*rank - 1, c)
        })
        .collect();
    // Stable: within a round the CPU numbers stay ascending.
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(topology: &[(usize, u32, u32)]) -> Vec<HostCpu> {
        topology.iter().map(|&(cpu, package, core)| HostCpu { cpu, package, core }).collect()
    }

    fn ids(cpus: &[HostCpu]) -> Vec<usize> {
        cpus.iter().map(|c| c.cpu).collect()
    }

    #[test]
    fn siblings_far_apart_give_the_cores_first() {
        // Cores 0 and 1, with threads 0,2 and 1,3.
        let cpus = host(&[(0, 0, 0), (1, 0, 1), (2, 0, 0), (3, 0, 1)]);
        assert_eq!(ids(&order_cpus(&cpus)), [0, 1, 2, 3]);
    }

    #[test]
    fn adjacent_siblings_still_give_the_cores_first() {
        // Cores 0 and 1, with threads 0,1 and 2,3.
        let cpus = host(&[(0, 0, 0), (1, 0, 0), (2, 0, 1), (3, 0, 1)]);
        assert_eq!(ids(&order_cpus(&cpus)), [0, 2, 1, 3]);
    }

    #[test]
    fn without_smt_the_order_is_by_number() {
        let cpus = host(&[(3, 0, 3), (0, 0, 0), (2, 0, 2), (1, 0, 1)]);
        assert_eq!(ids(&order_cpus(&cpus)), [0, 1, 2, 3]);
    }

    #[test]
    fn a_restricted_list_keeps_only_what_is_allowed() {
        // CPUs 0 and 2 are core 0's; only 1, 2 and 3 are allowed.
        let cpus = host(&[(1, 0, 1), (2, 0, 0), (3, 0, 1)]);
        assert_eq!(ids(&order_cpus(&cpus)), [1, 2, 3]);
    }

    #[test]
    fn a_core_id_in_another_package_is_another_core() {
        let cpus = host(&[(0, 0, 0), (1, 1, 0), (2, 0, 0), (3, 1, 0)]);
        assert_eq!(ids(&order_cpus(&cpus)), [0, 1, 2, 3]);
    }

    #[test]
    fn cpu_lists_are_read() {
        assert_eq!(parse_cpu_list("0-2,5\n"), Some(vec![0, 1, 2, 5]));
        assert_eq!(parse_cpu_list("3"), Some(vec![3]));
        assert_eq!(parse_cpu_list("\n"), None);
        assert_eq!(parse_cpu_list("a-b"), None);
    }

    #[test]
    fn vcpu_threads_come_in_cpu_order() {
        let reply = r#"{"return": [
            {"cpu-index": 1, "thread-id": 202, "props": {"core-id": 0}},
            {"cpu-index": 0, "thread-id": 201, "props": {"core-id": 1}}]}"#;
        assert_eq!(parse_vcpu_threads(reply), Ok(vec![201, 202]));
        assert!(parse_vcpu_threads(r#"{"return": {}}"#).is_err());
    }
}
