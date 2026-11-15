// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the web console asks authd(8) (ADR 0514): signing people in with
//! a password and a second step, starting each session's worker,
//! unlocking a session to make changes, running its elevated commands,
//! checking API tokens, and each person's own second steps and tokens.
//!
//! These are [`Request::Console`](crate::request::Request::Console)'s,
//! sent as any request is, with `console` and a verb first:
//!
//! ```text
//! console\0begin\0192.0.2.7\0chase\0secret\0
//! console\0trusted\09f86d081...\0e3b0c442...\0
//! console\0code\09f86d081...\0123456\0
//! console\0challenge\0sign-in\09f86d081...\0duke-nas.local\0
//! console\0passkey\0alone\0192.0.2.7\0<challenge>\0https://duke-nas.local:9090\0duke-nas.local\0<id>\0<client data>\0<authenticator data>\0<signature>\0<user handle>\0
//! console\0start\09f86d081...\0trust\0Firefox on Linux\0
//! console\0unlock\03f9a...\0password\0secret\0
//! console\0extend\03f9a...\0
//! console\0elevate\0session\03f9a...\0LANG=C.UTF-8\0--\0service\0restart\0nginx\0
//! console\0token\0dkv_ansible_...\0192.0.2.7\0
//! console\0terminal\03f9a...\0own\0xterm-256color\0LANG=en_US.UTF-8\0
//! console\0follow\03f9a...\0all\0nginx\0\0warning\0timed out\0
//! console\0follow-end\03f9a...\04321\0
//! console\0steps\0
//! console\0token-add\0\0ansible\0*\0365\0
//! ```
//!
//! Binary fields (challenges, IDs, client data, authenticator data,
//! signatures, user handles, tickets and tokens) go as hexadecimal, so
//! that no NUL in them ends a field. The first group of requests is
//! admind's, which runs as root, to ask; the second, from `steps` on,
//! the person's own, about their own account, asked by their worker or
//! by `admin` at their terminal.

use crate::request::hex;
use crate::wipe;

/// Why a passkey is asked for.
#[derive(Clone, PartialEq, Eq)]
pub enum Purpose {
    /// The second step of the sign-in with this ticket.
    SignIn(Vec<u8>),
    /// A sign-in with a passkey alone, which names its account, for a
    /// browser at this address.
    Alone(String),
    /// Unlocking the session with this ID.
    Unlock(String),
}

/// A passkey's answer to a challenge, as the browser gave it.
#[derive(Clone, PartialEq, Eq)]
pub struct Assertion {
    /// The challenge it answers.
    pub challenge: Vec<u8>,
    /// The console's origin the browser was at.
    pub origin: String,
    /// The relying party's ID.
    pub relying_party: String,
    /// The passkey's credential ID.
    pub credential: Vec<u8>,
    /// The browser's client data, as JSON.
    pub client_data: Vec<u8>,
    /// The authenticator's data.
    pub authenticator_data: Vec<u8>,
    /// The signature.
    pub signature: Vec<u8>,
    /// The user handle, which a passkey alone gives.
    pub user_handle: Vec<u8>,
}

/// How a person proves again that it is them, to unlock a session.
#[derive(Clone, PartialEq, Eq)]
pub enum Proof {
    /// Their password.
    Password(Vec<u8>),
    /// A code from their authenticator app.
    Code(Vec<u8>),
    /// A passkey, answering a challenge given for [`Purpose::Unlock`].
    Passkey(Assertion),
}

/// Whose right an elevated command runs by.
#[derive(Clone, PartialEq, Eq)]
pub enum Elevator {
    /// An unlocked session's, by its ID.
    Session(String),
    /// An API token's, for an action of one area of the console.
    Token {
        /// The token, as the request brought it.
        token: Vec<u8>,
        /// The action's area, which the token's scopes must name, or be
        /// `*`.
        area: String,
    },
}

/// What a follow of the log shows (ADR 0518): `log`'s options, each
/// left out when it is not given. authd checks them and writes the
/// command itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    /// One service's records (`--service`).
    pub service: Option<String>,
    /// The kernel's records (`--kernel`).
    pub kernel: bool,
    /// The least urgent level shown (`--priority`).
    pub priority: Option<String>,
    /// What a record's text matches (`--grep`).
    pub grep: Option<String>,
}

/// A request of the web console's.
#[derive(Clone, PartialEq, Eq)]
pub enum Console {
    /// Begin signing `name` in with `password`, for a browser at `from`
    /// (an address). The answer is `ok done TICKET` when no second step
    /// is needed, `ok step TICKET KINDS` when one is (`KINDS` from `app`,
    /// `passkey` and `recovery`, joined by commas), or `ok needs-step
    /// TICKET` when the sign-in rules ask for one the account has not
    /// got; a wrong password is answered once the next try may be made.
    Begin {
        /// The browser's address.
        from: String,
        /// The account.
        name: String,
        /// The password typed.
        password: Vec<u8>,
    },
    /// Skip the second step of sign-in `ticket` for a browser that
    /// "Trust this browser for 30 days" gave `token`.
    Trusted {
        /// The sign-in's ticket.
        ticket: Vec<u8>,
        /// The browser's token.
        token: Vec<u8>,
    },
    /// An app's code or a recovery code for sign-in `ticket`.
    Code {
        /// The sign-in's ticket.
        ticket: Vec<u8>,
        /// The code typed.
        code: Vec<u8>,
    },
    /// A passkey challenge, for `purpose`, for relying party
    /// `relying_party`. The answer is `ok` and the challenge, then the
    /// IDs of the account's passkeys for that relying party, all in
    /// hexadecimal, separated by spaces (none for [`Purpose::Alone`]).
    Challenge {
        /// What it is for.
        purpose: Purpose,
        /// The relying party's ID.
        relying_party: String,
    },
    /// A passkey's answer, for `purpose`. A passkey alone is answered as
    /// `begin` is when no step is needed: `ok done TICKET NAME`.
    Passkey {
        /// What it is for.
        purpose: Purpose,
        /// The answer.
        assertion: Assertion,
    },
    /// Start the session of sign-in `ticket`, once done: its worker,
    /// with the three descriptors sent: the pipe it reads admind's calls
    /// from, the pipe it writes replies to, and the log.
    /// With `trust`, the browser, described by `browser`, is trusted for
    /// 30 days. The answer is `ok PROCESS SESSION`, and the browser's
    /// token when it is trusted.
    Start {
        /// The sign-in's ticket.
        ticket: Vec<u8>,
        /// Whether to trust the browser for 30 days.
        trust: bool,
        /// The browser, in the person's words for it: "Firefox on Linux".
        browser: String,
    },
    /// Unlock session `session` for 15 minutes. The answer is `ok` and
    /// when it locks, in Unix seconds.
    Unlock {
        /// The session.
        session: String,
        /// How the person proved it is them.
        proof: Proof,
    },
    /// Keep session `session` unlocked for 15 minutes from now, while it
    /// is unlocked.
    Extend {
        /// The session.
        session: String,
    },
    /// Lock session `session` now.
    Lock {
        /// The session.
        session: String,
    },
    /// Session `session` ended: signed out, or timed out.
    End {
        /// The session.
        session: String,
    },
    /// Run `command` as an administrator, as `elevate` does, by `by`'s
    /// right, with the four descriptors sent: its 0, 1 and 2, and the
    /// pipe of signals. Answered as `elevate` is.
    Elevate {
        /// Whose right.
        by: Elevator,
        /// The language settings, as `NAME=value`.
        language: Vec<String>,
        /// The command and its arguments.
        command: Vec<Vec<u8>>,
    },
    /// Whether `token` is an API token, used from `from`. The answer is
    /// `ok NAME SCOPES UNTIL`.
    Token {
        /// The token.
        token: Vec<u8>,
        /// The address it came from.
        from: String,
    },
    /// Start a worker for `token`'s owner, as [`Console::Start`] does.
    TokenStart {
        /// The token.
        token: Vec<u8>,
    },
    /// Start a shell for session `session` on the terminal sent, the one
    /// descriptor, whose other side admind keeps (ADR 0517): the person's
    /// own login shell, as an SSH session's on a terminal, or, with
    /// `admin`, root's login shell for an administrator whose session is
    /// unlocked, which ends when the session locks. The answer is `ok
    /// PROCESS`; authd does not hold the request.
    Terminal {
        /// The console session.
        session: String,
        /// Whether it is root's shell.
        admin: bool,
        /// The terminal's type, as `TERM`.
        term: String,
        /// The browser's language settings, as `NAME=value`.
        language: Vec<String>,
    },
    /// Follow the log for session `session` (ADR 0518): `log --output
    /// json --follow --lines 0` with `filter`'s options, which authd checks
    /// and writes itself, on the two descriptors sent, the write ends of
    /// the pipes admind reads its output and its errors from. As the
    /// person, or with `all` as root for an administrator whose session is
    /// unlocked, which ends when the session locks. The answer is `ok
    /// PROCESS`; authd does not hold the request.
    Follow {
        /// The console session.
        session: String,
        /// Whether the whole log is read, as root.
        all: bool,
        /// What is shown of it.
        filter: Filter,
    },
    /// End the follow `process` of session `session`, which admind no
    /// longer reads (ADR 0518).
    FollowEnd {
        /// The console session.
        session: String,
        /// The follow's process, as `follow` answered it.
        process: i32,
    },
    /// The asker's own second steps and tokens, as lines of the answer.
    Steps,
    /// Begin adding an authenticator app: the answer is `ok` and the
    /// `otpauth://` URI, for the machine named `host`.
    AppBegin {
        /// The asker's password, or empty when they unlocked their
        /// session.
        proof: Vec<u8>,
        /// The machine's name, for the app to show.
        host: String,
    },
    /// Finish adding the app with a code from it. The first second step
    /// also makes the recovery codes, which the answer gives, once.
    AppConfirm {
        /// The code.
        code: Vec<u8>,
    },
    /// Remove the authenticator app.
    AppRemove {
        /// As for [`Console::AppBegin`].
        proof: Vec<u8>,
    },
    /// Begin adding a passkey for `relying_party`: the answer is `ok`, the
    /// challenge, the user handle, and the IDs of the asker's passkeys
    /// there already, in hexadecimal, separated by spaces.
    PasskeyBegin {
        /// As for [`Console::AppBegin`].
        proof: Vec<u8>,
        /// The relying party's ID.
        relying_party: String,
    },
    /// Add the passkey the browser made, named `name`.
    PasskeyAdd {
        /// The challenge it answers.
        challenge: Vec<u8>,
        /// The console's origin.
        origin: String,
        /// The relying party's ID.
        relying_party: String,
        /// The person's name for it.
        name: String,
        /// The browser's client data.
        client_data: Vec<u8>,
        /// The attestation object.
        attestation: Vec<u8>,
    },
    /// Remove the passkey with credential ID `id`.
    PasskeyRemove {
        /// As for [`Console::AppBegin`].
        proof: Vec<u8>,
        /// Its ID.
        id: Vec<u8>,
    },
    /// Rename the passkey with credential ID `id`.
    PasskeyRename {
        /// Its ID.
        id: Vec<u8>,
        /// Its new name.
        name: String,
    },
    /// Make 10 new recovery codes, in place of those there are: the
    /// answer gives them, once.
    Codes {
        /// As for [`Console::AppBegin`].
        proof: Vec<u8>,
    },
    /// Trust no browser of the asker's any more.
    TrustedForget,
    /// Make an API token: the answer gives it, once.
    TokenAdd {
        /// As for [`Console::AppBegin`].
        proof: Vec<u8>,
        /// Its name.
        name: String,
        /// The areas it may act in, or `*`.
        scopes: Vec<String>,
        /// The days it lasts, 1 to 366.
        days: u32,
    },
    /// Remove the API token named `name`.
    TokenRemove {
        /// Its name.
        name: String,
    },
}

/// The bytes of hexadecimal `text`, or `None`.
pub fn from_hex(text: &[u8]) -> Option<Vec<u8>> {
    let (pairs, odd) = text.as_chunks::<2>();
    if !odd.is_empty() {
        return None;
    }
    let digit = |byte: u8| char::from(byte).to_digit(16);
    pairs.iter().map(|&[high, low]| u8::try_from(digit(high)? * 16 + digit(low)?).ok()).collect()
}

impl Purpose {
    fn fields(&self) -> (&'static [u8], Vec<u8>) {
        match self {
            Purpose::SignIn(ticket) => (b"sign-in", hex(ticket).into_bytes()),
            Purpose::Alone(from) => (b"alone", from.as_bytes().to_vec()),
            Purpose::Unlock(session) => (b"unlock", session.as_bytes().to_vec()),
        }
    }

    fn decode(kind: &[u8], value: &[u8]) -> Option<Purpose> {
        match kind {
            b"sign-in" => Some(Purpose::SignIn(from_hex(value)?)),
            b"alone" => Some(Purpose::Alone(String::from_utf8(value.to_vec()).ok()?)),
            b"unlock" => Some(Purpose::Unlock(String::from_utf8(value.to_vec()).ok()?)),
            _ => None,
        }
    }
}

impl Assertion {
    fn fields(&self) -> Vec<Vec<u8>> {
        vec![
            hex(&self.challenge).into_bytes(),
            self.origin.as_bytes().to_vec(),
            self.relying_party.as_bytes().to_vec(),
            hex(&self.credential).into_bytes(),
            hex(&self.client_data).into_bytes(),
            hex(&self.authenticator_data).into_bytes(),
            hex(&self.signature).into_bytes(),
            hex(&self.user_handle).into_bytes(),
        ]
    }

    fn decode(fields: &[&[u8]]) -> Option<Assertion> {
        let [challenge, origin, party, credential, client, data, signature, handle] = fields else {
            return None;
        };
        Some(Assertion {
            challenge: from_hex(challenge)?,
            origin: String::from_utf8(origin.to_vec()).ok()?,
            relying_party: String::from_utf8(party.to_vec()).ok()?,
            credential: from_hex(credential)?,
            client_data: from_hex(client)?,
            authenticator_data: from_hex(data)?,
            signature: from_hex(signature)?,
            user_handle: from_hex(handle)?,
        })
    }
}

impl Console {
    /// Its fields after `console`, as [`Request::encode`] sends them.
    ///
    /// [`Request::encode`]: crate::request::Request::encode
    pub fn fields(&self) -> Vec<Vec<u8>> {
        let words = |words: &[&[u8]]| words.iter().map(|w| w.to_vec()).collect::<Vec<_>>();
        match self {
            Console::Begin { from, name, password } => {
                words(&[b"begin", from.as_bytes(), name.as_bytes(), password])
            }
            Console::Trusted { ticket, token } => {
                words(&[b"trusted", hex(ticket).as_bytes(), hex(token).as_bytes()])
            }
            Console::Code { ticket, code } => words(&[b"code", hex(ticket).as_bytes(), code]),
            Console::Challenge { purpose, relying_party } => {
                let (kind, value) = purpose.fields();
                words(&[b"challenge", kind, &value, relying_party.as_bytes()])
            }
            Console::Passkey { purpose, assertion } => {
                let (kind, value) = purpose.fields();
                let mut fields = words(&[b"passkey", kind, &value]);
                fields.extend(assertion.fields());
                fields
            }
            Console::Start { ticket, trust, browser } => words(&[
                b"start",
                hex(ticket).as_bytes(),
                if *trust { b"trust" } else { b"once" },
                browser.as_bytes(),
            ]),
            Console::Unlock { session, proof } => {
                let mut fields = words(&[b"unlock", session.as_bytes()]);
                match proof {
                    Proof::Password(password) => fields.extend(words(&[b"password", password])),
                    Proof::Code(code) => fields.extend(words(&[b"code", code])),
                    Proof::Passkey(assertion) => {
                        fields.push(b"passkey".to_vec());
                        fields.extend(assertion.fields());
                    }
                }
                fields
            }
            Console::Extend { session } => words(&[b"extend", session.as_bytes()]),
            Console::Lock { session } => words(&[b"lock", session.as_bytes()]),
            Console::End { session } => words(&[b"end", session.as_bytes()]),
            Console::Elevate { by, language, command } => {
                let mut fields = match by {
                    Elevator::Session(session) => {
                        words(&[b"elevate", b"session", session.as_bytes()])
                    }
                    Elevator::Token { token, area } => {
                        words(&[b"elevate", b"token", hex(token).as_bytes(), area.as_bytes()])
                    }
                };
                fields.extend(language.iter().map(|s| s.as_bytes().to_vec()));
                fields.push(b"--".to_vec());
                fields.extend(command.iter().cloned());
                fields
            }
            Console::Token { token, from } => {
                words(&[b"token", hex(token).as_bytes(), from.as_bytes()])
            }
            Console::TokenStart { token } => words(&[b"token-start", hex(token).as_bytes()]),
            Console::Terminal { session, admin, term, language } => {
                let whose: &[u8] = if *admin { b"admin" } else { b"own" };
                let mut fields = words(&[b"terminal", session.as_bytes(), whose, term.as_bytes()]);
                fields.extend(language.iter().map(|s| s.as_bytes().to_vec()));
                fields
            }
            Console::Follow { session, all, filter } => {
                let whose: &[u8] = if *all { b"all" } else { b"own" };
                let kernel: &[u8] = if filter.kernel { b"kernel" } else { b"" };
                let given =
                    |text: &Option<String>| text.as_deref().unwrap_or("").as_bytes().to_vec();
                let mut fields = words(&[b"follow", session.as_bytes(), whose]);
                fields.extend([given(&filter.service), kernel.to_vec()]);
                fields.extend([given(&filter.priority), given(&filter.grep)]);
                fields
            }
            Console::FollowEnd { session, process } => {
                words(&[b"follow-end", session.as_bytes(), process.to_string().as_bytes()])
            }
            Console::Steps => words(&[b"steps"]),
            Console::AppBegin { proof, host } => words(&[b"app-begin", proof, host.as_bytes()]),
            Console::AppConfirm { code } => words(&[b"app-confirm", code]),
            Console::AppRemove { proof } => words(&[b"app-remove", proof]),
            Console::PasskeyBegin { proof, relying_party } => {
                words(&[b"passkey-begin", proof, relying_party.as_bytes()])
            }
            Console::PasskeyAdd {
                challenge,
                origin,
                relying_party,
                name,
                client_data,
                attestation,
            } => words(&[
                b"passkey-add",
                hex(challenge).as_bytes(),
                origin.as_bytes(),
                relying_party.as_bytes(),
                name.as_bytes(),
                hex(client_data).as_bytes(),
                hex(attestation).as_bytes(),
            ]),
            Console::PasskeyRemove { proof, id } => {
                words(&[b"passkey-remove", proof, hex(id).as_bytes()])
            }
            Console::PasskeyRename { id, name } => {
                words(&[b"passkey-rename", hex(id).as_bytes(), name.as_bytes()])
            }
            Console::Codes { proof } => words(&[b"codes", proof]),
            Console::TrustedForget => words(&[b"trusted-forget"]),
            Console::TokenAdd { proof, name, scopes, days } => words(&[
                b"token-add",
                proof,
                name.as_bytes(),
                scopes.join(",").as_bytes(),
                days.to_string().as_bytes(),
            ]),
            Console::TokenRemove { name } => words(&[b"token-remove", name.as_bytes()]),
        }
    }

    /// The request whose fields after `console` are `fields`.
    pub fn decode(fields: &[&[u8]]) -> Option<Console> {
        let text = |field: &[u8]| String::from_utf8(field.to_vec()).ok();
        Some(match *fields {
            [b"begin", from, name, password] => {
                Console::Begin { from: text(from)?, name: text(name)?, password: password.to_vec() }
            }
            [b"trusted", ticket, token] => {
                Console::Trusted { ticket: from_hex(ticket)?, token: from_hex(token)? }
            }
            [b"code", ticket, code] => {
                Console::Code { ticket: from_hex(ticket)?, code: code.to_vec() }
            }
            [b"challenge", kind, value, party] => Console::Challenge {
                purpose: Purpose::decode(kind, value)?,
                relying_party: text(party)?,
            },
            [b"passkey", kind, value, ref assertion @ ..] => Console::Passkey {
                purpose: Purpose::decode(kind, value)?,
                assertion: Assertion::decode(assertion)?,
            },
            [b"start", ticket, trust @ (b"trust" | b"once"), browser] => Console::Start {
                ticket: from_hex(ticket)?,
                trust: trust == b"trust",
                browser: text(browser)?,
            },
            [b"unlock", session, b"password", password] => Console::Unlock {
                session: text(session)?,
                proof: Proof::Password(password.to_vec()),
            },
            [b"unlock", session, b"code", code] => {
                Console::Unlock { session: text(session)?, proof: Proof::Code(code.to_vec()) }
            }
            [b"unlock", session, b"passkey", ref assertion @ ..] => Console::Unlock {
                session: text(session)?,
                proof: Proof::Passkey(Assertion::decode(assertion)?),
            },
            [b"extend", session] => Console::Extend { session: text(session)? },
            [b"lock", session] => Console::Lock { session: text(session)? },
            [b"end", session] => Console::End { session: text(session)? },
            [b"elevate", kind, by, ref rest @ ..] => {
                let (by, rest) = match kind {
                    b"session" => (Elevator::Session(text(by)?), rest),
                    b"token" => {
                        let (area, rest) = rest.split_first()?;
                        (Elevator::Token { token: from_hex(by)?, area: text(area)? }, rest)
                    }
                    _ => return None,
                };
                let split = rest.iter().position(|field| *field == b"--")?;
                let language = rest[..split].iter().map(|f| text(f)).collect::<Option<Vec<_>>>()?;
                let command: Vec<Vec<u8>> = rest[split + 1..].iter().map(|f| f.to_vec()).collect();
                if command.is_empty() {
                    return None;
                }
                Console::Elevate { by, language, command }
            }
            [b"token", token, from] => {
                Console::Token { token: from_hex(token)?, from: text(from)? }
            }
            [b"token-start", token] => Console::TokenStart { token: from_hex(token)? },
            [b"terminal", session, whose @ (b"own" | b"admin"), term, ref language @ ..] => {
                Console::Terminal {
                    session: text(session)?,
                    admin: whose == b"admin",
                    term: text(term)?,
                    language: language.iter().map(|f| text(f)).collect::<Option<Vec<_>>>()?,
                }
            }
            [
                b"follow",
                session,
                whose @ (b"own" | b"all"),
                service,
                kernel @ (b"" | b"kernel"),
                priority,
                grep,
            ] => {
                let given = |field: &[u8]| -> Option<Option<String>> {
                    if field.is_empty() { Some(None) } else { text(field).map(Some) }
                };
                Console::Follow {
                    session: text(session)?,
                    all: whose == b"all",
                    filter: Filter {
                        service: given(service)?,
                        kernel: kernel == b"kernel",
                        priority: given(priority)?,
                        grep: given(grep)?,
                    },
                }
            }
            [b"follow-end", session, process] => Console::FollowEnd {
                session: text(session)?,
                process: text(process)?.parse().ok().filter(|process: &i32| *process > 0)?,
            },
            [b"steps"] => Console::Steps,
            [b"app-begin", proof, host] => {
                Console::AppBegin { proof: proof.to_vec(), host: text(host)? }
            }
            [b"app-confirm", code] => Console::AppConfirm { code: code.to_vec() },
            [b"app-remove", proof] => Console::AppRemove { proof: proof.to_vec() },
            [b"passkey-begin", proof, party] => {
                Console::PasskeyBegin { proof: proof.to_vec(), relying_party: text(party)? }
            }
            [b"passkey-add", challenge, origin, party, name, client, attestation] => {
                Console::PasskeyAdd {
                    challenge: from_hex(challenge)?,
                    origin: text(origin)?,
                    relying_party: text(party)?,
                    name: text(name)?,
                    client_data: from_hex(client)?,
                    attestation: from_hex(attestation)?,
                }
            }
            [b"passkey-remove", proof, id] => {
                Console::PasskeyRemove { proof: proof.to_vec(), id: from_hex(id)? }
            }
            [b"passkey-rename", id, name] => {
                Console::PasskeyRename { id: from_hex(id)?, name: text(name)? }
            }
            [b"codes", proof] => Console::Codes { proof: proof.to_vec() },
            [b"trusted-forget"] => Console::TrustedForget,
            [b"token-add", proof, name, scopes, days] => Console::TokenAdd {
                proof: proof.to_vec(),
                name: text(name)?,
                scopes: text(scopes)?
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect(),
                days: text(days)?.parse().ok()?,
            },
            [b"token-remove", name] => Console::TokenRemove { name: text(name)? },
            _ => return None,
        })
    }

    /// Whether admind, which runs as root, asks it, rather than the
    /// person whose account it is about.
    pub fn is_admind(&self) -> bool {
        matches!(
            self,
            Console::Begin { .. }
                | Console::Trusted { .. }
                | Console::Code { .. }
                | Console::Challenge { .. }
                | Console::Passkey { .. }
                | Console::Start { .. }
                | Console::Unlock { .. }
                | Console::Extend { .. }
                | Console::Lock { .. }
                | Console::End { .. }
                | Console::Elevate { .. }
                | Console::Token { .. }
                | Console::TokenStart { .. }
                | Console::Terminal { .. }
                | Console::Follow { .. }
                | Console::FollowEnd { .. }
        )
    }

    /// Its secrets wiped: passwords, codes, tickets and tokens.
    pub fn wipe(&mut self) {
        match self {
            Console::Begin { password, .. } => wipe(password),
            Console::Trusted { ticket, token } => {
                wipe(ticket);
                wipe(token);
            }
            Console::Code { ticket, code } => {
                wipe(ticket);
                wipe(code);
            }
            Console::Challenge { purpose: Purpose::SignIn(ticket), .. }
            | Console::Passkey { purpose: Purpose::SignIn(ticket), .. }
            | Console::Start { ticket, .. } => wipe(ticket),
            Console::Unlock { proof: Proof::Password(secret) | Proof::Code(secret), .. }
            | Console::Elevate { by: Elevator::Token { token: secret, .. }, .. }
            | Console::Token { token: secret, .. }
            | Console::TokenStart { token: secret }
            | Console::AppBegin { proof: secret, .. }
            | Console::AppConfirm { code: secret }
            | Console::AppRemove { proof: secret }
            | Console::PasskeyBegin { proof: secret, .. }
            | Console::PasskeyRemove { proof: secret, .. }
            | Console::Codes { proof: secret }
            | Console::TokenAdd { proof: secret, .. } => wipe(secret),
            Console::Challenge { .. }
            | Console::Passkey { .. }
            | Console::Unlock { proof: Proof::Passkey(_), .. }
            | Console::Extend { .. }
            | Console::Lock { .. }
            | Console::End { .. }
            | Console::Elevate { by: Elevator::Session(_), .. }
            | Console::Terminal { .. }
            | Console::Follow { .. }
            | Console::FollowEnd { .. }
            | Console::Steps
            | Console::PasskeyAdd { .. }
            | Console::PasskeyRename { .. }
            | Console::TrustedForget
            | Console::TokenRemove { .. } => {}
        }
    }

    /// Its verb, for the log: what it is, never what it holds.
    pub fn verb(&self) -> &'static str {
        match self {
            Console::Begin { .. } => "begin",
            Console::Trusted { .. } => "trusted",
            Console::Code { .. } => "code",
            Console::Challenge { .. } => "challenge",
            Console::Passkey { .. } => "passkey",
            Console::Start { .. } => "start",
            Console::Unlock { .. } => "unlock",
            Console::Extend { .. } => "extend",
            Console::Lock { .. } => "lock",
            Console::End { .. } => "end",
            Console::Elevate { .. } => "elevate",
            Console::Token { .. } => "token",
            Console::TokenStart { .. } => "token-start",
            Console::Terminal { .. } => "terminal",
            Console::Follow { .. } => "follow",
            Console::FollowEnd { .. } => "follow-end",
            Console::Steps => "steps",
            Console::AppBegin { .. } => "app-begin",
            Console::AppConfirm { .. } => "app-confirm",
            Console::AppRemove { .. } => "app-remove",
            Console::PasskeyBegin { .. } => "passkey-begin",
            Console::PasskeyAdd { .. } => "passkey-add",
            Console::PasskeyRemove { .. } => "passkey-remove",
            Console::PasskeyRename { .. } => "passkey-rename",
            Console::Codes { .. } => "codes",
            Console::TrustedForget => "trusted-forget",
            Console::TokenAdd { .. } => "token-add",
            Console::TokenRemove { .. } => "token-remove",
        }
    }
}

impl std::fmt::Debug for Console {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Names and places only: never a password, code, ticket or token.
        match self {
            Console::Begin { from, name, .. } => {
                write!(f, "Console::Begin {{ from: {from:?}, name: {name:?} }}")
            }
            Console::Elevate { command, .. } => {
                let command: Vec<_> = command.iter().map(|w| String::from_utf8_lossy(w)).collect();
                write!(f, "Console::Elevate {{ command: {command:?} }}")
            }
            Console::Token { from, .. } => write!(f, "Console::Token {{ from: {from:?} }}"),
            Console::PasskeyRename { name, .. } | Console::PasskeyAdd { name, .. } => {
                write!(f, "Console::{} {{ name: {name:?} }}", self.verb())
            }
            Console::TokenAdd { name, scopes, days, .. } => write!(
                f,
                "Console::TokenAdd {{ name: {name:?}, scopes: {scopes:?}, days: {days} }}"
            ),
            Console::TokenRemove { name } => write!(f, "Console::TokenRemove {{ name: {name:?} }}"),
            _ => write!(f, "Console::{}", self.verb()),
        }
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        self.wipe();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assertion() -> Assertion {
        Assertion {
            challenge: vec![1; 32],
            origin: "https://duke-nas.local:9090".into(),
            relying_party: "duke-nas.local".into(),
            credential: vec![0, 2, 0],
            client_data: br#"{"type":"webauthn.get"}"#.to_vec(),
            authenticator_data: vec![0; 37],
            signature: vec![0x30, 0x45, 0],
            user_handle: Vec::new(),
        }
    }

    fn round_trip(request: &Console) -> Option<Console> {
        let fields = request.fields();
        let borrowed: Vec<&[u8]> = fields.iter().map(Vec::as_slice).collect();
        Console::decode(&borrowed)
    }

    #[test]
    fn every_request_reads_back_as_sent() {
        let requests = [
            Console::Begin {
                from: "192.0.2.7".into(),
                name: "chase".into(),
                password: b"a\x01b".to_vec(),
            },
            Console::Trusted { ticket: vec![9; 32], token: vec![0; 32] },
            Console::Code { ticket: vec![9; 32], code: b"123456".to_vec() },
            Console::Challenge {
                purpose: Purpose::SignIn(vec![9; 32]),
                relying_party: "duke-nas.local".into(),
            },
            Console::Challenge {
                purpose: Purpose::Alone("192.0.2.7".into()),
                relying_party: "example.org".into(),
            },
            Console::Challenge {
                purpose: Purpose::Unlock("3f9a".into()),
                relying_party: "example.org".into(),
            },
            Console::Passkey {
                purpose: Purpose::Alone("192.0.2.7".into()),
                assertion: assertion(),
            },
            Console::Passkey { purpose: Purpose::SignIn(vec![1]), assertion: assertion() },
            Console::Start { ticket: vec![9; 32], trust: true, browser: "Firefox on Linux".into() },
            Console::Start { ticket: vec![9; 32], trust: false, browser: String::new() },
            Console::Unlock { session: "3f9a".into(), proof: Proof::Password(b"secret".to_vec()) },
            Console::Unlock { session: "3f9a".into(), proof: Proof::Code(b"123456".to_vec()) },
            Console::Unlock { session: "3f9a".into(), proof: Proof::Passkey(assertion()) },
            Console::Extend { session: "3f9a".into() },
            Console::Lock { session: "3f9a".into() },
            Console::End { session: "3f9a".into() },
            Console::Elevate {
                by: Elevator::Session("3f9a".into()),
                language: vec!["LANG=C.UTF-8".into()],
                command: vec![b"service".to_vec(), b"restart".to_vec(), b"--".to_vec()],
            },
            Console::Elevate {
                by: Elevator::Token { token: vec![5; 40], area: "services".into() },
                language: vec![],
                command: vec![b"id".to_vec()],
            },
            Console::Token { token: vec![5; 40], from: "2001:db8::7".into() },
            Console::TokenStart { token: vec![5; 40] },
            Console::Terminal {
                session: "3f9a".into(),
                admin: false,
                term: "xterm-256color".into(),
                language: vec!["LANG=en_US.UTF-8".into(), "LC_TIME=de_DE.UTF-8".into()],
            },
            Console::Terminal {
                session: "3f9a".into(),
                admin: true,
                term: "xterm-256color".into(),
                language: vec![],
            },
            Console::Follow { session: "3f9a".into(), all: false, filter: Filter::default() },
            Console::Follow {
                session: "3f9a".into(),
                all: true,
                filter: Filter {
                    service: Some("nginx".into()),
                    kernel: false,
                    priority: Some("warning".into()),
                    grep: Some("timed out".into()),
                },
            },
            Console::Follow {
                session: "3f9a".into(),
                all: false,
                filter: Filter { kernel: true, ..Filter::default() },
            },
            Console::FollowEnd { session: "3f9a".into(), process: 4321 },
            Console::Steps,
            Console::AppBegin { proof: Vec::new(), host: "duke-nas".into() },
            Console::AppConfirm { code: b"123456".to_vec() },
            Console::AppRemove { proof: b"secret".to_vec() },
            Console::PasskeyBegin { proof: Vec::new(), relying_party: "duke-nas.local".into() },
            Console::PasskeyAdd {
                challenge: vec![1; 32],
                origin: "https://duke-nas.local:9090".into(),
                relying_party: "duke-nas.local".into(),
                name: "YubiKey 5C".into(),
                client_data: b"{}".to_vec(),
                attestation: vec![0xa3],
            },
            Console::PasskeyRemove { proof: Vec::new(), id: vec![1, 2] },
            Console::PasskeyRename { id: vec![1, 2], name: "Phone".into() },
            Console::Codes { proof: Vec::new() },
            Console::TrustedForget,
            Console::TokenAdd {
                proof: Vec::new(),
                name: "ansible".into(),
                scopes: vec!["*".into()],
                days: 365,
            },
            Console::TokenAdd {
                proof: Vec::new(),
                name: "nas".into(),
                scopes: vec!["snapshots".into(), "services:read".into()],
                days: 30,
            },
            Console::TokenRemove { name: "ansible".into() },
        ];
        for request in &requests {
            assert_eq!(round_trip(request).as_ref(), Some(request), "{request:?}");
        }
    }

    #[test]
    fn malformed_requests_are_refused() {
        for fields in [
            &[&b"begin"[..], b"a", b"b"][..],
            &[b"trusted", b"zz", b"00"],
            &[b"code", b"123", b"1"],
            &[b"challenge", b"alone", b"\xff", b"party"],
            &[b"challenge", b"other", b"", b"party"],
            &[b"passkey", b"alone", b"", b"00"],
            &[b"start", b"00", b"always", b""],
            &[b"unlock", b"s", b"fingerprint", b"x"],
            &[b"elevate", b"session", b"s", b"LANG=C"],
            &[b"elevate", b"session", b"s", b"--"],
            &[b"elevate", b"password", b"s", b"--", b"id"],
            &[b"token-add", b"", b"n", b"*", b"many"],
            &[b"steps", b"more"],
            &[b"terminal", b"s", b"root", b"xterm"],
            &[b"terminal", b"s", b"own"],
            &[b"terminal", b"s", b"own", b"xterm", b"\xff"],
            // A follow has its seven fields, and says whose and kernel or
            // nothing in its own words; a NUL in one of them is one more.
            &[b"follow", b"s", b"root", b"", b"", b"", b""],
            &[b"follow", b"s", b"own", b"", b"yes", b"", b""],
            &[b"follow", b"s", b"own", b"", b"", b""],
            &[b"follow", b"s", b"own", b"a", b"b", b"", b"", b""],
            &[b"follow", b"s", b"own", b"\xff", b"", b"", b""],
            &[b"follow-end", b"s", b"0"],
            &[b"follow-end", b"s", b"-1"],
            &[b"follow-end", b"s", b"x"],
            &[b"follow-end", b"s"],
            &[b"nothing"],
        ] {
            assert_eq!(Console::decode(fields), None, "{fields:?}");
        }
    }

    #[test]
    fn hexadecimal_reads_back() {
        assert_eq!(from_hex(b""), Some(vec![]));
        assert_eq!(from_hex(b"00ff7A"), Some(vec![0, 255, 0x7a]));
        assert_eq!(from_hex(b"0"), None);
        assert_eq!(from_hex(b"0g"), None);
        assert_eq!(from_hex(b"+1"), None);
    }

    #[test]
    fn debug_shows_no_secret() {
        let shown = format!(
            "{:?} {:?} {:?}",
            Console::Begin {
                from: "x".into(),
                name: "chase".into(),
                password: b"hunter2".to_vec()
            },
            Console::Unlock { session: "s".into(), proof: Proof::Code(b"654321".to_vec()) },
            Console::TokenStart { token: b"dkv_secret".to_vec() },
        );
        for secret in ["hunter2", "654321", "dkv_secret"] {
            assert!(!shown.contains(secret), "{shown}");
        }
    }
}
