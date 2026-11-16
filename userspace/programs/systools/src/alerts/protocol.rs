// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What goes over alertd's socket (ADR 0485): a request, one line of
//! JSON, then the answer, one line of JSON for each item, after which
//! alertd closes the connection.
//!
//! | ask       | with      | answer                                          |
//! |-----------|-----------|-------------------------------------------------|
//! | `raise`   | `alert`   | [`Done`]                                        |
//! | `event`   | `alert`   | [`Done`]: told once, kept in the history, never active |
//! | `clear`   | `key`     | [`Done`], also when it was not active           |
//! | `set`     | `alerts`  | [`Done`]: the sender's active alerts not listed are cleared |
//! | `list`    |           | an [`Active`] a line, then [`End`]              |
//! | `history` | `count`   | a [`Change`] a line, oldest first, then [`End`] |
//! | `test`    |           | a [`Delivery`] a line, one per channel, then [`End`] |
//! | `status`  |           | a [`Channel`] a line, then [`End`]              |
//! | `push-key` |          | [`PushKey`]: the key a browser subscribes with  |
//! | `push-add` | `subscribe` | [`Added`]: a browser's subscription, kept    |
//! | `push-remove` | `id`  | [`Done`]                                        |
//! | `push-list` |         | a [`Browser`] a line, then [`End`]              |
//! | `push-about` | `id`, `about` | [`Done`]: the kinds a browser is told of |
//!
//! A request alertd does not take is answered with a [`Refusal`]. Only
//! root's processes may ask, and each alert belongs to the resource
//! domain of the process that raised it, its source: a service's is the
//! service's name. A sender clears and sets only its own.

use serde::{Deserialize, Serialize};

/// The socket alertd answers on.
pub const SOCKET: &str = "/system/live/alerts/socket";

/// The longest a key may be, and a title; a title is one line.
pub const KEY_MAX: usize = 128;
pub const TITLE_MAX: usize = 120;
/// The longest a detail may be, line breaks and all.
pub const DETAIL_MAX: usize = 2000;
/// The longest a request line may be.
pub const REQUEST_MAX: usize = 16 * 1024;

/// The longest word an alert's `about` may be.
pub const ABOUT_MAX: usize = 32;

/// How bad it is.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Something to see to soon, before it costs data: a disk filling,
    /// a backup late.
    #[default]
    Warning,
    /// Something costing data or about to: a disk failing, a RAID
    /// volume without its second copy.
    Critical,
}

impl Severity {
    /// The word for it.
    pub fn name(self) -> &'static str {
        match self {
            Severity::Warning => "warning",
            Severity::Critical => "critical",
        }
    }
}

/// The words of the kinds of alert, as the commands, alerts.toml and the
/// console name them.
pub const ABOUT: [&str; 3] = ["data", "stopped", "hardware"];

/// What an alert is about, so that a channel can be told of some kinds
/// and not others (ADR 0566).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum About {
    /// Data at risk: a disk failing or wearing out, a filesystem nearly
    /// full, a mirror missing a disk, a backup failed or late.
    Data,
    /// Something stopped: a service, an update or a start that failed,
    /// a certificate running out, the kernel stopping, memory running low.
    Stopped,
    /// Hardware: a sensor or a disk running hot, the memory test at the
    /// start.
    Hardware,
}

impl About {
    /// Every kind, in the order of [`ABOUT`].
    pub const ALL: [About; 3] = [About::Data, About::Stopped, About::Hardware];

    /// Its word.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            About::Data => ABOUT[0],
            About::Stopped => ABOUT[1],
            About::Hardware => ABOUT[2],
        }
    }

    /// The kind that `word` names, or none.
    #[must_use]
    pub fn parse(word: &str) -> Option<About> {
        About::ALL.into_iter().find(|about| about.word() == word)
    }

    /// What it is, as a sentence says it: "told of: data at risk".
    #[must_use]
    pub fn phrase(self) -> &'static str {
        match self {
            About::Data => "data at risk",
            About::Stopped => "something stopped",
            About::Hardware => "hardware",
        }
    }

    fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// A set of kinds of alert, in one byte: the kinds a channel is told of.
/// Every kind is the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kinds(u8);

impl Kinds {
    /// Every kind: a channel that is told of all of them.
    pub const ALL: Kinds = Kinds(0b111);

    /// The set of `kinds`.
    #[must_use]
    pub fn of(kinds: &[About]) -> Kinds {
        Kinds(kinds.iter().fold(0, |bits, about| bits | about.bit()))
    }

    /// Whether `about` is in it.
    #[must_use]
    pub fn holds(self, about: About) -> bool {
        self.0 & about.bit() != 0
    }

    /// Whether it is every kind.
    #[must_use]
    pub fn is_all(self) -> bool {
        self == Kinds::ALL
    }

    /// The kinds in it, in the order of [`ABOUT`].
    #[must_use]
    pub fn list(self) -> Vec<About> {
        About::ALL.into_iter().filter(|about| self.holds(*about)).collect()
    }

    /// Its words, as a channel's `about` is written: none for every kind.
    #[must_use]
    pub fn words(self) -> Option<Vec<String>> {
        (!self.is_all()).then(|| self.list().iter().map(|about| about.word().to_string()).collect())
    }

    /// What it is told of, in words: "data at risk, hardware", or "every
    /// kind".
    #[must_use]
    pub fn phrases(self) -> String {
        if self.is_all() {
            return "every kind".into();
        }
        let phrases: Vec<&str> = self.list().iter().map(|about| about.phrase()).collect();
        phrases.join(", ")
    }

    /// The kinds that `words` name, read so that a mistake can only tell
    /// a channel more, never less: a word that is no kind is left out, and
    /// is the second of the answer; when no word is a kind, or there is no
    /// word, it is every kind.
    #[must_use]
    pub fn read(words: &[String]) -> (Kinds, Vec<String>) {
        let (known, unknown): (Vec<_>, Vec<_>) =
            words.iter().partition(|word| About::parse(word).is_some());
        let kinds: Vec<About> = known.iter().filter_map(|word| About::parse(word)).collect();
        let set = if kinds.is_empty() { Kinds::ALL } else { Kinds::of(&kinds) };
        (set, unknown.into_iter().cloned().collect())
    }
}

impl Default for Kinds {
    fn default() -> Kinds {
        Kinds::ALL
    }
}

/// An alert as its source raises it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alert {
    /// The source's own name for it, of `a-z`, `0-9` and `._:/-`, up to
    /// [`KEY_MAX`] long: `fs-full:/data`, `smart:wd-wcc4e1234567`.
    pub key: String,
    pub severity: Severity,
    /// What is wrong, in one line of up to [`TITLE_MAX`] characters:
    /// "/data is 93% full (1.2 TB of 1.3 TB)".
    pub title: String,
    /// What to do about it, up to [`DETAIL_MAX`] characters.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// What it is about: one of [`ABOUT`]'s words, which decides which
    /// channels it is sent to. It is text, not [`About`], so that an
    /// alert with a word this alertd does not know is still read, kept
    /// and sent, as one that has none. None: every channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
}

impl Alert {
    /// An alert of `key` with `title` cut to one line of [`TITLE_MAX`]
    /// characters and `detail` to [`DETAIL_MAX`], so that nothing a
    /// producer reads from a disk or a log makes alertd refuse it. The
    /// key is the caller's to make right ([`key_part`]).
    pub fn new(key: String, severity: Severity, title: &str, detail: &str) -> Alert {
        let line = title.lines().next().unwrap_or_default().trim();
        Alert {
            key,
            severity,
            title: clip(line, TITLE_MAX),
            detail: clip(detail.trim_end(), DETAIL_MAX),
            about: None,
        }
    }

    /// The alert, about `about`.
    #[must_use]
    pub fn about(mut self, about: About) -> Alert {
        self.about = Some(about.word().to_string());
        self
    }

    /// What it is about, when it has a word that is a kind.
    #[must_use]
    pub fn kind(&self) -> Option<About> {
        self.about.as_deref().and_then(About::parse)
    }

    /// What is wrong with it for alertd to refuse it, if anything.
    pub fn invalid(&self) -> Option<String> {
        let key_char =
            |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "._:/-".contains(c);
        if self.key.is_empty() || self.key.len() > KEY_MAX || !self.key.chars().all(key_char) {
            return Some(format!(
                "the key {:?} is not 1 to {KEY_MAX} of a-z, 0-9 and ._:/-",
                self.key
            ));
        }
        if self.title.trim().is_empty() {
            return Some(format!("the alert {} has no title", self.key));
        }
        if self.title.chars().count() > TITLE_MAX || self.title.contains(['\n', '\r']) {
            return Some(format!(
                "the alert {}'s title is not one line of up to {TITLE_MAX} characters",
                self.key
            ));
        }
        if self.detail.chars().count() > DETAIL_MAX {
            return Some(format!(
                "the alert {}'s detail is longer than {DETAIL_MAX} characters",
                self.key
            ));
        }
        // Any word is taken, so that none is lost; it is logged, so it is
        // one short word and no line break.
        if let Some(about) = &self.about
            && (about.chars().count() > ABOUT_MAX || about.chars().any(char::is_control))
        {
            return Some(format!(
                "the alert {}'s about is not a word of up to {ABOUT_MAX} characters",
                self.key
            ));
        }
        None
    }
}

/// `text`, cut to `most` characters, the last of them `…` when cut.
fn clip(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(most - 1).collect();
    cut.push('…');
    cut
}

/// `text` as part of a key: lower case, with each character a key may
/// not have, a space or a `+`, made `-`. A disk's serial number or a
/// destination's name becomes part of a key this way.
pub fn key_part(text: &str) -> String {
    text.trim()
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9' | '.' | '_' | ':' | '/' | '-') => c,
            _ => '-',
        })
        .collect()
}

/// A request to alertd: a line of JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Request {
    /// `raise`, `event`, `clear`, `set`, `list`, `history`, `test`,
    /// `status`, `push-key`, `push-add`, `push-remove`, `push-list` or
    /// `push-about`.
    pub ask: String,
    /// raise and event: the alert.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alert: Option<Alert>,
    /// clear: the key of the sender's alert to clear.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// set: every alert the sender has now; [`None`] for an ask that is
    /// not a set, so that an empty list clears them all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alerts: Option<Vec<Alert>>,
    /// history: the newest this many changes; 20 if not given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    /// event: the sender shows it on the consoles itself, so alertd
    /// sends it to the channels and keeps it in the history without a
    /// notification of its own, which would take the sender's place on
    /// the screen's bottom line.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub shown: bool,
    /// push-add: the browser's subscription.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscribe: Option<Subscribe>,
    /// push-remove and push-about: the ID of the subscription.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// push-about: the kinds of alert it is to be told of, or none for
    /// every kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub about: Option<Vec<String>>,
}

/// A browser's subscription to alerts by Web Push (ADR 0566), as the
/// page that asks for it gives it: what the browser's `PushManager`
/// made, a name for it, and what it is to be told.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Subscribe {
    /// What the person calls it: "Firefox on a phone".
    pub name: String,
    /// Where its messages go: the browser maker's push service.
    pub endpoint: String,
    /// The browser's public key and its authentication secret, base64url.
    pub p256dh: String,
    pub auth: String,
    /// The least severe alert to tell: warning when not given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    /// Whether clears are told: yes when not given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleared: Option<bool>,
    /// The kinds of alert to tell, or none for every kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub about: Option<Vec<String>>,
    /// The account the subscription is for: the one signed in to the
    /// console that asked; root when not given.
    pub by: u32,
}

/// Names nothing that lets one send to the browser: whoever holds the
/// endpoint and the keys can.
impl std::fmt::Debug for Subscribe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscribe").field("name", &self.name).finish_non_exhaustive()
    }
}

/// The answer to push-add: the subscription's ID, which removes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Added {
    pub id: String,
}

/// The answer to push-key: the machine's public key, the uncompressed
/// P-256 point in base64url, which a browser subscribes with
/// (`applicationServerKey`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushKey {
    pub public_key: String,
}

/// A browser that gets alerts, as push-list shows it: never its endpoint
/// or keys, only the host of its push service.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Browser {
    pub id: String,
    pub name: String,
    /// The push service's host: `updates.push.services.mozilla.com`.
    pub host: String,
    /// The account it is for, and when it was added, in milliseconds
    /// since 1970.
    pub by: u32,
    pub added_ms: u64,
    pub severity: Severity,
    pub cleared: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<Vec<String>>,
}

/// The answer to a raise, event, clear or set that alertd took.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    pub ok: bool,
}

/// The line after the last item of a list, history, test or status.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct End {
    pub end: bool,
}

/// Why alertd did not take a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    pub error: String,
}

/// An alert that is active: raised and not yet cleared.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Active {
    /// The resource domain of the process that raised it: a service's
    /// name.
    pub source: String,
    pub alert: Alert,
    /// When it was raised, and when its title, detail or severity last
    /// changed, in seconds since 1970.
    pub since: u64,
    pub changed: u64,
}

/// What a change did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Raised, or raised again from warning to critical.
    Raised,
    Cleared,
    /// Told once and never active: a fallback to the previous version.
    Event,
    /// `alerts test`'s.
    Test,
}

/// A change, as the history keeps it and the channels are told it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// When, in seconds since 1970.
    pub at: u64,
    pub source: String,
    pub state: State,
    pub alert: Alert,
}

/// What a channel did with `alerts test`'s alert.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    /// The channel, as `alerts status` names it: "email to
    /// ops@example.com", "ntfy (ntfy.sh)".
    pub channel: String,
    pub ok: bool,
    /// Why it was not sent, in the words of the server that refused it
    /// where there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The ID of the page that says what to do about it (`ALR-101`), when
    /// alertd knows the kind of the failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_id: Option<String>,
}

/// Where a channel stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub channel: String,
    /// When it last took a message, in seconds since 1970.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok: Option<u64>,
    /// When it last failed to, and why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<(u64, String)>,
    /// The messages waiting to be tried again.
    pub waiting: usize,
    /// What it is: `email`, `ntfy`, `matrix`, `slack`, `discord`, `json`
    /// or `push`.
    #[serde(default)]
    pub kind: String,
    /// The host of its server, or of a browser's push service; never
    /// the address, which can be a secret.
    #[serde(default)]
    pub host: Option<String>,
    /// The least severe alert it is told, and whether it is told when
    /// one clears.
    #[serde(default)]
    pub severity: Severity,
    #[serde(default)]
    pub cleared: bool,
    /// A webhook's place among alerts.toml's `[[webhook]]` tables, from
    /// 1, which `alerts webhook remove` takes.
    #[serde(default)]
    pub number: Option<u32>,
    /// A browser's ID and name.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// The ID of the page for the last failure, when alertd knows its kind.
    #[serde(default)]
    pub error_id: Option<String>,
    /// The kinds of alert it is told of, or none for every kind.
    #[serde(default)]
    pub about: Option<Vec<String>>,
}

impl Channel {
    /// The kinds of alert it is told of in words, "data at risk, hardware",
    /// when that is fewer than every kind.
    #[must_use]
    pub fn told_of(&self) -> Option<String> {
        let words = self.about.as_ref()?;
        let (kinds, _) = Kinds::read(words);
        (!kinds.is_all()).then(|| kinds.phrases())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert(key: &str, title: &str) -> Alert {
        Alert {
            key: key.into(),
            severity: Severity::Warning,
            title: title.into(),
            detail: String::new(),
            about: None,
        }
    }

    #[test]
    fn keys_titles_and_details_are_checked() {
        assert_eq!(alert("fs-full:/data", "/data is 93% full").invalid(), None);
        assert!(alert("smart:WD-1", "x").invalid().is_some(), "upper case");
        assert!(alert("", "x").invalid().is_some());
        assert!(alert(&"k".repeat(KEY_MAX + 1), "x").invalid().is_some());
        assert!(alert("k", " ").invalid().is_some());
        assert!(alert("k", "two\nlines").invalid().is_some());
        assert!(alert("k", &"é".repeat(TITLE_MAX)).invalid().is_none(), "characters, not bytes");
        assert!(alert("k", &"t".repeat(TITLE_MAX + 1)).invalid().is_some());
        let mut long = alert("k", "x");
        long.detail = "d".repeat(DETAIL_MAX + 1);
        assert!(long.invalid().is_some());
    }

    #[test]
    fn requests_leave_out_what_they_do_not_carry() {
        let clear = Request { ask: "clear".into(), key: Some("k".into()), ..Request::default() };
        assert_eq!(serde_json::to_string(&clear).unwrap(), r#"{"ask":"clear","key":"k"}"#);
        let set = Request { ask: "set".into(), alerts: Some(Vec::new()), ..Request::default() };
        assert_eq!(serde_json::to_string(&set).unwrap(), r#"{"ask":"set","alerts":[]}"#);
        let raise: Request = serde_json::from_str(
            r#"{"ask":"raise","alert":{"key":"k","severity":"critical","title":"t"}}"#,
        )
        .unwrap();
        let alert = raise.alert.unwrap();
        assert_eq!((alert.severity, alert.detail.as_str()), (Severity::Critical, ""));
    }

    #[test]
    fn an_alert_is_read_without_about_and_with_a_word_that_is_no_kind_and_written_only_when_set() {
        let line = r#"{"key":"k","severity":"warning","title":"t"}"#;
        let old: Alert = serde_json::from_str(line).unwrap();
        assert_eq!((old.about.as_deref(), old.kind()), (None, None));
        assert_eq!(serde_json::to_string(&old).unwrap(), line);
        let odd: Alert = serde_json::from_str(
            r#"{"key":"k","severity":"warning","title":"t","about":"colour"}"#,
        )
        .unwrap();
        // Kept as text, so that it is still sent; it is none of the kinds.
        assert_eq!((odd.about.as_deref(), odd.kind()), (Some("colour"), None));
        assert_eq!(odd.invalid(), None);
        assert!(serde_json::to_string(&odd).unwrap().ends_with(r#""about":"colour"}"#));
        let tagged = alert("k", "t").about(About::Hardware);
        assert_eq!(
            (tagged.about.as_deref(), tagged.kind()),
            (Some("hardware"), Some(About::Hardware))
        );
        // A new alert is about none until its source says.
        assert_eq!(Alert::new("k".into(), Severity::Warning, "t", "").about, None);
    }

    #[test]
    fn a_word_that_would_forge_a_log_line_or_is_not_a_word_is_refused() {
        let about = |about: &str| Alert { about: Some(about.into()), ..alert("k", "t") };
        assert_eq!(about("data").invalid(), None);
        assert_eq!(about("Colour").invalid(), None, "any word is taken");
        assert!(about("a\nalertd: forged").invalid().is_some());
        assert!(about(&"w".repeat(ABOUT_MAX + 1)).invalid().is_some());
        assert_eq!(about(&"w".repeat(ABOUT_MAX)).invalid(), None);
    }

    #[test]
    fn the_kinds_have_their_words_their_phrases_and_a_set_that_defaults_to_every_kind() {
        assert_eq!(ABOUT, ["data", "stopped", "hardware"]);
        for (about, word, phrase) in [
            (About::Data, "data", "data at risk"),
            (About::Stopped, "stopped", "something stopped"),
            (About::Hardware, "hardware", "hardware"),
        ] {
            assert_eq!(
                (about.word(), about.phrase(), About::parse(word)),
                (word, phrase, Some(about))
            );
        }
        assert_eq!(About::parse("Data"), None);
        assert_eq!(About::parse("sign-ins"), None);
        assert_eq!(Kinds::default(), Kinds::ALL);
        assert!(Kinds::ALL.is_all() && Kinds::ALL.words().is_none());
        assert_eq!(Kinds::ALL.phrases(), "every kind");
        let two = Kinds::of(&[About::Hardware, About::Data]);
        assert!(two.holds(About::Data) && two.holds(About::Hardware) && !two.holds(About::Stopped));
        assert_eq!(two.words(), Some(vec!["data".to_string(), "hardware".to_string()]));
        assert_eq!(two.phrases(), "data at risk, hardware");
        assert_eq!(Kinds::of(&About::ALL), Kinds::ALL);
    }

    #[test]
    fn kinds_are_read_from_words_so_that_a_mistake_never_narrows() {
        let read = |words: &[&str]| {
            Kinds::read(&words.iter().map(ToString::to_string).collect::<Vec<_>>())
        };
        assert_eq!(read(&["data"]), (Kinds::of(&[About::Data]), Vec::new()));
        assert_eq!(
            read(&["stopped", "colour", "data", "colour"]),
            (Kinds::of(&[About::Data, About::Stopped]), vec!["colour".to_string(); 2])
        );
        assert_eq!(read(&[]), (Kinds::ALL, Vec::new()));
        assert_eq!(read(&["colour"]), (Kinds::ALL, vec!["colour".to_string()]));
    }

    #[test]
    fn a_channel_says_what_it_is_told_of_only_when_that_is_fewer_than_every_kind() {
        let channel = |about: Option<Vec<&str>>| Channel {
            channel: "email to ops".into(),
            last_ok: None,
            last_error: None,
            waiting: 0,
            kind: "email".into(),
            host: None,
            severity: Severity::Warning,
            cleared: true,
            number: None,
            id: None,
            name: None,
            error_id: None,
            about: about.map(|words| words.into_iter().map(String::from).collect()),
        };
        assert_eq!(channel(None).told_of(), None);
        assert_eq!(channel(Some(vec!["data", "stopped", "hardware"])).told_of(), None);
        assert_eq!(
            channel(Some(vec!["data", "hardware"])).told_of().as_deref(),
            Some("data at risk, hardware")
        );
        // A word this alertd does not know narrows nothing.
        assert_eq!(channel(Some(vec!["colour"])).told_of(), None);
    }

    #[test]
    fn severities_order_from_warning_to_critical() {
        assert!(Severity::Warning < Severity::Critical);
        assert_eq!(serde_json::to_string(&State::Cleared).unwrap(), r#""cleared""#);
    }

    #[test]
    fn a_new_alert_is_cut_to_what_alertd_takes() {
        let long = "x".repeat(TITLE_MAX + 10);
        let made = Alert::new("k".into(), Severity::Warning, &format!("{long}\nmore"), "d\n");
        assert_eq!(made.title.chars().count(), TITLE_MAX);
        assert!(made.title.ends_with('…'));
        assert_eq!(made.detail, "d");
        assert_eq!(made.invalid(), None);
        let made = Alert::new("k".into(), Severity::Warning, "t", &"y".repeat(DETAIL_MAX * 2));
        assert_eq!(made.detail.chars().count(), DETAIL_MAX);
        assert_eq!(made.invalid(), None);
    }

    #[test]
    fn any_text_becomes_a_key_part() {
        assert_eq!(key_part("WD-WCC4E1234567"), "wd-wcc4e1234567");
        assert_eq!(key_part(" QM00001 "), "qm00001");
        assert_eq!(key_part("Off Site+2"), "off-site-2");
        let alert = Alert::new(format!("smart:{}", key_part("S/N ÄB")), Severity::Warning, "t", "");
        assert_eq!(alert.invalid(), None);
    }
}
