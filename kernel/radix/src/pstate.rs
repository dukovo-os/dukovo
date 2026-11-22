// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! CPU frequency (Phase 11c, ADR 0202): each CPU picks its own speed,
//! through Intel's HWP or AMD's CPPC, within the levels the kernel asks
//! for.
//!
//! At the end of boot each CPU's work thread turns its control on and
//! asks for every level from its lowest to its highest, at its own
//! choice, with the firmware's energy preference or the one `epp=` names,
//! as Linux's `intel_pstate` and `amd-pstate-epp` drivers do in their
//! active mode; the kernel setting `cpu.energy_preference` changes it
//! later ([`set_preference`], ADR 0476). The settings `cpu.boost` and
//! `cpu.speed` keep the CPUs at their base frequency or below, and hold
//! them at the top of what they may run at ([`set_boost`], [`set_held`],
//! ADR 0557), which together keep the clock steady for measuring.
//! `pstate=off` leaves the CPUs to the firmware. The thermal zones'
//! passive cooling caps every CPU's highest level ([`cool`], ADR 0203).
//! Every change asks every CPU again for what all of them together allow
//! ([`radix_pstate::Request::at`]).
//!
//! `/kernel/devices/system/cpu/cpufreq` shows each CPU's levels and
//! request as frequencies ([`write_file`]), and the frequency it runs at,
//! measured from its APERF and MPERF counters over [`MEASURE_NS`] when a
//! file asks and kept for [`MEASURE_LIFE_NS`].

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering};

use radix_hal::{Cpu, CpuLocal, MAX_CPUS, Timer, UserMode};
use radix_klog::{error, info, warn};
use radix_pstate::{Driver, Levels, Preference, Request, Scale, Speed};
use radix_sync::{Mutex, Once};

use crate::{Arch, work};

/// How long the running frequency is measured over.
const MEASURE_NS: u64 = 10_000_000;
/// How long a measurement stands for the next look.
const MEASURE_LIFE_NS: u64 = 100_000_000;

/// No preference was asked for (`epp=`): the firmware's stays.
const FIRMWARE_PREFERENCE: u16 = u16::MAX;

/// What Linux's `energy_performance_available_preferences` lists.
const AVAILABLE_PREFERENCES: &str = "default performance balance_performance balance_power power";

/// The control the CPUs have, once it is on.
static DRIVER: Once<Driver> = Once::new();
/// The energy preference asked for, or [`FIRMWARE_PREFERENCE`].
static PREFERENCE: AtomicU16 = AtomicU16::new(FIRMWARE_PREFERENCE);
/// Each CPU's energy preference as its firmware left it, which
/// [`FIRMWARE_PREFERENCE`] asks for again.
static FIRMWARE_PREFERENCES: [AtomicU8; MAX_CPUS] = [const { AtomicU8::new(0) }; MAX_CPUS];
/// Whether the CPUs may run above their base frequency (`cpu.boost`).
static BOOST: AtomicBool = AtomicBool::new(Speed::BOOT.boost);
/// Whether the CPUs are held at the highest level they may run at
/// (`cpu.speed`).
static HELD: AtomicBool = AtomicBool::new(Speed::BOOT.held);
/// One change of what the CPUs are asked for at a time, of a setting or
/// of passive cooling: each asks every CPU again, and two at once could
/// leave a CPU with the request of the one that read the values first.
/// Neither the settings' callers nor the thermal thread is a work thread,
/// which [`on_every_cpu`] waits for while it is held.
static ASKING: Mutex<()> = Mutex::new(());
/// Whether the CPUs' highest levels differ, as AMD's do on CPUs that rank
/// their cores.
static RANKED: Once<bool> = Once::new();

/// A CPU's control, once on.
#[derive(Clone, Copy, Debug)]
struct Control {
    levels: Levels,
    scale: Scale,
    /// The level its highest frequency is that of where the CPUs rank
    /// their cores ([`Cpu::performance_scale`]).
    ranked_top: u8,
}

impl Control {
    /// The level its highest frequency is that of.
    fn top(&self) -> u8 {
        if RANKED.get().copied().unwrap_or(false) { self.ranked_top } else { self.levels.highest }
    }

    /// The frequency of a request's `level`, in kilohertz: its highest
    /// level is the top one's.
    fn khz(&self, level: u8) -> u64 {
        let top = self.top();
        self.scale.khz(if level >= self.levels.highest { top } else { level.min(top) })
    }
}

/// Each CPU's control, once on and if its levels made sense.
static CONTROLS: [Once<Control>; MAX_CPUS] = [const { Once::new() }; MAX_CPUS];
/// Each CPU's request, as its four bytes.
static REQUESTS: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];

/// Each CPU's APERF and MPERF at the start of the last measurement.
static COUNTS: [[AtomicU64; 2]; MAX_CPUS] =
    [const { [AtomicU64::new(0), AtomicU64::new(0)] }; MAX_CPUS];
/// The frequency each CPU ran at in the last measurement, in kilohertz;
/// 0 if it slept throughout.
static RUNNING_KHZ: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// When the last measurement ended; 0 before the first.
static MEASURED_NS: AtomicU64 = AtomicU64::new(0);
/// One measurement at a time.
static MEASURING: Mutex<()> = Mutex::new(());

/// Each thermal zone's step of passive cooling.
static ZONE_STEPS: [AtomicU8; crate::acpi::thermal::MAX_ZONES] =
    [const { AtomicU8::new(0) }; crate::acpi::thermal::MAX_ZONES];
/// The step every CPU is cooled at: the most any zone asks for.
static STEP: AtomicU8 = AtomicU8::new(0);

/// Turns every CPU's control on, unless `off` (`pstate=off`), with the
/// energy preference `epp` names (`epp=`). Called once, at the end of
/// boot, once the work threads run.
pub fn start(off: bool, epp: Option<&str>) {
    let Some(driver) = Arch::performance_driver() else {
        info!(
            "CPU frequency: left to the firmware: the CPUs have neither Intel's HWP nor AMD's CPPC"
        );
        return;
    };
    if off {
        info!("CPU frequency: left to the firmware (`pstate=off`), though the CPUs have {driver}");
        return;
    }
    match epp.map(|name| (name, Preference::parse(name))) {
        None => {}
        Some((_, Some(preference))) if driver.has_preference() => {
            PREFERENCE.store(u16::from(preference.value(driver)), Ordering::Relaxed);
        }
        Some((_, Some(_))) => warn!(
            "CPU frequency: {driver} has no energy preference on these CPUs; `epp` is ignored"
        ),
        Some((name, None)) => warn!(
            "CPU frequency: `epp={name}` is not performance, balance_performance, balance_power or power; \
             the firmware's preference stays"
        ),
    }
    let _ = DRIVER.set(driver);
    on_every_cpu(start_here);

    let mut controls = CONTROLS.iter().filter_map(Once::get);
    let Some(first) = controls.next().copied() else {
        error!(
            "CPU frequency: {driver} gave levels that make no sense; the CPUs are left to the firmware"
        );
        return;
    };
    let (mut count, mut ranked) = (1, false);
    for control in controls {
        count += 1;
        ranked |= control.levels.highest != first.levels.highest;
    }
    // Only AMD's highest frequency depends on the ranks.
    let ranked = ranked && driver == Driver::Cppc;
    let _ = RANKED.set(ranked);
    let skipped = crate::sched::online().len().saturating_sub(count);

    let mut line = radix_klog::TextBuf::<192>::new();
    let _ = write!(line, "{driver} on {count} CPUs");
    let (min_khz, max_khz) =
        CONTROLS.iter().filter_map(Once::get).fold((u64::MAX, 0), |(min, max), c| {
            (min.min(c.khz(c.levels.lowest)), max.max(c.khz(c.levels.highest)))
        });
    let _ = write!(
        line,
        ", {} to {} MHz, base {} MHz",
        min_khz / 1000,
        max_khz / 1000,
        first.scale.khz(first.levels.nominal) / 1000
    );
    if ranked {
        let _ = write!(line, ", its cores ranked");
    }
    if driver.has_preference() {
        let preference = request(first_cpu()).preference;
        let _ = match Preference::of(driver, preference) {
            Some(preference) => write!(line, ", energy preference {}", preference.name()),
            None => write!(line, ", energy preference {preference}"),
        };
    }
    info!("CPU frequency: {}", line.as_str());
    if skipped > 0 {
        error!(
            "CPU frequency: {skipped} CPUs gave levels that make no sense, and are left to the firmware"
        );
    }
}

/// On each CPU, from [`start`]: turns its control on and asks for what
/// the speed at boot allows.
fn start_here() {
    let cpu = Arch::current_cpu();
    let Some((levels, firmware)) = Arch::start_performance() else { return };
    if !levels.valid() {
        return;
    }
    FIRMWARE_PREFERENCES[cpu].store(firmware.preference, Ordering::Relaxed);
    let request = Request::at(levels, speed(), preference_here(cpu));
    Arch::request_performance(request);
    let Some((scale, ranked_top)) = Arch::performance_scale(levels) else { return };
    REQUESTS[cpu].store(pack(request), Ordering::Relaxed);
    let _ = CONTROLS[cpu].set(Control { levels, scale, ranked_top });
}

/// The energy preference the CPUs were asked for: `None` for their
/// firmware's, and wherever their speed is the firmware's or their
/// control has no preference.
pub fn preference() -> Option<Preference> {
    let driver = driver().filter(|driver| driver.has_preference())?;
    match PREFERENCE.load(Ordering::Relaxed) {
        FIRMWARE_PREFERENCE => None,
        value => Preference::of(driver, value as u8),
    }
}

/// Asks every CPU for the energy preference `preference`, or for its
/// firmware's again (`None`), as `epp=` does at boot (ADR 0476). Returns
/// false, and asks nothing, for a preference where the CPUs can have
/// none: their speed is the firmware's, or their control has no
/// preference.
pub fn set_preference(preference: Option<Preference>) -> bool {
    let Some(driver) = driver().filter(|driver| driver.has_preference()) else {
        return preference.is_none();
    };
    let _one = ASKING.lock();
    let value = preference.map_or(FIRMWARE_PREFERENCE, |p| u16::from(p.value(driver)));
    if PREFERENCE.swap(value, Ordering::Relaxed) != value {
        on_every_cpu(ask_here);
    }
    true
}

/// Whether the CPUs may run above their base frequency.
pub fn boost() -> bool {
    BOOST.load(Ordering::Relaxed)
}

/// Lets every CPU run above its base frequency, or keeps it at most at
/// that frequency, its nominal level, as Linux's `no_turbo` does (the
/// setting `cpu.boost`, ADR 0557). Returns false, and changes nothing,
/// for keeping it there where the CPUs' speed is the firmware's.
pub fn set_boost(boost: bool) -> bool {
    change(&BOOST, boost, Speed::BOOT.boost)
}

/// Whether the CPUs are held at the highest level they may run at.
pub fn held() -> bool {
    HELD.load(Ordering::Relaxed)
}

/// Holds every CPU at the highest level it may run at, as Linux's
/// performance governor does, or lets each pick its own speed again (the
/// setting `cpu.speed`, ADR 0557). Returns false, and changes nothing,
/// for holding them where the CPUs' speed is the firmware's.
pub fn set_held(held: bool) -> bool {
    change(&HELD, held, Speed::BOOT.held)
}

/// Sets `setting` to `value`, and asks every CPU again when that changes
/// it. Where the CPUs' speed is the firmware's, only `at_boot`, the value
/// the setting starts with, can be had.
fn change(setting: &AtomicBool, value: bool, at_boot: bool) -> bool {
    if driver().is_none() {
        return value == at_boot;
    }
    let _one = ASKING.lock();
    if setting.swap(value, Ordering::Relaxed) != value {
        on_every_cpu(ask_here);
    }
    true
}

/// How fast the kernel lets the CPUs run now.
fn speed() -> Speed {
    Speed {
        boost: BOOST.load(Ordering::Relaxed),
        held: HELD.load(Ordering::Relaxed),
        cooling_step: STEP.load(Ordering::Relaxed),
    }
}

/// The energy preference CPU `cpu` is asked for: the one set, or its
/// firmware's.
fn preference_here(cpu: usize) -> u8 {
    match PREFERENCE.load(Ordering::Relaxed) {
        FIRMWARE_PREFERENCE => FIRMWARE_PREFERENCES[cpu].load(Ordering::Relaxed),
        value => value as u8,
    }
}

/// On each CPU, after a setting or passive cooling changed: asks for what
/// they all allow now.
fn ask_here() {
    let cpu = Arch::current_cpu();
    let Some(control) = CONTROLS[cpu].get() else { return };
    let request = Request::at(control.levels, speed(), preference_here(cpu));
    Arch::request_performance(request);
    REQUESTS[cpu].store(pack(request), Ordering::Relaxed);
}

/// Runs `job` once on every CPU: here first, then on the others.
fn on_every_cpu(job: fn()) {
    Arch::preempt_disable();
    let me = Arch::current_cpu();
    job();
    Arch::preempt_enable();
    work::on_others(me, job);
}

fn pack(request: Request) -> u32 {
    let Request { min, max, desired, preference } = request;
    u32::from_le_bytes([min, max, desired, preference])
}

fn request(cpu: usize) -> Request {
    let [min, max, desired, preference] = REQUESTS[cpu].load(Ordering::Relaxed).to_le_bytes();
    Request { min, max, desired, preference }
}

fn first_cpu() -> usize {
    CONTROLS.iter().position(|control| control.get().is_some()).unwrap_or(0)
}

/// The control the CPUs have, once on.
pub fn driver() -> Option<Driver> {
    DRIVER.get().copied().filter(|_| CONTROLS.iter().any(|control| control.get().is_some()))
}

/// The CPUs whose control is on.
pub fn cpus() -> impl Iterator<Item = usize> {
    (0..MAX_CPUS).filter(|&cpu| CONTROLS[cpu].get().is_some())
}

/// A file of a CPU's `cpufreq` policy, as Linux's drivers have them.
#[derive(Clone, Copy, Debug)]
pub enum File {
    AffectedCpus,
    CpuinfoMaxFreq,
    CpuinfoMinFreq,
    RelatedCpus,
    ScalingAvailableGovernors,
    ScalingCurFreq,
    ScalingDriver,
    ScalingGovernor,
    ScalingMaxFreq,
    ScalingMinFreq,
    EnergyPerformanceAvailablePreferences,
    EnergyPerformancePreference,
    BaseFrequency,
    AmdHighestPerf,
    AmdLowestNonlinearFreq,
    AmdMaxFreq,
}

/// Every policy's files.
const FILES: &[(&str, File)] = &[
    ("affected_cpus", File::AffectedCpus),
    ("cpuinfo_max_freq", File::CpuinfoMaxFreq),
    ("cpuinfo_min_freq", File::CpuinfoMinFreq),
    ("related_cpus", File::RelatedCpus),
    ("scaling_available_governors", File::ScalingAvailableGovernors),
    ("scaling_cur_freq", File::ScalingCurFreq),
    ("scaling_driver", File::ScalingDriver),
    ("scaling_governor", File::ScalingGovernor),
    ("scaling_max_freq", File::ScalingMaxFreq),
    ("scaling_min_freq", File::ScalingMinFreq),
];

/// A policy's files where the request has an energy preference.
const PREFERENCE_FILES: &[(&str, File)] = &[
    ("energy_performance_available_preferences", File::EnergyPerformanceAvailablePreferences),
    ("energy_performance_preference", File::EnergyPerformancePreference),
];

/// Intel's policies' own file.
const INTEL_FILES: &[(&str, File)] = &[("base_frequency", File::BaseFrequency)];

/// AMD's policies' own files.
const AMD_FILES: &[(&str, File)] = &[
    ("amd_pstate_highest_perf", File::AmdHighestPerf),
    ("amd_pstate_lowest_nonlinear_freq", File::AmdLowestNonlinearFreq),
    ("amd_pstate_max_freq", File::AmdMaxFreq),
];

/// The files of a policy of `driver`'s.
pub fn files(driver: Driver) -> impl Iterator<Item = &'static (&'static str, File)> {
    let preference: &[_] = if driver.has_preference() { PREFERENCE_FILES } else { &[] };
    let own = match driver {
        Driver::Hwp { .. } => INTEL_FILES,
        Driver::Cppc => AMD_FILES,
    };
    FILES.iter().chain(preference).chain(own)
}

/// Writes `cpu`'s `file` and its newline.
pub fn write_file(cpu: usize, file: File, out: &mut dyn Write) -> fmt::Result {
    let (Some(&driver), Some(control)) = (DRIVER.get(), CONTROLS[cpu].get()) else {
        return Err(fmt::Error);
    };
    let request = request(cpu);
    match file {
        File::AffectedCpus | File::RelatedCpus => writeln!(out, "{cpu}"),
        File::CpuinfoMaxFreq => writeln!(out, "{}", control.khz(control.levels.highest)),
        File::CpuinfoMinFreq => writeln!(out, "{}", control.khz(control.levels.lowest)),
        // Linux's intel_pstate and amd-pstate-epp offer these two: the
        // performance governor holds a CPU at the top of its range.
        File::ScalingAvailableGovernors => writeln!(out, "performance powersave"),
        File::ScalingGovernor => {
            writeln!(out, "{}", if held() { "performance" } else { "powersave" })
        }
        File::ScalingCurFreq => {
            measure();
            match RUNNING_KHZ[cpu].load(Ordering::Relaxed) {
                // A CPU that slept throughout: its base frequency, as Linux.
                0 => writeln!(out, "{}", control.scale.khz(control.levels.nominal)),
                khz => writeln!(out, "{khz}"),
            }
        }
        File::ScalingDriver => writeln!(out, "{}", driver.linux_name()),
        File::ScalingMaxFreq => writeln!(out, "{}", control.khz(request.max)),
        File::ScalingMinFreq => writeln!(out, "{}", control.khz(request.min)),
        File::EnergyPerformanceAvailablePreferences => writeln!(out, "{AVAILABLE_PREFERENCES}"),
        File::EnergyPerformancePreference => match Preference::of(driver, request.preference) {
            Some(preference) => writeln!(out, "{}", preference.name()),
            None => writeln!(out, "{}", request.preference),
        },
        File::BaseFrequency => writeln!(out, "{}", control.scale.khz(control.levels.nominal)),
        File::AmdHighestPerf => writeln!(out, "{}", control.top()),
        File::AmdLowestNonlinearFreq => {
            writeln!(out, "{}", control.scale.khz(control.levels.efficient))
        }
        File::AmdMaxFreq => writeln!(out, "{}", control.khz(control.levels.highest)),
    }
}

/// Sets thermal zone `zone`'s step of passive cooling (ADR 0203): every
/// CPU's highest level is capped at the most any zone asks for
/// ([`radix_pstate::cooled`]). Returns false where the CPUs' speed is the
/// firmware's, which nothing here lowers.
pub fn cool(zone: usize, step: u8) -> bool {
    if let Some(slot) = ZONE_STEPS.get(zone) {
        slot.store(step, Ordering::Relaxed);
    }
    if driver().is_none() {
        return false;
    }
    let _one = ASKING.lock();
    let wanted = ZONE_STEPS.iter().map(|step| step.load(Ordering::Relaxed)).max().unwrap_or(0);
    if STEP.swap(wanted, Ordering::Relaxed) != wanted {
        on_every_cpu(ask_here);
    }
    true
}

/// The step every CPU is cooled at.
pub fn step() -> u8 {
    STEP.load(Ordering::Relaxed)
}

/// Measures the frequency every CPU runs at, unless the last measurement
/// is recent.
fn measure() {
    let _one = MEASURING.lock();
    let last = MEASURED_NS.load(Ordering::Relaxed);
    if last != 0 && Arch::now_ns().saturating_sub(last) < MEASURE_LIFE_NS {
        return;
    }
    on_every_cpu(count_from);
    crate::sched::sleep_ns(MEASURE_NS);
    on_every_cpu(count_to);
    MEASURED_NS.store(Arch::now_ns().max(1), Ordering::Relaxed);
}

/// On each CPU: the measurement starts.
fn count_from() {
    let cpu = Arch::current_cpu();
    if let Some((aperf, mperf)) = Arch::performance_counts() {
        COUNTS[cpu][0].store(aperf, Ordering::Relaxed);
        COUNTS[cpu][1].store(mperf, Ordering::Relaxed);
    }
}

/// On each CPU: the measurement ends.
fn count_to() {
    let cpu = Arch::current_cpu();
    let khz = Arch::performance_counts().and_then(|(aperf, mperf)| {
        let aperf = aperf.wrapping_sub(COUNTS[cpu][0].load(Ordering::Relaxed));
        let mperf = mperf.wrapping_sub(COUNTS[cpu][1].load(Ordering::Relaxed));
        radix_pstate::running_khz(Arch::cycle_counter_hz() / 1000, aperf, mperf)
    });
    RUNNING_KHZ[cpu].store(khz.unwrap_or(0), Ordering::Relaxed);
}
