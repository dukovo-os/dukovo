// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The one-time links (ADR 0568): a report's text that a phone downloads
//! once, by a QR code the Report a problem page draws.
//!
//! A signed-in person asks for a link to a draft reportd keeps for them;
//! admind reads the draft's text as that person and keeps it in memory
//! under a token of 128 random bits, for [`MINUTES`] minutes, at most
//! [`EACH`] links a session and [`ALL`] in all. The token is in the
//! link's address, which needs no session: the phone has none. The first
//! request for it gets the text; one that comes while that one is being
//! answered, or after, is told the link was used (410), and a token that
//! is not kept is told nothing more (404). A text whose answer was not
//! taken whole, its connection lost before the text was handed to it, is
//! kept for the next request, as `report --save`'s own download does. A
//! `HEAD` counts as the download, since a program that only looks at a
//! link, as a chat program does to show a preview, is a request that a
//! person did not make.
//!
//! Links are kept by the SHA-256 of their token, so that how long a lookup
//! takes says nothing of the tokens kept, and no token is in the log. A
//! restart forgets them all.

use std::fmt::Write as _;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use ring::digest::{SHA256, digest};
use ring::rand::{SecureRandom, SystemRandom};

/// How long a link works.
pub const MINUTES: u64 = 10;
/// The most links a session may have at once, and the most in all.
pub const EACH: usize = 4;
pub const ALL: usize = 32;

/// The length of a token in the address: 16 bytes in hex.
pub const TOKEN_LENGTH: usize = 32;

/// Where a link's text is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Kept, and not asked for yet.
    Ready,
    /// Being answered.
    Sending,
    /// Taken: later requests are told so.
    Used,
}

/// A link kept.
#[derive(Debug)]
struct Kept {
    /// The SHA-256 of the token.
    key: [u8; 32],
    /// The SHA-256 of the session's cookie, whose links count together.
    session: [u8; 32],
    until: Instant,
    state: State,
    /// The text, until it is taken.
    text: Option<Bytes>,
}

/// The links admind keeps, shared with the bodies that send them.
#[derive(Clone, Debug, Default)]
pub struct Links(Arc<Mutex<Vec<Kept>>>);

/// A new link.
#[derive(Debug)]
pub struct Made {
    /// The token, for the address.
    pub token: String,
    /// When it ends.
    pub until: Instant,
}

/// What a request for a token gets.
#[derive(Debug)]
pub enum Fetched {
    /// The text, which counts as taken once [`Delivery`] hands it over.
    Text(Delivery),
    /// The link was used, or is being.
    Used,
    /// No link has the token: it was never made, it has ended, or admind
    /// restarted.
    Unknown,
}

/// Why a link is not made.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// The session has as many as it may: the seconds until its oldest
    /// ends.
    Session(u64),
    /// admind keeps as many as it may: the seconds until the oldest ends.
    All(u64),
    /// No random token could be made.
    NoRandom,
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    key.copy_from_slice(digest(&SHA256, bytes).as_ref());
    key
}

/// The seconds from `now` until `until`, at least one.
fn seconds_until(until: Instant, now: Instant) -> u64 {
    until.saturating_duration_since(now).as_secs().max(1)
}

impl Links {
    fn kept(&self) -> std::sync::MutexGuard<'_, Vec<Kept>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keeps `text` for the session of `cookie`, and gives its new token.
    ///
    /// # Errors
    ///
    /// When the session or admind keeps as many links as it may, or no
    /// random token can be made.
    pub fn add(&self, cookie: &str, text: Bytes, now: Instant) -> Result<Made, Refused> {
        let session = sha256(cookie.as_bytes());
        let mut kept = self.kept();
        kept.retain(|link| link.until > now);
        let oldest = |links: &mut dyn Iterator<Item = &Kept>| {
            links.map(|link| link.until).min().map_or(1, |until| seconds_until(until, now))
        };
        if kept.iter().filter(|link| link.session == session).count() >= EACH {
            return Err(Refused::Session(oldest(
                &mut kept.iter().filter(|l| l.session == session),
            )));
        }
        if kept.len() >= ALL {
            return Err(Refused::All(oldest(&mut kept.iter())));
        }
        let mut bytes = [0u8; TOKEN_LENGTH / 2];
        SystemRandom::new().fill(&mut bytes).map_err(|_| Refused::NoRandom)?;
        let token = bytes.iter().fold(String::with_capacity(TOKEN_LENGTH), |mut token, b| {
            let _ = write!(token, "{b:02x}");
            token
        });
        let until = now + Duration::from_secs(MINUTES * 60);
        kept.push(Kept {
            key: sha256(token.as_bytes()),
            session,
            until,
            state: State::Ready,
            text: Some(text),
        });
        Ok(Made { token, until })
    }

    /// What a request for `token` gets now.
    pub fn fetch(&self, token: &str, now: Instant) -> Fetched {
        let key = sha256(token.as_bytes());
        let mut kept = self.kept();
        kept.retain(|link| link.until > now);
        let Some(link) = kept.iter_mut().find(|link| link.key == key) else {
            return Fetched::Unknown;
        };
        match (link.state, &link.text) {
            (State::Ready, Some(text)) => {
                link.state = State::Sending;
                Fetched::Text(Delivery { text: text.clone(), key, links: self.clone() })
            }
            _ => Fetched::Used,
        }
    }

    /// Marks the link of `key` used, or ready again when its text was not
    /// handed over.
    fn ended(&self, key: &[u8; 32], handed: bool) {
        if let Some(link) = self.kept().iter_mut().find(|link| link.key == *key) {
            if handed {
                link.state = State::Used;
                link.text = None;
            } else if link.state == State::Sending {
                link.state = State::Ready;
            }
        }
    }

    /// How many links are kept, ended or not.
    #[cfg(test)]
    fn count(&self) -> usize {
        self.kept().len()
    }
}

/// The text of a link on its way to the phone.
#[derive(Debug)]
pub struct Delivery {
    text: Bytes,
    key: [u8; 32],
    links: Links,
}

impl Delivery {
    /// The text's length in bytes, for the answer's headers.
    pub fn bytes(&self) -> usize {
        self.text.len()
    }

    /// The link as taken, with no text sent: the answer to a `HEAD`.
    pub fn taken(self) {
        self.links.ended(&self.key, true);
    }

    /// The answer's body, which marks the link taken when it hands the text
    /// over, and ready again if it is dropped before that.
    pub fn body(self) -> OnceBody {
        OnceBody { text: Some(self.text), key: self.key, links: self.links, handed: false }
    }
}

/// The body of a link's answer: the text, once.
#[derive(Debug)]
pub struct OnceBody {
    text: Option<Bytes>,
    key: [u8; 32],
    links: Links,
    handed: bool,
}

impl Body for OnceBody {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        match self.text.take() {
            Some(text) => {
                self.handed = true;
                self.links.ended(&self.key, true);
                Poll::Ready(Some(Ok(Frame::data(text))))
            }
            None => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.text.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.text.as_ref().map_or(0, |text| text.len() as u64))
    }
}

impl Drop for OnceBody {
    fn drop(&mut self) {
        if !self.handed {
            self.links.ended(&self.key, false);
        }
    }
}

/// Whether `token` is one admind could have made: 32 lowercase hex digits.
pub fn is_token(token: &str) -> bool {
    token.len() == TOKEN_LENGTH
        && token.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    fn text() -> Bytes {
        Bytes::from_static(b"Dukovo problem report\n")
    }

    #[test]
    fn a_link_gives_its_text_once_and_then_says_it_was_used() {
        let links = Links::default();
        let now = Instant::now();
        let made = links.add("cookie", text(), now).unwrap();
        assert!(is_token(&made.token));
        assert_eq!(made.until, now + Duration::from_secs(600));
        let Fetched::Text(delivery) = links.fetch(&made.token, now) else { panic!("no text") };
        // A second request while the first is answered is told it was used.
        assert!(matches!(links.fetch(&made.token, now), Fetched::Used));
        assert_eq!(collected(delivery.body()), text());
        assert!(matches!(links.fetch(&made.token, now), Fetched::Used));
        assert!(matches!(links.fetch(&"0".repeat(32), now), Fetched::Unknown));
    }

    #[test]
    fn a_text_not_handed_over_is_kept_for_the_next_request_and_a_head_takes_it() {
        let links = Links::default();
        let now = Instant::now();
        let made = links.add("cookie", text(), now).unwrap();
        let Fetched::Text(delivery) = links.fetch(&made.token, now) else { panic!("no text") };
        drop(delivery.body());
        let Fetched::Text(delivery) = links.fetch(&made.token, now) else { panic!("not kept") };
        delivery.taken();
        assert!(matches!(links.fetch(&made.token, now), Fetched::Used));
    }

    #[test]
    fn a_link_ends_after_ten_minutes_and_is_then_unknown() {
        let links = Links::default();
        let now = Instant::now();
        let made = links.add("cookie", text(), now).unwrap();
        let later = now + Duration::from_secs(600);
        assert!(matches!(links.fetch(&made.token, later), Fetched::Unknown));
        assert_eq!(links.count(), 0);
    }

    #[test]
    fn a_session_has_four_links_at_once_and_admind_thirty_two() {
        let links = Links::default();
        let now = Instant::now();
        for _ in 0..EACH {
            links.add("one", text(), now).unwrap();
        }
        let later = now + Duration::from_secs(60);
        assert_eq!(links.add("one", text(), later).unwrap_err(), Refused::Session(540));
        for at in 0..ALL - EACH {
            links.add(&format!("other {at}"), text(), now).unwrap();
        }
        assert_eq!(links.add("new", text(), later).unwrap_err(), Refused::All(540));
        // Used links count until they end: a used one says 410, not 404.
        assert!(links.add("one", text(), now + Duration::from_secs(600)).is_ok());
    }

    #[test]
    fn a_token_is_thirty_two_small_hex_digits() {
        assert!(is_token(&"a1".repeat(16)));
        for token in ["A1".repeat(16), "a1".repeat(15), "g1".repeat(16), "a1".repeat(17)] {
            assert!(!is_token(&token), "{token}");
        }
    }

    /// The bytes `body` gives, to its end.
    fn collected(body: OnceBody) -> Bytes {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        runtime.block_on(body.collect()).unwrap().to_bytes()
    }
}
