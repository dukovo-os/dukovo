// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A UPS's state, from the reports of a HID power device (*Universal
//! Serial Bus Usage Tables for HID Power Devices 1.0*; ADR 0741).
//!
//! A UPS that plugs in by USB is a HID device whose top-level collection
//! is on the Power Device page. It says how much charge its battery holds,
//! how long that lasts, whether the mains is there and whether the battery
//! is low, mostly in Feature reports that the host reads with GET_REPORT,
//! some in Input reports too. [`Layout::parse`] keeps where each value of
//! [`WANTED`] is; [`Layout::power_reports`] says which reports to ask for,
//! and [`Layout::read_power`] reads one into a [`State`].
//!
//! [`State::low`] is when the machine shuts down: the UPS says so itself,
//! as its Below Remaining Capacity Limit, Remaining Time Limit Expired or
//! Shutdown Imminent, or its charge or run time is at the limits it sets
//! ([`State::capacity_limit`], [`State::time_limit`]), else at
//! [`LOW_PERCENT`] or [`LOW_SECONDS`], and in each case only while it runs
//! on its battery.

use core::alloc::Allocator;

use super::{Layout, Usage, value};

/// The most values a layout keeps.
pub const MAX_VALUES: usize = 24;

/// The charge, in percent, at or under which the battery is low when the
/// UPS sets no limit of its own.
pub const LOW_PERCENT: u32 = 10;
/// The run time left, in seconds, at or under which it is low when the UPS
/// sets no limit of its own.
pub const LOW_SECONDS: u32 = 120;

/// The usages a UPS's state is read from.
pub mod usage {
    use super::Usage;

    /// A UPS: the top-level collection of one.
    pub const UPS: Usage = 0x0084_0004;
    /// What the load draws, in percent of what the UPS can give.
    pub const PERCENT_LOAD: Usage = 0x0084_0035;
    /// The UPS will turn its outlets off soon.
    pub const SHUTDOWN_IMMINENT: Usage = 0x0084_0069;
    /// The charge under which the UPS says the battery is low.
    pub const REMAINING_CAPACITY_LIMIT: Usage = 0x0085_0029;
    /// The run time under which it says so, in seconds.
    pub const REMAINING_TIME_LIMIT: Usage = 0x0085_002a;
    /// The unit of the capacities: 0 mAh, 1 mWh, 2 percent.
    pub const CAPACITY_MODE: Usage = 0x0085_002c;
    /// The charge is under the limit.
    pub const BELOW_REMAINING_CAPACITY_LIMIT: Usage = 0x0085_0042;
    /// The run time is under the limit.
    pub const REMAINING_TIME_LIMIT_EXPIRED: Usage = 0x0085_0043;
    /// The battery charges.
    pub const CHARGING: Usage = 0x0085_0044;
    /// It powers the load.
    pub const DISCHARGING: Usage = 0x0085_0045;
    /// It is full.
    pub const FULLY_CHARGED: Usage = 0x0085_0046;
    /// It is worn out and should be replaced.
    pub const NEED_REPLACEMENT: Usage = 0x0085_004b;
    /// The charge it holds, in the capacity mode's unit.
    pub const REMAINING_CAPACITY: Usage = 0x0085_0066;
    /// The charge it holds when full.
    pub const FULL_CHARGE_CAPACITY: Usage = 0x0085_0067;
    /// How long the charge lasts at the load now, in seconds.
    pub const RUN_TIME_TO_EMPTY: Usage = 0x0085_0068;
    /// The mains is there.
    pub const AC_PRESENT: Usage = 0x0085_00d0;
}

/// The usages [`Layout::parse`] keeps the values of.
pub const WANTED: [Usage; 16] = [
    usage::PERCENT_LOAD,
    usage::SHUTDOWN_IMMINENT,
    usage::REMAINING_CAPACITY_LIMIT,
    usage::REMAINING_TIME_LIMIT,
    usage::CAPACITY_MODE,
    usage::BELOW_REMAINING_CAPACITY_LIMIT,
    usage::REMAINING_TIME_LIMIT_EXPIRED,
    usage::CHARGING,
    usage::DISCHARGING,
    usage::FULLY_CHARGED,
    usage::NEED_REPLACEMENT,
    usage::REMAINING_CAPACITY,
    usage::FULL_CHARGE_CAPACITY,
    usage::RUN_TIME_TO_EMPTY,
    usage::AC_PRESENT,
    // The Battery System page's own Shutdown Imminent.
    0x0085_0069,
];

/// The type of a report a value is in, as GET_REPORT names it (HID 1.11,
/// 7.2.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An Input report: 1.
    Input = 1,
    /// A Feature report: 3.
    Feature = 3,
}

/// Where a value of [`WANTED`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Value {
    pub(crate) usage: Usage,
    pub(crate) kind: Kind,
    pub(crate) report: u8,
    /// In bits from the start of the report's data, after its ID.
    pub(crate) offset: u32,
    pub(crate) size: u8,
    pub(crate) signed: bool,
}

/// A report to ask for: its type, its ID (0 when the device numbers
/// none) and how many bytes it is, with the ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    /// Its type.
    pub kind: Kind,
    /// Its ID.
    pub id: u8,
    /// Its length in bytes, with the ID.
    pub bytes: usize,
}

/// What a UPS said, each value None until a report says it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// The charge, in the unit of `capacity_mode`.
    pub remaining: Option<u32>,
    /// The charge when full, in the same unit.
    pub full: Option<u32>,
    /// 0 mAh, 1 mWh, 2 percent.
    pub capacity_mode: Option<u32>,
    /// How long the charge lasts, in seconds.
    pub run_time: Option<u32>,
    /// The UPS's own limits: the charge, in the capacity mode's unit, and
    /// the run time, in seconds.
    pub capacity_limit: Option<u32>,
    /// The run time limit.
    pub time_limit: Option<u32>,
    /// What the load draws, in percent.
    pub load: Option<u32>,
    /// Its flags.
    pub ac_present: Option<bool>,
    /// The battery charges.
    pub charging: Option<bool>,
    /// The battery powers the load.
    pub discharging: Option<bool>,
    /// The battery is full.
    pub fully_charged: Option<bool>,
    /// The charge is under the UPS's limit.
    pub below_capacity_limit: Option<bool>,
    /// The run time is under the UPS's limit.
    pub time_limit_expired: Option<bool>,
    /// The UPS will turn its outlets off soon.
    pub shutdown_imminent: Option<bool>,
    /// The battery should be replaced.
    pub need_replacement: Option<bool>,
}

/// What the battery is doing, as Linux's power supply class's `status`
/// names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The battery charges.
    Charging,
    /// It powers the load: the mains is gone.
    Discharging,
    /// It is full.
    Full,
    /// On the mains, neither charging nor full.
    NotCharging,
    /// The UPS does not say.
    Unknown,
}

impl Status {
    /// Its name in `status`.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Charging => "Charging",
            Status::Discharging => "Discharging",
            Status::Full => "Full",
            Status::NotCharging => "Not charging",
            Status::Unknown => "Unknown",
        }
    }
}

impl State {
    /// The charge in percent of full, when the UPS says it.
    pub fn percent(&self) -> Option<u32> {
        let remaining = self.remaining?;
        match (self.capacity_mode, self.full) {
            // In mAh or mWh, against the charge when full.
            (Some(0 | 1), Some(full)) if full > 0 => {
                Some((u64::from(remaining) * 100 / u64::from(full)).min(100) as u32)
            }
            (Some(0 | 1), _) => None,
            // In percent, as nearly every UPS says it, or a mode it does
            // not say.
            _ => Some(remaining.min(100)),
        }
    }

    /// Whether the load runs on the battery: the mains is gone.
    pub fn on_battery(&self) -> bool {
        self.ac_present == Some(false) || self.discharging == Some(true)
    }

    /// What the battery is doing.
    pub fn status(&self) -> Status {
        if self.on_battery() {
            Status::Discharging
        } else if self.charging == Some(true) {
            Status::Charging
        } else if self.fully_charged == Some(true) || self.percent() == Some(100) {
            Status::Full
        } else if self.ac_present == Some(true) {
            Status::NotCharging
        } else {
            Status::Unknown
        }
    }

    /// The charge, in percent, at or under which the battery is low: the
    /// UPS's own limit, else [`LOW_PERCENT`].
    pub fn low_percent(&self) -> u32 {
        let own = self.capacity_limit.and_then(|limit| {
            let in_percent = State { remaining: Some(limit), ..*self };
            in_percent.percent()
        });
        own.filter(|&p| p > 0).unwrap_or(LOW_PERCENT)
    }

    /// The run time, in seconds, at or under which it is low.
    pub fn low_seconds(&self) -> u32 {
        self.time_limit.filter(|&s| s > 0).unwrap_or(LOW_SECONDS)
    }

    /// Whether the machine should shut down: on the battery, and the UPS
    /// says it is low or its charge or run time is at the limit.
    pub fn low(&self) -> bool {
        if !self.on_battery() {
            return false;
        }
        let said = [self.below_capacity_limit, self.time_limit_expired, self.shutdown_imminent]
            .contains(&Some(true));
        said || self.percent().is_some_and(|p| p <= self.low_percent())
            || self.run_time.is_some_and(|s| s <= self.low_seconds())
    }

    /// Sets the value of `usage` to `value`.
    fn set(&mut self, usage: Usage, value: i32) {
        let number = u32::try_from(value).unwrap_or(0);
        let flag = value != 0;
        match usage {
            usage::REMAINING_CAPACITY => self.remaining = Some(number),
            usage::FULL_CHARGE_CAPACITY => self.full = Some(number),
            usage::CAPACITY_MODE => self.capacity_mode = Some(number),
            usage::RUN_TIME_TO_EMPTY => self.run_time = Some(number),
            usage::REMAINING_CAPACITY_LIMIT => self.capacity_limit = Some(number),
            usage::REMAINING_TIME_LIMIT => self.time_limit = Some(number),
            usage::PERCENT_LOAD => self.load = Some(number),
            usage::AC_PRESENT => self.ac_present = Some(flag),
            usage::CHARGING => self.charging = Some(flag),
            usage::DISCHARGING => self.discharging = Some(flag),
            usage::FULLY_CHARGED => self.fully_charged = Some(flag),
            usage::BELOW_REMAINING_CAPACITY_LIMIT => self.below_capacity_limit = Some(flag),
            usage::REMAINING_TIME_LIMIT_EXPIRED => self.time_limit_expired = Some(flag),
            usage::NEED_REPLACEMENT => self.need_replacement = Some(flag),
            _ => self.shutdown_imminent = Some(flag),
        }
    }
}

impl<A: Allocator + Clone> Layout<A> {
    /// Whether it is a UPS: a top-level collection on the Power Device
    /// page, and a charge or run time to read. A mouse that says how full
    /// its own battery is, on the Battery System page, is not one.
    pub fn is_ups(&self) -> bool {
        let power =
            self.applications().iter().any(|&u| u >> 16 == u32::from(super::page::POWER_DEVICE));
        power
            && self
                .power
                .iter()
                .any(|v| matches!(v.usage, usage::REMAINING_CAPACITY | usage::RUN_TIME_TO_EMPTY))
    }

    /// The reports that hold a value of [`WANTED`], each once, in the order
    /// the descriptor first names them.
    pub fn power_reports(&self, each: &mut dyn FnMut(Report)) {
        for (at, v) in self.power.iter().enumerate() {
            if self.power[..at].iter().any(|seen| seen.kind == v.kind && seen.report == v.report) {
                continue;
            }
            let bits = self.reports.iter().find(|r| r.id == v.report).map_or(0, |r| match v.kind {
                Kind::Input => r.input,
                Kind::Feature => r.feature,
            });
            let bytes = usize::from(bits).div_ceil(8) + usize::from(self.numbered);
            each(Report { kind: v.kind, id: v.report, bytes });
        }
    }

    /// Reads the report `data` of type `kind` and ID `id`, as GET_REPORT
    /// gave it, with its ID first when the device numbers its reports,
    /// into `state`. A value past the end of a short report is left as it
    /// was.
    pub fn read_power(&self, kind: Kind, id: u8, data: &[u8], state: &mut State) {
        let data = match (self.numbered, data.split_first()) {
            (true, Some((&first, rest))) if first == id => rest,
            (true, _) => return,
            (false, _) => data,
        };
        for v in self.power.iter().filter(|v| v.kind == kind && v.report == id) {
            if let Some(found) = value(data, v.offset, v.size, v.signed) {
                state.set(v.usage, found);
            }
        }
    }
}
