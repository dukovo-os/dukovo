// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The MCP client behind a message (ADR 0690): whose it is and how far
//! it reaches, as authd says; the worker that reads as its owner; and
//! the count of what it has running.
//!
//! admind keeps no client. A credential comes with every message, a
//! token by `/mcp` or a pass by the socket, and authd is asked about it
//! each time ([`check`]), so a client that is removed, ends or whose
//! owner may no longer sign in stops at its next message. A wrong token
//! from an address is counted with the wrong API tokens, and the
//! address waits after 10 in a minute. A wrong pass is not counted: it
//! comes from this machine, where a count would let any program hold a
//! client's owner back.
//!
//! A client's reads that run as its owner run in a worker of the
//! owner's, which authd starts for the credential and admind keeps
//! among the API tokens' workers. Its calls of tools are counted
//! ([`begin`]): four at once and 120 in a minute, whichever way they
//! come in.

use std::collections::HashMap;
use std::net::IpAddr;
use std::rc::Rc;
use std::time::{Duration, Instant};

use accounts::console::{Console, Way};
use accounts::request::Answer;

use super::super::authd;
use super::super::caller::{self, Holder, Token};
use super::super::errors::{Attempt, Failure, Kind, refusal};
use super::super::link::Link;
use super::super::reach;
use super::super::shared::Shared;
use super::offer::Reach;

/// The most calls of tools a client has running at once.
pub const CALLS_AT_ONCE: usize = 4;
/// The most it makes in a minute.
pub const CALLS_A_MINUTE: u32 = 120;
/// The minute they are counted in.
const MINUTE: Duration = Duration::from_secs(60);

/// How a pass begins, which comes by the socket alone.
pub const PASS_PREFIX: &[u8] = b"dkp_";

/// Where a credential came in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Door {
    /// `/mcp`, from this address.
    Https(IpAddr),
    /// The socket, from a program on this machine.
    Socket,
}

/// An MCP client, as authd answered for its credential.
pub struct Client {
    credential: Token,
    /// The administrator whose client it is.
    pub owner: String,
    /// Its name, which is its owner's to choose.
    pub name: String,
    /// How it connects.
    pub way: Way,
    /// What its scopes give.
    pub reach: Reach,
}

impl Client {
    /// Its token or pass, for what authd is asked on its behalf.
    pub fn credential(&self) -> &[u8] {
        self.credential.bytes()
    }

    /// How the log names it.
    pub fn told(&self) -> String {
        format!("AI client {} of {}", self.name, self.owner)
    }

    /// What its calls are counted under.
    fn key(&self) -> (String, String) {
        (self.owner.clone(), self.name.clone())
    }
}

/// What authd answers about a credential: `OWNER NAME WAY SCOPES UNTIL`.
fn given(text: &str, credential: Token) -> Option<Client> {
    let mut words = text.split(' ');
    let (owner, name, way, scopes, until) =
        (words.next()?, words.next()?, words.next()?, words.next()?, words.next()?);
    if words.next().is_some() || owner.is_empty() || name.is_empty() {
        return None;
    }
    // The time it ends is authd's to keep: here it only has to be one.
    until.parse::<i64>().ok()?;
    Some(Client {
        credential,
        owner: owner.into(),
        name: name.into(),
        way: Way::of(way.as_bytes())?,
        reach: Reach::parse(scopes)?,
    })
}

/// The failure that authd's refusal `why` of a credential is: ADM-143,
/// unless authd gave it another ID.
fn refused(why: &str) -> Failure {
    match why.split_once(": ") {
        Some((id, _)) if Kind::of(id).is_some() => refusal(Attempt::Other, why),
        _ => Failure::new(Kind::ClientRefused, why),
    }
}

/// Asks authd whose client `credential` is, which came in by `door`.
///
/// # Errors
///
/// ADM-143 when authd does not take it (unknown, ended, removed, or of
/// an owner who may not sign in or is no administrator), or when it is a
/// client that connects the other way; ADM-105 after 10 wrong tokens
/// from the address in a minute; ADM-111 when authd does not answer.
pub async fn check(shared: &Shared, door: Door, credential: Token) -> Result<Client, Failure> {
    let (address, from) = match door {
        Door::Https(peer) => (Some(reach::counted(peer)), peer.to_string()),
        Door::Socket => (None, String::new()),
    };
    if let Some(address) = address
        && let Some(seconds) = shared.wrong_tokens.borrow().waits(address, Instant::now())
    {
        return Err(Failure::wait("too many wrong tokens from this address", seconds));
    }
    let console = Console::Client { token: credential.bytes().to_vec(), from };
    let (answer, _) = authd::ask(&shared.authd, console, &[])
        .await
        .map_err(|why| Failure::new(Kind::AuthdSilent, why))?;
    let text = match answer {
        Answer::Done(text) => text,
        Answer::Refused(why) => {
            if let Some(address) = address {
                // Counted whether or not the table has room, as a wrong
                // API token is.
                let _ = shared.wrong_tokens.borrow_mut().take(address, Instant::now());
            }
            return Err(refused(&why));
        }
    };
    let client = given(&text, credential).ok_or_else(|| {
        Failure::new(Kind::Internal, "authd's answer about an AI client did not read")
    })?;
    match (door, client.way) {
        (Door::Https(_), Way::Https) | (Door::Socket, Way::Ssh) => Ok(client),
        (Door::Https(_), Way::Ssh) => Err(Failure::new(
            Kind::ClientRefused,
            format!("{} connects over SSH, with `admin mcp {0}`, and not here", client.name),
        )),
        (Door::Socket, Way::Https) => Err(Failure::new(
            Kind::ClientRefused,
            format!("{} connects over HTTPS, at /mcp, and not here", client.name),
        )),
    }
}

/// Asks authd what `owner`'s client `name` reaches now, for a request
/// of its that an administrator is about to approve: the client may
/// have been removed since it asked, or made again with less.
///
/// # Errors
///
/// ADM-143 when the owner has no such client that may still connect;
/// ADM-111 when authd does not answer.
pub async fn reach_now(shared: &Shared, owner: &str, name: &str) -> Result<Reach, Failure> {
    let console = Console::ClientOf { owner: owner.into(), name: name.into() };
    let (answer, _) = authd::ask(&shared.authd, console, &[])
        .await
        .map_err(|why| Failure::new(Kind::AuthdSilent, why))?;
    match answer {
        Answer::Done(text) => {
            given(&text, Token::new(Vec::new())).map(|client| client.reach).ok_or_else(|| {
                Failure::new(Kind::Internal, "authd's answer about an AI client did not read")
            })
        }
        Answer::Refused(why) => Err(refused(&why)),
    }
}

/// The worker of `client`'s owner, for a read that runs as the owner:
/// the one kept, or a new one authd starts for the credential.
///
/// # Errors
///
/// ADM-120 when as many tokens and clients have workers as may, ADM-143
/// when authd does not start one, ADM-111 when it does not answer.
pub async fn worker(shared: &Shared, client: &Client) -> Result<Link, Failure> {
    let start = Console::ClientStart { token: client.credential().to_vec() };
    let key = (Holder::Client, client.owner.clone(), client.name.clone());
    caller::kept_worker(shared, key, start, Kind::ClientRefused).await
}

/// What each client has running and has made lately, by owner and name.
#[derive(Default)]
pub struct Calls(HashMap<(String, String), Count>);

/// One client's calls.
struct Count {
    /// Those that run now.
    running: usize,
    /// When the minute that `made` counts began.
    since: Instant,
    /// Those made in it.
    made: u32,
}

impl Calls {
    /// Forgets the clients with nothing running and nothing made in the
    /// last minute.
    pub fn tidy(&mut self, now: Instant) {
        self.0.retain(|_, count| count.running > 0 || now.duration_since(count.since) < MINUTE);
    }
}

/// A call that runs, counted out when it goes.
pub struct Running {
    shared: Rc<Shared>,
    key: (String, String),
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(count) = self.shared.client_calls.borrow_mut().0.get_mut(&self.key) {
            count.running = count.running.saturating_sub(1);
        }
    }
}

/// Counts a call of `client`'s in, at `now`.
///
/// # Errors
///
/// ADM-105 when the client has [`CALLS_AT_ONCE`] running, or has made
/// [`CALLS_A_MINUTE`] in the minute, with the seconds to wait.
pub fn begin(shared: &Rc<Shared>, client: &Client, now: Instant) -> Result<Running, Failure> {
    let key = client.key();
    let mut calls = shared.client_calls.borrow_mut();
    let count = calls.0.entry(key.clone()).or_insert(Count { running: 0, since: now, made: 0 });
    if now.duration_since(count.since) >= MINUTE {
        (count.since, count.made) = (now, 0);
    }
    if count.running >= CALLS_AT_ONCE {
        let why = format!("{CALLS_AT_ONCE} calls of this AI client's run already");
        return Err(Failure::wait(why, 1));
    }
    if count.made >= CALLS_A_MINUTE {
        let left = MINUTE.saturating_sub(now.duration_since(count.since)).as_secs().max(1);
        let why = format!("this AI client made {CALLS_A_MINUTE} calls in a minute");
        return Err(Failure::wait(why, left));
    }
    count.running += 1;
    count.made += 1;
    Ok(Running { shared: Rc::clone(shared), key })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::admind::fixture::{self, Asked, Console as World, run};
    use crate::admind::mcp::offer::Level;

    /// A client of `owner` called `name`, with `scopes`, that connects
    /// by `way`.
    pub(crate) fn client(owner: &str, name: &str, way: Way, scopes: &str) -> Client {
        let text = format!("{owner} {name} {} {scopes} 1900000000", way.word());
        given(&text, Token::new(b"dkc_test".to_vec())).expect("a client")
    }

    #[test]
    fn authd_s_answer_gives_the_client() {
        let found = given("chase helper ssh *:read,services 1900000000", Token::new(Vec::new()))
            .expect("a client");
        assert_eq!((found.owner.as_str(), found.name.as_str()), ("chase", "helper"));
        assert_eq!(found.way, Way::Ssh);
        assert_eq!(found.reach.level("services"), Level::All);
        assert_eq!(found.reach.level("logs"), Level::Read);
        assert_eq!(found.told(), "AI client helper of chase");
        for wrong in [
            "",
            "chase helper ssh *",
            "chase helper ssh * soon",
            "chase helper carrier * 1900000000",
            "chase helper ssh Services 1900000000",
            "chase helper ssh * 1900000000 more",
            " helper ssh * 1900000000",
            "chase  ssh * 1900000000",
        ] {
            assert!(given(wrong, Token::new(Vec::new())).is_none(), "{wrong:?}");
        }
    }

    #[test]
    fn authd_s_refusal_is_the_client_s_unless_it_has_an_id_of_its_own() {
        let plain = refused("that AI client's token is not known");
        assert_eq!(plain.kind, Kind::ClientRefused);
        assert_eq!(plain.detail, "that AI client's token is not known");
        let named = refused("ADM-105: wait 30 seconds");
        assert_eq!((named.kind, named.detail.as_str()), (Kind::Wait, "wait 30 seconds"));
        // A colon alone names nothing.
        assert_eq!(refused("chase: no such account").kind, Kind::ClientRefused);
    }

    /// An authd that knows the token `dkc_good` as chase's HTTPS client
    /// and the pass `dkp_good` as his SSH client.
    fn script() -> Arc<fixture::Script> {
        Arc::new(|asked: &Asked, _| {
            let hex = |text: &str| accounts::request::hex(text.as_bytes());
            match (asked.verb.as_str(), asked.text(0)) {
                ("client", token) if token == hex("dkc_good") => {
                    vec!["ok chase web https services 1900000000".into()]
                }
                ("client", token) if token == hex("dkp_good") => {
                    vec!["ok chase shell ssh *:read 1900000000".into()]
                }
                _ => vec!["no that AI client's token is not known".into()],
            }
        })
    }

    fn token(text: &str) -> Token {
        Token::new(text.as_bytes().to_vec())
    }

    #[test]
    fn a_credential_is_taken_only_at_its_own_door() {
        run(async {
            let world = World::new("mcp-doors", script());
            let peer: IpAddr = "192.0.2.7".parse().unwrap();
            let web = check(&world.shared, Door::Https(peer), token("dkc_good")).await;
            assert_eq!(web.map(|client| client.name).ok().as_deref(), Some("web"));
            let shell = check(&world.shared, Door::Socket, token("dkp_good")).await;
            assert_eq!(shell.map(|client| client.name).ok().as_deref(), Some("shell"));

            let wrong = check(&world.shared, Door::Https(peer), token("dkp_good")).await;
            let wrong = wrong.err().expect("a pass at /mcp");
            assert_eq!(wrong.kind, Kind::ClientRefused);
            assert!(wrong.detail.contains("admin mcp shell"), "{}", wrong.detail);
            let wrong = check(&world.shared, Door::Socket, token("dkc_good")).await;
            assert!(wrong.err().expect("a token at the socket").detail.contains("at /mcp"));
            // authd hears where the credential came from.
            let lines = world.authd.lines();
            assert!(lines[0].ends_with(" 192.0.2.7"), "{}", lines[0]);
            assert!(!lines[1].contains("192.0.2.7"), "{}", lines[1]);
        });
    }

    #[test]
    fn wrong_tokens_from_an_address_make_it_wait_and_wrong_passes_do_not() {
        run(async {
            let world = World::new("mcp-wrong", script());
            let peer: IpAddr = "192.0.2.9".parse().unwrap();
            for _ in 0..crate::admind::limits::TRIES_MAX {
                let wrong = check(&world.shared, Door::Https(peer), token("dkc_bad")).await;
                assert_eq!(wrong.err().map(|failure| failure.kind), Some(Kind::ClientRefused));
            }
            let asked = world.authd.lines().len();
            // The next is refused before authd hears of it, a good one too.
            let waits = check(&world.shared, Door::Https(peer), token("dkc_good")).await;
            let waits = waits.err().expect("an address that waits");
            assert_eq!(waits.kind, Kind::Wait);
            assert!(waits.retry_after.is_some());
            assert_eq!(world.authd.lines().len(), asked);
            // Another address is not held back; and the socket counts
            // the passes it is brought by account, not here.
            let other: IpAddr = "192.0.2.10".parse().unwrap();
            assert!(check(&world.shared, Door::Https(other), token("dkc_good")).await.is_ok());
            for _ in 0..2 * crate::admind::limits::TRIES_MAX {
                let wrong = check(&world.shared, Door::Socket, token("dkp_bad")).await;
                assert_eq!(wrong.err().map(|failure| failure.kind), Some(Kind::ClientRefused));
            }
            assert!(check(&world.shared, Door::Socket, token("dkp_good")).await.is_ok());
        });
    }

    #[test]
    fn a_client_s_calls_are_counted_at_once_and_in_a_minute() {
        run(async {
            let world = World::new("mcp-calls", script());
            let shared = &world.shared;
            let helper = client("chase", "helper", Way::Ssh, "*");
            let start = Instant::now();
            let mut running: Vec<Running> = (0..CALLS_AT_ONCE)
                .map(|_| begin(shared, &helper, start).expect("a call"))
                .collect();
            let full = begin(shared, &helper, start).err().expect("a fifth at once");
            assert_eq!((full.kind, full.retry_after), (Kind::Wait, Some(1)));
            // Another client, and the same name of another owner, have
            // their own counts.
            assert!(begin(shared, &client("chase", "other", Way::Ssh, "*"), start).is_ok());
            assert!(begin(shared, &client("alice", "helper", Way::Ssh, "*"), start).is_ok());
            // One that ends makes room.
            running.pop();
            running.push(begin(shared, &helper, start).expect("room again"));
            running.clear();

            // 120 in a minute, counted from the first: 5 are made.
            for _ in 5..CALLS_A_MINUTE {
                drop(begin(shared, &helper, start + Duration::from_secs(20)).expect("a call"));
            }
            let late = start + Duration::from_secs(45);
            let many = begin(shared, &helper, late).err().expect("the 121st");
            assert_eq!((many.kind, many.retry_after), (Kind::Wait, Some(15)));
            assert!(begin(shared, &helper, start + MINUTE).is_ok());

            // Idle clients are forgotten, and one with a call running is
            // not.
            let held = begin(shared, &client("chase", "other", Way::Ssh, "*"), start + MINUTE);
            shared.client_calls.borrow_mut().tidy(start + MINUTE * 3);
            assert_eq!(shared.client_calls.borrow().0.len(), 1);
            drop(held);
            shared.client_calls.borrow_mut().tidy(start + MINUTE * 3);
            assert!(shared.client_calls.borrow().0.is_empty());
        });
    }
}
