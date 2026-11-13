// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! shutdown(8), reboot(8), poweroff(8), halt(8) and service(8): requests
//! to Spark over its control socket, `/system/live/spark/control`
//! (spark(8)); service(8)'s `check`, `new` and `edit` are in
//! `servicefiles`.
//!
//! Spark stops the services in order and then turns the machine off,
//! restarts it or halts it; with `--force` it skips the services. The
//! options are systemd's and sysvinit's, and service(8) is Debian's.

use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;

use crate::args::{Arg, Args};
use crate::servicefiles;
use crate::util::{self, Fail, fail, fail_with, usage};
use spark_api::show::{self, HealthState, State};

/// Spark's control socket.
pub const CONTROL: &str = "/system/live/spark/control";

/// Sends Spark `request` and returns its answer.
pub fn request(request: &str) -> Result<String, Fail> {
    let mut stream = UnixStream::connect(CONTROL).map_err(|e| util::failed(&CONTROL, e))?;
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

/// shutdown(8): `shutdown [-H|-P|-r|-h] [-k] [--no-wall] [TIME] [MESSAGE...]`, or
/// `shutdown -c`.
pub fn shutdown(args: &mut Args) -> Result<i32, Fail> {
    let mut end = End::PowerOff;
    let (mut cancel, mut warn_only, mut wall) = (false, false, true);
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
                     \x20 -c             Cancel a pending shutdown\n\n\
                     TIME is `now', `+MINUTES' or `HH:MM'; the default is +1."
                );
                return Ok(0);
            }
            Arg::Operand(value) => operands.push(value.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    if cancel {
        ask("cancel")?;
        if wall {
            eprintln!("The system shutdown has been cancelled.");
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

/// What Spark says of the services, or of service `name` alone.
fn shown(name: Option<&str>) -> Result<Vec<show::Service>, Fail> {
    let request = name.map_or_else(|| String::from("show"), |name| format!("show {name}"));
    let answer = ask(&request)?;
    show::parse(&answer).map(|document| document.services).map_err(fail)
}

/// `service NAME status`: what Spark says of `service`, as systemctl
/// status lays it out; 0 when it runs or is done, 3 otherwise (LSB).
fn print_status(service: &show::Service) -> i32 {
    println!("{} - {}", service.name, service.description);
    let pid = service.pid.map_or_else(String::new, |pid| format!(" as process {pid}"));
    let active = match service.state {
        State::Running => format!("active (running){pid}"),
        State::Done => String::from("active (exited)"),
        State::Starting => format!("activating (starting{pid}, ready once {})", service.ready_when),
        State::Pending => format!("activating (waiting for {})", service.waiting_for.join(", ")),
        State::Waiting => String::from("activating (waiting to start again)"),
        State::Stopping if service.pid.is_some() => format!("deactivating (stopping{pid})"),
        State::Stopping => String::from("deactivating (stopping what it left running)"),
        State::Failed => String::from("failed (Spark gave up on it)"),
        State::Disabled => String::from("inactive (disabled)"),
        State::Stopped => String::from("inactive (dead)"),
    };
    println!("   Active: {active}");
    if let Some(why) = &service.why {
        println!("      Why: {why}");
    }
    let links: Vec<String> = [("requires", &service.requires), ("wants", &service.wants)]
        .into_iter()
        .chain([("after", &service.after)])
        .filter(|(_, names)| !names.is_empty())
        .map(|(key, names)| format!("{key} {}", names.join(", ")))
        .collect();
    if !links.is_empty() {
        println!("    Needs: {}", links.join("; "));
    }
    if let Some(by) = &service.started_by {
        println!("  Started: by {by}");
    }
    if let Some(health) = &service.health {
        let state = match health.state {
            HealthState::Unknown => String::from("not checked yet"),
            HealthState::Passing => String::from("passing"),
            HealthState::Failing => {
                let last = health.last_failure.as_deref().unwrap_or("no reason given");
                format!("failing, {} checks in a row ({last})", health.failures)
            }
        };
        println!("   Health: {state}");
    }
    let give_up = match service.give_up_after {
        0 => String::new(),
        n => format!("; gives up after {n} failed starts in a row"),
    };
    println!("  Restart: {}, {} restarts so far{give_up}", service.restart, service.restarts);
    if let Some(end) = &service.last_end {
        println!("     Last: {end}");
    }
    if let Some(complaint) = &service.complaint {
        println!("     File: {complaint}; it runs its last good copy");
    }
    if matches!(service.state, State::Running | State::Done) { 0 } else { 3 }
}

/// service(8): `service NAME start|stop|restart|status`, or
/// `service --status-all`.
pub fn service(args: &mut Args) -> Result<i32, Fail> {
    args.stop_at_operand();
    let mut operands = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(l) if l == "status-all" => {
                for service in shown(None)? {
                    let mark = match service.state {
                        State::Running | State::Done => '+',
                        State::Pending | State::Starting | State::Waiting => '?',
                        _ => '-',
                    };
                    println!(" [ {mark} ]  {}", service.name);
                }
                return Ok(0);
            }
            Arg::Long(l) if l == "help" => {
                println!(
                    "Usage: service < option > | --status-all | [ service_name [ command | --full-restart ] ]"
                );
                println!("       service service_name enable | disable");
                println!(
                    "       service check [ service_name | file ]... | new service_name | edit service_name"
                );
                println!(
                    "       service secret set service_name secret | list service_name | remove service_name secret"
                );
                return Ok(0);
            }
            Arg::Operand(value) => operands.push(value.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    operands.extend(args.rest().into_iter().map(|a| a.to_string_lossy().into_owned()));
    // The files' own commands come first, as `service check logd`.
    match operands.first().map(String::as_str) {
        Some("check") => return servicefiles::check(&operands[1..]),
        Some("new") => return servicefiles::new(&operands[1..]),
        Some("edit") => return servicefiles::edit(&operands[1..]),
        Some("secret") => return servicefiles::secret(&operands[1..]),
        _ => {}
    }
    let (Some(name), Some(command)) = (operands.first(), operands.get(1)) else {
        return Err(usage(
            "Usage: service < option > | --status-all | [ service_name [ command | --full-restart ] ]"
                .into(),
        ));
    };
    match command.as_str() {
        "enable" => servicefiles::enable(name, true),
        "disable" => servicefiles::enable(name, false),
        "start" | "stop" | "restart" | "reload" => {
            ask(&format!("{command} {name}"))?;
            Ok(0)
        }
        "--full-restart" => {
            ask(&format!("restart {name}"))?;
            Ok(0)
        }
        "status" => match shown(Some(name)) {
            Ok(services) => Ok(services.first().map_or(3, print_status)),
            // LSB: 4 is an unknown service.
            Err(fail) if fail.message.starts_with("no service is called") => {
                Err(fail_with(4, format!("{name}: unrecognized service")))
            }
            Err(fail) => Err(fail),
        },
        other => {
            Err(usage(format!("{name}: unknown command `{other}' (start, stop, restart, status)")))
        }
    }
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
}
