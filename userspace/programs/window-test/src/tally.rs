// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! MEASUREMENT ONLY, never merged: what each program and the kernel
//! spent over a step of `window-test cost`, as each system counts it,
//! printed as `window-test: tally` lines for a person to read.
//!
//! On Dukovo the counts are `/processes/stat` and each process's
//! `cpu_ns`; on Linux `/proc/stat`, `pgfault` of `/proc/vmstat` and each
//! thread's `schedstat`.

use std::collections::BTreeMap;
use std::time::Instant;

/// The counts at a moment.
pub(crate) struct Tally {
    at: Instant,
    /// The system's own counters, by name.
    system: BTreeMap<String, u64>,
    /// The nanoseconds each process has run and the times it was given a
    /// CPU, by number and name.
    ran: BTreeMap<(u32, String), (u64, u64)>,
}

/// The `key: value` numbers of `text`.
#[cfg(not(target_os = "linux"))]
fn numbers(text: &str, into: &mut BTreeMap<String, u64>) {
    for line in text.lines() {
        if let Some((key, value)) = line.split_once(": ")
            && let Ok(value) = value.trim().parse::<u64>()
        {
            into.insert(key.trim().to_string(), value);
        }
    }
}

/// The processes' folders under `root`, by number.
fn processes(root: &str) -> Vec<u32> {
    let Ok(listed) = std::fs::read_dir(root) else { return Vec::new() };
    listed.flatten().filter_map(|entry| entry.file_name().to_string_lossy().parse().ok()).collect()
}

#[cfg(not(target_os = "linux"))]
fn read(system: &mut BTreeMap<String, u64>, ran: &mut BTreeMap<(u32, String), (u64, u64)>) {
    if let Ok(text) = std::fs::read_to_string("/processes/stat") {
        numbers(&text, system);
    }
    for number in processes("/processes") {
        let Ok(text) = std::fs::read_to_string(format!("/processes/{number}/status")) else {
            continue;
        };
        let of =
            |key: &str| text.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix(": "));
        let name = of("name").unwrap_or("?").to_string();
        let count = |key: &str| of(key).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
        if let Some(ns) = of("cpu_ns").and_then(|ns| ns.parse().ok()) {
            let runs = count("voluntary_switches") + count("involuntary_switches");
            ran.insert((number, name), (ns, runs));
        }
    }
}

#[cfg(target_os = "linux")]
fn read(system: &mut BTreeMap<String, u64>, ran: &mut BTreeMap<(u32, String), (u64, u64)>) {
    if let Ok(text) = std::fs::read_to_string("/proc/stat") {
        for line in text.lines() {
            let mut words = line.split_whitespace();
            let Some(key) = words.next() else { continue };
            let values: Vec<u64> = words.filter_map(|word| word.parse().ok()).collect();
            match key {
                // Hundredths of a second.
                "cpu" if values.len() >= 7 => {
                    for (name, value) in
                        ["user", "nice", "system", "idle", "iowait", "irq", "softirq"]
                            .iter()
                            .zip(&values)
                    {
                        system.insert(format!("{name}_ns"), value * 10_000_000);
                    }
                }
                "ctxt" | "intr" | "softirq" => {
                    if let Some(first) = values.first() {
                        system.insert(format!("{key}_count"), *first);
                    }
                }
                _ => {}
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string("/proc/vmstat") {
        for line in text.lines() {
            if let Some(count) = line.strip_prefix("pgfault ").and_then(|n| n.parse().ok()) {
                system.insert("faults".into(), count);
            }
        }
    }
    for number in processes("/proc") {
        let name = std::fs::read_to_string(format!("/proc/{number}/comm"))
            .map_or_else(|_| "?".into(), |name| name.trim().to_string());
        let (mut ns, mut runs) = (0u64, 0u64);
        for thread in processes(&format!("/proc/{number}/task")) {
            let file = format!("/proc/{number}/task/{thread}/schedstat");
            let text = std::fs::read_to_string(file).unwrap_or_default();
            let mut words = text.split_whitespace().map(|word| word.parse::<u64>().unwrap_or(0));
            // Time on a CPU, time waiting for one, times given one.
            ns += words.next().unwrap_or(0);
            runs += words.nth(1).unwrap_or(0);
        }
        ran.insert((number, name), (ns, runs));
    }
}

impl Tally {
    pub(crate) fn now() -> Tally {
        let (mut system, mut ran) = (BTreeMap::new(), BTreeMap::new());
        read(&mut system, &mut ran);
        Tally { at: Instant::now(), system, ran }
    }

    /// What was counted from `self` until `then`, for the step `step` of
    /// `count` turns: the whole, and a turn's share.
    pub(crate) fn lines_until(&self, then: &Tally, step: &str, count: u32) -> Vec<String> {
        let took = (then.at - self.at).as_micros();
        let turn = |value: u64| value as f64 / f64::from(count);
        let mut lines = vec![format!("window-test: tally {step}: {count} turns in {took} us")];
        for (key, value) in &then.system {
            let since = value.saturating_sub(self.system.get(key).copied().unwrap_or(0));
            if since > 0 {
                lines.push(format!(
                    "window-test: tally {step}: system {key} {since} ({:.2} a turn)",
                    turn(since)
                ));
            }
        }
        let mut programs: Vec<(u64, u64, &(u32, String))> = then
            .ran
            .iter()
            .map(|(who, (ns, runs))| {
                let (ns_then, runs_then) = self.ran.get(who).copied().unwrap_or((0, 0));
                (ns.saturating_sub(ns_then), runs.saturating_sub(runs_then), who)
            })
            .filter(|(ns, _, _)| *ns > 0)
            .collect();
        programs.sort_by(|one, other| other.0.cmp(&one.0));
        for (ns, runs, (number, name)) in programs.into_iter().take(20) {
            lines.push(format!(
                "window-test: tally {step}: program {name}[{number}] {ns} ns ({:.1} us a turn), \
                 {runs} runs ({:.2} a turn)",
                turn(ns) / 1000.0,
                turn(runs)
            ));
        }
        lines
    }
}
