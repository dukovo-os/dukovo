// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What all of admind's connections share (ADR 0514): the sessions, the
//! counts that keep it usable, the certificate it serves, the machine's
//! names, and the console's files. admind runs on one thread, so the
//! parts are `RefCell`s in an `Rc`; none is borrowed across an `await`,
//! so that two connections never meet in one.
//!
//! This is also where sessions end. Whatever ends one (sign-out, the
//! hours or idle time of the sign-in rules, its worker going, admind
//! stopping) gives it to [`Shared::finish`], which notes why, closes the
//! worker's link, which ends the worker, and tells authd.

use std::cell::RefCell;
use std::net::IpAddr;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use accounts::console::Console;
use tokio::task::JoinHandle;

use super::authd;
use super::files::Files;
use super::limits::{Connections, Endings, Quiet, Tickets, Tries};
use super::link::Link;
use super::log;
use super::reach::{self, Network};
use super::sessions::{Session, Sessions};
use super::tls::Certificate;
use crate::admin_settings::Settings;
use crate::cert::machine;
use crate::util;

/// The machine's names and addresses as admind knows them now.
#[derive(Debug, PartialEq, Eq)]
pub struct Identity {
    /// The machine's name.
    pub host: String,
    /// The names a request's `Host` may be: the machine's name, that name
    /// with `.local`, `localhost`, and the certificate's.
    pub names: Vec<String>,
    /// The addresses a request's `Host` may be: the cards', and the
    /// certificate's.
    pub addresses: Vec<IpAddr>,
    /// The networks of the machine's cards, which `reach` answers.
    pub networks: Vec<Network>,
}

impl Identity {
    /// What a machine called `host`, with cards on `networks` and a
    /// certificate for `certificate`, answers to.
    pub fn new(host: &str, networks: Vec<Network>, certificate: &[String]) -> Identity {
        let mut names = vec![host.to_string(), format!("{host}.local"), String::from("localhost")];
        let mut addresses: Vec<IpAddr> =
            networks.iter().map(|network| reach::plain(network.ip)).collect();
        for name in certificate {
            match name.parse::<IpAddr>() {
                Ok(address) => addresses.push(reach::plain(address)),
                Err(_) if names.iter().any(|known| known.eq_ignore_ascii_case(name)) => {}
                Err(_) => names.push(name.clone()),
            }
        }
        Identity { host: host.to_string(), names, addresses, networks }
    }

    /// What this machine answers to now.
    pub fn read(certificate: &[String]) -> Identity {
        Identity::new(&util::hostname(), reach::own_networks(), certificate)
    }
}

/// Seconds since 1970.
pub fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| {
        i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
    })
}

/// Why a session that `now` finds ended did: it ran its hours, or it was
/// left with no page open.
pub fn ending_reason(started: Instant, now: Instant, lasts: Duration) -> &'static str {
    if now.saturating_duration_since(started) >= lasts { "expired" } else { "idle" }
}

/// Tells authd that its session `authd` ended. authd has forgotten a
/// session whose worker ended, and then says so, which is no trouble.
pub async fn tell_end(authd: String) {
    if let Err(why) = authd::ask(Console::End { session: authd }, &[]).await {
        log(4, &why);
    }
}

/// Everything the connections share.
pub struct Shared {
    /// The port admind listens on, which a restart changes.
    pub port: u16,
    /// admin.toml as last read.
    pub settings: RefCell<Settings>,
    /// The machine's names and addresses.
    pub identity: RefCell<Identity>,
    /// The certificate served now.
    pub certificate: RefCell<Certificate>,
    /// The console's files.
    pub files: Files,
    /// The sessions signed in.
    pub sessions: RefCell<Sessions<Link>>,
    /// The sign-ins waiting for a second step.
    pub tickets: RefCell<Tickets>,
    /// The sign-in requests of each address.
    pub tries: RefCell<Tries>,
    /// What the log has said of each refused address.
    pub quiet: RefCell<Quiet>,
    /// The connections held.
    pub connections: Rc<RefCell<Connections>>,
    /// Why sessions ended.
    pub endings: RefCell<Endings>,
    /// The elevated commands of the test route running now.
    #[cfg(feature = "lab")]
    pub elevating: std::cell::Cell<usize>,
}

impl Shared {
    /// What admind starts with.
    pub fn new(
        settings: Settings,
        identity: Identity,
        certificate: Certificate,
        files: Files,
    ) -> Rc<Shared> {
        Rc::new(Shared {
            port: settings.console.listen,
            settings: RefCell::new(settings),
            identity: RefCell::new(identity),
            certificate: RefCell::new(certificate),
            files,
            sessions: RefCell::new(Sessions::default()),
            tickets: RefCell::default(),
            tries: RefCell::default(),
            quiet: RefCell::default(),
            connections: Rc::default(),
            endings: RefCell::default(),
            #[cfg(feature = "lab")]
            elevating: std::cell::Cell::new(0),
        })
    }

    /// Whether admind serves an authority's certificate, and so asks
    /// browsers to come back over HTTPS alone: `this-machine` is its own.
    pub fn authority(&self) -> bool {
        self.settings.borrow().console.certificate != machine::NAME
    }

    /// Runs `with` on the session of `cookie`, if there is one.
    pub fn with_session<T>(
        &self,
        cookie: &str,
        with: impl FnOnce(&mut Session<Link>) -> T,
    ) -> Option<T> {
        self.sessions.borrow_mut().get(cookie).map(with)
    }

    /// Ends `session` for `reason`, one of the words the page is told:
    /// `signed-out`, `expired`, `idle`, `restarted` or `worker`. Its link
    /// closes, which ends its worker, and the task returned tells authd;
    /// most callers let it go.
    pub fn finish(&self, session: Session<Link>, reason: &'static str) -> JoinHandle<()> {
        log(6, &format!("{}'s console session from {} ended: {reason}", session.name, session.from));
        self.endings.borrow_mut().note(&session.authd, reason);
        let authd = session.authd.clone();
        drop(session);
        tokio::task::spawn_local(tell_end(authd))
    }

    /// Ends the session `authd` once its worker's `link` is closed.
    ///
    /// # Panics
    ///
    /// Outside a `LocalSet`.
    pub fn watch(self: &Rc<Shared>, link: &Link, authd: String) {
        let (shared, closed) = (Rc::clone(self), link.closed());
        tokio::task::spawn_local(async move {
            closed.await;
            let gone = shared.sessions.borrow_mut().take_where(|session| session.authd == authd);
            for session in gone {
                drop(shared.finish(session, "worker"));
            }
        });
    }

    /// What is done every 30 seconds: ends the sessions whose time is up
    /// by the sign-in rules, and forgets what is old.
    pub fn maintain(&self, now: Instant) {
        let rules = self.settings.borrow().sign_in;
        let lasts = Duration::from_hours(u64::from(rules.session_hours));
        let idle = Duration::from_mins(u64::from(rules.idle_minutes));
        let ended = self.sessions.borrow_mut().take_ended(now, lasts, idle);
        for session in ended {
            let reason = ending_reason(session.started, now, lasts);
            drop(self.finish(session, reason));
        }
        self.tries.borrow_mut().tidy(now);
        self.tickets.borrow_mut().tidy(now);
        self.quiet.borrow_mut().tidy(now);
    }

    /// Ends every session, as admind stops, waiting a moment for authd to
    /// be told.
    pub async fn end_all(&self) {
        let all = self.sessions.borrow_mut().take_where(|_| true);
        let told: Vec<_> = all.into_iter().map(|session| self.finish(session, "restarted")).collect();
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            for task in told {
                let _ = task.await;
            }
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn a_machine_answers_to_its_name_the_certificates_and_its_cards() {
        let cards = vec![
            Network { ip: ip("192.168.1.10"), prefix: 24 },
            Network { ip: ip("fd00::10"), prefix: 64 },
        ];
        let certificate = vec!["duke-nas.local".to_string(), "admin.example.org".into(), "10.0.0.5".into()];
        let identity = Identity::new("duke-nas", cards.clone(), &certificate);
        assert_eq!(identity.host, "duke-nas");
        assert_eq!(
            identity.names,
            ["duke-nas", "duke-nas.local", "localhost", "admin.example.org"]
        );
        assert_eq!(identity.addresses, [ip("192.168.1.10"), ip("fd00::10"), ip("10.0.0.5")]);
        assert_eq!(identity.networks, cards);
    }

    #[test]
    fn a_session_that_ran_its_hours_expired_and_any_other_was_idle() {
        let started = Instant::now();
        let lasts = Duration::from_hours(12);
        assert_eq!(ending_reason(started, started + lasts, lasts), "expired");
        assert_eq!(ending_reason(started, started + lasts + Duration::from_mins(1), lasts), "expired");
        assert_eq!(ending_reason(started, started + Duration::from_mins(31), lasts), "idle");
    }

    #[test]
    fn the_clock_is_in_seconds_since_1970() {
        assert!(unix_now() > 1_790_000_000);
    }
}
