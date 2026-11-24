// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What all of admind's connections share (ADR 0514): the sessions, the
//! counts that keep it usable, the certificate it serves, the machine's
//! names, the console's files, and the store of the journal and the tasks
//! (ADR 0516). admind runs on one thread, so the parts are `RefCell`s in
//! an `Rc`; none is borrowed across an `await`, so that two connections
//! never meet in one.
//!
//! This is also where sessions end. Whatever ends one (sign-out, the
//! hours or idle time of the sign-in rules, its worker going, admind
//! stopping) gives it to [`Shared::finish`], which notes why, closes the
//! worker's link, which ends the worker, and tells authd. A tab's event
//! stream learns of it, and of an unlock that changed, from
//! [`Shared::changes`].

use std::cell::{Cell, RefCell};
use std::net::IpAddr;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use accounts::console::Console;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::actions::{self, Registered};
use super::authd;
use super::caller::TokenWorkers;
use super::errors::{Failure, Kind};
use super::files::Files;
use super::follow::Follows;
use super::journal::{Journal, SETTINGS};
use super::limits::{Connections, Endings, Quiet, Tickets, Tries};
use super::link::Link;
use super::log;
use super::reach::{self, Network};
use super::sessions::{Session, Sessions};
use super::store::Store;
use super::tasks::Tasks;
use super::terminal::Terminals;
use super::terminal::protocol::Why;
use super::tls::Certificate;
use super::trials::{self, Power, Trials};
use super::websocket::Code;
use super::who::Sources;
use crate::admin_settings::Settings;
use crate::cert::machine;
use crate::util;

/// How long the machine's names and addresses are taken as current
/// before a request for another host has them read again.
pub const IDENTITY_FRESH: Duration = Duration::from_secs(1);

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
    /// certificate for `certificate`, answers to: the names its own
    /// certificate holds ([`machine::names`]), whichever certificate it
    /// serves, and the served certificate's.
    pub fn new(host: &str, networks: Vec<Network>, certificate: &[String]) -> Identity {
        let mut names = machine::names(host);
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
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
}

/// Why a session that `now` finds ended did: it ran its hours, or it was
/// left with no page open.
pub fn ending_reason(started: Instant, now: Instant, lasts: Duration) -> &'static str {
    if now.saturating_duration_since(started) >= lasts { "expired" } else { "idle" }
}

/// The seconds left of an unlock that ends at Unix time `until`, at Unix
/// time `now`; `None` when there is none or it is over.
pub fn seconds_left(until: Option<i64>, now: i64) -> Option<i64> {
    until.map(|until| until.saturating_sub(now)).filter(|left| *left > 0)
}

/// Tells the authd at `socket` that its session `session` ended, and who
/// ended it from the Accounts page, if someone did. authd has forgotten a
/// session whose worker ended, and then says so, which is no trouble.
async fn tell_end(socket: String, session: String, by: Option<String>) {
    if let Err(why) = authd::ask(&socket, Console::End { session, by }, &[]).await {
        log(4, &why);
    }
}

/// Writes the log line for a refusal that `table` says is due: `why`,
/// then `detail` when there is one.
fn say_refusal(
    table: &RefCell<Quiet>,
    what: &str,
    address: IpAddr,
    why: &'static str,
    detail: &str,
) {
    let Some(count) = table.borrow_mut().note(address, why, Instant::now()) else { return };
    let detail = if detail.is_empty() { String::new() } else { format!(": {detail}") };
    let times = if count > 1 { format!(" ({count} since the last line)") } else { String::new() };
    log(6, &format!("refused {what} from {address}: {why}{detail}{times}"));
}

/// Everything the connections share.
pub struct Shared {
    /// The port admind listens on, which a restart changes.
    pub port: u16,
    /// authd's socket.
    pub authd: String,
    /// admin.toml as last read.
    pub settings: RefCell<Settings>,
    /// The machine's names and addresses.
    pub identity: RefCell<Identity>,
    /// The certificate served now.
    pub certificate: RefCell<Certificate>,
    /// The console's files.
    pub files: Files,
    /// Where the account files are, which admind reads for what a person
    /// is.
    pub accounts: Sources,
    /// The sessions signed in.
    pub sessions: RefCell<Sessions<Link>>,
    /// The sign-ins waiting for a second step.
    pub tickets: RefCell<Tickets>,
    /// The sign-in requests of each address.
    pub tries: RefCell<Tries>,
    /// What the log has said of each address whose connections were
    /// refused, for each reason.
    pub quiet: RefCell<Quiet>,
    /// What it has said of each address whose requests were refused, for
    /// each kind of error.
    pub refused: RefCell<Quiet>,
    /// The connections held.
    pub connections: Rc<RefCell<Connections>>,
    /// Why sessions ended.
    pub endings: RefCell<Endings>,
    /// Sent to when a session ends, its unlock changes or the running
    /// trials are told ([`Shared::tell_trials`]), which the event streams
    /// of the session's tabs wait for.
    pub changes: watch::Sender<()>,
    /// The wrong API tokens each address gave in the last minute.
    pub wrong_tokens: RefCell<Tries>,
    /// The workers of the API tokens and the MCP clients in use.
    pub token_workers: RefCell<TokenWorkers>,
    /// What each MCP client has running (ADR 0690).
    pub client_calls: RefCell<super::mcp::client::Calls>,
    /// What MCP clients have asked to change, until an administrator
    /// answers.
    pub client_requests: RefCell<super::mcp::requests::Requests>,
    /// What MCP clients did, for the page.
    pub client_history: RefCell<super::mcp::history::History>,
    /// How long a client's `ask` waits for an administrator before it
    /// answers that the request waits: [`tools::ASK_WAIT`], or a test's
    /// own.
    ///
    /// [`tools::ASK_WAIT`]: super::mcp::tools::ASK_WAIT
    pub ask_wait: Cell<Duration>,
    /// The elevated commands running now, of every session and token.
    pub elevating: Cell<usize>,
    /// The actions the API offers: [`actions::offered`], or a test's own.
    pub actions: Cell<&'static [&'static dyn Registered]>,
    /// The store of the journal and the tasks; none when it would not
    /// open, which the log says.
    pub store: Option<Store>,
    /// What the journal keeps in memory.
    pub journal: Journal,
    /// The tasks that wait, run or ended lately.
    pub tasks: Tasks,
    /// The console's terminals, their panes and their sockets.
    pub terminals: RefCell<Terminals>,
    /// The follows of the log that run (ADR 0518).
    pub follows: RefCell<Follows>,
    /// The trials of settings, as admind knows them (ADR 0518).
    pub trials: RefCell<Trials>,
    /// Spark's read socket: [`trials::READ`], or a test's own.
    pub spark: Cell<&'static str>,
    /// The restart or shut down that is set, as Spark last said
    /// (ADR 0565).
    pub power: RefCell<Power>,
    /// The command that says whether the search for a bad change runs,
    /// which a restart is refused during: [`SEARCH`], or a test's own.
    pub search: Cell<&'static [&'static str]>,
    /// The one-time links to reports' texts (ADR 0568).
    pub once: super::once::Links,
    /// How the machine's names and addresses are read, given the
    /// certificate's names: [`Identity::read`], or a test's own.
    pub reads_identity: Cell<fn(&[String]) -> Identity>,
    /// When [`Shared::identity`] was last read.
    pub identity_read: Cell<Instant>,
}

/// `changes bisect --status --json`, which says whether the search for a
/// bad change runs (`running`). It runs as the caller's administrator:
/// the search's state is not admind's to read.
pub const SEARCH: &[&str] = &["changes", "bisect", "--status", "--json"];

impl Shared {
    /// What admind starts with, asking the authd at `authd`.
    pub fn new(
        settings: Settings,
        identity: Identity,
        certificate: Certificate,
        files: Files,
        authd: &str,
        store: Option<Store>,
    ) -> Rc<Shared> {
        Rc::new(Shared {
            port: settings.console.listen,
            authd: authd.to_string(),
            settings: RefCell::new(settings),
            identity: RefCell::new(identity),
            certificate: RefCell::new(certificate),
            files,
            accounts: Sources::default(),
            sessions: RefCell::new(Sessions::default()),
            tickets: RefCell::default(),
            tries: RefCell::default(),
            quiet: RefCell::default(),
            refused: RefCell::default(),
            connections: Rc::default(),
            endings: RefCell::default(),
            changes: watch::channel(()).0,
            wrong_tokens: RefCell::default(),
            token_workers: RefCell::default(),
            client_calls: RefCell::default(),
            client_requests: RefCell::default(),
            client_history: RefCell::default(),
            ask_wait: Cell::new(super::mcp::tools::ASK_WAIT),
            elevating: Cell::new(0),
            actions: Cell::new(actions::offered()),
            store,
            journal: Journal::new(Path::new(SETTINGS)),
            tasks: Tasks::new(),
            terminals: RefCell::default(),
            follows: RefCell::default(),
            trials: RefCell::default(),
            spark: Cell::new(trials::READ),
            power: RefCell::default(),
            search: Cell::new(SEARCH),
            once: super::once::Links::default(),
            reads_identity: Cell::new(Identity::read),
            identity_read: Cell::new(Instant::now()),
        })
    }

    /// Reads the machine's names and addresses again, for the
    /// certificate's names.
    pub fn read_identity(&self) {
        let names = self.certificate.borrow().names.clone();
        *self.identity.borrow_mut() = (self.reads_identity.get())(&names);
        self.identity_read.set(Instant::now());
    }

    /// Reads the machine's names and addresses again for a request whose
    /// host is none of them, unless they were read in the last
    /// [`IDENTITY_FRESH`], and says whether it did. A card takes its
    /// address after admind starts, as DHCP gives one a few seconds into a
    /// start, and a console opened by that address is answered at once,
    /// not at the next upkeep. The limit keeps requests for other hosts
    /// from reading them on each request.
    pub fn read_identity_for_miss(&self) -> bool {
        if self.identity_read.get().elapsed() < IDENTITY_FRESH {
            return false;
        }
        self.read_identity();
        true
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

    /// Notes that the session of `cookie` is unlocked until the Unix time
    /// `until`, or locked, as authd answered, and tells its tabs.
    pub fn set_unlocked(&self, cookie: &str, until: Option<i64>) {
        let set = self.with_session(cookie, |session| {
            session.unlocked_until = until;
            session.authd.clone()
        });
        let Some(authd) = set else { return };
        self.changes.send_replace(());
        // The administrators' panes end with the unlock, here as authd
        // ends their shells.
        if until.is_none_or(|until| until <= unix_now()) {
            self.terminals.borrow_mut().end_admin_panes(&authd, Why::Locked);
        }
    }

    /// Tells every open tab, of every session, which trials run at `now`,
    /// or that none does: `event: trial` (`routes::events`).
    pub fn tell_trials(&self, now: Instant) {
        self.trials.borrow_mut().tell(now);
        self.changes.send_replace(());
    }

    /// Ends `session` for `reason`, one of the words the page is told:
    /// `signed-out`, `expired`, `idle`, `restarted` or `worker`. Its link
    /// closes, which ends its worker, and the task returned tells authd;
    /// most callers let it go.
    ///
    /// # Panics
    ///
    /// Outside a `LocalSet`.
    pub fn finish(&self, session: Session<Link>, reason: &'static str) -> JoinHandle<()> {
        self.finish_by(session, reason, None)
    }

    /// The same, for a session that account `by` ended from the Accounts
    /// page (ADR 0562), with `reason` `by-administrator` or `by-you`:
    /// authd is told who, for its audit line and the person's next
    /// sign-in.
    pub fn finish_by(
        &self,
        session: Session<Link>,
        reason: &'static str,
        by: Option<&str>,
    ) -> JoinHandle<()> {
        log(
            6,
            &format!("{}'s console session from {} ended: {reason}", session.name, session.from),
        );
        self.endings.borrow_mut().note(&session.authd, session.key, reason);
        let authd = session.authd.clone();
        drop(session);
        let (code, why) = match reason {
            "signed-out" | "by-you" => (Code::SessionEnded, Why::SignedOut),
            "by-administrator" => (Code::SessionEnded, Why::Ended),
            "restarted" => (Code::Stopping, Why::Restart),
            _ => (Code::SessionEnded, Why::Idle),
        };
        let mut terminals = self.terminals.borrow_mut();
        terminals.close_session(&authd, code);
        // Held until authd has ended their shells, for its audit.
        let panes = terminals.close_admin_panes(&authd, why);
        drop(terminals);
        self.changes.send_replace(());
        let told = tell_end(self.authd.clone(), authd, by.map(String::from));
        tokio::task::spawn_local(async move {
            told.await;
            drop(panes);
        })
    }

    /// Keeps `session` and gives the cookie that names it. When the
    /// console holds as many sessions as it may, the session ends again.
    ///
    /// # Errors
    ///
    /// With [`Kind::Busy`], when it holds no more.
    ///
    /// # Panics
    ///
    /// Outside a `LocalSet`.
    pub fn admit(&self, session: Session<Link>) -> Result<String, Failure> {
        let authd = session.authd.clone();
        let added = self.sessions.borrow_mut().add(session);
        added.map_err(|why| {
            tokio::task::spawn_local(tell_end(self.authd.clone(), authd, None));
            Failure::new(Kind::Busy, why)
        })
    }

    /// Says in the log that a connection from `address` was refused for
    /// `why` (and `detail`, which may be empty), the first time and then
    /// once a minute with how many came for that reason.
    pub fn refused_connection(&self, address: IpAddr, why: &'static str, detail: &str) {
        say_refusal(&self.quiet, "a connection", address, why, detail);
    }

    /// Says the same of a request, which was answered with an error of
    /// kind `id`.
    pub fn refused_request(&self, address: IpAddr, id: &'static str, detail: &str) {
        say_refusal(&self.refused, "a request", address, id, detail);
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
        self.wrong_tokens.borrow_mut().tidy(now);
        self.tickets.borrow_mut().tidy(now);
        self.quiet.borrow_mut().tidy(now);
        self.refused.borrow_mut().tidy(now);
        self.token_workers.borrow_mut().tidy(now);
        super::mcp::upkeep(self, now);
    }

    /// What is looked at every moment the server wakes: the terminals'
    /// [`Terminals::tick`], with the sessions unlocked now.
    pub fn look_at_terminals(&self) {
        let now = unix_now();
        let unlocked = |authd: &str| self.unlocked_at(authd, now);
        self.terminals.borrow_mut().tick((Instant::now(), now), unlocked);
    }

    /// Whether the console session that authd names `authd` is unlocked at
    /// the Unix time `now`.
    pub fn unlocked_at(&self, authd: &str, now: i64) -> bool {
        self.sessions.borrow().any(|session| {
            session.authd == authd && session.unlocked_until.is_some_and(|until| until > now)
        })
    }

    /// Ends every session, as admind stops, waiting a moment for authd to
    /// be told. The terminals end first.
    pub async fn end_all(&self) {
        self.terminals.borrow_mut().end_all();
        self.follows.borrow_mut().stop_all();
        if self.sessions.borrow().is_empty() {
            return;
        }
        let all = self.sessions.borrow_mut().take_where(|_| true);
        let told: Vec<_> =
            all.into_iter().map(|session| self.finish(session, "restarted")).collect();
        let waiting = async {
            for task in told {
                // A task that failed told no one, which authd finds out
                // for itself when the session's worker ends.
                if task.await.is_err() {
                    log(4, "a session's end was not told to authd");
                }
            }
        };
        if tokio::time::timeout(Duration::from_secs(5), waiting).await.is_err() {
            log(4, "authd was not told of every session before admind stopped");
        }
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
        let certificate =
            vec!["duke-nas.local".to_string(), "admin.example.org".into(), "10.0.0.5".into()];
        let identity = Identity::new("duke-nas", cards.clone(), &certificate);
        assert_eq!(identity.host, "duke-nas");
        assert_eq!(
            identity.names,
            ["duke-nas", "duke-nas.local", "localhost", "admin.example.org"]
        );
        assert_eq!(identity.addresses, [ip("192.168.1.10"), ip("fd00::10"), ip("10.0.0.5")]);
        assert_eq!(identity.networks, cards);
        // With a domain, the first word and its `.local` name too, whose
        // own certificate is another's.
        let identity = Identity::new("duke-nas.example.org", cards, &["shop.example.org".into()]);
        assert_eq!(
            identity.names,
            ["duke-nas.example.org", "duke-nas", "duke-nas.local", "localhost", "shop.example.org"]
        );
    }

    #[test]
    fn a_session_that_ran_its_hours_expired_and_any_other_was_idle() {
        let started = Instant::now();
        let lasts = Duration::from_hours(12);
        assert_eq!(ending_reason(started, started + lasts, lasts), "expired");
        assert_eq!(
            ending_reason(started, started + lasts + Duration::from_mins(1), lasts),
            "expired"
        );
        assert_eq!(ending_reason(started, started + Duration::from_mins(31), lasts), "idle");
    }

    #[test]
    fn the_clock_is_in_seconds_since_1970() {
        assert!(unix_now() > 1_790_000_000);
    }
}
