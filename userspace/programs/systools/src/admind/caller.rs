// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! API tokens in requests (ADR 0515): reading one from a request, asking
//! authd whose it is and what it may do, and the worker that acts for its
//! owner.
//!
//! A token comes as `Authorization: Bearer dkv_...` and nowhere else:
//! never in an address, which logs and histories keep, nor in a cookie,
//! which a browser sends by itself. authd checks it on every request, so
//! a token that is removed, ends or whose account is disabled stops at
//! once; it answers with the owner, the token's name and its scopes, and
//! it slows the guessing of tokens as it slows that of passwords. admind
//! holds no token: it passes each one on and forgets it.
//!
//! A token's calls go to a worker of its owner's, which authd starts as it
//! starts a session's, and which admind keeps for five minutes after the
//! token's last call, so that a script's run of calls does not start one
//! each time.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use accounts::console::Console;
use accounts::request::Answer;
use http::header::{self, HeaderMap, HeaderValue};

use super::authd;
use super::errors::{Failure, Kind};
use super::link::{self, Link};
use super::reach;
use super::scopes::Scopes;
use super::shared::Shared;

/// The longest token admind passes on: `dkv_`, an account's name of up
/// to 32 bytes, `_` and 64 hex digits fit with room.
const TOKEN_MAX: usize = 160;

/// How long a token's worker is kept after its last call.
pub const WORKER_IDLE: Duration = Duration::from_mins(5);

/// The most tokens with a worker at once; a token past them waits for
/// one to go idle.
const WORKERS_MAX: usize = 32;

/// An API token from a request, wiped when it goes.
pub struct Token(Vec<u8>);

impl Token {
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for Token {
    fn drop(&mut self) {
        accounts::wipe(&mut self.0);
    }
}

/// The API token `headers` bring, if they bring one: an `Authorization`
/// of the scheme `Bearer`, whatever its case. Another scheme is not a
/// token and is let be, as a proxy in front of the console that asks for
/// its own password sends it on.
///
/// # Errors
///
/// ADM-108 for more than one bearer, or one that is not a token of this
/// console's form.
pub fn bearer(headers: &HeaderMap) -> Result<Option<Token>, Failure> {
    let mut bearers = headers
        .get_all(header::AUTHORIZATION)
        .iter()
        .map(HeaderValue::as_bytes)
        .filter_map(of_bearer);
    let Some(token) = bearers.next() else { return Ok(None) };
    let form = || {
        Failure::new(
            Kind::BadRequest,
            "an API token comes as one header, Authorization: Bearer dkv_…",
        )
    };
    if bearers.next().is_some()
        || !token.starts_with(b"dkv_")
        || token.len() > TOKEN_MAX
        || !token.iter().all(u8::is_ascii_graphic)
    {
        return Err(form());
    }
    Ok(Some(Token(token.to_vec())))
}

/// What follows the scheme of `value` when it is `Bearer`: the spaces
/// after it skipped, and empty for a bearer with nothing.
fn of_bearer(value: &[u8]) -> Option<&[u8]> {
    let (scheme, rest) = value.split_at_checked(6)?;
    let apart = rest.first().is_none_or(|&byte| byte == b' ');
    (scheme.eq_ignore_ascii_case(b"bearer") && apart).then(|| rest.trim_ascii_start())
}

/// A request made with a token, as authd answered for it.
pub struct TokenCall {
    pub token: Token,
    /// The account whose token it is.
    pub owner: String,
    /// The token's name, which logs and the journal show.
    pub name: String,
    pub scopes: Scopes,
}

/// What authd answers to `console-token`: `OWNER NAME SCOPES UNTIL`.
fn given(text: &str) -> Option<(String, String, Scopes)> {
    let mut words = text.split(' ');
    let (owner, name, scopes, until) = (words.next()?, words.next()?, words.next()?, words.next()?);
    if words.next().is_some() || owner.is_empty() || name.is_empty() {
        return None;
    }
    until.parse::<i64>().ok()?;
    Some((owner.into(), name.into(), Scopes::parse(scopes)?))
}

/// Asks authd whether `token`, from `peer`, is a token, and gives the
/// request made with it.
///
/// authd slows the guessing of a token's owner's tokens; admind refuses an
/// address that gave 10 wrong tokens in a minute before authd hears of
/// the 11th, as it refuses the 11th sign-in.
///
/// # Errors
///
/// ADM-115 when authd does not take it (unknown, ended, or of an account
/// that may not sign in), ADM-105 after 10 wrong ones from the address in
/// a minute, ADM-111 when authd does not answer.
pub async fn check(shared: &Shared, peer: IpAddr, token: Token) -> Result<TokenCall, Failure> {
    let address = reach::counted(peer);
    if let Some(seconds) = shared.wrong_tokens.borrow().waits(address, Instant::now()) {
        return Err(Failure::wait("too many wrong API tokens from this address", seconds));
    }
    let console = Console::Token { token: token.bytes().to_vec(), from: peer.to_string() };
    let (answer, _) = authd::ask(&shared.authd, console, &[])
        .await
        .map_err(|why| Failure::new(Kind::AuthdSilent, why))?;
    let text = match answer {
        Answer::Done(text) => text,
        Answer::Refused(why) => {
            // Counted whether or not the table has room: a full table
            // refuses every address it does not hold already.
            let _ = shared.wrong_tokens.borrow_mut().take(address, Instant::now());
            return Err(Failure::new(Kind::TokenRefused, why));
        }
    };
    let (owner, name, scopes) = given(&text)
        .ok_or_else(|| Failure::new(Kind::Internal, "authd's answer about a token did not read"))?;
    Ok(TokenCall { token, owner, name, scopes })
}

/// The workers of tokens, by owner and token name.
#[derive(Default)]
pub struct TokenWorkers(HashMap<(String, String), Kept>);

struct Kept {
    link: Link,
    last: Instant,
}

impl TokenWorkers {
    /// Lets go of the workers idle for [`WORKER_IDLE`] or gone; dropping
    /// the last link to one ends it.
    pub fn tidy(&mut self, now: Instant) {
        self.0.retain(|_, kept| !kept.link.is_closed() && now - kept.last < WORKER_IDLE);
    }
}

/// The worker of the token of `call`, for an action that runs as its
/// owner: the one kept, or a new one authd starts. An administrator's
/// change runs through authd and needs none.
///
/// # Errors
///
/// ADM-120 when as many tokens have workers as may, ADM-115 when authd
/// does not start one, ADM-111 when it does not answer.
pub async fn worker(shared: &Shared, call: &TokenCall) -> Result<Link, Failure> {
    let TokenCall { token, owner, name, .. } = call;
    let key = (owner.clone(), name.clone());
    let now = Instant::now();
    {
        let mut workers = shared.token_workers.borrow_mut();
        workers.tidy(now);
        if let Some(kept) = workers.0.get_mut(&key) {
            kept.last = now;
            return Ok(kept.link.clone());
        }
        if workers.0.len() >= WORKERS_MAX {
            return Err(Failure::new(Kind::Busy, "as many API tokens as may are in use"));
        }
    }
    let internal =
        |err: std::io::Error| Failure::new(Kind::Internal, format!("no link to a worker: {err}"));
    let (ours, theirs) = link::pipes().map_err(internal)?;
    let console = Console::TokenStart { token: token.bytes().to_vec() };
    let [calls, replies] = theirs.fds();
    let fds = [calls, replies, sysapi::fd::Standard::Error.fd()];
    let (answer, _) = authd::ask(&shared.authd, console, &fds)
        .await
        .map_err(|why| Failure::new(Kind::AuthdSilent, why))?;
    drop(theirs);
    if let Answer::Refused(why) = answer {
        return Err(Failure::new(Kind::TokenRefused, why));
    }
    let link = Link::new(ours).map_err(internal)?;
    // Another request of the same token may have started one meanwhile:
    // the first kept is used, and this one goes with its link.
    let mut workers = shared.token_workers.borrow_mut();
    let kept = workers.0.entry(key).or_insert(Kept { link, last: now });
    Ok(kept.link.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    const TOKEN: &str =
        "dkv_chase_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn a_request_without_a_token_has_none() {
        assert!(bearer(&HeaderMap::new()).unwrap().is_none());
    }

    #[test]
    fn a_bearer_token_is_read_whatever_the_scheme_s_case_and_spaces() {
        for value in
            [format!("Bearer {TOKEN}"), format!("bearer {TOKEN}"), format!("BEARER   {TOKEN}")]
        {
            let token = bearer(&headers(&[&value])).unwrap().unwrap();
            assert_eq!(token.bytes(), TOKEN.as_bytes(), "{value}");
        }
    }

    #[test]
    fn a_bearer_that_is_not_a_token_of_this_console_s_is_refused() {
        let long = format!("Bearer dkv_{}", "a".repeat(TOKEN_MAX));
        for value in ["Bearer", "Bearer ", "bearer abc", "Bearer dkv_a b", "Bearer x dkv_a", &long]
        {
            let failure = bearer(&headers(&[value])).err().unwrap_or_else(|| panic!("{value}"));
            assert_eq!(failure.kind, Kind::BadRequest, "{value}");
        }
        // Not UTF-8, which a header may be.
        let mut odd = HeaderMap::new();
        odd.append(header::AUTHORIZATION, HeaderValue::from_bytes(b"Bearer dkv_\xff").unwrap());
        assert_eq!(bearer(&odd).err().map(|f| f.kind), Some(Kind::BadRequest));
    }

    #[test]
    fn another_scheme_is_no_token_and_is_let_be() {
        for value in
            ["Basic Y2hhc2U6c2VjcmV0", "Token dkv_chase_00", "dkv_chase_00", "Bearerdkv_chase_00"]
        {
            assert!(bearer(&headers(&[value])).unwrap().is_none(), "{value}");
        }
        // A proxy's own password and a token together: the token is read.
        let both = headers(&["Basic Y2hhc2U6c2VjcmV0", &format!("Bearer {TOKEN}")]);
        assert_eq!(bearer(&both).unwrap().unwrap().bytes(), TOKEN.as_bytes());
    }

    #[test]
    fn two_bearers_are_refused() {
        let value = format!("Bearer {TOKEN}");
        let failure = bearer(&headers(&[&value, &value])).err().unwrap();
        assert_eq!(failure.kind, Kind::BadRequest);
    }

    #[test]
    fn authd_s_answer_gives_the_owner_the_name_and_the_scopes() {
        let (owner, name, scopes) =
            given("chase ansible services:read,firewall 1800000000").unwrap();
        assert_eq!((owner.as_str(), name.as_str()), ("chase", "ansible"));
        assert!(scopes.allow("services", false) && !scopes.allow("services", true));
        assert!(scopes.allow("firewall", true));
        for wrong in [
            "",
            "chase ansible services",
            "chase ansible services soon",
            "chase ansible services 1800000000 more",
            "chase ansible Services 1800000000",
            " ansible * 1800000000",
        ] {
            assert!(given(wrong).is_none(), "{wrong:?}");
        }
    }
}
