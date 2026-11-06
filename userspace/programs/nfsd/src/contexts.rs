// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The RPCSEC_GSS contexts nfsd holds (RFC 2203 5.3.3, ADR 0399): each
//! set up from a client's ticket for `nfs/HOST` with the machine's
//! keytab, and kept with the account its principal maps to and the
//! sequence numbers its calls used.
//!
//! - A principal of one part in the server's own realm, `alice@REALM`,
//!   is the account of that name, if it is a person's; any other, a
//!   machine's `host/client` among them, is served as no one, as root is
//!   with AUTH_SYS.
//! - A context lasts as long as the ticket it was set up with; then the
//!   client is told so (CREDPROBLEM) and sets up another.
//! - The keytab and the accounts are read again at each new context, so
//!   that a new key or a new account needs no restart.
//!
//! Where to start when a client's contexts are refused: nfsd's log says
//! `nfsd: Kerberos: ...` with the reason; `klist -k` on the server lists
//! the keytab's keys, which must hold `nfs/HOST`'s at the KDC's kvno.

use std::collections::HashMap;

use kerberos::gss::{self, Context};
use kerberos::keytab;
use kerberos::messages::Principal;

use crate::gss::{GSS_S_FAILURE, InitResult, MAXSEQ};
use crate::rpc::NOBODY;

/// How many sequence numbers below the highest seen a call may still
/// carry: calls on several connections, or several at once on one,
/// arrive out of order.
pub const WINDOW: u32 = 128;
/// The most contexts held: past it, the one used longest ago goes, and
/// its client sets up another.
const CONTEXTS_MAX: usize = 1024;

/// Where the keys come from.
pub enum Keys {
    /// The keytab, at `kerberos::keytab::path()`.
    Keytab,
    /// These, as the tests give them.
    #[cfg(any(test, feature = "testing"))]
    Given(Vec<keytab::Entry>),
}

/// Where the accounts come from.
pub enum Accounts {
    /// The accounts file, `accounts::PASSWD`.
    File,
    /// These names and numbers, as the tests give them.
    #[cfg(any(test, feature = "testing"))]
    Given(Vec<(String, u32)>),
}

/// The contexts.
pub struct Contexts {
    held: HashMap<Vec<u8>, Held>,
    /// The start of each handle, new at each start of nfsd, so that a
    /// handle from before is not mistaken for one of now.
    salt: [u8; 4],
    /// How many contexts were set up: the rest of each handle.
    made: u64,
    /// When a context was last used, by a count of the calls.
    clock: u64,
    keys: Keys,
    accounts: Accounts,
}

/// A context held, and what goes with it.
pub struct Held {
    /// The context.
    pub context: Context,
    /// The account its calls are served as.
    pub uid: u32,
    window: Window,
    used: u64,
}

/// Why a call under a context is refused (RFC 2203 5.3.3.3).
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// No such context, or a verifier that does not check, or a ticket
    /// that ran out: the client sets up another (CREDPROBLEM).
    Credential,
    /// A sequence number past [`MAXSEQ`]: the context is used up
    /// (CTXPROBLEM).
    Context,
    /// A sequence number seen already, or too far behind: a replay, or a
    /// call the client gave up on, which gets no answer.
    Dropped,
}

impl Contexts {
    /// No contexts yet, with handles that start with `salt`, keys from
    /// `keys` and accounts from `accounts`.
    pub fn new(salt: [u8; 4], keys: Keys, accounts: Accounts) -> Contexts {
        Contexts { held: HashMap::new(), salt, made: 0, clock: 0, keys, accounts }
    }

    /// Sets up a context from GSS-API's first token, `token`, from a
    /// client at `from`: INIT's results, and the verifier its reply
    /// carries, a MIC of the window, when the context was set up.
    pub fn init(&mut self, token: &[u8], from: &str) -> (InitResult, Option<Vec<u8>>) {
        match self.accept(token) {
            Ok((result, verifier)) => (result, Some(verifier)),
            Err(why) => {
                eprintln!("nfsd: Kerberos: refused a context from {from}: {why}");
                let result = InitResult {
                    handle: Vec::new(),
                    major: GSS_S_FAILURE,
                    minor: 0,
                    window: 0,
                    token: Vec::new(),
                };
                (result, None)
            }
        }
    }

    /// Accepts `token`: INIT's results, with the new context's handle and
    /// GSS-API's answering token, and the reply's verifier.
    fn accept(&mut self, token: &[u8]) -> Result<(InitResult, Vec<u8>), String> {
        let read;
        let entries = match &self.keys {
            #[cfg(any(test, feature = "testing"))]
            Keys::Given(entries) => entries,
            Keys::Keytab => {
                read = keytab::read(&keytab::path()).map_err(|err| err.to_string())?;
                &read
            }
        };
        let (mut context, answer) = gss::accept(entries, token).map_err(|err| err.to_string())?;
        // The keytab may hold the machine's other keys, as host/HOST's;
        // only nfs/HOST's open NFS.
        if context.service().parts.first().map(String::as_str) != Some("nfs") {
            return Err(format!(
                "the ticket is for {}, not for nfs/HOST: mount the server by the name its \
                 nfs/ principal has",
                context.service()
            ));
        }
        let uid = self.account_of(context.peer(), &context.service().realm);
        let verifier = context.get_mic(&WINDOW.to_be_bytes());
        self.made += 1;
        let mut handle = self.salt.to_vec();
        handle.extend_from_slice(&self.made.to_be_bytes());
        if self.held.len() >= CONTEXTS_MAX {
            let oldest = self.held.iter().min_by_key(|(_, held)| held.used).map(|(h, _)| h.clone());
            if let Some(oldest) = oldest {
                self.held.remove(&oldest);
            }
        }
        self.clock += 1;
        let held = Held { context, uid, window: Window::default(), used: self.clock };
        self.held.insert(handle.clone(), held);
        let token = answer.unwrap_or_default();
        Ok((InitResult { handle, major: 0, minor: 0, window: WINDOW, token }, verifier))
    }

    /// The account `principal` is served as, in the server's `realm`: a
    /// person's account of the same name, or no one.
    fn account_of(&self, principal: &Principal, realm: &str) -> u32 {
        let [name] = principal.parts.as_slice() else { return NOBODY };
        if principal.realm != realm {
            return NOBODY;
        }
        let people = accounts::FIRST_USER..=accounts::LAST_USER;
        let uid = match &self.accounts {
            #[cfg(any(test, feature = "testing"))]
            Accounts::Given(given) => given.iter().find(|(n, _)| n == name).map(|(_, uid)| *uid),
            Accounts::File => std::fs::read_to_string(accounts::PASSWD).ok().and_then(|text| {
                accounts::users(&text).entries().find(|user| &user.name == name).map(|u| u.uid)
            }),
        };
        uid.filter(|uid| people.contains(uid)).unwrap_or(NOBODY)
    }

    /// The context `handle` names, for a call numbered `seq` whose
    /// header `header` its verifier `mic` signs.
    ///
    /// # Errors
    ///
    /// Why the call is refused: see [`Refused`].
    pub fn check(
        &mut self,
        handle: &[u8],
        seq: u32,
        header: &[u8],
        mic: &[u8],
    ) -> Result<&mut Held, Refused> {
        let (now, _) = kerberos::time::now();
        let held = self.held.get_mut(handle).ok_or(Refused::Credential)?;
        if held.context.expires() <= now {
            self.held.remove(handle);
            return Err(Refused::Credential);
        }
        held.context.verify_mic(header, mic).map_err(|_| Refused::Credential)?;
        if seq >= MAXSEQ {
            self.held.remove(handle);
            return Err(Refused::Context);
        }
        let held = self.held.get_mut(handle).ok_or(Refused::Credential)?;
        if !held.window.take(seq) {
            return Err(Refused::Dropped);
        }
        self.clock += 1;
        held.used = self.clock;
        Ok(held)
    }

    /// The context `handle` names, already checked for this call.
    pub fn context(&mut self, handle: &[u8]) -> Option<&mut Context> {
        self.held.get_mut(handle).map(|held| &mut held.context)
    }

    /// Whether a context has the handle `handle`.
    pub fn holds(&self, handle: &[u8]) -> bool {
        self.held.contains_key(handle)
    }

    /// Forgets the context `handle` names, at its client's DESTROY.
    pub fn destroy(&mut self, handle: &[u8]) {
        self.held.remove(handle);
    }
}

/// The sequence numbers a context's calls carried (RFC 2203 5.3.3.1):
/// the highest, and which of the [`WINDOW`] below it were seen.
#[derive(Default)]
struct Window {
    highest: u32,
    /// Bit `n`: whether `highest - n` was seen.
    seen: u128,
}

impl Window {
    /// Whether a call numbered `seq` is new and within the window, and
    /// so to be served; it is marked seen.
    fn take(&mut self, seq: u32) -> bool {
        if seq > self.highest || self.seen == 0 {
            let ahead = seq.wrapping_sub(self.highest);
            self.seen = if self.seen == 0 || ahead >= WINDOW { 0 } else { self.seen << ahead };
            self.seen |= 1;
            self.highest = seq;
            return true;
        }
        let behind = self.highest - seq;
        if behind >= WINDOW || self.seen & (1 << behind) != 0 {
            return false;
        }
        self.seen |= 1 << behind;
        true
    }
}

#[cfg(test)]
mod tests {
    use kerberos::crypto::Enctype;
    use kerberos::gss::{Initiator, flags, testing};

    use super::*;

    /// Contexts with a keytab of [`testing::SERVICE`]'s, where alice is
    /// number 1000 and root is 0: and the keytab's key.
    fn contexts() -> (Contexts, keytab::Entry) {
        let (_, keytab) =
            testing::issue("alice@EXAMPLE.ORG", testing::SERVICE, Enctype::Aes256Sha1);
        let key = keytab[1].clone();
        let accounts = vec![("alice".into(), 1000), ("root".into(), 0)];
        (Contexts::new([1, 2, 3, 4], Keys::Given(keytab), Accounts::Given(accounts)), key)
    }

    /// Sets up a context for `client` in `contexts`, whose keytab holds
    /// `key`: the client's side, and the handle.
    fn set_up(contexts: &mut Contexts, key: &keytab::Entry, client: &str) -> (Context, Vec<u8>) {
        let credential = testing::ticket(client, key, Enctype::Aes256Sha1);
        let (initiator, token) = Initiator::start(&credential, flags::MUTUAL).unwrap();
        let (result, verifier) = contexts.init(&token, "test");
        assert_eq!((result.major, result.window), (0, WINDOW));
        let ours = initiator.finish(Some(&result.token)).unwrap();
        ours.verify_mic(&WINDOW.to_be_bytes(), &verifier.unwrap()).unwrap();
        (ours, result.handle)
    }

    #[test]
    fn people_are_served_as_themselves_and_others_as_no_one() {
        let (mut contexts, key) = contexts();
        let clients = [
            ("alice@EXAMPLE.ORG", 1000),
            ("root@EXAMPLE.ORG", NOBODY),
            ("host/client.example.org@EXAMPLE.ORG", NOBODY),
            ("alice@ELSEWHERE.ORG", NOBODY),
        ];
        for (client, uid) in clients {
            let (mut ours, handle) = set_up(&mut contexts, &key, client);
            assert_eq!(&handle[..4], [1, 2, 3, 4]);
            let mic = ours.get_mic(b"header");
            let held = contexts.check(&handle, 0, b"header", &mic).unwrap();
            assert_eq!(held.uid, uid, "{client}");
        }
    }

    #[test]
    fn calls_are_checked_and_replays_dropped() {
        let (mut contexts, key) = contexts();
        let (mut ours, handle) = set_up(&mut contexts, &key, "alice@EXAMPLE.ORG");
        let mic = ours.get_mic(b"header");
        assert!(contexts.check(&handle, 5, b"header", &mic).is_ok());
        assert_eq!(contexts.check(&handle, 5, b"header", &mic).err(), Some(Refused::Dropped));
        assert!(contexts.check(&handle, 3, b"header", &mic).is_ok());
        assert!(contexts.check(&handle, 300, b"header", &mic).is_ok());
        assert_eq!(contexts.check(&handle, 100, b"header", &mic).err(), Some(Refused::Dropped));
        assert!(contexts.check(&handle, 299, b"header", &mic).is_ok());
        // Another header, another handle, a used-up context.
        let refused = contexts.check(&handle, 301, b"other", &mic).err();
        assert_eq!(refused, Some(Refused::Credential));
        let refused = contexts.check(b"nonesuch", 301, b"header", &mic).err();
        assert_eq!(refused, Some(Refused::Credential));
        let refused = contexts.check(&handle, MAXSEQ, b"header", &mic).err();
        assert_eq!(refused, Some(Refused::Context));
        assert_eq!(contexts.check(&handle, 302, b"header", &mic).err(), Some(Refused::Credential));
    }

    #[test]
    fn a_ticket_for_another_service_is_refused() {
        let (credential, keytab) = testing::issue(
            "alice@EXAMPLE.ORG",
            "host/server.example.org@EXAMPLE.ORG",
            Enctype::Aes256Sha1,
        );
        let mut host = Contexts::new([0; 4], Keys::Given(keytab), Accounts::Given(Vec::new()));
        let (_, token) = Initiator::start(&credential, flags::MUTUAL).unwrap();
        let (result, verifier) = host.init(&token, "test");
        assert_eq!((result.major, verifier), (GSS_S_FAILURE, None));
        // A ticket under a key the keytab does not hold.
        let (_, token) = Initiator::start(&credential, flags::MUTUAL).unwrap();
        let (result, _) = contexts().0.init(&token, "test");
        assert_eq!(result.major, GSS_S_FAILURE);
    }

    #[test]
    fn the_window_takes_each_number_once() {
        let mut window = Window::default();
        assert!(window.take(0));
        assert!(!window.take(0));
        assert!(window.take(2));
        assert!(window.take(1));
        assert!(!window.take(1));
        assert!(window.take(2 + WINDOW));
        assert!(!window.take(2));
        assert!(window.take(3));
        assert!(!window.take(3));
    }
}
