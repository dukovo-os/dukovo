// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! certd(8): keeps the machine's certificates (ADR 0512).
//!
//! At its start, and then every 12 hours and up to an hour more (so that
//! machines do not all wake together), certd makes `this-machine` if it is
//! missing or does not read, and makes it again when 30 days or fewer are
//! left or when the machine's name is not the certificate's first name
//! (the certificate lives in the store of [`crate::cert::store`]); each time
//! it tells Spark, which copies the certificate for the services that name
//! it. It puts a sign-in notice, and tells notifyd, when a certificate added
//! by hand has 21 days or fewer left, and when it has remade
//! `this-machine`, whose fingerprint a person who trusted the old one
//! needs.
//!
//! While the clock is before the system's making it does neither (a
//! certificate made then would be dated 1970), and looks again each
//! minute. SIGHUP has it look now; SIGTERM ends it.
//!
//! Its readiness is `cert check` (certd.toml's health check). When it
//! misbehaves, `log certd` shows each thing it did, one line each, and
//! nothing when nothing was due; `cert list` shows the store, and the
//! `certificates` boot test covers it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use ring::rand::{SecureRandom, SystemRandom};

use crate::args::Args;
use crate::cert::store::{Kind, Store};
use crate::cert::{self, inspect, machine};
use crate::notifyd::{self, Urgency};
use crate::sign_in_notices;
use crate::util::{self, Fail};

/// Spark's socket for certd, in a folder of its own that certd's view
/// holds read-only (spark(8)).
const SPARK_SOCKET: &str = "/system/live/spark/certd/socket";

/// The time between looks at the certificates, and the most a look is
/// put off to spread machines out.
const CHECK_EVERY: Duration = Duration::from_hours(12);
const SPREAD_MAX: u32 = 3600;

/// How soon certd looks again while the clock is wrong, and after a
/// certificate could not be made.
const CLOCK_WAIT: Duration = Duration::from_mins(1);
const FAILED_WAIT: Duration = Duration::from_mins(5);

/// A certificate added by hand with this many days or fewer left is
/// told of, and with [`CRITICAL_DAYS`] or fewer, or once it has run out,
/// told of as critical.
const NOTICE_DAYS: i64 = 21;
const CRITICAL_DAYS: i64 = 7;

static STOP: AtomicBool = AtomicBool::new(false);
static CHECK_NOW: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

extern "C" fn on_hangup(_: libc::c_int) {
    CHECK_NOW.store(true, Ordering::Relaxed);
}

/// What certd carries from one look to the next.
struct Certd {
    store: Store,
    /// The notice of the last time it made `this-machine` again, kept
    /// until certd ends.
    remade: Option<String>,
    /// Whether the log already says the clock is wrong.
    waiting: bool,
}

/// certd(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    for (signal, handler) in
        [(libc::SIGTERM, on_term as extern "C" fn(libc::c_int)), (libc::SIGHUP, on_hangup)]
    {
        // SAFETY: all zeros is a valid sigaction: numbers, a signal set
        // and a function address.
        let mut action: libc::sigaction = unsafe { util::zeroed() };
        action.sa_sigaction = handler as *const () as usize;
        // SAFETY: the handlers only store to an atomic.
        unsafe { libc::sigaction(signal, &raw const action, std::ptr::null_mut()) };
    }
    let mut certd = Certd { store: Store::system(), remade: None, waiting: false };
    println!("certd: keeping certificates in {}", certd.store.dir().display());
    while !STOP.load(Ordering::Relaxed) {
        CHECK_NOW.store(false, Ordering::Relaxed);
        let wait = certd.look();
        sleep(wait);
    }
    Ok(0)
}

/// Sleeps for `time`, or until SIGTERM or SIGHUP.
fn sleep(time: Duration) {
    let until = Instant::now() + time;
    while Instant::now() < until
        && !STOP.load(Ordering::Relaxed)
        && !CHECK_NOW.load(Ordering::Relaxed)
    {
        thread::sleep(Duration::from_secs(1).min(until.saturating_duration_since(Instant::now())));
    }
}

/// The time to the next look when this one went well: [`CHECK_EVERY`]
/// and `spread` seconds more.
fn next_look(spread: u32) -> Duration {
    CHECK_EVERY + Duration::from_secs(u64::from(spread % SPREAD_MAX))
}

impl Certd {
    /// One look at the certificates; the time to the next.
    fn look(&mut self) -> Duration {
        let now = cert::now();
        if let Some(why) = machine::wrong_clock(now) {
            if !self.waiting {
                println!("certd: {why}; certificates wait until it is set");
                self.waiting = true;
            }
            return CLOCK_WAIT;
        }
        if self.waiting {
            println!("certd: the clock is set; looking at the certificates");
            self.waiting = false;
        }
        let kept = self.keep_this_machine(now);
        self.warn_of_expiry(now);
        if kept {
            let mut spread = [0u8; 4];
            // Without random numbers every machine looks on the hour
            // after 12: late is better than no look.
            let _ = SystemRandom::new().fill(&mut spread);
            next_look(u32::from_le_bytes(spread))
        } else {
            FAILED_WAIT
        }
    }

    /// Makes `this-machine` if it is due, and tells Spark; whether it is
    /// as it should be.
    fn keep_this_machine(&mut self, now: i64) -> bool {
        let host = util::hostname();
        let loaded = machine::load_with_key(&self.store);
        let Some(why) = machine::remake_reason(&loaded, &host, now) else { return true };
        let existed = !matches!(loaded, Ok(None));
        let made =
            self.store.lock().and_then(|lock| machine::renew(&self.store, &lock, &host, now));
        let details = match made {
            Ok(details) => details,
            Err(err) => {
                eprintln!("<3>certd: cannot make {}: {err}", machine::NAME);
                return false;
            }
        };
        println!(
            "certd: made {} for {host}: SHA-256 {}, runs out {}; {why}",
            machine::NAME,
            details.fingerprint_colons(),
            util::local_time(details.not_after, "%F")
        );
        if existed {
            self.remade = Some(format!(
                "This machine's certificate was remade on {}: its fingerprint is now {}; a \
                 browser that trusted the old one asks again",
                util::local_time(now, "%F"),
                details.fingerprint_colons()
            ));
        }
        if let Err(why) = cert::tell_spark(SPARK_SOCKET, machine::NAME) {
            eprintln!("<4>certd: {}", cert::spark_warning(&why));
        }
        true
    }

    /// Puts the notice of what needs a person's attention, or takes it
    /// away when nothing does, and tells notifyd when it changed.
    fn warn_of_expiry(&self, now: i64) {
        let ends = self.added_certificates();
        let (mut lines, urgency) = expiry_lines(&ends, now);
        let mut shown: Vec<String> = self.remade.iter().cloned().collect();
        shown.append(&mut lines);
        if !sign_in_notices::put("certd", &shown) {
            return;
        }
        let sent = notifyd::send("certd", "Certificates", &shown.join("\n"), urgency);
        if let Err(err) = sent {
            eprintln!("<4>certd: cannot tell notifyd: {}", err.message);
        }
    }

    /// The name and end time of each certificate added by hand.
    fn added_certificates(&self) -> Vec<(String, i64)> {
        let names = match self.store.names() {
            Ok(names) => names,
            Err(err) => {
                eprintln!("<3>certd: cannot list the certificates: {err}");
                return Vec::new();
            }
        };
        let mut ends = Vec::new();
        for name in names {
            let read = self.store.get(&name).and_then(|stored| {
                let text = String::from_utf8_lossy(&stored.chain).into_owned();
                let chain = inspect::chain_of(&text)?;
                let first = chain.first().ok_or_else(|| String::from("it holds no certificate"))?;
                Ok((stored.kind, inspect::details(first)?))
            });
            match read {
                Ok((Kind::Added, details)) => ends.push((name, details.not_after)),
                Ok((Kind::SelfSigned, _)) => {}
                Err(why) => eprintln!("<4>certd: the certificate {name} does not read: {why}"),
            }
        }
        ends
    }
}

/// The notice lines for the certificates `ends`, each a name and the time
/// it runs out, at time `now`: one for each with [`NOTICE_DAYS`] or
/// fewer left, and how urgent they are together.
fn expiry_lines(ends: &[(String, i64)], now: i64) -> (Vec<String>, Urgency) {
    let mut urgency = Urgency::Normal;
    let mut lines = Vec::new();
    for (name, not_after) in ends {
        let days = (not_after - now).div_euclid(86_400);
        if days > NOTICE_DAYS {
            continue;
        }
        if days <= CRITICAL_DAYS {
            urgency = Urgency::Critical;
        }
        let verb = if *not_after <= now { "ran out" } else { "runs out" };
        lines.push(format!(
            "The certificate {name} {verb} on {}: add its new version with `elevate cert add \
             {name} --replace`.",
            util::local_time(*not_after, "%F")
        ));
    }
    (lines, urgency)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;
    const DAY: i64 = 86_400;

    #[test]
    fn the_next_look_is_twelve_hours_and_up_to_an_hour_later() {
        let twelve = Duration::from_hours(12);
        assert_eq!(next_look(0), twelve);
        assert_eq!(next_look(1800), twelve + Duration::from_mins(30));
        assert_eq!(next_look(3599), twelve + Duration::from_secs(3599));
        assert_eq!(next_look(3600), twelve);
        assert_eq!(next_look(u32::MAX), twelve + Duration::from_secs(u64::from(u32::MAX % 3600)));
    }

    #[test]
    fn a_certificate_with_more_than_21_days_left_is_not_told_of() {
        let ends = [(String::from("a.example"), NOW + 22 * DAY)];
        assert_eq!(expiry_lines(&ends, NOW).0, Vec::<String>::new());
        assert!(expiry_lines(&[], NOW).0.is_empty());
    }

    #[test]
    fn a_certificate_with_21_days_left_gets_a_line_and_a_normal_notification() {
        let ends = [
            (String::from("a.example"), NOW + 21 * DAY + 600),
            (String::from("b.example"), NOW + 90 * DAY),
        ];
        let (lines, urgency) = expiry_lines(&ends, NOW);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("The certificate a.example runs out on 2027-02-05: add its new version with `elevate cert add a.example --replace`."), "{}", lines[0]);
        assert_eq!(urgency, Urgency::Normal);
    }

    #[test]
    fn at_seven_days_and_once_it_ran_out_the_notification_is_critical() {
        let seven = [(String::from("a.example"), NOW + 7 * DAY + 600)];
        assert_eq!(expiry_lines(&seven, NOW).1, Urgency::Critical);
        let eight = [(String::from("a.example"), NOW + 8 * DAY + 600)];
        assert_eq!(expiry_lines(&eight, NOW).1, Urgency::Normal);
        let (lines, urgency) = expiry_lines(&[(String::from("a.example"), NOW - DAY)], NOW);
        assert!(
            lines[0].starts_with("The certificate a.example ran out on 2027-01-14:"),
            "{}",
            lines[0]
        );
        assert_eq!(urgency, Urgency::Critical);
    }
}
