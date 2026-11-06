// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A sandboxed app's keyring (ADR 0396): the part of your keyring an app
//! reaches from its sandbox. The runner serves it, for as long as the
//! program runs, on a socket in a folder of its own, which the sandbox
//! sees at authd's path; so the app's programs, `keyring` among them, ask
//! as they would ask authd.
//!
//! Only the keyring's set, get, list and remove pass, and the runner asks
//! authd for them as you: no sign-in, password or account, and no unlock
//! or lock. Each name the app says is an entry of its own, `app ID NAME`
//! in your keyring, and it lists its own alone, by the names it gave. With
//! `keyring.shared`, which you give with a warning on the trusted prompt,
//! the names pass as they are: the app reaches every entry, as your own
//! programs do.
//!
//! The socket is made before the sandbox, whose view holds its folder,
//! but nothing is taken from it until the runner says so with
//! [`Served::open`], once the sandbox is built. While it is built, the
//! runner keeps spare descriptor slots empty to put capabilities in, and
//! an empty slot is a free descriptor to the C library: a connection
//! taken then could land in one, and be deleted with the spares. The app
//! may already be running by then; what it asks waits on the socket.
//!
//! Where to start when it misbehaves: `keyring list` outside the sandbox
//! shows an app's entries as `app ID NAME`. When the runner cannot serve
//! the keyring it says so on the standard error, and the app runs without
//! it, finding authd unreachable. When a request fails although authd
//! did it (a secret kept, but `keyring set` saying it was not), suspect a
//! descriptor of this thread's that another one closed.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{self, Sender};

use accounts::request::{Answer, REQUEST_MAX, Request};
use accounts::wipe;

use crate::account::ask;

/// The socket's name in its folder, as authd's is in its.
const SOCKET: &str = "socket";

/// Where an app's programs find authd, and so the keyring served: its
/// path in the view, which only Dukovo's sandbox builds.
#[cfg_attr(not(target_os = "radix"), allow(dead_code))]
pub const AT: &str = "system/live/authd";

/// What the app is told of anything but its keyring.
const ONLY: &str = "an app in a sandbox reaches its keyring alone: it may set, get, list and \
                    remove entries, and your keyring is unlocked and locked outside it";

/// The keyring served for one run: its folder and socket go when it is
/// dropped. The thread serving it ends with the runner, or with this if
/// it was never opened.
pub struct Served {
    folder: String,
    /// Tells the thread to start taking connections.
    open: Sender<()>,
}

impl Served {
    /// The folder the sandbox sees at authd's path.
    #[cfg_attr(not(target_os = "radix"), allow(dead_code))]
    pub fn folder(&self) -> &str {
        &self.folder
    }

    /// Starts answering the app, once the runner opens nothing more of its
    /// own while the app runs (see the module's notes).
    #[cfg_attr(not(target_os = "radix"), allow(dead_code))]
    pub fn open(&self) {
        let _ = self.open.send(());
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(format!("{}/{SOCKET}", self.folder));
        let _ = std::fs::remove_dir(&self.folder);
    }
}

/// Serves the keyring of the app `id` in `folder`, which the caller made
/// as theirs alone, mode 0700, so that no one else reaches it; with
/// `shared`, every entry. The socket takes connections at once, but they
/// are answered only after [`Served::open`].
pub fn serve(folder: String, id: &str, shared: bool) -> Result<Served, String> {
    let path = format!("{folder}/{SOCKET}");
    // One left by a run that ended without removing it.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).map_err(|err| format!("{path}: {err}"))?;
    let names = Names { prefix: format!("app {id} "), shared };
    let (open, opened) = mpsc::channel();
    std::thread::Builder::new()
        .name("keyring".into())
        .spawn(move || {
            // Dropped unopened: the program never started.
            if opened.recv().is_err() {
                return;
            }
            for stream in listener.incoming().flatten() {
                answer(stream, &names);
            }
        })
        .map_err(|err| format!("serving its keyring: {err}"))?;
    Ok(Served { folder, open })
}

/// How the app's names are the keyring's.
struct Names {
    /// What its own entries' names start with: `app ID `.
    prefix: String,
    /// `keyring.shared`: every entry, by its own name.
    shared: bool,
}

impl Names {
    /// The keyring's name for the app's `name`.
    fn entry(&self, name: String) -> String {
        if self.shared { name } else { format!("{}{name}", self.prefix) }
    }

    /// The request to forward for the app's `request`, if it may make it;
    /// the secret moves to it, and a request is wiped as it is dropped.
    fn forwarded(&self, mut request: Request) -> Option<Request> {
        let take = std::mem::take::<String>;
        Some(match &mut request {
            Request::KeyringSet { name, secret } => {
                Request::KeyringSet { name: self.entry(take(name)), secret: std::mem::take(secret) }
            }
            Request::KeyringGet { name } => Request::KeyringGet { name: self.entry(take(name)) },
            Request::KeyringRemove { name } => {
                Request::KeyringRemove { name: self.entry(take(name)) }
            }
            Request::KeyringList => Request::KeyringList,
            _ => return None,
        })
    }

    /// authd's answer as the app is told it: its own entries listed by
    /// its names for them, and a refusal in those names.
    fn told(&self, listing: bool, answer: Answer) -> Answer {
        if self.shared {
            return answer;
        }
        match answer {
            Answer::Done(names) if listing => Answer::Done(
                names
                    .split('\t')
                    .filter_map(|name| name.strip_prefix(&self.prefix))
                    .collect::<Vec<_>>()
                    .join("\t"),
            ),
            Answer::Refused(why) => Answer::Refused(why.replace(&self.prefix, "")),
            answer => answer,
        }
    }
}

/// Answers one connection: a request, read to its end as authd reads
/// one, and the answer.
fn answer(mut stream: UnixStream, names: &Names) {
    let mut bytes = Vec::new();
    let read = (&mut stream).take(REQUEST_MAX as u64 + 1).read_to_end(&mut bytes);
    let decoded = read.map_err(|err| err.to_string()).and_then(|_| Request::decode(&bytes));
    wipe(&mut bytes);
    let told = match decoded {
        Err(why) => Answer::Refused(why),
        Ok(request) => {
            let listing = matches!(request, Request::KeyringList);
            match names.forwarded(request) {
                None => Answer::Refused(ONLY.into()),
                Some(forwarded) => {
                    let answer =
                        ask(&forwarded).unwrap_or_else(|fail| Answer::Refused(fail.message));
                    names.told(listing, answer)
                }
            }
        }
    };
    let mut line = told.encode().into_bytes();
    let _ = stream.write_all(&line);
    wipe(&mut line);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(shared: bool) -> Names {
        Names { prefix: "app org.example.notes ".into(), shared }
    }

    #[test]
    fn its_names_are_its_own_entries() {
        let own = names(false);
        let get = own.forwarded(Request::KeyringGet { name: "token".into() });
        assert_eq!(get, Some(Request::KeyringGet { name: "app org.example.notes token".into() }));
        let listed = Answer::Done(
            "git https://alice@example.org\tapp org.example.notes token\tapp org.other x".into(),
        );
        assert_eq!(own.told(true, listed), Answer::Done("token".into()));
        let refused =
            Answer::Refused("your keyring has no entry app org.example.notes token".into());
        assert_eq!(
            own.told(false, refused),
            Answer::Refused("your keyring has no entry token".into())
        );
    }

    #[test]
    fn shared_it_reaches_every_entry() {
        let shared = names(true);
        let get = shared.forwarded(Request::KeyringGet { name: "git https://x@y".into() });
        assert_eq!(get, Some(Request::KeyringGet { name: "git https://x@y".into() }));
        let listed = Answer::Done("git https://x@y\tapp org.example.notes token".into());
        assert_eq!(shared.told(true, listed.clone()), listed);
    }

    /// Nothing is answered before the runner opens it, as the sandbox's
    /// spare slots are held until then; what was asked waits, and is
    /// answered after.
    #[test]
    fn answers_only_once_opened() {
        use std::time::Duration;
        let folder = std::env::temp_dir().join(format!("keyring-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let served = serve(folder.to_str().unwrap().into(), "org.example.notes", false).unwrap();
        let mut stream = UnixStream::connect(folder.join(SOCKET)).unwrap();
        // Not a request: refused without asking authd.
        stream.write_all(b"not a request\n").unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        stream.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
        let mut told = Vec::new();
        assert!(stream.read_to_end(&mut told).is_err(), "answered before it was opened");
        served.open();
        stream.set_read_timeout(None).unwrap();
        stream.read_to_end(&mut told).unwrap();
        let line = String::from_utf8(told).unwrap();
        assert!(matches!(Answer::decode(&line), Answer::Refused(_)), "{line}");
        drop(served);
        assert!(!folder.exists(), "its folder outlived it");
    }

    #[test]
    fn nothing_but_the_keyring_passes() {
        for request in [Request::KeyringLock, Request::KeyringUnlock { password: b"x".to_vec() }] {
            assert_eq!(names(true).forwarded(request), None);
        }
    }
}
