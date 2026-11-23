// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What metricsd reads of the machine's baseboard management controller
//! (ADR 0661): the temperatures, fans and power it measures, as the series
//! `bmc.temp{name}`, `bmc.fan{name}` and `bmc.power{name}`, and the entries
//! its event log gained since the last look, which go to the log and, when
//! they are critical, to alertd.
//!
//! The BMC is asked through the `ipmi` module, over `/devices/ipmi0`. The
//! sensor data repository is read when metricsd starts and again when
//! Get SDR Repository Info says it changed; each step after that sends
//! Get Sensor Reading for the sensors it keeps, and Get SEL Info to see
//! whether the log moved. The log is read only when it did.
//!
//! The last entry looked at is kept in `bmc-events` of the history's folder,
//! so that a restart tells only of what came after it. With no such file,
//! as on the first run, the entries that are there are taken as seen: a boot
//! does not replay the log. A machine with no `/devices/ipmi0`, or whose
//! BMC the service cannot ask, has no series and no events; the second is
//! said once in the log.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::sample::named;
use crate::alerts::protocol::{About, Alert, Severity as AlertSeverity};
use crate::ipmi::{self, Changed, Entry, Kind, Measured, Sensor, Severity, Transport, When};
use crate::util;

/// A way to open the BMC's interface.
pub(crate) type Open = Box<dyn FnMut() -> io::Result<Box<dyn Transport>>>;

/// A way to tell alertd of an event.
pub(crate) type Tell = Box<dyn FnMut(&Alert) -> Result<(), String>>;

/// The file of the history's folder that holds the last entry looked at.
const REMEMBERED: &str = "bmc-events";

/// The last entry of the log that was looked at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Last {
    pub id: u16,
    /// The entry's time as it is in the record, which tells a record from
    /// one that has the same number after the log was erased.
    pub stamp: u32,
}

impl Last {
    fn of(entry: &Entry) -> Last {
        let stamp = match entry.when {
            Some(When::Clock(seconds) | When::SinceStart(seconds)) => seconds,
            None => 0,
        };
        Last { id: entry.id, stamp }
    }
}

/// How far the log was looked at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Remembered {
    /// Never: the entries the first look finds are taken as seen.
    Never,
    /// The log had no entry.
    Empty,
    /// The log's last entry was this.
    Entry(Last),
}

impl Remembered {
    /// What the file `path` says, which is [`Remembered::Never`] when the
    /// file is not there or does not make sense.
    fn read(path: &Path) -> Remembered {
        let Ok(text) = fs::read_to_string(path) else { return Remembered::Never };
        let text = text.trim();
        if text == "empty" {
            return Remembered::Empty;
        }
        let parsed = text.split_once(' ').and_then(|(id, stamp)| {
            Some(Last { id: id.parse().ok()?, stamp: stamp.parse().ok()? })
        });
        parsed.map_or(Remembered::Never, Remembered::Entry)
    }

    fn text(self) -> Option<String> {
        match self {
            Remembered::Never => None,
            Remembered::Empty => Some("empty\n".into()),
            Remembered::Entry(last) => Some(format!("{} {}\n", last.id, last.stamp)),
        }
    }
}

/// An entry of the log that is new, as the log says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Event {
    pub id: u16,
    pub severity: Severity,
    /// The sensor it came from, when the repository names it.
    pub sensor: Option<String>,
    /// What happened, in the log's words: `upper critical going high,
    /// asserted`.
    pub text: String,
    /// When the BMC logged it.
    pub when: String,
}

/// What one look at the BMC found.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Look {
    pub series: Vec<(String, f32)>,
    pub events: Vec<Event>,
}

/// What metricsd has learned of one BMC: its sensors, and how far its log
/// was read.
pub(crate) struct Watch {
    /// The sensors of the repository; `None` before it was read.
    sensors: Option<Vec<Sensor>>,
    /// The repository's change times when it was read.
    changed: Option<Changed>,
    remembered: Remembered,
    /// What Get SEL Info said when the log was last read.
    log: Option<(u16, u32, u32)>,
}

/// The name of a sensor made safe for a series: braces and control
/// characters would end the instance early or break the line of `metrics
/// now`, so each is an underscore.
fn safe(name: &str) -> String {
    let flat: String = name
        .chars()
        .map(|c| if c.is_control() || c == '{' || c == '}' { '_' } else { c })
        .collect();
    flat.trim().to_string()
}

/// The series of each of `measured`, in order: `bmc.temp{name}`,
/// `bmc.fan{name}` or `bmc.power{name}`, and none for a voltage or a
/// current. Two sensors of one kind with one name are told apart by their
/// numbers: `bmc.temp{Temp #4}`.
pub(crate) fn series_names(measured: &[Measured]) -> Vec<Option<String>> {
    let name_of = |sensor: &Measured| {
        let name = safe(&sensor.name);
        if name.is_empty() { format!("sensor {}", sensor.number) } else { name }
    };
    measured
        .iter()
        .map(|sensor| {
            let metric = match sensor.kind {
                Kind::Temperature => "bmc.temp",
                Kind::Fan => "bmc.fan",
                Kind::Power => "bmc.power",
                Kind::Voltage | Kind::Current => return None,
            };
            let name = name_of(sensor);
            let twins =
                measured.iter().filter(|other| other.kind == sensor.kind && name_of(other) == name);
            let instance =
                if twins.count() > 1 { format!("{name} #{}", sensor.number) } else { name };
            Some(named(metric, &instance))
        })
        .collect()
}

impl Watch {
    /// A watch that goes on from `remembered`.
    pub(crate) fn new(remembered: Remembered) -> Watch {
        Watch { sensors: None, changed: None, remembered, log: None }
    }

    /// How far the log was looked at.
    pub(crate) fn remembered(&self) -> Remembered {
        self.remembered
    }

    /// Looks at the BMC: what its sensors read, and the entries its log
    /// gained.
    ///
    /// # Errors
    ///
    /// If the BMC does not answer.
    pub(crate) fn look(&mut self, link: &mut dyn Transport) -> ipmi::Result<Look> {
        self.refresh_sensors(link)?;
        let series = self.series(link)?;
        let events = self.new_events(link)?;
        Ok(Look { series, events })
    }

    /// Reads the sensor data repository when it was not read, or when its
    /// change times moved.
    fn refresh_sensors(&mut self, link: &mut dyn Transport) -> ipmi::Result<()> {
        let changed = match ipmi::repository_changed(link) {
            Ok(changed) => Some(changed),
            // A BMC that does not say when its repository changed keeps
            // what was read at the start.
            Err(ipmi::Error::Completion(_)) => None,
            Err(error) => return Err(error),
        };
        if self.sensors.is_none() || (changed.is_some() && changed != self.changed) {
            self.sensors = Some(ipmi::sensors(link)?);
            self.changed = changed;
        }
        Ok(())
    }

    /// The series of the sensors that have a value now.
    fn series(&self, link: &mut dyn Transport) -> ipmi::Result<Vec<(String, f32)>> {
        let sensors = self.sensors.as_deref().unwrap_or_default();
        // The voltages and currents are left out: a series is kept only
        // for what the history is for.
        let kept: Vec<Sensor> = sensors
            .iter()
            .filter(|sensor| {
                matches!(sensor.kind(), Some(Kind::Temperature | Kind::Fan | Kind::Power))
            })
            .cloned()
            .collect();
        let measured = ipmi::measure(link, &kept)?;
        let names = series_names(&measured);
        Ok(measured
            .iter()
            .zip(names)
            .filter_map(|(sensor, name)| Some((name?, sensor.value? as f32)))
            .collect())
    }

    /// The entries of the log after the one that was last looked at. The
    /// first look takes what is there as seen.
    fn new_events(&mut self, link: &mut dyn Transport) -> ipmi::Result<Vec<Event>> {
        let info = ipmi::log_info(link)?;
        let moved = (info.entries, info.added, info.erased);
        if self.log == Some(moved) && self.remembered != Remembered::Never {
            return Ok(Vec::new());
        }
        let entries = ipmi::read_log(link)?;
        self.log = Some(moved);
        let after = match self.remembered {
            Remembered::Never => entries.len(),
            Remembered::Empty => 0,
            // An entry that is gone means the log was erased since, and
            // every entry in it is new.
            Remembered::Entry(last) => entries
                .iter()
                .position(|entry| Last::of(entry) == last)
                .map_or(0, |position| position + 1),
        };
        self.remembered =
            entries.last().map_or(Remembered::Empty, |entry| Remembered::Entry(Last::of(entry)));
        let sensors = self.sensors.as_deref().unwrap_or_default();
        Ok(entries[after..].iter().map(|entry| event_of(entry, sensors)).collect())
    }
}

/// `entry` as an event, with the sensor of the repository it names.
fn event_of(entry: &Entry, sensors: &[Sensor]) -> Event {
    let when = match entry.when {
        Some(When::Clock(seconds)) => util::local_time(i64::from(seconds), "%Y-%m-%d %H:%M:%S"),
        Some(When::SinceStart(seconds)) => {
            format!("start +{}", util::clock_time(u64::from(seconds)))
        }
        None => "no time".into(),
    };
    let Some(event) = &entry.event else {
        let text = format!("OEM record, type 0x{:02X}", entry.kind);
        return Event { id: entry.id, severity: Severity::Info, sensor: None, text, when };
    };
    let (words, severity) = event.describe();
    let direction = if event.asserted { "asserted" } else { "deasserted" };
    let sensor = sensors
        .iter()
        .find(|sensor| {
            sensor.number == event.sensor_number && sensor.sensor_type == event.sensor_type
        })
        .map(|sensor| sensor.name.clone());
    Event { id: entry.id, severity, sensor, text: format!("{words}, {direction}"), when }
}

/// metricsd's side of the BMC: opens it at each step, writes the events
/// to the log, tells alertd of the critical ones and keeps the place the
/// log was read to.
pub(crate) struct Bmc {
    open: Open,
    tell: Tell,
    /// Where the last entry looked at is kept.
    file: PathBuf,
    /// Made when the BMC first opened, from what the file says.
    watch: Option<Watch>,
    /// What the file holds, so that it is written when that changes.
    saved: Remembered,
    said_open: bool,
    said_look: bool,
    said_tell: bool,
    said_save: bool,
}

impl Bmc {
    /// The BMC as this machine has it, with the file kept in `state`.
    pub(crate) fn system(state: &Path, tell: Tell) -> Bmc {
        let open: Open = Box::new(|| {
            let device = ipmi::Device::open()?;
            Ok(Box::new(device) as Box<dyn Transport>)
        });
        Bmc::new(open, tell, state)
    }

    pub(crate) fn new(open: Open, tell: Tell, state: &Path) -> Bmc {
        Bmc {
            open,
            tell,
            file: state.join(REMEMBERED),
            watch: None,
            saved: Remembered::Never,
            said_open: false,
            said_look: false,
            said_tell: false,
            said_save: false,
        }
    }

    /// One step: the series the BMC gives now, with its new events
    /// written to the log and the critical ones told to alertd. A machine
    /// with no BMC gives nothing.
    pub(crate) fn step(&mut self) -> Vec<(String, f32)> {
        let mut link = match (self.open)() {
            Ok(link) => link,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
            Err(error) => {
                if !std::mem::replace(&mut self.said_open, true) {
                    eprintln!(
                        "<4>metricsd: cannot open the BMC at /devices/ipmi0: {}; trying again at \
                         each step",
                        util::strip_os_error(&error)
                    );
                }
                return Vec::new();
            }
        };
        if std::mem::replace(&mut self.said_open, false) {
            println!("metricsd: the BMC opens again");
        }
        let file = &self.file;
        let watch = self.watch.get_or_insert_with(|| Watch::new(Remembered::read(file)));
        let look = match watch.look(&mut *link) {
            Ok(look) => look,
            Err(error) => {
                if !std::mem::replace(&mut self.said_look, true) {
                    eprintln!(
                        "<4>metricsd: cannot read the BMC: {error}; trying again at each step"
                    );
                }
                return Vec::new();
            }
        };
        if std::mem::replace(&mut self.said_look, false) {
            println!("metricsd: the BMC answers again");
        }
        for event in &look.events {
            self.report(event);
        }
        self.save();
        look.series
    }

    /// Writes `event` to the log, at the priority its severity has, and
    /// tells alertd if it is critical.
    fn report(&mut self, event: &Event) {
        let what = match &event.sensor {
            Some(sensor) => format!("{sensor}: {}", event.text),
            None => event.text.clone(),
        };
        let line = format!("metricsd: BMC event log, record {}, {}: {what}", event.id, event.when);
        match event.severity {
            Severity::Info => println!("{line}"),
            Severity::Warning => eprintln!("<4>{line}"),
            Severity::Critical => eprintln!("<3>{line}"),
            Severity::NonRecoverable => eprintln!("<2>{line}"),
        }
        if event.severity < Severity::Critical {
            return;
        }
        let detail = format!(
            "The BMC's event log has it as record {}. `bmc events` shows the log and `bmc \
             sensors` what each sensor reads now.",
            event.id
        );
        let alert = Alert::new(
            format!("bmc-event:{}", event.id),
            AlertSeverity::Critical,
            &format!("BMC: {what}"),
            &detail,
        )
        .about(About::Hardware);
        match (self.tell)(&alert) {
            Ok(()) => {
                if std::mem::replace(&mut self.said_tell, false) {
                    println!("metricsd: alertd takes the BMC's events again");
                }
            }
            Err(why) => {
                if !std::mem::replace(&mut self.said_tell, true) {
                    eprintln!(
                        "<3>metricsd: cannot tell alertd of the BMC's event {}: {why}",
                        event.id
                    );
                }
            }
        }
    }

    /// Writes the place the log was read to, when it moved.
    fn save(&mut self) {
        let Some(watch) = &self.watch else { return };
        let now = watch.remembered();
        if now == self.saved {
            return;
        }
        let Some(text) = now.text() else { return };
        let written = self
            .file
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&self.file, text));
        match written {
            Ok(()) => {
                self.saved = now;
                if std::mem::replace(&mut self.said_save, false) {
                    println!("metricsd: {} is written again", self.file.display());
                }
            }
            Err(error) => {
                if !std::mem::replace(&mut self.said_save, true) {
                    eprintln!(
                        "<4>metricsd: cannot write {}: {}; a restart would take the log as seen",
                        self.file.display(),
                        util::strip_os_error(&error)
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::ipmi::sim::{self, Sim};

    /// A BMC the test changes between steps and the daemon asks.
    #[derive(Clone)]
    struct Shared(Rc<RefCell<Sim>>);

    impl Transport for Shared {
        fn request(&mut self, netfn: u8, cmd: u8, data: &[u8]) -> io::Result<Vec<u8>> {
            self.0.borrow_mut().request(netfn, cmd, data)
        }
    }

    fn record(id: u16, number: u8, name: &str, sensor_type: u8, unit: u8) -> Vec<u8> {
        let mut bytes = sim::full_record(id, number, name);
        bytes[12] = sensor_type;
        bytes[21] = unit;
        bytes
    }

    /// A BMC with an inlet temperature, a fan, a voltage, a power sensor
    /// and a power supply's states, and a log of two entries.
    fn bmc() -> Shared {
        let mut bmc = Sim::new();
        bmc.sdr = vec![
            record(1, 4, "Inlet Temp", 0x01, 1),
            record(2, 5, "Fan1", 0x04, 18),
            record(3, 6, "12V", 0x02, 4),
            record(4, 7, "Pwr Consumption", 0x0B, 6),
            sim::compact_record(5, 9, "PS Status", 1),
        ];
        // Raw 100 is 30 in the unit of these records.
        for number in [4, 5, 6, 7] {
            bmc.readings.insert(number, vec![100, 0x40, 0]);
        }
        bmc.sel = vec![
            sim::sel_entry(1, 100, 0x08, 9, 0x6F, [0x00, 0, 0]),
            sim::sel_entry(2, 1_794_000_000, 0x01, 4, 0x01, [0x59, 0x5A, 0x55]),
        ];
        Shared(Rc::new(RefCell::new(bmc)))
    }

    /// A threshold event of the inlet sensor, at offset `offset` of event
    /// data 1: 0x07 is upper non-critical going high, 0x09 upper critical
    /// going high and 0x0B upper non-recoverable going high.
    fn heat(id: u16, offset: u8) -> [u8; 16] {
        sim::sel_entry(id, 1_794_000_100, 0x01, 4, 0x01, [0x50 | offset, 0x5A, 0x55])
    }

    fn open(bmc: &Shared) -> Open {
        let bmc = bmc.clone();
        Box::new(move || Ok(Box::new(bmc.clone()) as Box<dyn Transport>))
    }

    /// The alerts that were told.
    fn alerts() -> (Tell, Rc<RefCell<Vec<Alert>>>) {
        let told = Rc::new(RefCell::new(Vec::new()));
        let sink = told.clone();
        let tell: Tell = Box::new(move |alert| {
            sink.borrow_mut().push(alert.clone());
            Ok(())
        });
        (tell, told)
    }

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("metricsd-bmc-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_temperatures_fans_and_power_are_series_named_by_their_sensors() {
        let bmc = bmc();
        let mut watch = Watch::new(Remembered::Never);
        let look = watch.look(&mut bmc.clone()).unwrap();
        assert_eq!(
            look.series,
            [
                ("bmc.temp{Inlet Temp}".to_string(), 30.0),
                ("bmc.fan{Fan1}".to_string(), 30.0),
                ("bmc.power{Pwr Consumption}".to_string(), 30.0),
            ]
        );
    }

    #[test]
    fn names_are_made_safe_and_twins_are_told_apart_by_number() {
        let measured = |number, name: &str, kind| Measured {
            number,
            name: name.to_string(),
            kind,
            unit: String::new(),
            value: Some(1.0),
            status: ipmi::Status::Ok,
        };
        let names = series_names(&[
            measured(1, "Temp {1}", Kind::Temperature),
            measured(2, "Temp", Kind::Temperature),
            measured(3, "Temp", Kind::Temperature),
            measured(4, "Temp", Kind::Fan),
            measured(5, "12V", Kind::Voltage),
            measured(6, "", Kind::Power),
            measured(7, "Line\nbreak", Kind::Power),
        ]);
        assert_eq!(
            names,
            [
                Some("bmc.temp{Temp _1_}".to_string()),
                Some("bmc.temp{Temp #2}".to_string()),
                Some("bmc.temp{Temp #3}".to_string()),
                Some("bmc.fan{Temp}".to_string()),
                None,
                Some("bmc.power{sensor 6}".to_string()),
                Some("bmc.power{Line_break}".to_string()),
            ]
        );
    }

    #[test]
    fn the_repository_is_read_once_and_again_when_it_changes() {
        let bmc = bmc();
        let mut watch = Watch::new(Remembered::Never);
        watch.look(&mut bmc.clone()).unwrap();
        let reads = bmc.0.borrow().reservations;
        assert_eq!(reads, 1);
        watch.look(&mut bmc.clone()).unwrap();
        watch.look(&mut bmc.clone()).unwrap();
        assert_eq!(bmc.0.borrow().reservations, 1);
        // A sensor added to the repository moves its change time.
        {
            let mut sim = bmc.0.borrow_mut();
            sim.sdr.push(record(6, 8, "Exhaust Temp", 0x01, 1));
            sim.readings.insert(8, vec![120, 0x40, 0]);
            sim.sdr_added = 77;
        }
        let look = watch.look(&mut bmc.clone()).unwrap();
        assert_eq!(bmc.0.borrow().reservations, 2);
        assert!(look.series.contains(&("bmc.temp{Exhaust Temp}".to_string(), 40.0)));
    }

    #[test]
    fn the_first_look_takes_the_log_as_seen_and_a_later_one_tells_what_came() {
        let bmc = bmc();
        let mut watch = Watch::new(Remembered::Never);
        let first = watch.look(&mut bmc.clone()).unwrap();
        assert!(first.events.is_empty());
        assert_eq!(watch.remembered(), Remembered::Entry(Last { id: 2, stamp: 1_794_000_000 }));
        // Nothing new, and the log is not read again.
        let before = bmc.0.borrow().sel.len();
        assert!(watch.look(&mut bmc.clone()).unwrap().events.is_empty());
        assert_eq!(before, 2);
        {
            let mut sim = bmc.0.borrow_mut();
            sim.sel.push(heat(3, 0x09));
            sim.sel.push(sim::sel_entry(4, 1_794_000_200, 0x23, 0, 0xEF, [0xC1, 0x04, 0]));
            sim.sel_added = 5;
        }
        let events = watch.look(&mut bmc.clone()).unwrap().events;
        let ids: Vec<u16> = events.iter().map(|event| event.id).collect();
        assert_eq!(ids, [3, 4]);
        assert_eq!(events[0].severity, Severity::Critical);
        assert_eq!(events[0].sensor.as_deref(), Some("Inlet Temp"));
        assert_eq!(events[0].text, "upper critical going high, asserted");
        assert_eq!(events[1].severity, Severity::Info);
        // They are told once.
        assert!(watch.look(&mut bmc.clone()).unwrap().events.is_empty());
    }

    #[test]
    fn a_restart_goes_on_from_the_entry_it_remembered_and_an_erased_log_is_all_new() {
        let bmc = bmc();
        {
            let mut sim = bmc.0.borrow_mut();
            sim.sel.push(heat(3, 0x07));
        }
        let mut watch = Watch::new(Remembered::Entry(Last { id: 2, stamp: 1_794_000_000 }));
        let events = watch.look(&mut bmc.clone()).unwrap().events;
        assert_eq!(events.iter().map(|event| event.id).collect::<Vec<_>>(), [3]);
        assert_eq!(events[0].severity, Severity::Warning);
        // The remembered entry is no longer in the log: it was erased, and
        // what is there now is new.
        let mut watch = Watch::new(Remembered::Entry(Last { id: 9, stamp: 5 }));
        let events = watch.look(&mut bmc.clone()).unwrap().events;
        assert_eq!(events.len(), 3);
        // A log that was empty has only new entries.
        let mut watch = Watch::new(Remembered::Empty);
        assert_eq!(watch.look(&mut bmc.clone()).unwrap().events.len(), 3);
    }

    #[test]
    fn a_critical_event_is_logged_and_told_to_alertd_as_hardware() {
        let bmc = bmc();
        let dir = folder("alert");
        let (tell, told) = alerts();
        let mut daemon = Bmc::new(open(&bmc), tell, &dir);
        let series = daemon.step();
        assert_eq!(series.len(), 3);
        assert!(told.borrow().is_empty());
        {
            let mut sim = bmc.0.borrow_mut();
            sim.sel.push(heat(3, 0x07));
            sim.sel.push(heat(4, 0x09));
            sim.sel.push(heat(5, 0x0B));
            sim.sel_added = 9;
        }
        daemon.step();
        let told = told.borrow();
        // The warning is only logged.
        assert_eq!(told.len(), 2, "{told:?}");
        assert_eq!(told[0].key, "bmc-event:4");
        assert_eq!(told[0].title, "BMC: Inlet Temp: upper critical going high, asserted");
        assert_eq!(told[0].kind(), Some(About::Hardware));
        assert_eq!(told[0].severity, AlertSeverity::Critical);
        assert!(told[0].detail.contains("record 4"), "{}", told[0].detail);
        assert_eq!(told[1].key, "bmc-event:5");
        assert!(told[1].title.contains("upper non-recoverable going high"), "{}", told[1].title);
    }

    #[test]
    fn what_was_looked_at_is_kept_so_that_a_restart_does_not_replay_the_log() {
        let bmc = bmc();
        let dir = folder("restart");
        let (tell, told) = alerts();
        let mut first = Bmc::new(open(&bmc), tell, &dir);
        first.step();
        assert_eq!(fs::read_to_string(dir.join("bmc-events")).unwrap(), "2 1794000000\n");
        // metricsd restarts; the log has gained a critical entry while it
        // was away, and only that is new.
        {
            let mut sim = bmc.0.borrow_mut();
            sim.sel.push(heat(3, 0x09));
            sim.sel_added = 3;
        }
        let (tell, told_again) = alerts();
        let mut second = Bmc::new(open(&bmc), tell, &dir);
        second.step();
        assert!(told.borrow().is_empty());
        assert_eq!(told_again.borrow().len(), 1);
        assert_eq!(told_again.borrow()[0].key, "bmc-event:3");
        assert_eq!(fs::read_to_string(dir.join("bmc-events")).unwrap(), "3 1794000100\n");
        // A third start with the log as it is tells nothing.
        let (tell, told_third) = alerts();
        Bmc::new(open(&bmc), tell, &dir).step();
        assert!(told_third.borrow().is_empty());
    }

    #[test]
    fn a_machine_with_no_bmc_has_no_series_and_no_file() {
        let dir = folder("none");
        let (tell, told) = alerts();
        let open: Open = Box::new(|| Err(io::Error::from(io::ErrorKind::NotFound)));
        let mut daemon = Bmc::new(open, tell, &dir);
        assert!(daemon.step().is_empty());
        assert!(daemon.step().is_empty());
        assert!(!dir.join("bmc-events").exists());
        assert!(told.borrow().is_empty());
    }

    #[test]
    fn an_interface_that_cannot_be_opened_is_said_once_and_tried_again() {
        let bmc = bmc();
        let dir = folder("denied");
        let (tell, _) = alerts();
        let allowed = Rc::new(RefCell::new(false));
        let gate = allowed.clone();
        let inner = open(&bmc);
        let mut inner = inner;
        let open: Open = Box::new(move || {
            if *gate.borrow() {
                inner()
            } else {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }
        });
        let mut daemon = Bmc::new(open, tell, &dir);
        assert!(daemon.step().is_empty());
        assert!(daemon.said_open);
        *allowed.borrow_mut() = true;
        assert_eq!(daemon.step().len(), 3);
        assert!(!daemon.said_open);
    }

    #[test]
    fn a_bmc_that_stops_answering_gives_nothing_and_goes_on_when_it_answers() {
        let bmc = bmc();
        let dir = folder("silent");
        let (tell, _) = alerts();
        let calls = Rc::new(RefCell::new(0));
        let counted = calls.clone();
        let shared = bmc.clone();
        let open: Open = Box::new(move || {
            *counted.borrow_mut() += 1;
            let link = shared.clone();
            if *counted.borrow() == 2 {
                return Ok(Box::new(Refuses) as Box<dyn Transport>);
            }
            Ok(Box::new(link) as Box<dyn Transport>)
        });
        let mut daemon = Bmc::new(open, tell, &dir);
        assert_eq!(daemon.step().len(), 3);
        assert!(daemon.step().is_empty());
        assert!(daemon.said_look);
        assert_eq!(daemon.step().len(), 3);
        assert!(!daemon.said_look);
    }

    /// A link whose BMC never answers.
    struct Refuses;

    impl Transport for Refuses {
        fn request(&mut self, _: u8, _: u8, _: &[u8]) -> io::Result<Vec<u8>> {
            Err(io::Error::from(io::ErrorKind::TimedOut))
        }
    }

    #[test]
    fn the_remembered_place_is_read_from_its_file_and_a_file_that_does_not_read_is_never() {
        let dir = folder("file");
        let path = dir.join("bmc-events");
        assert_eq!(Remembered::read(&path), Remembered::Never);
        fs::write(&path, "empty\n").unwrap();
        assert_eq!(Remembered::read(&path), Remembered::Empty);
        fs::write(&path, "7 1794000000\n").unwrap();
        assert_eq!(
            Remembered::read(&path),
            Remembered::Entry(Last { id: 7, stamp: 1_794_000_000 })
        );
        for wrong in ["", "7", "x y", "70000 1", "1 -1"] {
            fs::write(&path, wrong).unwrap();
            assert_eq!(Remembered::read(&path), Remembered::Never, "{wrong:?}");
        }
    }
}
