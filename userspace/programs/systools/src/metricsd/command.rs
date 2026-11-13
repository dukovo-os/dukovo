// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! metrics(1): what metricsd(8) knows of the machine (ADR 0484).
//!
//! `metrics` shows the latest values, `metrics list` the series the
//! history holds, and `metrics show` the points of series over a time
//! range, as text or, with `--json`, for a script. The latest values are
//! the file metricsd writes at each step; the history is read from its
//! files, as the console reads it, so that neither needs metricsd to
//! answer.
//!
//! A series is named as it is in the history, a metric and, for what
//! there are several of, an instance in braces: `cpu`, `fs.used{/data}`.
//! A name without an instance means all of them. Values are shown with
//! their units, bytes in powers of 1000 as df(1)'s `-H` does; a script
//! reads `--json`, where they are plain numbers.

use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value, json};

use super::store::{self, Selector, Series};
use super::{NOW, STATE, size_text};
use crate::args::{Arg, Args};
use crate::log;
use crate::util::{self, Fail, fail, usage};

/// How far back `show` goes unless `--since` says another.
const SINCE_DEFAULT: &str = "1h";

/// The values are old enough to be said to be, when the file metricsd
/// writes at each step has not changed for this long.
const STALE: Duration = Duration::from_secs(180);

/// What a series is measured in, which says how its values are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Percent,
    Load,
    Bytes,
    BytesPerSecond,
    Celsius,
    Count,
}

/// The unit of the series `name`, as its metric says.
fn unit(name: &str) -> Unit {
    match name.split('{').next().unwrap_or(name) {
        "cpu" | "cpu.user" | "cpu.system" | "disk.busy" | "service.cpu" => Unit::Percent,
        "load" => Unit::Load,
        "memory.used" | "memory.available" | "swap.used" | "fs.used" | "fs.size"
        | "service.memory" => Unit::Bytes,
        "disk.read" | "disk.write" | "net.rx" | "net.tx" => Unit::BytesPerSecond,
        "temp" => Unit::Celsius,
        _ => Unit::Count,
    }
}

/// `value` of series `name` with its unit.
fn value_text(name: &str, value: f32) -> String {
    let bytes = || size_text(value.max(0.0) as u64);
    match unit(name) {
        Unit::Percent => format!("{value:.1}%"),
        Unit::Load => format!("{value:.2}"),
        Unit::Bytes => bytes(),
        Unit::BytesPerSecond => format!("{}/s", bytes()),
        Unit::Celsius => format!("{value:.1} °C"),
        Unit::Count if value.fract() == 0.0 => format!("{value}"),
        Unit::Count => format!("{value:.1}"),
    }
}

/// `value` as the number a script reads: the shortest text that reads
/// back as the `f32` the history holds, not its longer `f64` spelling.
fn plain(value: f32) -> f64 {
    value.to_string().parse().unwrap_or_else(|_| f64::from(value))
}

/// metrics(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let help = "Usage: metrics [now] [SERIES...] [--json]\n       metrics list [SERIES...] \
                [--json]\n       metrics show SERIES... [--since TIME] [--until TIME] [--every \
                TIME] [--json] [--utc]";
    let mut command: Option<String> = None;
    let mut options = Options::default();
    let mut operands = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(l) if l == "help" => {
                println!("{help}");
                return Ok(0);
            }
            Arg::Long(l) if l == "json" => options.json = true,
            Arg::Long(l) if l == "utc" => options.utc = true,
            Arg::Long(l) if l == "since" => options.since = Some(args.text()?),
            Arg::Long(l) if l == "until" => options.until = Some(args.text()?),
            Arg::Long(l) if l == "every" => options.every = Some(args.text()?),
            Arg::Operand(operand) => {
                let text = operand.into_string().map_err(|_| usage("a series is text".into()))?;
                // The first word is a command, unless it is none: any
                // other names a series for `now`.
                if command.is_none() && operands.is_empty() && COMMANDS.contains(&text.as_str()) {
                    command = Some(text);
                } else {
                    operands.push(Selector::parse(&text));
                }
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let command = command.unwrap_or_else(|| "now".into());
    if command != "show"
        && let Some(option) = options.only_for_show()
    {
        return Err(usage(format!("--{option} is for `metrics show`")));
    }
    match command.as_str() {
        "list" => list(Path::new(STATE), &operands, options.json),
        "show" => show(Path::new(STATE), &operands, &options, util::now_ns()),
        _ => now(Path::new(NOW), &operands, options.json),
    }
}

/// The commands, the first of which `metrics` does without one.
const COMMANDS: [&str; 3] = ["now", "list", "show"];

/// What the options ask for.
#[derive(Default)]
struct Options {
    json: bool,
    utc: bool,
    since: Option<String>,
    until: Option<String>,
    every: Option<String>,
}

impl Options {
    /// The name of an option that only `show` takes, if one was given.
    fn only_for_show(&self) -> Option<&'static str> {
        [
            ("utc", self.utc),
            ("since", self.since.is_some()),
            ("until", self.until.is_some()),
            ("every", self.every.is_some()),
        ]
        .into_iter()
        .find_map(|(name, given)| given.then_some(name))
    }
}

/// Why a file of metricsd's cannot be read, with what to do about it.
fn cannot_read(path: &Path, error: &io::Error) -> Fail {
    let why = match error.kind() {
        io::ErrorKind::PermissionDenied => {
            "only an administrator reads it: run it as `elevate metrics`".to_string()
        }
        io::ErrorKind::NotFound => {
            "is metricsd running? `service status metricsd` says".to_string()
        }
        _ => util::strip_os_error(error),
    };
    fail(format!("cannot read {}: {why}", path.display()))
}

/// Whether `selected` keeps the series `name`: it does when no series is
/// asked for, or one that matches is.
fn selected(select: &[Selector], name: &str) -> bool {
    select.is_empty() || select.iter().any(|selector| selector.matches(name))
}

/// `metrics now`: the latest values, from the file at `path`.
fn now(path: &Path, select: &[Selector], json: bool) -> Result<i32, Fail> {
    let text = fs::read_to_string(path).map_err(|error| cannot_read(path, &error))?;
    if let Some(note) = age_note(path) {
        eprintln!("metrics: {note}");
    }
    let values = parse_now(&text, select);
    if values.is_empty() && !select.is_empty() {
        return Err(fail("no such series in the latest values: `metrics list` shows them".into()));
    }
    print!("{}", render_now(&values, json));
    Ok(0)
}

/// The `name value` lines of the file metricsd writes for `metrics now`
/// that `select` keeps. A series name may hold spaces, so the value is
/// what follows the last one.
fn parse_now(text: &str, select: &[Selector]) -> Vec<(String, f32)> {
    text.lines()
        .filter_map(|line| {
            let (name, value) = line.rsplit_once(' ')?;
            selected(select, name).then(|| Some((name.to_string(), value.parse().ok()?)))?
        })
        .collect()
}

/// A note that the values are old, if the file at `path` has not been
/// written for [`STALE`].
fn age_note(path: &Path) -> Option<String> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).ok()?;
    (age >= STALE).then(|| {
        format!("these values are {} old: is metricsd running?", util::elapsed(age.as_secs()))
    })
}

/// The latest values as columns of names and values, or as one object.
fn render_now(values: &[(String, f32)], json: bool) -> String {
    if json {
        let object: Map<String, Value> =
            values.iter().map(|(name, value)| (name.clone(), json!(plain(*value)))).collect();
        return format!("{}\n", Value::Object(object));
    }
    let width = values.iter().map(|(name, _)| util::width(name)).max().unwrap_or(0).min(48);
    let mut out = String::new();
    for (name, value) in values {
        let pad = width.saturating_sub(util::width(name));
        out.push_str(&format!("{name}{}  {}\n", " ".repeat(pad), value_text(name, *value)));
    }
    out
}

/// `metrics list`: the series the history in `dir` holds that `select`
/// keeps.
fn list(dir: &Path, select: &[Selector], json: bool) -> Result<i32, Fail> {
    let names = store::series(dir).map_err(|error| cannot_read(dir, &error))?;
    let names: Vec<String> = names.into_iter().filter(|name| selected(select, name)).collect();
    if names.is_empty() {
        let why = if select.is_empty() { "no history yet" } else { "no such series" };
        return Err(fail(format!("{why}: is metricsd running? `service status metricsd` says")));
    }
    print!("{}", render_list(&names, json));
    Ok(0)
}

fn render_list(names: &[String], json: bool) -> String {
    if json {
        return format!("{}\n", json!(names));
    }
    names.iter().fold(String::new(), |out, name| out + name + "\n")
}

/// A time that `--since` or `--until` gives, in seconds since 1970: a
/// time as log(1) reads them, and a length of time alone, `10m`, is that
/// long ago.
fn time_option(text: &str, now_ns: u64, utc: bool) -> Result<i64, Fail> {
    let length = text.starts_with(|c: char| c.is_ascii_digit())
        && text.chars().all(|c| c.is_ascii_alphanumeric());
    let ns = if length {
        log::parse_time(&format!("-{text}"), now_ns, utc)
            .or_else(|_| log::parse_time(text, now_ns, utc))
    } else {
        log::parse_time(text, now_ns, utc)
    }?;
    Ok(i64::try_from(ns / 1_000_000_000).unwrap_or(i64::MAX))
}

/// The seconds `--every` gives: `30`, `30s`, `5m`, `1h`, `1d`.
fn every_option(text: &str) -> Result<u32, Fail> {
    let bad = || usage(format!("'{text}' is not a length of time: 30s, 5m, 1h or 1d"));
    let digits = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
    let number: u32 = text[..digits].parse().map_err(|_| bad())?;
    let unit = match &text[digits..] {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => return Err(bad()),
    };
    match number.checked_mul(unit) {
        Some(seconds) if seconds > 0 => Ok(seconds),
        _ => Err(bad()),
    }
}

/// `metrics show`: the points of the series `select` names, over the range
/// the options give, as it is at time `now_ns`.
fn show(dir: &Path, select: &[Selector], options: &Options, now_ns: u64) -> Result<i32, Fail> {
    if select.is_empty() {
        return Err(usage("which series? `metrics list` shows what there is".into()));
    }
    let from = time_option(options.since.as_deref().unwrap_or(SINCE_DEFAULT), now_ns, options.utc)?;
    let until = match &options.until {
        Some(text) => time_option(text, now_ns, options.utc)?,
        None => i64::try_from(now_ns / 1_000_000_000).unwrap_or(i64::MAX),
    };
    if from >= until {
        return Err(usage("--since has to be before --until".into()));
    }
    let every = options.every.as_deref().map(every_option).transpose()?;
    let series =
        store::read(dir, from, until, select, every).map_err(|error| cannot_read(dir, &error))?;
    if series.is_empty() {
        return Err(fail(format!(
            "no points of {} between {} and {}: `metrics list` shows the series, and the history \
             starts when metricsd first ran",
            names_of(select),
            util::time_text(from, "%Y-%m-%d %H:%M:%S", options.utc),
            util::time_text(until, "%Y-%m-%d %H:%M:%S", options.utc),
        )));
    }
    print!("{}", render_show(&series, options.json, options.utc));
    Ok(0)
}

/// The series asked for, as they were written.
fn names_of(select: &[Selector]) -> String {
    let named = |s: &Selector| match &s.instance {
        Some(instance) => format!("{}{{{instance}}}", s.metric),
        None => s.metric.clone(),
    };
    select.iter().map(named).collect::<Vec<_>>().join(", ")
}

/// The points of the series, each under its name, or as JSON.
fn render_show(series: &[Series], json: bool, utc: bool) -> String {
    if json {
        let all: Vec<Value> = series
            .iter()
            .map(|s| {
                let points: Vec<Value> =
                    s.points.iter().map(|&(at, value)| json!([at, plain(value)])).collect();
                json!({ "series": s.name, "points": points })
            })
            .collect();
        return format!("{}\n", Value::Array(all));
    }
    let mut out = String::new();
    for (index, s) in series.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&format!("{}\n", s.name));
        for &(at, value) in &s.points {
            let time = util::time_text(at, "%Y-%m-%d %H:%M:%S", utc);
            out.push_str(&format!("{time}  {}\n", value_text(&s.name, value)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;

    /// 2026-11-13 10:00:00 UTC.
    const TEN: i64 = 1_794_564_000;
    const NOW: u64 = (TEN as u64 + 1800) * 1_000_000_000;

    fn selectors(names: &[&str]) -> Vec<Selector> {
        names.iter().map(|name| Selector::parse(name)).collect()
    }

    fn folder(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("metricsd-command-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn values_have_the_units_of_their_series() {
        assert_eq!(value_text("cpu", 12.34), "12.3%");
        assert_eq!(value_text("service.cpu{metricsd}", 0.04), "0.0%");
        assert_eq!(value_text("load", 0.5), "0.50");
        assert_eq!(value_text("memory.used", 614_400.0), "615 KB");
        assert_eq!(value_text("fs.used{/data}", 1.2e12), "1.2 TB");
        assert_eq!(value_text("net.rx{net0}", 6000.0), "6.0 KB/s");
        assert_eq!(value_text("disk.busy{vda}", 50.0), "50.0%");
        assert_eq!(value_text("temp{coretemp-isa-0000/Package id 0}", 45.875), "45.9 °C");
        assert_eq!(value_text("btrfs.missing{data}", 1.0), "1");
        assert_eq!(value_text("net.errors{net0}", 0.0), "0");
        assert_eq!(value_text("something.new", 2.5), "2.5");
        assert_eq!(unit("fs.used"), Unit::Bytes);
        // A number a script reads is the shortest that is the f32.
        assert_eq!(plain(36.9), 36.9);
        assert_eq!(plain(0.1), 0.1);
    }

    #[test]
    fn the_latest_values_are_read_by_their_last_space_and_filtered() {
        let text = "cpu 25\nload 0.5\ntemp{cros_ec-isa-0000/Battery sensor} 36.9\nfs.used{/} 6e8\nbroken\nx nan-ish\n";
        let all = parse_now(text, &[]);
        assert_eq!(all.len(), 4);
        assert_eq!(all[2], ("temp{cros_ec-isa-0000/Battery sensor}".to_string(), 36.9));
        assert_eq!(parse_now(text, &selectors(&["temp"])).len(), 1);
        assert_eq!(parse_now(text, &selectors(&["fs.used{/}", "cpu"])).len(), 2);
        assert!(parse_now(text, &selectors(&["fs.used{/data}"])).is_empty());
    }

    #[test]
    fn the_latest_values_are_columns_or_an_object() {
        let values = vec![
            ("cpu".to_string(), 25.0),
            ("memory.used".to_string(), 614_400.0),
            ("temp{a b}".to_string(), 36.9),
        ];
        assert_eq!(
            render_now(&values, false),
            "cpu          25.0%\nmemory.used  615 KB\ntemp{a b}    36.9 °C\n"
        );
        assert_eq!(
            render_now(&values, true),
            "{\"cpu\":25.0,\"memory.used\":614400.0,\"temp{a b}\":36.9}\n"
        );
    }

    #[test]
    fn now_says_where_the_values_should_be_when_they_are_not() {
        let dir = folder("now");
        let missing = now(&dir.join("now"), &[], false).unwrap_err();
        assert!(
            missing.message.contains("cannot read")
                && missing.message.contains("is metricsd running?")
        );
        fs::write(dir.join("now"), "cpu 25\n").unwrap();
        assert_eq!(now(&dir.join("now"), &[], false).unwrap(), 0);
        let none = now(&dir.join("now"), &selectors(&["swap.used"]), false).unwrap_err();
        assert!(none.message.contains("no such series"));
    }

    #[test]
    fn a_length_of_time_alone_is_that_long_ago() {
        let at = |text: &str| time_option(text, NOW, true).unwrap();
        let now = TEN + 1800;
        assert_eq!(at("1m"), now - 60);
        assert_eq!(at("2h"), now - 7200);
        assert_eq!(at("1d"), now - 86_400);
        assert_eq!(at("30"), now - 30);
        assert_eq!(at("-1h"), now - 3600);
        assert_eq!(at("now"), now);
        assert_eq!(at("2026-11-13 10:00"), TEN);
        assert_eq!(at("2026-11-13T10:00:00Z"), TEN);
        assert_eq!(at("2026-11-13"), TEN - 10 * 3600);
        for bad in ["soon", "3fortnights", "2026-13-01"] {
            assert!(time_option(bad, NOW, true).is_err(), "{bad}");
        }
    }

    #[test]
    fn every_is_a_length_of_time() {
        assert_eq!(every_option("30").unwrap(), 30);
        assert_eq!(every_option("30s").unwrap(), 30);
        assert_eq!(every_option("5m").unwrap(), 300);
        assert_eq!(every_option("1h").unwrap(), 3600);
        assert_eq!(every_option("1d").unwrap(), 86_400);
        for bad in ["", "0", "0m", "m", "5x", "-5", "99999999999", "4294967295h"] {
            assert!(every_option(bad).is_err(), "{bad}");
        }
    }

    /// A history of `fs.used{/}` and `cpu`, a row a minute for ten
    /// minutes from the start of the ten-minute period before the last,
    /// and when it starts. The store reads by the clock of the machine, so
    /// the rows are in its recent past.
    fn history(name: &str) -> (PathBuf, i64) {
        let dir = folder(name);
        let now = i64::try_from(util::now_ns() / 1_000_000_000).unwrap();
        let start = now.div_euclid(600) * 600 - 600;
        let mut store = store::Store::open(&dir, start).unwrap();
        for minute in 0..10 {
            let used = 600_000_000.0 + 1_000_000.0 * minute as f32;
            let cpu = if minute == 0 { f32::NAN } else { 10.0 + minute as f32 };
            store.push(
                start + minute * 60,
                &[("fs.used{/}".to_string(), used), ("cpu".to_string(), cpu)],
            );
        }
        store.flush().unwrap();
        (dir, start)
    }

    fn options(since: &str, until: Option<&str>, every: Option<&str>, json: bool) -> Options {
        Options {
            json,
            utc: true,
            since: Some(since.into()),
            until: until.map(String::from),
            every: every.map(String::from),
        }
    }

    fn time(at: i64) -> String {
        util::time_text(at, "%Y-%m-%d %H:%M:%S", true)
    }

    #[test]
    fn a_series_is_shown_under_its_name_with_its_times() {
        let (dir, start) = history("show");
        let series = store::read(&dir, start, start + 180, &selectors(&["cpu"]), None).unwrap();
        assert_eq!(
            render_show(&series, false, true),
            format!("cpu\n{}  11.0%\n{}  12.0%\n", time(start + 60), time(start + 120))
        );
        // The missing value of the first minute is no point.
        let both =
            store::read(&dir, start, start + 120, &selectors(&["fs.used", "cpu"]), None).unwrap();
        let text = render_show(&both, false, true);
        assert!(text.starts_with("cpu\n") && text.contains("\n\nfs.used{/}\n"), "{text}");
        assert!(text.contains(&format!("{}  600 MB\n", time(start))), "{text}");
        assert_eq!(
            render_show(&series, true, true),
            format!(
                "[{{\"points\":[[{},11.0],[{},12.0]],\"series\":\"cpu\"}}]\n",
                start + 60,
                start + 120
            )
        );
    }

    #[test]
    fn show_averages_to_every() {
        let (dir, start) = history("every");
        let series =
            store::read(&dir, start, start + 600, &selectors(&["fs.used"]), Some(300)).unwrap();
        assert_eq!(
            render_show(&series, false, true),
            format!("fs.used{{/}}\n{}  602 MB\n{}  607 MB\n", time(start), time(start + 300))
        );
    }

    #[test]
    fn show_reads_the_history_by_the_options() {
        let (dir, _) = history("options");
        let now_ns = util::now_ns();
        let selected = selectors(&["fs.used"]);
        assert_eq!(show(&dir, &selected, &options("1h", None, None, false), now_ns).unwrap(), 0);
        assert_eq!(
            show(&dir, &selected, &options("1h", Some("1m"), Some("5m"), true), now_ns).unwrap(),
            0
        );
        // A length of time before the history began has no points.
        let none =
            show(&dir, &selected, &options("3d", Some("2d"), None, false), now_ns).unwrap_err();
        assert!(none.message.starts_with("no points of fs.used between "), "{}", none.message);
    }

    #[test]
    fn a_range_that_is_backwards_or_empty_is_said() {
        let (dir, _) = history("empty");
        let selected = selectors(&["fs.used"]);
        let backwards =
            show(&dir, &selected, &options("1m", Some("1h"), None, false), NOW).unwrap_err();
        assert!(backwards.usage && backwards.message.contains("--since has to be before --until"));
        let nothing = show(&dir, &selectors(&["nosuch"]), &options("1h", None, None, false), NOW)
            .unwrap_err();
        assert!(nothing.message.starts_with("no points of nosuch between "), "{}", nothing.message);
        let unnamed = show(&dir, &[], &options("1h", None, None, false), NOW).unwrap_err();
        assert!(unnamed.usage && unnamed.message.contains("which series?"));
    }

    #[test]
    fn the_series_are_listed_by_name() {
        let (dir, _) = history("list");
        let names = store::series(&dir).unwrap();
        assert_eq!(render_list(&names, false), "fs.used{/}\ncpu\n");
        assert_eq!(render_list(&names, true), "[\"fs.used{/}\",\"cpu\"]\n");
        assert_eq!(list(&dir, &[], false).unwrap(), 0);
        assert!(list(&dir, &selectors(&["nosuch"]), false).is_err());
        assert!(
            list(&folder("nohistory"), &[], false).unwrap_err().message.contains("no history yet")
        );
    }

    fn run(items: &[&str]) -> Result<i32, Fail> {
        main(&mut Args::new("metrics", items.iter().map(OsString::from).collect()))
    }

    #[test]
    fn the_command_line_is_checked() {
        assert_eq!(run(&["--help"]).unwrap(), 0);
        for wrong in [
            &["show"][..],
            &["now", "--since", "1h"],
            &["list", "--every", "5m"],
            &["cpu", "--utc"],
            &["--bogus"],
            &["show", "cpu", "--every", "0"],
        ] {
            assert!(run(wrong).is_err(), "{wrong:?}");
        }
    }
}
