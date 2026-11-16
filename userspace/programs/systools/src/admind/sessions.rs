// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The console's sessions (ADR 0514), in admind's memory alone: an
//! admind that starts again has signed everyone out. A browser holds its
//! session's cookie, 32 random bytes; admind keeps their SHA-256, so its
//! memory holds nothing a browser could send. A session ends at the
//! sign-in rules' `session-hours` after its sign-in, `idle-minutes` after
//! its last tab closed, at sign-out, or when its worker ends.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};

/// The most sessions at once, as authd keeps.
pub const SESSIONS_MAX: usize = 64;

/// A session, with `W`, the link to its worker.
pub struct Session<W> {
    /// The account.
    pub name: String,
    /// authd's name for the session.
    pub authd: String,
    /// The name the Accounts page shows and ends it by (ADR 0562): 16
    /// random hexadecimal digits, which open nothing. Neither the cookie
    /// nor authd's name is ever shown to another session.
    pub id: String,
    /// The person's words for the browser, as the sign-in found them in
    /// its `User-Agent`, such as `Firefox on Linux`.
    pub browser: String,
    /// The browser's address at sign-in.
    pub from: IpAddr,
    pub started: Instant,
    /// When it signed in, in Unix milliseconds, for the Accounts page.
    pub since_ms: i64,
    /// The last request, or the last moment a tab's event stream was open.
    pub seen: Instant,
    /// The tabs whose event streams are open now.
    pub tabs: u32,
    /// When the last tab's stream closed, while none is open: what locks
    /// an unlocked session a minute on. A request is no tab, and does not
    /// move it.
    pub closed: Option<Instant>,
    /// The token a request that changes anything carries.
    pub csrf: [u8; 32],
    /// Until when it is unlocked, in Unix seconds, as authd answered.
    pub unlocked_until: Option<i64>,
    /// Whether it may only add a second step.
    pub needs_step: bool,
    pub worker: W,
    /// The SHA-256 of its cookie, which `Sessions` keeps it by: set when
    /// it is kept, so that the reason it ends can be found from its cookie.
    pub key: [u8; 32],
}

impl<W> Session<W> {
    /// A session of `name`'s, signed in now from `from` with `browser`,
    /// as authd started it.
    pub fn new(
        name: String,
        authd: String,
        (from, browser): (IpAddr, String),
        needs_step: bool,
        worker: W,
    ) -> Result<Session<W>, String> {
        let now = Instant::now();
        let csrf = random::<32>()?;
        let id = random::<8>()?.iter().fold(String::with_capacity(16), |mut id, byte| {
            let _ = write!(id, "{byte:02x}");
            id
        });
        Ok(Session {
            name,
            authd,
            id,
            browser,
            from,
            started: now,
            since_ms: unix_ms(),
            seen: now,
            tabs: 0,
            closed: None,
            csrf,
            unlocked_until: None,
            needs_step,
            worker,
            key: [0; 32],
        })
    }

    /// Whether it has ended by `now`, with sessions lasting `lasts` and
    /// `idle` with no tab open.
    pub fn ended(&self, now: Instant, lasts: Duration, idle: Duration) -> bool {
        now.saturating_duration_since(self.started) >= lasts
            || (self.tabs == 0 && now.saturating_duration_since(self.seen) >= idle)
    }

    /// Its CSRF token, as the page reads it.
    pub fn csrf_text(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.csrf)
    }
}

/// The sessions, by the SHA-256 of their cookies.
pub struct Sessions<W> {
    map: HashMap<[u8; 32], Session<W>>,
}

impl<W> Default for Sessions<W> {
    fn default() -> Sessions<W> {
        Sessions { map: HashMap::new() }
    }
}

/// The key a cookie is kept by: the SHA-256 of its 32 bytes, or `None`
/// for a cookie admind did not make.
pub fn key(cookie: &str) -> Option<[u8; 32]> {
    let bytes = URL_SAFE_NO_PAD.decode(cookie).ok().filter(|bytes| bytes.len() == 32)?;
    let mut key = [0u8; 32];
    key.copy_from_slice(digest::digest(&digest::SHA256, &bytes).as_ref());
    Some(key)
}

impl<W> Sessions<W> {
    /// Keeps `session`: its cookie, for the browser.
    pub fn add(&mut self, mut session: Session<W>) -> Result<String, String> {
        if self.map.len() >= SESSIONS_MAX {
            return Err(format!("{SESSIONS_MAX} sessions are signed in already"));
        }
        let cookie = URL_SAFE_NO_PAD.encode(random::<32>()?);
        let key = key(&cookie).ok_or("a cookie made does not read")?;
        session.key = key;
        self.map.insert(key, session);
        Ok(cookie)
    }

    /// The session of `cookie`.
    pub fn get(&mut self, cookie: &str) -> Option<&mut Session<W>> {
        self.map.get_mut(&key(cookie)?)
    }

    /// Takes the session of `cookie` out, as it signs out.
    pub fn remove(&mut self, cookie: &str) -> Option<Session<W>> {
        self.map.remove(&key(cookie)?)
    }

    /// Takes out each session that has ended by `now`.
    pub fn take_ended(&mut self, now: Instant, lasts: Duration, idle: Duration) -> Vec<Session<W>> {
        let ended: Vec<[u8; 32]> =
            self.map.iter().filter(|(_, s)| s.ended(now, lasts, idle)).map(|(k, _)| *k).collect();
        ended.iter().filter_map(|key| self.map.remove(key)).collect()
    }

    /// Takes out each session that `keep` does not keep.
    pub fn take_where(&mut self, gone: impl Fn(&Session<W>) -> bool) -> Vec<Session<W>> {
        let ended: Vec<[u8; 32]> =
            self.map.iter().filter(|(_, s)| gone(s)).map(|(k, _)| *k).collect();
        ended.iter().filter_map(|key| self.map.remove(key)).collect()
    }

    /// The sessions of account `name`.
    pub fn of_account<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Session<W>> {
        self.map.values().filter(move |session| session.name == name)
    }

    /// Takes out the session of account `name` whose [`Session::id`] is
    /// `id`, as an administrator or the account ends it.
    pub fn take_id(&mut self, name: &str, id: &str) -> Option<Session<W>> {
        let key = self.map.iter().find(|(_, s)| s.name == name && s.id == id).map(|(k, _)| *k)?;
        self.map.remove(&key)
    }

    /// Whether a session `which` picks is signed in.
    pub fn any(&self, which: impl Fn(&Session<W>) -> bool) -> bool {
        self.map.values().any(which)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// The Unix time now, in milliseconds.
pub fn unix_ms() -> i64 {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    since.map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

/// The Unix time in milliseconds of `at`, a moment before `now`, which is
/// `now_ms`.
pub fn ms_at(at: Instant, now: Instant, now_ms: i64) -> i64 {
    let ago = i64::try_from(now.saturating_duration_since(at).as_millis()).unwrap_or(i64::MAX);
    now_ms.saturating_sub(ago)
}

/// `N` bytes from the random source.
pub fn random<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0u8; N];
    SystemRandom::new().fill(&mut bytes).map_err(|_| String::from("no random bytes to be had"))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(name: &str) -> Session<()> {
        let from = ("192.0.2.7".parse().unwrap(), "Firefox on Linux".into());
        Session::new(name.into(), "3f9a".into(), from, false, ()).unwrap()
    }

    #[test]
    fn a_session_is_found_by_its_cookie_alone() {
        let mut sessions = Sessions::default();
        let cookie = sessions.add(session("chase")).unwrap();
        assert_eq!(cookie.len(), 43);
        assert_eq!(sessions.get(&cookie).map(|s| s.name.clone()), Some("chase".into()));
        assert!(sessions.get("AAAA").is_none());
        assert!(sessions.get(&URL_SAFE_NO_PAD.encode([0u8; 32])).is_none());
        assert!(sessions.get("not base64!").is_none());
        // What admind keeps is not the cookie.
        assert!(!sessions.map.keys().any(|k| URL_SAFE_NO_PAD.encode(k) == cookie));
        assert!(sessions.remove(&cookie).is_some());
        assert!(sessions.get(&cookie).is_none());
    }

    #[test]
    fn a_session_ends_by_the_hours_or_idle_with_no_tab() {
        let lasts = Duration::from_hours(12);
        let idle = Duration::from_mins(30);
        let mut one = session("chase");
        let start = one.started;
        assert!(!one.ended(start + Duration::from_mins(29), lasts, idle));
        assert!(one.ended(start + idle, lasts, idle));
        one.tabs = 1;
        assert!(!one.ended(start + Duration::from_hours(11), lasts, idle));
        assert!(one.ended(start + lasts, lasts, idle));
        let mut sessions = Sessions::default();
        sessions.add(session("a")).unwrap();
        let mut open = session("b");
        open.tabs = 2;
        sessions.add(open).unwrap();
        let ended = sessions.take_ended(start + Duration::from_hours(1), lasts, idle);
        assert_eq!(ended.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["a"]);
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn a_session_is_shown_and_ended_by_an_id_of_its_own_that_opens_nothing() {
        let mut sessions = Sessions::default();
        let cookie = sessions.add(session("helper")).unwrap();
        sessions.add(session("helper")).unwrap();
        sessions.add(session("owner")).unwrap();
        let ids: Vec<String> = sessions.of_account("helper").map(|s| s.id.clone()).collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        for id in &ids {
            assert_eq!(id.len(), 16);
            assert!(id.bytes().all(|b| b.is_ascii_hexdigit()), "{id}");
            assert!(!cookie.contains(id.as_str()));
        }
        // Another account's name does not reach it.
        assert!(sessions.take_id("owner", &ids[0]).is_none());
        let taken = sessions.take_id("helper", &ids[0]).unwrap();
        assert_eq!((taken.name.as_str(), taken.browser.as_str()), ("helper", "Firefox on Linux"));
        assert!(sessions.take_id("helper", &ids[0]).is_none());
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn a_moment_is_given_in_unix_milliseconds_from_the_clock_now() {
        let now = Instant::now();
        assert_eq!(ms_at(now - Duration::from_secs(3), now, 10_000), 7_000);
        assert_eq!(ms_at(now, now, 10_000), 10_000);
        // A moment after now, which cannot be, is now.
        assert_eq!(ms_at(now + Duration::from_secs(1), now, 10_000), 10_000);
        assert!(unix_ms() > 1_700_000_000_000);
    }

    #[test]
    fn each_session_has_its_own_token_and_there_is_a_most() {
        let mut sessions = Sessions::default();
        let (a, b) = (session("a"), session("b"));
        assert_ne!(a.csrf, b.csrf);
        assert_eq!(a.csrf_text().len(), 43);
        for n in 0..SESSIONS_MAX {
            sessions.add(session(&format!("p{n}"))).unwrap();
        }
        assert!(sessions.add(a).is_err());
    }
}
