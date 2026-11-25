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
//! A certificate from an authority that was proved on port 80 (ADR 0513)
//! it renews on its own: at each look it asks the authority when it would
//! have the certificate renewed (its renewal information, RFC 9773), picks
//! a time inside the window and keeps it as `renew_at`, and when the time
//! comes gets a new certificate as `cert get` did, with a key made anew.
//! An authority without renewal information has it renewed two thirds of the
//! way through its life. A renewal that fails is tried again at the next
//! look, and a notice says so from 14 days before the certificate runs out
//! (critical from 7). One proved by DNS by hand is told of at 21 and 7 days
//! as an added one is, and a revoked one is told of until it is replaced.
//!
//! A certificate that `cert get` could not get is kept as a record in
//! [`crate::cert::wanted`], and certd asks for it again as `cert get` did: at
//! each look, for each record whose time has come, every hour for a day and
//! every 6 hours after, for 14 days. It installs what it gets, tells Spark and
//! takes the record away; a try that fails is told in the history and the
//! record, and an authority's request to wait is waited for. A record whose
//! account was taken away, or whose name was added by hand since, is given
//! up with that reason, as is one that failed for 14 days; it is kept, so
//! that `cert wanted` shows it, until `cert forget` takes it away. certd
//! looks again when the soonest record is due, if that is before the look it
//! would make anyway.
//!
//! Each look also tells alertd the certificates that need attention, as the
//! alerts `certificate:NAME` with the words of the notice: a warning, and
//! critical from 7 days or once the certificate has run out; and the same
//! key for a certificate that could not be got for a day, and
//! `certificate-given-up:NAME` for one certd gave up. It sends the
//! whole list with alertd's `set`, so that an alert whose certificate was
//! renewed, replaced or removed clears by itself. What certd and `cert` did
//! to the certificates is kept in `/system/state/certd/history`.
//!
//! While the clock is before the system's making it does neither (a
//! certificate made then would be dated 1970), and looks again each
//! minute. SIGHUP has it look now, as `cert` asks when it changes what is
//! wanted; SIGTERM ends it.
//!
//! Its readiness is `cert check` (certd.toml's health check). When it
//! misbehaves, `log certd` shows each thing it did, one line each, and
//! nothing when nothing was due; `cert list` shows the store, and the
//! `certificates` boot test covers it.

use std::collections::BTreeMap;
use std::thread;
use std::time::{Duration, Instant};

use ring::rand::{SecureRandom, SystemRandom};
use sysapi::signal::{self, Caught};

use crate::alerts::protocol::{About, Alert, Severity};
use crate::args::Args;
use crate::cert::acme::{self, Who};
use crate::cert::history::{self, By, Event};
use crate::cert::store::{Acme, Challenge, Kind, Store};
use crate::cert::wanted::{self, Record};
use crate::cert::{self, accounts, inspect, machine};
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

/// A certificate from an authority that could not be renewed is told of
/// with this many days or fewer left.
const FAILED_NOTICE_DAYS: i64 = 14;

static STOP: Caught = Caught::new();
static CHECK_NOW: Caught = Caught::new();

/// What certd carries from one look to the next.
struct Certd {
    store: Store,
    /// The notice of the last time it made `this-machine` again, kept
    /// until certd ends.
    remade: Option<String>,
    /// Whether the log already says the clock is wrong.
    waiting: bool,
    /// When each certificate from an authority is next asked about, in
    /// seconds since 1970: what the authority said to wait.
    asked: BTreeMap<String, i64>,
    /// Why each certificate from an authority was not renewed at the last
    /// look, until it is.
    failed: BTreeMap<String, String>,
    /// The certificates that could not be got, as the last look left them:
    /// what the alerts and the wait to the next look follow.
    wanted: Vec<Record>,
    /// Whether the log already says that alertd could not be told.
    alertd_unreached: bool,
}

/// certd(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    for (number, caught) in [(libc::SIGTERM, &STOP), (libc::SIGHUP, &CHECK_NOW)] {
        let _ = signal::catch(number, caught, 0);
    }
    let mut certd = Certd {
        store: Store::system(),
        remade: None,
        waiting: false,
        asked: BTreeMap::new(),
        failed: BTreeMap::new(),
        wanted: Vec::new(),
        alertd_unreached: false,
    };
    println!("certd: keeping certificates in {}", certd.store.dir().display());
    while !STOP.is_set() {
        CHECK_NOW.take();
        let wait = certd.look();
        sleep(wait);
    }
    Ok(0)
}

/// Sleeps for `time`, or until SIGTERM or SIGHUP.
fn sleep(time: Duration) {
    let until = Instant::now() + time;
    while Instant::now() < until && !STOP.is_set() && !CHECK_NOW.is_set() {
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
        self.keep_authority_certificates(now);
        let listed = self.keep_wanted_certificates(now);
        // The summary of the accounts at authorities that anyone may read:
        // made for the accounts of a machine that had them before it.
        if let Err(why) = accounts::refresh(&self.store) {
            eprintln!("<4>certd: the summary of the accounts cannot be written: {why}");
        }
        self.warn_of_expiry(now);
        let wait = if kept && listed {
            let mut spread = [0u8; 4];
            // Without random numbers every machine looks on the hour
            // after 12: late is better than no look.
            let _ = SystemRandom::new().fill(&mut spread);
            next_look(u32::from_le_bytes(spread))
        } else {
            FAILED_WAIT
        };
        self.wait_for_wanted(wait, cert::now())
    }

    /// `wait`, cut short to the time the soonest wanted certificate is next
    /// tried at, if that comes sooner, as seen at `now`.
    fn wait_for_wanted(&self, wait: Duration, now: i64) -> Duration {
        let soonest = self.wanted.iter().filter(|record| !record.given_up).map(|r| r.next_ms).min();
        let Some(next_ms) = soonest else { return wait };
        let seconds = (next_ms.div_euclid(1_000) - now).max(1);
        wait.min(Duration::from_secs(u64::try_from(seconds).unwrap_or(1)))
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
        let entry = history::Entry::new(
            now,
            machine::NAME,
            Event::Made,
            &format!("for {host}: {why}"),
            By::Certd,
        );
        history::note(&self.store, &entry);
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

    /// Renews each certificate from an authority whose time has come, and
    /// keeps the time of each, which the authority's renewal information
    /// gives.
    fn keep_authority_certificates(&mut self, now: i64) {
        let names = match self.store.names() {
            Ok(names) => names,
            Err(err) => {
                eprintln!("<3>certd: cannot list the certificates: {err}");
                return;
            }
        };
        for name in names {
            let Ok(stored) = self.store.get(&name) else { continue };
            let Some(acme) = stored.acme.clone().filter(|_| stored.kind == Kind::Acme) else {
                continue;
            };
            if acme.revoked || acme.challenge != Challenge::Http01 {
                self.failed.remove(&name);
                continue;
            }
            let chain = String::from_utf8_lossy(&stored.chain).into_owned();
            let details = inspect::chain_of(&chain).and_then(|certificates| {
                let first =
                    certificates.first().ok_or_else(|| String::from("it holds no certificate"))?;
                inspect::details(first)
            });
            let details = match details {
                Ok(details) => details,
                Err(why) => {
                    eprintln!("<4>certd: the certificate {name} does not read: {why}");
                    continue;
                }
            };
            let renew_at = self.renewal_time(
                &name,
                &acme,
                &chain,
                (details.not_before, details.not_after),
                now,
            );
            if now < renew_at {
                continue;
            }
            self.renew(&name, now);
        }
    }

    /// Asks again for each wanted certificate whose time has come, and keeps
    /// how each try went; whether the records could be listed. A record that
    /// does not read is left as it is.
    fn keep_wanted_certificates(&mut self, now: i64) -> bool {
        let listed = self.store.lock().and_then(|lock| wanted::list(&self.store, &lock));
        let found = match listed {
            Ok(found) => found,
            Err(why) => {
                eprintln!("<3>certd: cannot list the certificates that are wanted: {why}");
                return false;
            }
        };
        let mut kept = Vec::new();
        for each in found {
            match each {
                Ok(record) if record.is_due(now) => kept.extend(self.try_again(&record, now)),
                Ok(record) => kept.push(record),
                Err(unreadable) => eprintln!(
                    "<4>certd: the record of {} does not read, so it is left alone: {}; `elevate \
                     cert forget {}` takes it away",
                    unreadable.name, unreadable.why, unreadable.name
                ),
            }
        }
        self.wanted = kept;
        true
    }

    /// Asks for the certificate `record` is for, and tells Spark when it came:
    /// the record as it is kept after, or none when nothing is wanted any
    /// more.
    fn try_again(&mut self, record: &Record, now: i64) -> Option<Record> {
        let name = record.order.name.clone();
        if let Some(why) = wanted::cannot_go_on(&self.store, record) {
            eprintln!("<4>certd: stops trying to get {name}: {why}");
            let entry = history::Entry::new(now, &name, Event::Failed, &why, By::Certd);
            history::note(&self.store, &entry);
            return self.keep(record, record.gave_up(now, &why));
        }
        let mut say = |_: &str| {};
        match acme::retry_wanted(&self.store, record, now, &mut say) {
            Ok((details, notes)) => {
                println!(
                    "certd: got {name}: {}, runs out {}",
                    details.names.join(", "),
                    util::local_time(details.not_after, "%F")
                );
                for note in notes {
                    println!("certd: {name}: {note}");
                }
                self.failed.remove(&name);
                self.asked.remove(&name);
                if let Err(why) = cert::tell_spark(SPARK_SOCKET, &name) {
                    eprintln!("<4>certd: {}", cert::spark_warning(&why));
                }
                None
            }
            Err(refused) => {
                eprintln!("<4>certd: cannot get {name}: {}", refused.why);
                let next = record.failed_again(now, &refused.why, refused.not_before);
                if next.given_up {
                    eprintln!("<4>certd: stops trying to get {name}: it failed for 14 days");
                }
                self.keep(record, next)
            }
        }
    }

    /// Keeps `next` in place of `record`, which was read before a try:
    /// `next`, or none when `record` was taken away or made again since, and
    /// what is there is left.
    fn keep(&self, record: &Record, next: Record) -> Option<Record> {
        let name = &record.order.name;
        let lock = match self.store.lock() {
            Ok(lock) => lock,
            Err(why) => {
                eprintln!("<3>certd: cannot keep how the try for {name} went: {why}");
                return Some(next);
            }
        };
        if !wanted::is_current(&self.store, &lock, record) {
            println!("certd: the record of {name} was changed while it was tried: left as it is");
            return None;
        }
        if let Err(why) = wanted::write(&self.store, &lock, &next) {
            eprintln!("<3>certd: cannot keep how the try for {name} went: {why}");
        }
        Some(next)
    }

    /// When certificate `name`, which `acme` says how it was got, is to be
    /// renewed: what is kept, or what the authority says now when it is time
    /// to ask, which is kept for the next look.
    fn renewal_time(
        &mut self,
        name: &str,
        acme: &Acme,
        chain: &str,
        life: (i64, i64),
        now: i64,
    ) -> i64 {
        let due = acme.renew_at.is_none() || self.asked.get(name).is_none_or(|next| now >= *next);
        if !due {
            return acme.renew_at.unwrap_or_else(|| acme::third_rule(life.0, life.1));
        }
        let (window, next) = match acme::look_at_window(&self.store, &acme.directory, chain, now) {
            Ok(Some(window)) => (
                Some((window.start, window.end)),
                now + window.ask_again.clamp(acme::LOOK_AGAIN.0, acme::LOOK_AGAIN.1),
            ),
            Ok(None) => (None, now + acme::LOOK_AGAIN.1),
            Err(why) => {
                eprintln!("<4>certd: cannot ask about renewing {name}: {why}");
                (None, now + acme::LOOK_AGAIN.0)
            }
        };
        self.asked.insert(name.to_string(), next);
        let at = acme::renew_time(acme.renew_at, window, life, now, acme::random());
        if acme.renew_at != Some(at) {
            let kept = Acme { renew_at: Some(at), ..acme.clone() };
            let set = self.store.lock().and_then(|lock| self.store.set_acme(&lock, name, &kept));
            match set {
                Ok(()) => {
                    println!("certd: {name} is to be renewed on {}", util::local_time(at, "%F %T"));
                }
                Err(why) => eprintln!("<4>certd: cannot keep when to renew {name}: {why}"),
            }
        }
        at
    }

    /// Renews certificate `name` now, and tells Spark.
    fn renew(&mut self, name: &str, now: i64) {
        let mut say = |_: &str| {};
        match acme::renew(&self.store, name, Who::Certd, now, &mut say) {
            Ok((details, notes)) => {
                println!(
                    "certd: renewed {name}: {}, runs out {}",
                    details.names.join(", "),
                    util::local_time(details.not_after, "%F")
                );
                for note in notes {
                    println!("certd: {name}: {note}");
                }
                self.failed.remove(name);
                self.asked.remove(name);
                if let Err(why) = cert::tell_spark(SPARK_SOCKET, name) {
                    eprintln!("<4>certd: {}", cert::spark_warning(&why));
                }
            }
            Err(why) => {
                eprintln!("<4>certd: cannot renew {name}: {why}");
                self.failed.insert(name.to_string(), why);
            }
        }
    }

    /// Puts the notice of what needs a person's attention, or takes it
    /// away when nothing does, and tells notifyd when it changed; and tells
    /// alertd the same, which clears what no longer does.
    fn warn_of_expiry(&mut self, now: i64) {
        let (added, authority) = self.watched_certificates();
        let mut notices = expiry_notices(&added, now);
        notices.append(&mut authority_notices(&authority, &self.failed, now));
        let alerts = with_wanted(alerts_of(&notices), wanted_alerts(&self.wanted, now));
        self.tell_alertd(&alerts);
        let urgency = if notices.iter().any(|notice| notice.critical) {
            Urgency::Critical
        } else {
            Urgency::Normal
        };
        let mut shown: Vec<String> = self.remade.iter().cloned().collect();
        shown.extend(notices.into_iter().map(|notice| notice.line));
        if !sign_in_notices::put("certd", &shown) {
            return;
        }
        let sent = notifyd::send("certd", "Certificates", &shown.join("\n"), urgency);
        if let Err(err) = sent {
            eprintln!("<4>certd: cannot tell notifyd: {}", err.message);
        }
    }

    /// Sets certd's alerts in alertd to `alerts`, each raised and each of
    /// certd's others cleared. alertd not running is said once, until it
    /// is.
    fn tell_alertd(&mut self, alerts: &[Alert]) {
        match crate::alerts::client::set(alerts) {
            Ok(()) => self.alertd_unreached = false,
            Err(why) if !self.alertd_unreached => {
                self.alertd_unreached = true;
                eprintln!("<4>certd: cannot tell alertd: {why}");
            }
            Err(_) => {}
        }
    }

    /// The name and end time of each certificate added by hand, and what
    /// is watched of each certificate from an authority.
    fn watched_certificates(&self) -> (Vec<(String, i64)>, Vec<Watched>) {
        let names = match self.store.names() {
            Ok(names) => names,
            Err(err) => {
                eprintln!("<3>certd: cannot list the certificates: {err}");
                return (Vec::new(), Vec::new());
            }
        };
        let (mut added, mut authority) = (Vec::new(), Vec::new());
        for name in names {
            let read = self.store.get(&name).and_then(|stored| {
                let text = String::from_utf8_lossy(&stored.chain).into_owned();
                let chain = inspect::chain_of(&text)?;
                let first = chain.first().ok_or_else(|| String::from("it holds no certificate"))?;
                Ok((stored.kind, stored.acme, inspect::details(first)?))
            });
            match read {
                Ok((Kind::Added, _, details)) => added.push((name, details.not_after)),
                Ok((Kind::SelfSigned, _, _)) => {}
                Ok((Kind::Acme, acme, details)) => {
                    let Some(acme) = acme else { continue };
                    authority.push(Watched {
                        name,
                        not_after: details.not_after,
                        by_hand: acme.challenge == Challenge::DnsManual,
                        revoked: acme.revoked,
                    });
                }
                Err(why) => eprintln!("<4>certd: the certificate {name} does not read: {why}"),
            }
        }
        (added, authority)
    }
}

/// A certificate from an authority, as the notices see it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Watched {
    name: String,
    not_after: i64,
    /// Proved by DNS by hand, which certd cannot repeat.
    by_hand: bool,
    revoked: bool,
}

/// What certd says of one certificate that needs attention: a line of the
/// sign-in notice, and the alert that has the same words.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Notice {
    name: String,
    /// The line of the notice.
    line: String,
    /// What the alert says is wrong, in a line.
    title: String,
    /// From 7 days before the certificate runs out, or once it has.
    critical: bool,
    /// The error page of the alert, when it has one.
    id: Option<&'static str>,
}

/// The notices for the certificates from an authority, `failures` being why
/// each was not renewed at the last look, at time `now`: one for a revoked
/// certificate, one for one proved by hand with [`NOTICE_DAYS`] or fewer
/// left, and one for one whose renewal failed with [`FAILED_NOTICE_DAYS`]
/// or fewer left.
fn authority_notices(
    watched: &[Watched],
    failures: &BTreeMap<String, String>,
    now: i64,
) -> Vec<Notice> {
    let mut notices = Vec::new();
    for one in watched {
        let days = (one.not_after - now).div_euclid(86_400);
        let ends = util::local_time(one.not_after, "%F");
        let ran_out = one.not_after <= now;
        let verb = if ran_out { "ran out" } else { "runs out" };
        let by_hand = if one.by_hand { " --dns" } else { "" };
        let name = &one.name;
        let (line, title, critical, id) = if one.revoked {
            (
                format!(
                    "The certificate {name} was revoked: get a new one with `elevate cert get \
                     {name}{by_hand}`; the services keep using it until it is replaced."
                ),
                format!("The certificate {name} was revoked"),
                false,
                None,
            )
        } else if one.by_hand && days <= NOTICE_DAYS {
            (
                format!(
                    "The certificate {name} {verb} on {ends}: renew it with `elevate cert get \
                     {name} --dns`."
                ),
                format!("The certificate {name} {verb} on {ends}"),
                days <= CRITICAL_DAYS,
                ran_out.then_some(RAN_OUT),
            )
        } else if let Some(why) = failures.get(name).filter(|_| days <= FAILED_NOTICE_DAYS) {
            (
                format!("The certificate {name} could not be renewed: {why}. It {verb} on {ends}."),
                format!("The certificate {name} could not be renewed"),
                days <= CRITICAL_DAYS,
                Some(if ran_out { RAN_OUT } else { KEEPS_FAILING }),
            )
        } else {
            continue;
        };
        notices.push(Notice { name: name.clone(), line, title, critical, id });
    }
    notices
}

/// The notices for the certificates `ends`, each a name and the time it
/// runs out, at time `now`: one for each with [`NOTICE_DAYS`] or fewer left.
fn expiry_notices(ends: &[(String, i64)], now: i64) -> Vec<Notice> {
    let mut notices = Vec::new();
    for (name, not_after) in ends {
        let days = (not_after - now).div_euclid(86_400);
        if days > NOTICE_DAYS {
            continue;
        }
        let ran_out = *not_after <= now;
        let verb = if ran_out { "ran out" } else { "runs out" };
        let ends = util::local_time(*not_after, "%F");
        notices.push(Notice {
            name: name.clone(),
            line: format!(
                "The certificate {name} {verb} on {ends}: add its new version with `elevate cert \
                 add {name} --replace`."
            ),
            title: format!("The certificate {name} {verb} on {ends}"),
            critical: days <= CRITICAL_DAYS,
            id: ran_out.then_some(RAN_OUT),
        });
    }
    notices
}

/// The error ID of a certificate whose renewal keeps failing.
const KEEPS_FAILING: &str = "CRT-121";

/// The error ID of a certificate that has run out.
const RAN_OUT: &str = "CRT-122";

/// The error ID of a certificate that could not be got for a day.
const NOT_GOT_FOR_A_DAY: &str = "CRT-123";

/// The error ID of a certificate certd gave up getting.
const GAVE_UP: &str = "CRT-124";

/// The notice lines for the certificates from an authority: see
/// [`authority_notices`]; and how urgent they are together.
#[cfg(test)]
fn authority_lines(
    watched: &[Watched],
    failures: &BTreeMap<String, String>,
    now: i64,
) -> (Vec<String>, Urgency) {
    lines_of(authority_notices(watched, failures, now))
}

/// The notice lines for the certificates `ends`: see [`expiry_notices`];
/// and how urgent they are together.
#[cfg(test)]
fn expiry_lines(ends: &[(String, i64)], now: i64) -> (Vec<String>, Urgency) {
    lines_of(expiry_notices(ends, now))
}

/// The lines of `notices`, and Critical when any is.
#[cfg(test)]
fn lines_of(notices: Vec<Notice>) -> (Vec<String>, Urgency) {
    let urgency = if notices.iter().any(|notice| notice.critical) {
        Urgency::Critical
    } else {
        Urgency::Normal
    };
    (notices.into_iter().map(|notice| notice.line).collect(), urgency)
}

/// The alerts for `notices`: `certificate:NAME`, a warning, and critical
/// from 7 days or once the certificate has run out, with the notice's words
/// and the error ID of the page for what is wrong at the end. An alert alertd
/// would refuse, as for a name that is not a key, is left out.
fn alerts_of(notices: &[Notice]) -> Vec<Alert> {
    notices
        .iter()
        .filter_map(|notice| {
            let severity = if notice.critical { Severity::Critical } else { Severity::Warning };
            let detail = match notice.id {
                Some(id) => format!("{} ({id})", notice.line),
                None => notice.line.clone(),
            };
            let alert = Alert::new(
                format!("certificate:{}", notice.name),
                severity,
                &notice.title,
                &detail,
            )
            .about(About::Stopped);
            match alert.invalid() {
                None => Some(alert),
                Some(why) => {
                    eprintln!("<4>certd: no alert for {}: {why}", notice.name);
                    None
                }
            }
        })
        .collect()
}

/// `alerts`, and `wanted` after them but for those of a key `alerts` has: a
/// certificate that runs out says so before it says it could not be got.
fn with_wanted(mut alerts: Vec<Alert>, wanted: Vec<Alert>) -> Vec<Alert> {
    for alert in wanted {
        if !alerts.iter().any(|have| have.key == alert.key) {
            alerts.push(alert);
        }
    }
    alerts
}

/// The alerts for the wanted certificates `records`, at time `now`: one,
/// `certificate:NAME`, for each that has failed for a day and is still tried,
/// and `certificate-given-up:NAME` for each certd stopped trying; both
/// warnings, since alertd has no lesser. An alert alertd would refuse is left
/// out.
fn wanted_alerts(records: &[Record], now: i64) -> Vec<Alert> {
    let mut alerts = Vec::new();
    for record in records {
        let name = &record.order.name;
        let (key, title, hint) = if record.given_up {
            (
                format!("certificate-given-up:{name}"),
                format!("certd gave up getting the certificate {name}: {}", record.why),
                format!(
                    "Get it again with `elevate cert get {name}`; `elevate cert forget {name}` \
                     clears this. ({GAVE_UP})"
                ),
            )
        } else if record.has_failed_for_a_day(now) {
            (
                format!("certificate:{name}"),
                format!("The certificate {name} could not be had for a day: {}", record.why),
                format!(
                    "certd tries again until {}; `cert wanted` shows how the tries went and \
                     `elevate cert forget {name}` stops them. ({NOT_GOT_FOR_A_DAY})",
                    util::local_time(record.gives_up_at(), "%F")
                ),
            )
        } else {
            continue;
        };
        let alert = Alert::new(key, Severity::Warning, &title, &format!("{title}. {hint}"))
            .about(About::Stopped);
        match alert.invalid() {
            None => alerts.push(alert),
            Some(why) => eprintln!("<4>certd: no alert for {name}: {why}"),
        }
    }
    alerts
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

    fn watched(name: &str, left: i64, by_hand: bool, revoked: bool) -> Watched {
        Watched { name: name.into(), not_after: NOW + left, by_hand, revoked }
    }

    fn failures(list: &[(&str, &str)]) -> BTreeMap<String, String> {
        list.iter().map(|(name, why)| ((*name).to_string(), (*why).to_string())).collect()
    }

    #[test]
    fn a_renewal_that_failed_is_told_of_from_14_days_and_critical_from_7() {
        let failed = failures(&[("a.example", "port 80 is taken")]);
        let none = authority_lines(&[watched("a.example", 15 * DAY, false, false)], &failed, NOW);
        assert_eq!(none, (Vec::new(), Urgency::Normal));
        let (lines, urgency) =
            authority_lines(&[watched("a.example", 14 * DAY + 600, false, false)], &failed, NOW);
        assert_eq!(
            lines,
            [
                "The certificate a.example could not be renewed: port 80 is taken. It runs out on 2027-01-29."
            ]
        );
        assert_eq!(urgency, Urgency::Normal);
        let (_, urgency) =
            authority_lines(&[watched("a.example", 7 * DAY + 600, false, false)], &failed, NOW);
        assert_eq!(urgency, Urgency::Critical);
        let (lines, urgency) =
            authority_lines(&[watched("a.example", -DAY, false, false)], &failed, NOW);
        assert!(lines[0].ends_with("It ran out on 2027-01-14."), "{}", lines[0]);
        assert_eq!(urgency, Urgency::Critical);
    }

    #[test]
    fn a_certificate_that_renewed_or_has_not_been_tried_is_not_told_of() {
        let ones = [watched("a.example", 3 * DAY, false, false)];
        assert_eq!(authority_lines(&ones, &BTreeMap::new(), NOW), (Vec::new(), Urgency::Normal));
        let other = failures(&[("b.example", "x")]);
        assert_eq!(authority_lines(&ones, &other, NOW), (Vec::new(), Urgency::Normal));
    }

    #[test]
    fn a_certificate_proved_by_hand_is_told_of_at_21_and_7_days_as_one_added_is() {
        let ones = |left| [watched("a.example", left, true, false)];
        assert!(authority_lines(&ones(22 * DAY), &BTreeMap::new(), NOW).0.is_empty());
        let (lines, urgency) = authority_lines(&ones(21 * DAY + 600), &BTreeMap::new(), NOW);
        assert_eq!(
            lines,
            [
                "The certificate a.example runs out on 2027-02-05: renew it with `elevate cert get a.example --dns`."
            ]
        );
        assert_eq!(urgency, Urgency::Normal);
        assert_eq!(
            authority_lines(&ones(7 * DAY + 600), &BTreeMap::new(), NOW).1,
            Urgency::Critical
        );
    }

    #[test]
    fn a_revoked_certificate_is_told_of_whatever_time_it_has_left() {
        let (lines, urgency) = authority_lines(
            &[
                watched("a.example", 80 * DAY, false, true),
                watched("b.example", 80 * DAY, true, true),
            ],
            &BTreeMap::new(),
            NOW,
        );
        assert_eq!(
            lines,
            [
                "The certificate a.example was revoked: get a new one with `elevate cert get a.example`; the services keep using it until it is replaced.",
                "The certificate b.example was revoked: get a new one with `elevate cert get b.example --dns`; the services keep using it until it is replaced.",
            ]
        );
        assert_eq!(urgency, Urgency::Normal);
    }

    fn certd_at(scratch: &cert::fixtures::Scratch) -> Certd {
        Certd {
            store: Store::at(&scratch.0.join("certificates"), Duration::from_secs(1)),
            remade: None,
            waiting: false,
            asked: BTreeMap::new(),
            failed: BTreeMap::new(),
            wanted: Vec::new(),
            alertd_unreached: false,
        }
    }

    fn acme_with(renew_at: Option<i64>) -> Acme {
        Acme {
            directory: String::from("https://pebble.test:14000/dir"),
            names: vec![String::from("a.example")],
            challenge: Challenge::Http01,
            key: cert::store::KeyKind::EcdsaP256,
            profile: None,
            renew_at,
            revoked: false,
        }
    }

    #[test]
    fn a_time_that_is_kept_stands_until_the_authority_is_to_be_asked_again() {
        let scratch = cert::fixtures::Scratch::new("certd-kept");
        let mut certd = certd_at(&scratch);
        let life = (NOW - 60 * DAY, NOW + 30 * DAY);
        certd.asked.insert("a.example".into(), NOW + 3600);
        let kept = acme_with(Some(NOW + 11 * DAY));
        assert_eq!(certd.renewal_time("a.example", &kept, "", life, NOW), NOW + 11 * DAY);
        // Until the authority is to be asked again nothing is asked, even
        // for a time long past.
        let past = acme_with(Some(NOW - DAY));
        assert_eq!(certd.renewal_time("a.example", &past, "", life, NOW), NOW - DAY);
    }

    #[test]
    fn when_the_authority_cannot_be_asked_the_kept_time_or_the_third_rule_is_used() {
        let scratch = cert::fixtures::Scratch::new("certd-asked");
        let mut certd = certd_at(&scratch);
        let life = (NOW - 60 * DAY, NOW + 30 * DAY);
        // No account is kept: asking fails, and the third of its life decides.
        let fresh = acme_with(None);
        assert_eq!(certd.renewal_time("a.example", &fresh, "", life, NOW), NOW);
        // The failure is not asked again until the next look but one hour on.
        assert_eq!(certd.asked["a.example"], NOW + acme::LOOK_AGAIN.0);
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

    fn added(name: &str, left: i64) -> Vec<Alert> {
        alerts_of(&expiry_notices(&[(name.into(), NOW + left)], NOW))
    }

    #[test]
    fn an_added_certificate_alerts_from_21_days_and_critical_from_7() {
        assert!(added("a.example", 22 * DAY).is_empty());
        let warning = added("a.example", 21 * DAY + 600);
        assert_eq!(warning.len(), 1);
        assert_eq!(warning[0].key, "certificate:a.example");
        assert_eq!(warning[0].severity, Severity::Warning);
        assert_eq!(warning[0].kind(), Some(About::Stopped));
        assert_eq!(warning[0].title, "The certificate a.example runs out on 2027-02-05");
        assert_eq!(
            warning[0].detail,
            "The certificate a.example runs out on 2027-02-05: add its new version with \
             `elevate cert add a.example --replace`."
        );
        assert_eq!(added("a.example", 8 * DAY + 600)[0].severity, Severity::Warning);
        assert_eq!(added("a.example", 7 * DAY + 600)[0].severity, Severity::Critical);
        // Once it has run out the alert is critical and has CRT-122's page.
        let out = added("a.example", -DAY);
        assert_eq!(out[0].severity, Severity::Critical);
        assert!(out[0].detail.starts_with("The certificate a.example ran out on 2027-01-14:"));
        assert!(out[0].detail.ends_with("--replace`. (CRT-122)"), "{}", out[0].detail);
        assert!(added("a.example", 7 * DAY + 600)[0].detail.ends_with("--replace`."));
    }

    #[test]
    fn a_renewal_that_keeps_failing_alerts_from_14_days_with_the_notice_s_words() {
        let failed = failures(&[("a.example", "port 80 was closed (CRT-103)")]);
        let alerts = |left: i64| {
            alerts_of(&authority_notices(&[watched("a.example", left, false, false)], &failed, NOW))
        };
        assert!(alerts(15 * DAY).is_empty());
        let warning = alerts(14 * DAY + 600);
        assert_eq!(warning[0].severity, Severity::Warning);
        assert_eq!(warning[0].title, "The certificate a.example could not be renewed");
        assert_eq!(
            warning[0].detail,
            "The certificate a.example could not be renewed: port 80 was closed (CRT-103). It \
             runs out on 2027-01-29. (CRT-121)"
        );
        assert_eq!(alerts(7 * DAY + 600)[0].severity, Severity::Critical);
        let out = alerts(-DAY);
        assert_eq!(out[0].severity, Severity::Critical);
        assert!(out[0].detail.ends_with("ran out on 2027-01-14. (CRT-122)"), "{}", out[0].detail);
        // A certificate that renewed, or was replaced or removed, is no alert,
        // and `set` with what is left clears the rest.
        let renewed =
            authority_notices(&[watched("a.example", 5 * DAY, false, false)], &failures(&[]), NOW);
        assert!(alerts_of(&renewed).is_empty());
    }

    #[test]
    fn one_proved_by_hand_and_a_revoked_one_alert_as_their_notices_do() {
        let none = failures(&[]);
        let one = |left: i64, by_hand: bool, revoked: bool| {
            alerts_of(&authority_notices(
                &[watched("a.example", left, by_hand, revoked)],
                &none,
                NOW,
            ))
        };
        assert!(one(22 * DAY, true, false).is_empty());
        let hand = one(21 * DAY + 600, true, false);
        assert_eq!(hand[0].severity, Severity::Warning);
        assert!(hand[0].detail.ends_with("renew it with `elevate cert get a.example --dns`."));
        assert_eq!(one(6 * DAY, true, false)[0].severity, Severity::Critical);
        let revoked = one(60 * DAY, false, true);
        assert_eq!(revoked[0].severity, Severity::Warning);
        assert_eq!(revoked[0].title, "The certificate a.example was revoked");
        assert!(revoked[0].detail.contains("was revoked: get a new one"), "{}", revoked[0].detail);
    }

    #[test]
    fn the_alerts_and_the_notice_have_the_same_lines_in_the_same_order() {
        let ends = [
            (String::from("b.example"), NOW + 3 * DAY),
            (String::from("a.example"), NOW + 90 * DAY),
        ];
        let from_authority = [watched("c.example", 10 * DAY, true, false)];
        let mut notices = expiry_notices(&ends, NOW);
        notices.append(&mut authority_notices(&from_authority, &failures(&[]), NOW));
        let alerts = alerts_of(&notices);
        let keys: Vec<&str> = alerts.iter().map(|alert| alert.key.as_str()).collect();
        assert_eq!(keys, ["certificate:b.example", "certificate:c.example"]);
        for (alert, notice) in alerts.iter().zip(&notices) {
            assert!(alert.detail.starts_with(&notice.line));
        }
        // Nothing that needs attention is the empty list, which clears them all.
        assert!(alerts_of(&[]).is_empty());
    }

    fn order(name: &str) -> wanted::Order {
        wanted::Order {
            name: name.into(),
            names: vec![format!("{name}.example.org")],
            directory: String::from("https://pebble.example:14000/dir"),
            key: cert::store::KeyKind::EcdsaP256,
            profile: None,
            account: String::from("pebble.example-14000.json"),
            email: None,
            agree_terms: false,
        }
    }

    fn failed(name: &str) -> Record {
        Record::first(order(name), NOW, "port 80 is closed (CRT-103)", None)
    }

    #[test]
    fn a_certificate_that_could_not_be_got_alerts_from_a_day_and_not_before() {
        let record = failed("shop");
        assert!(wanted_alerts(std::slice::from_ref(&record), NOW + DAY - 1).is_empty());
        let alerts = wanted_alerts(std::slice::from_ref(&record), NOW + DAY);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].key, "certificate:shop");
        assert_eq!(alerts[0].severity, Severity::Warning);
        assert_eq!(alerts[0].kind(), Some(About::Stopped));
        assert_eq!(
            alerts[0].title,
            "The certificate shop could not be had for a day: port 80 is closed (CRT-103)"
        );
        assert_eq!(
            alerts[0].detail,
            "The certificate shop could not be had for a day: port 80 is closed (CRT-103). \
             certd tries again until 2027-01-29; `cert wanted` shows how the tries went and \
             `elevate cert forget shop` stops them. (CRT-123)"
        );
        // The same a week on, as long as it is tried.
        assert_eq!(wanted_alerts(&[record], NOW + 7 * DAY).len(), 1);
    }

    #[test]
    fn a_certificate_certd_gave_up_alerts_under_a_key_of_its_own() {
        let gave =
            failed("shop").gave_up(NOW + 14 * DAY, "the account at pebble.example was taken away");
        // Given up at once, a day has not passed and it is told all the same.
        let alerts = wanted_alerts(&[gave], NOW + 14 * DAY);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].key, "certificate-given-up:shop");
        assert_eq!(alerts[0].severity, Severity::Warning);
        assert_eq!(alerts[0].kind(), Some(About::Stopped));
        assert_eq!(
            alerts[0].title,
            "certd gave up getting the certificate shop: the account at pebble.example was taken away"
        );
        assert!(
            alerts[0].detail.ends_with(
                "Get it again with `elevate cert get shop`; `elevate cert forget shop` clears \
                 this. (CRT-124)"
            ),
            "{}",
            alerts[0].detail
        );
        let early = failed("shop").gave_up(NOW + 60, "x");
        assert_eq!(wanted_alerts(&[early], NOW + 60).len(), 1);
    }

    #[test]
    fn a_certificate_nothing_is_wanted_of_is_no_alert() {
        assert!(wanted_alerts(&[], NOW + 30 * DAY).is_empty());
        let bad = failed("Bad Name");
        assert!(wanted_alerts(&[bad], NOW + 2 * DAY).is_empty());
    }

    #[test]
    fn an_alert_of_the_certificate_running_out_comes_before_the_one_that_it_was_not_got() {
        let ends = alerts_of(&expiry_notices(&[(String::from("shop"), NOW + 3 * DAY)], NOW));
        let not_got = wanted_alerts(&[failed("shop"), failed("blog")], NOW + 2 * DAY);
        let both = with_wanted(ends, not_got);
        let keys: Vec<&str> = both.iter().map(|alert| alert.key.as_str()).collect();
        assert_eq!(keys, ["certificate:shop", "certificate:blog"]);
        assert!(both[0].title.contains("runs out"), "{}", both[0].title);
        assert!(with_wanted(Vec::new(), Vec::new()).is_empty());
    }

    #[test]
    fn certd_looks_when_the_soonest_try_is_due_if_that_is_before_its_next_look() {
        let scratch = cert::fixtures::Scratch::new("certd-wait");
        let mut certd = certd_at(&scratch);
        let twelve = Duration::from_hours(12);
        assert_eq!(certd.wait_for_wanted(twelve, NOW), twelve);
        certd.wanted = vec![
            failed("shop").failed_again(NOW + 600, "x", None),
            failed("blog").gave_up(NOW, "gone"),
        ];
        // Given up is never tried: the soonest is the one that is not.
        assert_eq!(certd.wait_for_wanted(twelve, NOW + 600), Duration::from_hours(1));
        assert_eq!(certd.wait_for_wanted(twelve, NOW + 600 + 1800), Duration::from_mins(30));
        // Late, certd looks at once, a second on.
        assert_eq!(certd.wait_for_wanted(twelve, NOW + 3 * DAY), Duration::from_secs(1));
        // A look that comes sooner stays.
        assert_eq!(certd.wait_for_wanted(FAILED_WAIT, NOW + 600), FAILED_WAIT);
    }

    fn keep(certd: &Certd, record: &Record) {
        let lock = certd.store.lock().unwrap();
        wanted::write(&certd.store, &lock, record).unwrap();
    }

    fn kept(certd: &Certd, name: &str) -> Option<Record> {
        let lock = certd.store.lock().unwrap();
        wanted::get(&certd.store, &lock, name).unwrap()
    }

    #[test]
    fn a_look_leaves_a_record_that_is_not_due_and_one_that_does_not_read_as_they_are() {
        let scratch = cert::fixtures::Scratch::new("certd-not-due");
        let mut certd = certd_at(&scratch);
        let shop = failed("shop");
        keep(&certd, &shop);
        let junk = certd.store.wanted().join("blog.toml");
        std::fs::write(&junk, "nonsense = = =").unwrap();
        assert!(certd.keep_wanted_certificates(NOW + 600));
        assert_eq!(certd.wanted, std::slice::from_ref(&shop));
        assert_eq!(kept(&certd, "shop"), Some(shop));
        assert_eq!(std::fs::read_to_string(&junk).unwrap(), "nonsense = = =");
        // With none, there is nothing to keep.
        let empty = cert::fixtures::Scratch::new("certd-no-records");
        let mut none = certd_at(&empty);
        assert!(none.keep_wanted_certificates(NOW));
        assert!(none.wanted.is_empty());
    }

    #[test]
    fn a_record_whose_account_was_taken_away_is_given_up_with_that_reason() {
        let scratch = cert::fixtures::Scratch::new("certd-no-account");
        let mut certd = certd_at(&scratch);
        keep(&certd, &failed("shop"));
        assert!(certd.keep_wanted_certificates(NOW + 3600));
        let gave = kept(&certd, "shop").unwrap();
        assert!(gave.given_up);
        assert_eq!(gave.by, By::Certd);
        assert_eq!(gave.last_ms, (NOW + 3600) * 1_000);
        assert!(
            gave.why.starts_with("the account at pebble.example was taken away"),
            "{}",
            gave.why
        );
        assert_eq!(certd.wanted, std::slice::from_ref(&gave));
        let entries = history::read(&certd.store).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!((entries[0].event, entries[0].by), (Event::Failed, By::Certd));
        assert!(
            entries[0].detail.starts_with("the account at pebble.example"),
            "{}",
            entries[0].detail
        );
        // Given up, it is not tried at the next look, nor given up again.
        assert!(certd.keep_wanted_certificates(NOW + 2 * 3600));
        assert_eq!(history::read(&certd.store).unwrap().len(), 1);
        assert_eq!(kept(&certd, "shop"), Some(gave));
    }

    #[test]
    fn what_a_try_found_is_not_kept_over_a_record_that_was_forgotten_or_made_again() {
        let scratch = cert::fixtures::Scratch::new("certd-keep");
        let certd = certd_at(&scratch);
        let read = failed("shop");
        keep(&certd, &read);
        let next = read.failed_again(NOW + 3600, "again", None);
        assert_eq!(certd.keep(&read, next.clone()), Some(next.clone()));
        assert_eq!(kept(&certd, "shop"), Some(next.clone()));
        // `cert get` ordered again meanwhile.
        let again = Record::first(order("shop"), NOW + 7200, "new", None);
        keep(&certd, &again);
        assert_eq!(certd.keep(&read, next), None);
        assert_eq!(kept(&certd, "shop"), Some(again));
        // `cert forget` took it away.
        let lock = certd.store.lock().unwrap();
        wanted::remove(&certd.store, &lock, "shop").unwrap();
        drop(lock);
        assert_eq!(certd.keep(&read, read.failed_again(NOW + 9000, "x", None)), None);
        assert_eq!(kept(&certd, "shop"), None);
    }

    #[test]
    fn a_name_alertd_would_refuse_is_left_out_not_sent() {
        let ends = [(String::from("Bad Name"), NOW + DAY)];
        assert!(alerts_of(&expiry_notices(&ends, NOW)).is_empty());
        let long = "a".repeat(130);
        assert!(alerts_of(&expiry_notices(&[(long, NOW + DAY)], NOW)).is_empty());
    }
}
