// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! shutdown(8), reboot(8), poweroff(8) and halt(8): requests to Spark
//! over its control socket, `/system/live/spark/control` (spark(8)),
//! whose `request` the other clients use too; service(8) is in
//! `service`.
//!
//! Spark stops the services in order and then turns the machine off,
//! restarts it or halts it; with `--force` it skips the services. The
//! options are systemd's and sysvinit's.
//!
//! `shutdown --show` says the shutdown that is set for later, if one is,
//! from Spark's read socket, which answers anyone; with `--json` as one
//! object, which the web console reads (`power.status`).

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;

use serde::Serialize;
use spark_api::show;

use crate::args::{Arg, Args};
use crate::util::{self, Fail, fail, usage};

/// Spark's control socket.
pub const CONTROL: &str = "/system/live/spark/control";

/// Spark's read socket, which answers `status` and `show` to anyone.
pub const READ: &str = "/system/live/spark/read";

/// Sends Spark `request` on its control socket and returns its answer.
pub fn request(request: &str) -> Result<String, Fail> {
    request_at(CONTROL, request)
}

/// Sends Spark `request`, `status` or `show`, which only reads, on its
/// read socket, and returns its answer. A Spark that has no read socket,
/// one of an earlier release still running until the machine restarts,
/// is asked on its control socket, which answers root.
pub fn read(request: &str) -> Result<String, Fail> {
    match UnixStream::connect(READ) {
        Ok(stream) => exchange(stream, request),
        Err(err) if err.kind() == io::ErrorKind::NotFound => request_at(CONTROL, request),
        Err(err) => Err(util::failed(&READ, err)),
    }
}

/// Sends `request` to Spark's socket `socket`, the control socket or
/// certd's (spark(8)), and returns its answer. Only root may write to the
/// control socket, which connecting takes.
pub fn request_at(socket: &str, request: &str) -> Result<String, Fail> {
    let stream = UnixStream::connect(socket).map_err(|err| match err.kind() {
        io::ErrorKind::PermissionDenied => util::fail_with(
            4,
            format!("only root may ask this of Spark ({socket}): run the command with elevate"),
        ),
        _ => util::failed(&socket, err),
    })?;
    exchange(stream, request)
}

/// Sends `request` on `stream`, a connection to one of Spark's sockets,
/// and returns its answer.
fn exchange(mut stream: UnixStream, request: &str) -> Result<String, Fail> {
    stream.write_all(format!("{request}\n").as_bytes())?;
    let _ = stream.shutdown(Shutdown::Write);
    let mut answer = String::new();
    stream.read_to_string(&mut answer)?;
    Ok(answer)
}

/// Sends `request`, and fails with what Spark said if it refused.
fn ask(request: &str) -> Result<String, Fail> {
    let answer = self::request(request)?;
    match answer.strip_prefix("error: ") {
        Some(why) => Err(fail(why.trim_end().to_string())),
        None => Ok(answer),
    }
}

/// Spark's words when it is asked to cancel and nothing is set.
const NOTHING_SET: &str = "no shutdown is pending";

/// How the machine ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    PowerOff,
    Reboot,
    Halt,
}

impl End {
    fn verb(self) -> &'static str {
        match self {
            End::PowerOff => "poweroff",
            End::Reboot => "reboot",
            End::Halt => "halt",
        }
    }
}

/// The seconds from `now` (seconds since the epoch, with `local_hm` its
/// local hour and minute) until `time`: `now`, `+MINUTES` or `HH:MM`, the
/// next time the clock says so.
fn delay(time: &str, local_hm: (u32, u32)) -> Option<u64> {
    if time == "now" {
        return Some(0);
    }
    if let Some(minutes) = time.strip_prefix('+') {
        return minutes.parse::<u64>().ok().map(|m| m.saturating_mul(60));
    }
    let (h, m) = time.split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    if h > 23 || m > 59 {
        return None;
    }
    let (now_h, now_m) = local_hm;
    let (at, now) = (h * 60 + m, now_h * 60 + now_m);
    let minutes = if at > now { at - now } else { at + 24 * 60 - now };
    Some(u64::from(minutes) * 60)
}

/// The local hour and minute now.
fn local_hm() -> (u32, u32) {
    let now = (util::now_ns() / 1_000_000_000) as i64;
    let hm = util::local_time(now, "%H %M");
    let mut parts = hm.split(' ').map(|p| p.parse().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

/// A shutdown that is set for later, as `shutdown --show --json` prints
/// it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Pending {
    /// What it ends in: `reboot`, `poweroff` or `halt`, the words of its
    /// command.
    pub end: &'static str,
    /// How long until it begins, as Spark said it.
    pub in_seconds: u64,
    /// When it begins, in milliseconds since 1970: the time of the read
    /// and [`Pending::in_seconds`], so to the second. Spark's own times are
    /// since the machine started, which a page cannot show as a time of
    /// day.
    pub at_ms: u64,
}

/// The shutdown that Spark's `show` document `document`, read at `now_ms`
/// (milliseconds since 1970), says is set, if one is. An end that this
/// program does not know the words of is told as Spark said it would
/// never be: a halt.
pub fn pending(document: &show::Show, now_ms: u64) -> Option<Pending> {
    let shutdown = document.shutdown.as_ref()?;
    let end = match shutdown.end.as_str() {
        "restart" => End::Reboot,
        "power off" => End::PowerOff,
        _ => End::Halt,
    };
    Some(Pending {
        end: end.verb(),
        in_seconds: shutdown.in_seconds,
        at_ms: now_ms.saturating_add(shutdown.in_seconds.saturating_mul(1000)),
    })
}

/// What `shutdown --show --json` prints.
#[derive(Serialize)]
struct Shown {
    format: u32,
    shutdown: Option<Pending>,
}

/// `shutdown --show [--json]`: the shutdown that is set, from Spark's read
/// socket.
fn show(json: bool) -> Result<i32, Fail> {
    let document = show::parse(&read("show")?).map_err(fail)?;
    let pending = pending(&document, util::now_ns() / 1_000_000);
    if json {
        let shown = Shown { format: 1, shutdown: pending };
        println!("{}", serde_json::to_string(&shown).map_err(|err| fail(err.to_string()))?);
        return Ok(0);
    }
    match pending {
        None => println!("No shutdown is set."),
        Some(pending) => {
            let at = i64::try_from(pending.at_ms / 1000).unwrap_or(i64::MAX);
            let when = util::local_time(at, "%a %Y-%m-%d %H:%M:%S %Z");
            println!(
                "The system {} is set for {when}, in {} seconds; `shutdown -c` cancels it.",
                pending.end, pending.in_seconds
            );
        }
    }
    Ok(0)
}

/// shutdown(8): `shutdown [-H|-P|-r|-h] [-k] [--no-wall] [TIME] [MESSAGE...]`,
/// `shutdown -c`, or `shutdown --show [--json]`.
pub fn shutdown(args: &mut Args) -> Result<i32, Fail> {
    let mut end = End::PowerOff;
    let (mut cancel, mut warn_only, mut wall) = (false, false, true);
    let (mut showing, mut json) = (false, false);
    let mut operands = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('H') => end = End::Halt,
            Arg::Long(l) if l == "halt" => end = End::Halt,
            Arg::Short('P' | 'h') => end = End::PowerOff,
            Arg::Long(l) if l == "poweroff" => end = End::PowerOff,
            Arg::Short('r') => end = End::Reboot,
            Arg::Long(l) if l == "reboot" => end = End::Reboot,
            Arg::Short('k') => warn_only = true,
            Arg::Long(l) if l == "no-wall" => wall = false,
            Arg::Short('c') => cancel = true,
            Arg::Long(l) if l == "show" => showing = true,
            Arg::Long(l) if l == "json" => json = true,
            Arg::Long(l) if l == "help" => {
                println!(
                    "shutdown [OPTIONS...] [TIME] [WALL...]\n\n\
                     Shut down the system.\n\n\
                     \x20 -H --halt      Halt the machine\n\
                     \x20 -P --poweroff  Power off the machine (the default)\n\
                     \x20 -r --reboot    Reboot the machine\n\
                     \x20 -h             Equivalent to --poweroff\n\
                     \x20 -k             Don't halt/power off/reboot, just send warnings\n\
                     \x20    --no-wall   Don't send a wall message\n\
                     \x20 -c             Cancel a pending shutdown\n\
                     \x20    --show      Show the pending shutdown (--json as an object)\n\n\
                     TIME is `now', `+MINUTES' or `HH:MM'; the default is +1."
                );
                return Ok(0);
            }
            Arg::Operand(value) => operands.push(value.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    if json && !showing {
        return Err(usage("--json goes with --show".into()));
    }
    if showing {
        if cancel || !operands.is_empty() {
            return Err(usage("--show takes no time and goes with no other request".into()));
        }
        return show(json);
    }
    if cancel {
        // As systemd's: with nothing set there is nothing to cancel, and
        // that is no failure, so a script or a play can cancel twice.
        match self::request("cancel")?.strip_prefix("error: ") {
            Some(why) if why.trim_end() == NOTHING_SET => {
                if wall {
                    eprintln!("No shutdown was set.");
                }
            }
            Some(why) => return Err(fail(why.trim_end().to_string())),
            None if wall => eprintln!("The system shutdown has been cancelled."),
            None => {}
        }
        return Ok(0);
    }
    let time = operands.first().cloned().unwrap_or_else(|| "+1".into());
    let Some(seconds) = delay(&time, local_hm()) else {
        return Err(usage(format!("Failed to parse time specification: {time}")));
    };
    let message = operands.get(1..).map_or_default(|words| words.join(" "));
    let what = match end {
        End::PowerOff => "power-off",
        End::Reboot => "reboot",
        End::Halt => "halt",
    };
    if wall && !message.is_empty() {
        eprintln!("{message}");
    }
    if warn_only {
        eprintln!("The system will not {}: -k only warns.", end.verb());
        return Ok(0);
    }
    ask(&format!("{} {seconds}", end.verb()))?;
    if seconds > 0 {
        let now = (util::now_ns() / 1_000_000_000) as i64;
        let at = now.saturating_add(i64::try_from(seconds).unwrap_or(i64::MAX));
        let when = util::local_time(at, "%a %Y-%m-%d %H:%M:%S %Z");
        eprintln!("The system {what} is scheduled for {when}, use 'shutdown -c' to cancel.");
    }
    Ok(0)
}

/// reboot(8), poweroff(8) and halt(8): `NAME [-f] [-p] [--reboot|--poweroff|--halt]`.
fn end_now(args: &mut Args, mut end: End) -> Result<i32, Fail> {
    let mut force = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('f') => force = true,
            Arg::Long(l) if l == "force" => force = true,
            Arg::Short('p') if end == End::Halt => end = End::PowerOff,
            Arg::Long(l) if l == "poweroff" => end = End::PowerOff,
            Arg::Long(l) if l == "reboot" => end = End::Reboot,
            Arg::Long(l) if l == "halt" => end = End::Halt,
            // Kept for scripts: there is no wtmp to write, and the kernel
            // syncs the disks whatever is asked.
            Arg::Short('w' | 'd' | 'n') => {}
            Arg::Long(l)
                if ["wtmp-only", "no-wtmp", "no-sync", "no-wall"].contains(&l.as_str()) => {}
            Arg::Long(l) if l == "help" => {
                println!(
                    "{} [OPTIONS...]\n\n{} the system.\n\n\
                     \x20    --halt      Halt the machine\n\
                     \x20 -p --poweroff  Switch off the machine\n\
                     \x20    --reboot    Reboot the machine\n\
                     \x20 -f --force     Force immediate {}, without stopping the services",
                    args.tool,
                    match end {
                        End::PowerOff => "Power off",
                        End::Reboot => "Reboot",
                        End::Halt => "Halt",
                    },
                    end.verb()
                );
                return Ok(0);
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let request = if force { format!("{} --force", end.verb()) } else { end.verb().to_string() };
    ask(&request)?;
    Ok(0)
}

/// reboot(8).
pub fn reboot(args: &mut Args) -> Result<i32, Fail> {
    end_now(args, End::Reboot)
}

/// poweroff(8).
pub fn poweroff(args: &mut Args) -> Result<i32, Fail> {
    end_now(args, End::PowerOff)
}

/// halt(8).
pub fn halt(args: &mut Args) -> Result<i32, Fail> {
    end_now(args, End::Halt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(delay("now", (10, 0)), Some(0));
        assert_eq!(delay("+5", (10, 0)), Some(300));
        // 584 years does not wrap round to 26 seconds.
        assert_eq!(delay("+307445735", (10, 0)), Some(18_446_744_100));
        assert_eq!(delay("+18446744073709551615", (10, 0)), Some(u64::MAX));
        assert_eq!(delay("10:30", (10, 0)), Some(1800));
        // A time that has passed today is tomorrow's.
        assert_eq!(delay("09:00", (10, 0)), Some(23 * 3600));
        assert_eq!(delay("24:00", (10, 0)), None);
        assert_eq!(delay("soon", (10, 0)), None);
    }

    /// Spark's `show` document with `shutdown`, written an hour after the
    /// machine started.
    fn document(shutdown: Option<(&str, u64)>) -> show::Show {
        show::Show {
            show_version: show::VERSION,
            now_ms: 3_600_000,
            shutdown: shutdown
                .map(|(end, in_seconds)| show::Shutdown { end: end.into(), in_seconds }),
            ..show::Show::default()
        }
    }

    #[test]
    fn the_pending_shutdown_is_told_in_the_command_s_words_with_its_time() {
        assert_eq!(pending(&document(None), 5_000), None);
        // The time is the read's, by the clock of days, and not Spark's,
        // which counts from the start.
        let restart = pending(&document(Some(("restart", 60))), 5_000).unwrap();
        assert_eq!(restart, Pending { end: "reboot", in_seconds: 60, at_ms: 65_000 });
        assert_eq!(pending(&document(Some(("power off", 1))), 0).unwrap().end, "poweroff");
        assert_eq!(pending(&document(Some(("halt", 1))), 0).unwrap().end, "halt");
        let shown = Shown { format: 1, shutdown: Some(restart) };
        assert_eq!(
            serde_json::to_string(&shown).unwrap(),
            r#"{"format":1,"shutdown":{"end":"reboot","in_seconds":60,"at_ms":65000}}"#
        );
        let none = Shown { format: 1, shutdown: None };
        assert_eq!(serde_json::to_string(&none).unwrap(), r#"{"format":1,"shutdown":null}"#);
    }

    #[test]
    fn a_time_at_the_end_of_the_clock_does_not_wrap() {
        let far = pending(&document(Some(("restart", u64::MAX))), u64::MAX - 5).unwrap();
        assert_eq!(far.at_ms, u64::MAX);
    }
}
