// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The words of an event and of a discrete sensor's states: the threshold
//! events of table 42-2 (event type 0x01), the generic events (types 0x02
//! to 0x0C of the same table) and the sensor-specific events of table
//! 42-3 (type 0x6F) for the sensor types an administrator meets. What is
//! not listed reads as its numbers.

use Severity::{Critical, Info, NonRecoverable, Warning};

/// How much an event matters, as the event's own words say it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Severity {
    Info,
    Warning,
    Critical,
    NonRecoverable,
}

impl Severity {
    /// The word for the severity, as the JSON has it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Critical => "critical",
            Severity::NonRecoverable => "non-recoverable",
        }
    }
}

/// The event/reading type code of threshold sensors.
const THRESHOLD: u8 = 0x01;
/// The event/reading type code of sensor-specific events.
const SENSOR_SPECIFIC: u8 = 0x6F;

/// The threshold events by event data 1 offset (table 42-2, type 0x01).
const THRESHOLD_EVENTS: [(&str, Severity); 12] = [
    ("lower non-critical going low", Warning),
    ("lower non-critical going high", Warning),
    ("lower critical going low", Critical),
    ("lower critical going high", Critical),
    ("lower non-recoverable going low", NonRecoverable),
    ("lower non-recoverable going high", NonRecoverable),
    ("upper non-critical going low", Warning),
    ("upper non-critical going high", Warning),
    ("upper critical going low", Critical),
    ("upper critical going high", Critical),
    ("upper non-recoverable going low", NonRecoverable),
    ("upper non-recoverable going high", NonRecoverable),
];

const USAGE_STATE: &[(&str, Severity)] =
    &[("transition to idle", Info), ("transition to active", Info), ("transition to busy", Info)];
const STATE: &[(&str, Severity)] = &[("state deasserted", Info), ("state asserted", Info)];
const PREDICTIVE_FAILURE: &[(&str, Severity)] =
    &[("predictive failure deasserted", Info), ("predictive failure asserted", Warning)];
const LIMIT: &[(&str, Severity)] = &[("limit not exceeded", Info), ("limit exceeded", Warning)];
const PERFORMANCE: &[(&str, Severity)] =
    &[("performance met", Info), ("performance lags", Warning)];
const SEVERITY_TRANSITION: &[(&str, Severity)] = &[
    ("transition to OK", Info),
    ("transition to non-critical from OK", Warning),
    ("transition to critical from less severe", Critical),
    ("transition to non-recoverable from less severe", NonRecoverable),
    ("transition to non-critical from more severe", Warning),
    ("transition to critical from non-recoverable", Critical),
    ("transition to non-recoverable", NonRecoverable),
    ("monitor", Info),
    ("informational", Info),
];
const PRESENCE: &[(&str, Severity)] =
    &[("device removed or absent", Info), ("device inserted or present", Info)];
const ENABLED: &[(&str, Severity)] = &[("device disabled", Info), ("device enabled", Info)];
const AVAILABILITY: &[(&str, Severity)] = &[
    ("transition to running", Info),
    ("transition to in test", Info),
    ("transition to power off", Info),
    ("transition to on line", Info),
    ("transition to off line", Info),
    ("transition to off duty", Info),
    ("transition to degraded", Warning),
    ("transition to power save", Info),
    ("install error", Warning),
];
const REDUNDANCY: &[(&str, Severity)] = &[
    ("fully redundant", Info),
    ("redundancy lost", Critical),
    ("redundancy degraded", Warning),
    ("non-redundant, sufficient resources from redundant", Warning),
    ("non-redundant, sufficient resources from insufficient", Warning),
    ("non-redundant, insufficient resources", Critical),
    ("redundancy degraded from fully redundant", Warning),
    ("redundancy degraded from non-redundant", Warning),
];
const ACPI_POWER_STATE: &[(&str, Severity)] =
    &[("D0 state", Info), ("D1 state", Info), ("D2 state", Info), ("D3 state", Info)];

const PHYSICAL_SECURITY: &[(&str, Severity)] = &[
    ("general chassis intrusion", Warning),
    ("drive bay intrusion", Warning),
    ("I/O card area intrusion", Warning),
    ("processor area intrusion", Warning),
    ("LAN leash lost", Warning),
    ("unauthorized dock or undock", Warning),
    ("fan area intrusion", Warning),
];
const PROCESSOR: &[(&str, Severity)] = &[
    ("IERR", Critical),
    ("thermal trip", Critical),
    ("FRB1 or BIST failure", Critical),
    ("FRB2 or hang in POST failure", Critical),
    ("FRB3 or processor startup failure", Critical),
    ("configuration error", Warning),
    ("uncorrectable CPU-complex error", Critical),
    ("presence detected", Info),
    ("disabled", Warning),
    ("terminator presence detected", Info),
    ("throttled", Warning),
    ("uncorrectable machine check exception", Critical),
    ("correctable machine check error", Warning),
];
const POWER_SUPPLY: &[(&str, Severity)] = &[
    ("presence detected", Info),
    ("failure detected", Critical),
    ("predictive failure", Warning),
    ("input lost (AC or DC)", Critical),
    ("input lost or out of range", Critical),
    ("input out of range, but present", Warning),
    ("configuration error", Warning),
    ("in standby", Info),
];
const POWER_UNIT: &[(&str, Severity)] = &[
    ("power off or down", Info),
    ("power cycle", Info),
    ("240VA power down", Critical),
    ("interlock power down", Critical),
    ("AC lost", Critical),
    ("soft-power control failure", Critical),
    ("power unit failure", Critical),
    ("predictive failure", Warning),
];
const MEMORY: &[(&str, Severity)] = &[
    ("correctable ECC error", Warning),
    ("uncorrectable ECC error", Critical),
    ("parity error", Critical),
    ("memory scrub failed", Warning),
    ("memory device disabled", Warning),
    ("correctable ECC logging limit reached", Warning),
    ("presence detected", Info),
    ("configuration error", Warning),
    ("spare", Info),
    ("automatically throttled", Warning),
    ("critical overtemperature", Critical),
];
const DRIVE_SLOT: &[(&str, Severity)] = &[
    ("drive presence", Info),
    ("drive fault", Critical),
    ("predictive failure", Warning),
    ("hot spare", Info),
    ("consistency or parity check in progress", Info),
    ("in critical array", Critical),
    ("in failed array", Critical),
    ("rebuild or remap in progress", Info),
    ("rebuild or remap aborted", Warning),
];
const EVENT_LOGGING: &[(&str, Severity)] = &[
    ("correctable memory error logging disabled", Warning),
    ("event type logging disabled", Warning),
    ("log area reset or cleared", Info),
    ("all event logging disabled", Warning),
    ("SEL full", Warning),
    ("SEL almost full", Warning),
];
const SYSTEM_EVENT: &[(&str, Severity)] = &[
    ("system reconfigured", Info),
    ("OEM system boot event", Info),
    ("undetermined system hardware failure", Critical),
    ("entry added to auxiliary log", Info),
    ("PEF action", Info),
    ("timestamp clock synchronization", Info),
];
const CRITICAL_INTERRUPT: &[(&str, Severity)] = &[
    ("front panel NMI or diagnostic interrupt", Critical),
    ("bus timeout", Critical),
    ("I/O channel check NMI", Critical),
    ("software NMI", Critical),
    ("PCI PERR", Critical),
    ("PCI SERR", Critical),
    ("EISA fail safe timeout", Critical),
    ("bus correctable error", Warning),
    ("bus uncorrectable error", Critical),
    ("fatal NMI", Critical),
    ("bus fatal error", Critical),
    ("bus degraded", Warning),
];
/// Offsets 4 to 7 are reserved and have no words.
const WATCHDOG_2: &[(&str, Severity)] = &[
    ("timer expired", Critical),
    ("hard reset", Critical),
    ("power down", Critical),
    ("power cycle", Critical),
    ("", Info),
    ("", Info),
    ("", Info),
    ("", Info),
    ("timer interrupt", Info),
];

/// The words for `offset` in the table of a sensor with this type and
/// event/reading type: the generic table of table 42-2 for types 0x02 to
/// 0x0C, the sensor type's table of 42-3 for 0x6F.
fn table(sensor_type: u8, event_type: u8) -> Option<&'static [(&'static str, Severity)]> {
    match event_type {
        0x02 => Some(USAGE_STATE),
        0x03 => Some(STATE),
        0x04 => Some(PREDICTIVE_FAILURE),
        0x05 => Some(LIMIT),
        0x06 => Some(PERFORMANCE),
        0x07 => Some(SEVERITY_TRANSITION),
        0x08 => Some(PRESENCE),
        0x09 => Some(ENABLED),
        0x0A => Some(AVAILABILITY),
        0x0B => Some(REDUNDANCY),
        0x0C => Some(ACPI_POWER_STATE),
        SENSOR_SPECIFIC => match sensor_type {
            0x05 => Some(PHYSICAL_SECURITY),
            0x07 => Some(PROCESSOR),
            0x08 => Some(POWER_SUPPLY),
            0x09 => Some(POWER_UNIT),
            0x0C => Some(MEMORY),
            0x0D => Some(DRIVE_SLOT),
            0x10 => Some(EVENT_LOGGING),
            0x12 => Some(SYSTEM_EVENT),
            0x13 => Some(CRITICAL_INTERRUPT),
            0x23 => Some(WATCHDOG_2),
            _ => None,
        },
        _ => None,
    }
}

/// The name of one state of a discrete sensor: what offset `offset`
/// means for its type and event/reading type.
pub(super) fn offset_name(sensor_type: u8, event_type: u8, offset: u8) -> Option<String> {
    let (text, _) = table(sensor_type, event_type)?.get(usize::from(offset))?;
    Some((*text).to_string()).filter(|text| !text.is_empty())
}

/// What an event says, and how much it matters. `data1` is event data 1,
/// of which the low nibble is the offset and bits 7:6 say what event data
/// 2 holds (3 is a sensor-specific extension, which a watchdog 2 event
/// uses for the timer's use); `data2` is event data 2.
///
/// A deasserted event matters as information only: the condition it names
/// has gone.
pub(super) fn describe(
    sensor_type: u8,
    event_type: u8,
    asserted: bool,
    data1: u8,
    data2: u8,
) -> (String, Severity) {
    let offset = data1 & 0x0F;
    let (mut text, severity) = if event_type == THRESHOLD {
        match THRESHOLD_EVENTS.get(usize::from(offset)) {
            Some(&(text, severity)) => (text.to_string(), severity),
            None => (format!("threshold event offset {offset}"), Info),
        }
    } else {
        match offset_name(sensor_type, event_type, offset) {
            Some(text) => {
                let severity = table(sensor_type, event_type)
                    .and_then(|rows| rows.get(usize::from(offset)))
                    .map_or(Info, |&(_, severity)| severity);
                (text, severity)
            }
            None => (format!("event type 0x{event_type:02X}, offset {offset}"), Info),
        }
    };
    if sensor_type == 0x23 && data1 >> 6 == 3 {
        text.push_str(&format!(", {}", timer_use(data2 & 0x0F)));
    }
    (text, if asserted { severity } else { Info })
}

/// The timer use of a watchdog 2 event (table 42-3, event data 2).
fn timer_use(code: u8) -> String {
    match code {
        1 => "BIOS FRB2 timer".into(),
        2 => "BIOS or POST timer".into(),
        3 => "OS load timer".into(),
        4 => "SMS or OS timer".into(),
        5 => "OEM timer".into(),
        0 => "no timer use".into(),
        other => format!("timer use {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_events_say_which_threshold_and_which_way() {
        let (text, severity) = describe(0x01, 0x01, true, 0x09, 0);
        assert_eq!(text, "upper critical going high");
        assert_eq!(severity, Critical);
        let (_, severity) = describe(0x01, 0x01, false, 0x09, 0);
        assert_eq!(severity, Info);
        assert_eq!(describe(0x04, 0x01, true, 0x00, 0).0, "lower non-critical going low");
        assert_eq!(describe(0x01, 0x01, true, 0x0B, 0).1, NonRecoverable);
    }

    #[test]
    fn the_sensor_types_of_the_design_have_their_words() {
        let cases = [
            (0x08, 1, "failure detected"),
            (0x0C, 1, "uncorrectable ECC error"),
            (0x07, 1, "thermal trip"),
            (0x23, 0, "timer expired"),
            (0x12, 2, "undetermined system hardware failure"),
            (0x13, 9, "fatal NMI"),
            (0x0D, 1, "drive fault"),
        ];
        for (sensor_type, offset, expected) in cases {
            let (text, severity) = describe(sensor_type, 0x6F, true, offset, 0);
            assert_eq!(text, expected);
            assert!(severity >= Info);
        }
        assert_eq!(describe(0x08, 0x6F, true, 1, 0).1, Critical);
        assert_eq!(describe(0x0C, 0x6F, true, 0, 0).1, Warning);
    }

    #[test]
    fn a_watchdog_event_says_what_timer_expired() {
        let (text, _) = describe(0x23, 0x6F, true, 0xC1, 0x04);
        assert_eq!(text, "hard reset, SMS or OS timer");
        let (text, _) = describe(0x23, 0x6F, true, 0x01, 0x04);
        assert_eq!(text, "hard reset");
    }

    #[test]
    fn what_is_not_listed_reads_as_its_numbers() {
        assert_eq!(describe(0x2C, 0x6F, true, 0x03, 0).0, "event type 0x6F, offset 3");
        assert_eq!(describe(0x01, 0x01, true, 0x0F, 0).0, "threshold event offset 15");
        assert_eq!(describe(0x08, 0x6F, true, 0x0F, 0).0, "event type 0x6F, offset 15");
    }

    #[test]
    fn generic_discrete_states_have_names() {
        assert_eq!(offset_name(0x08, 0x0B, 1).as_deref(), Some("redundancy lost"));
        assert_eq!(offset_name(0x08, 0x08, 1).as_deref(), Some("device inserted or present"));
        assert_eq!(offset_name(0x23, 0x6F, 5), None);
        assert_eq!(offset_name(0x01, 0x01, 0), None);
    }
}
