// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What authd(8) is asked, and what it answers.
//!
//! A program connects to [`SOCKET`](crate::SOCKET), writes one request
//! and shuts its connection for writing; authd answers with one line and
//! closes it. A request is its verb and its fields, each ended by a NUL,
//! since no name or password a terminal sends holds one:
//!
//! ```text
//! check\0alice\0secret\0
//! passwd\0alice\0old secret\0new secret\0
//! add\0bob\0Bob Smith\0admin\0his secret\0
//! needs-owner\0
//! owner\0chase\0Chase\0her secret\0
//! session\0alice\0secret\0linux\0
//! reset\0alice\0new secret\0
//! disable\0bob\0
//! enable\0bob\0
//! remove\0bob\0
//! rename\0bob\0robert\0
//! set\0bob\0full-name\0Bob Smith\0admin\0no\0
//! elevate\0command\0ask\0\0linux\0/users/alice\0LANG=C.UTF-8\0--\0dmesg\0-w\0
//! unlock\0password\0secret\0
//! forget\0
//! prompts\0
//! prompted\07\0password\0secret\0
//! share-password\0alice\0secret\0set\0for windows\0
//! share-signin\0192.0.2.7\04e544c4d...\04e544c4d...\04e544c4d...\0
//! end-shares\0
//! nfs-worker\01000\0
//! ssh-key\0alice\0000000b7373682d6564...\0
//! ssh-password\0192.0.2.7 51234 192.0.2.1 22\0alice\0secret\0
//! ssh-session\0192.0.2.7 51234 192.0.2.1 22\0alice\0key\000000020...\0000000b...\0xterm\0shell\0\0LANG=C.UTF-8\0
//! ssh-session\0192.0.2.7 51234 192.0.2.1 22\0alice\0ticket\09f86d081...\0\0\0subsystem\0sftp\0
//! ssh-session\0192.0.2.7 51234 192.0.2.1 22\0alice\0key\000000020...\0000000b...\0\0forward\0\0
//! ssh-ended\09f86d081...\0
//! console\0begin\0192.0.2.7\0chase\0secret\0
//! job\0alice\0crontab alice\0/users/alice\0backup --quiet\0SHELL=/bin/sh\0
//! keep-running\0
//! keep-running\0on\0
//! ssh-failed\0192.0.2.7 51234 192.0.2.1 22\0root\0
//! blocked\0
//! unblock\0203.0.113.9\0
//! never-block\0192.168.1.0/24\0
//! never-block\0192.168.1.0/24\0remove\0
//! help-start\060\0\0\0xterm-256color\0\0
//! help-start\030\000000b7373682d6564...\0open-ssh\0xterm-256color\0her secret\0
//! help-end\0
//! help-log\0
//! help-log\0k7q2mx\0
//! help-allow-elevate\0her secret\0
//! ssh-help-key\0000000b7373682d6564...\0
//! ssh-help\0192.0.2.7 51234 192.0.2.1 22\0code\0KQ7RM2XW9HTP4BZE\0
//! ssh-help\0192.0.2.7 51234 192.0.2.1 22\0key\000000020...\0000000b...\0
//! ssh-help-ended\0k7q2mx\0
//! ```
//!
//! The web console's requests, `console` and a verb, are in
//! [`console`](crate::console) (ADR 0514).
//!
//! A `help-start` request (ADR 0540) comes with three descriptors: the
//! help screen's terminal, where authd starts the owner's shell, and the
//! read end of the pipe that carries the screen to the helper and the
//! write end of the one that brings their typing back, which authd hands
//! sshd once the helper signs in (pipes, as a socket cannot be sent,
//! ADR 0091). Its answer is `ok` and [`HelpStarted`]'s words, and its
//! connection then stays open: authd writes it [`HelpEvent`] lines until
//! the session ends, and ends the session when it closes. A right
//! `ssh-help` is answered `ok` and the session's ID with four
//! descriptors: those two pipes' ends, the read end of a pipe whose end
//! tells sshd the session ended, and the transcript.
//!
//! An answer is `ok`, maybe with words after it, or `no` and why, then a
//! newline. authd tells who asks by the user its connection gives
//! (SO_PEERCRED), never by what the request says.
//!
//! A `session` request comes with a terminal, sent with its first bytes
//! (SCM_RIGHTS), and its answer is `ok` and the number of the session's
//! first process; the connection then stays open until that process
//! ends, when authd writes [`ended`]'s line and closes it (ADR 0250).
//!
//! An `ssh-session` request (ADR 0347) comes with a terminal, or with
//! the three descriptors the session is to have as its 0, 1 and 2, and
//! is answered as a `session` request is, its connection then waiting
//! for the session's end. It shows that the person signed in with what
//! the client signed with their key, or with the ticket authd answered
//! an `ssh-password` request with (ADR 0348), which holds for the
//! connection that asked until it sends `ssh-ended`.
//!
//! A `job` request (ADR 0478) comes with one descriptor, where the job's
//! standard output and error go, and is answered as a `session` request
//! is: `ok` and the job's process, and [`ended`]'s line when it ends.
//!
//! An `elevate` request (ADR 0219) comes with the three descriptors the
//! command is to have as its 0, 1 and 2, and a pipe's read end, sent
//! together with its first bytes. Its answer is `ok password` when a
//! password is needed and none came, or `ok` and the command's process,
//! after which the connection works as a session's: authd writes
//! [`stopped`]'s line when the command stops and [`ended`]'s when it
//! ends, and reads [`signal`]'s and [`CONTINUE`] lines from the pipe.
//!
//! The consoles' own password prompt (ADR 0251): consoled(8) asks
//! `prompts`, and its connection stays open for authd to write
//! [`Prompt`] lines to. An `elevate` or `unlock` request that says `ask`
//! for its password, from a pane on a console, is then not answered
//! `ok password`: authd has consoled ask for the password where no
//! program can draw or read, and consoled answers with a `prompted`
//! request, which authd takes as the password the first request came
//! with. On any other terminal, or with no consoled, `ask` is answered
//! as `none` is.

use crate::console::Console;
use crate::wipe;

/// The hexadecimal of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0xf)]));
    }
    text
}

/// The bytes of `text`, hexadecimal.
pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

/// The longest request authd reads, in bytes: room for a share sign-in's
/// three NTLM messages, in hexadecimal, for the command an SSH client
/// asks to run ([`SSH_COMMAND_MAX`]), and for a person's job
/// ([`JOB_TEXT_MAX`]).
pub const REQUEST_MAX: usize = 192 * 1024;

/// The most bytes of a job's command and environment together: what
/// at(1) queues, 128 KiB, with room to spare, and less than the kernel
/// takes for a program's arguments and environment (EXEC_MAX, 256 KiB).
pub const JOB_TEXT_MAX: usize = 160 * 1024;

/// The longest command an SSH client may ask to run, in bytes: what
/// [`REQUEST_MAX`] leaves room for, with the rest of the request.
pub const SSH_COMMAND_MAX: usize = 32 * 1024;

/// The kinds of key whose signatures authd checks for an SSH sign-in
/// (ADR 0347).
pub const SSH_KEY_ALGORITHMS: &[&str] =
    &["ssh-ed25519", "ecdsa-sha2-nistp256", "ecdsa-sha2-nistp384"];

/// The longest value of a language setting an SSH session takes.
pub const SSH_SETTING_MAX: usize = 256;

/// Whether `setting`, `NAME=value`, is one of an SSH client's language
/// settings a session takes, as Debian's sshd takes LANG and LC_*
/// (AcceptEnv): `LANG`, `LANGUAGE` or an `LC_` name, with a value of
/// printable characters.
pub fn ssh_language(setting: &str) -> bool {
    let Some((name, value)) = setting.split_once('=') else { return false };
    let named = name == "LANG"
        || name == "LANGUAGE"
        || name.strip_prefix("LC_").is_some_and(|rest| {
            !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
        });
    named && value.len() <= SSH_SETTING_MAX && value.bytes().all(|b| b.is_ascii_graphic())
}

/// A request. Its `Debug` leaves out the passwords.
#[derive(PartialEq, Eq)]
pub enum Request {
    /// Whether `password` is user `name`'s.
    Check {
        /// The user.
        name: String,
        /// The password to check.
        password: Vec<u8>,
    },
    /// Change user `name`'s password from `current` to `new`. Root, or
    /// from 13b.6 an administrator inside elevate, gives no current one.
    Passwd {
        /// The user.
        name: String,
        /// Its password now; empty from root.
        current: Vec<u8>,
        /// Its new password.
        new: Vec<u8>,
    },
    /// Make the account `name`, with its group and home: root's to ask.
    Add {
        /// Its name.
        name: String,
        /// Its full name, which may be empty.
        full_name: String,
        /// Whether it is an administrator's.
        admin: bool,
        /// Its password, which may be empty.
        password: Vec<u8>,
    },
    /// Whether the machine needs its owner's account made: `ok yes` when
    /// no account can be signed in to, `ok no` otherwise. Anyone may ask.
    NeedsOwner,
    /// Make the owner's account, an administrator's, as sign-in does at
    /// first boot: only while no account can be signed in to, and
    /// root's to ask.
    Owner {
        /// Its name.
        name: String,
        /// Its full name, which may be empty.
        full_name: String,
        /// Its password, which may be empty.
        password: Vec<u8>,
    },
    /// Sign `name` in on the terminal sent with the request: start their
    /// shell there, as them, if `password` is theirs. Root's to ask, as
    /// sign-in programs run as root.
    Session {
        /// The user.
        name: String,
        /// The password to check.
        password: Vec<u8>,
        /// The terminal's type, for `TERM`.
        term: String,
        /// A command for the session to run, with the shell's `-c`, in
        /// place of a login shell, as the failure notice's report key
        /// runs `report failure`; the session ends with it. A request
        /// without one, as older sign-in programs send, starts the shell.
        command: Option<String>,
    },
    /// Set administrator `name`'s password to `password`, from the
    /// startup-trouble menu (ADR 0253): root's to ask, and only while the
    /// kernel was started with `startup-trouble`. Every administrator is
    /// told at their next sign-in.
    Reset {
        /// The administrator.
        name: String,
        /// Their new password, which may be empty.
        password: Vec<u8>,
    },
    /// Disable account `name` (ADR 0220): everything that acts for it
    /// ends at once, and nothing signs in to it until it is enabled
    /// again, with the password it had. Root's to ask; the answer is `ok`
    /// and how many of its processes ended.
    Disable {
        /// The account.
        name: String,
    },
    /// Enable account `name` again. Root's to ask.
    Enable {
        /// The account.
        name: String,
    },
    /// Remove account `name`: what acts for it ends, as with `Disable`,
    /// its lines go from the account files, and its number is never given
    /// again. Root's to ask; the answer is `ok` and its home, which the
    /// asker keeps, archives or deletes.
    Remove {
        /// The account.
        name: String,
    },
    /// Make the account `name` of a bundle's service (ADR 0385), whose
    /// home is `home`, its data folder, and whose full name `whose` says
    /// whose service it is; one made already with that home is kept. Root's
    /// to ask; the answer is `ok` and its number. It is removed as any
    /// account is.
    Service {
        /// The account.
        name: String,
        /// The bundle whose service it is.
        whose: String,
        /// Its home.
        home: String,
    },
    /// Make the group `name` of a socket that bundle `whose` offers to
    /// others (ADR 0477): its service account's name and `-socket`, with
    /// no members, a number as a service's account has. Root's to ask;
    /// the answer is `ok` and its number. A group of that name that the
    /// bundle made before is kept; one that is not its own is refused.
    /// It goes when its service's account is removed.
    SocketGroup {
        /// The group.
        name: String,
        /// The bundle that offers the socket.
        whose: String,
    },
    /// Rename account `name` to `new`, with its own group and its home in
    /// `/users`, while nothing acts for it. Root's to ask.
    Rename {
        /// The account.
        name: String,
        /// Its new name.
        new: String,
    },
    /// Change account `name`'s settings, those given. Root's to ask, but
    /// anyone may change their own full name, as with chfn(1).
    Set {
        /// The account.
        name: String,
        /// Its new full name, which may be empty.
        full_name: Option<String>,
        /// Whether it is to be an administrator's.
        admin: Option<bool>,
        /// Whether it is to elevate without a password (ADR 0328).
        without_password: Option<bool>,
        /// Whether it may sign in to the web console without being an
        /// administrator, in the console group (ADR 0514).
        console: Option<bool>,
        /// Whether its jobs may run while its person is signed out (ADR
        /// 0478): `false` bars it, whatever they chose.
        jobs_signed_out: Option<bool>,
    },
    /// Run a command as an administrator, with the admin set (ADR 0219):
    /// an administrator's to ask, and after their password unless their
    /// terminal pane typed it in the last 15 minutes.
    Elevate {
        /// What runs: a command, or root's shell.
        mode: Mode,
        /// The asker's password, if it typed one.
        password: Option<Vec<u8>>,
        /// With no password: whether the console may ask for it.
        ask: bool,
        /// The terminal's type, for `TERM`.
        term: String,
        /// The directory the command starts in.
        directory: Vec<u8>,
        /// The asker's language settings, `LANG`, `LANGUAGE` and `LC_*`,
        /// as `NAME=value`.
        language: Vec<String>,
        /// The command and its arguments: for a shell, what it runs with
        /// `-c`, if anything.
        command: Vec<Vec<u8>>,
    },
    /// Unlock elevate in the asker's terminal pane for 15 minutes, or for
    /// 15 more, as `sudo -v` does: after the password, unless it is
    /// unlocked already.
    Unlock {
        /// The asker's password, if it typed one.
        password: Option<Vec<u8>>,
        /// With no password: whether the console may ask for it.
        ask: bool,
    },
    /// Lock elevate in the asker's terminal pane again, as `sudo -k`
    /// does. Anyone may ask.
    Forget,
    /// Ask for passwords on the consoles (ADR 0251): authd writes
    /// [`Prompt`] lines on this connection, which stays open. Root's to
    /// ask, as consoled runs as root; a later one takes over.
    Prompts,
    /// What the person at the console typed for prompt `id`, from the
    /// program that asked [`Request::Prompts`].
    Prompted {
        /// The prompt's number, from its [`Prompt::Ask`] or
        /// [`Prompt::Choose`].
        id: u64,
        /// What came of it.
        typed: Prompted,
    },
    /// Ask the person at the console of `terminal`, `name`, a question of
    /// bundled(8)'s about an app's permission (ADR 0224), which consoled
    /// draws where no program can: root's to ask. authd answers with what
    /// they chose ([`Choice::word`]), `cancelled` when they closed it, or
    /// `elsewhere` when the terminal is not a console's or nothing asks
    /// on the consoles.
    Choose {
        /// The asker's terminal, as /processes names it.
        terminal: String,
        /// Whose question it is.
        name: String,
        /// What kind of question, which decides its keys and colours.
        question: Question,
        /// The question's words, one line of at most [`PROMPT_SHOWN_MAX`]
        /// characters, in the system's own words.
        text: String,
    },
    /// Set or clear `name`'s sharing password, which Windows PCs sign in
    /// to shares with (ADR 0228): their own to ask, with the password
    /// they sign in to Dukovo with, or root's, with none.
    SharePassword {
        /// The person.
        name: String,
        /// Their sign-in password; empty from root.
        current: Vec<u8>,
        /// The new sharing password, or none to clear it.
        new: Option<Vec<u8>>,
    },
    /// A Windows PC signs in to the shares (ADR 0228): smbd(8)'s, which
    /// runs as root, to ask. authd checks the NTLMv2 AUTHENTICATE message
    /// against the sharing password of the account it names, then starts
    /// smbd's worker as that person, with the three descriptors sent with
    /// the request: the pipe the worker reads its session from, the
    /// connection, and where it logs. The answer is `ok`, the account's
    /// name and the session key in hexadecimal.
    ShareSignIn {
        /// The PC's address, for the records of sign-ins.
        from: String,
        /// The NEGOTIATE message, as the PC sent it.
        negotiate: Vec<u8>,
        /// The CHALLENGE message smbd answered with.
        challenge: Vec<u8>,
        /// The AUTHENTICATE message.
        authenticate: Vec<u8>,
    },
    /// End every share session, as `share off` does: root's to ask. The
    /// answer is `ok` and how many ended.
    EndShares,
    /// Start an NFS worker (ADR 0343): nfsd(8)'s, which runs as root, to
    /// ask. authd starts `nfsd --worker` as the person numbered `uid`,
    /// or as no one for root and a number with no person's account, in
    /// a view of the folders of the shares NFS serves alone, with the
    /// three descriptors sent with the request: the read end of nfsd's
    /// pipe to it, the write end of its pipe back, and where it logs. The
    /// answer is `ok` and the name it runs as.
    NfsWorker {
        /// The number of the person an NFS call names.
        uid: u32,
    },
    /// Whether the public key `key` may sign `name` in over SSH (ADR
    /// 0347), which sshd(8) asks before a client signs with it: sshd's,
    /// which runs as root, to ask. The answer is `ok` for a key in the
    /// person's `~/.ssh/authorized_keys`, and `no` for any other key,
    /// name or account, alike.
    SshKey {
        /// The account.
        name: String,
        /// The key, as SSH carries it (RFC 4253 6.6): its algorithm's
        /// name, then its numbers.
        key: Vec<u8>,
    },
    /// Whether `password` is `name`'s, for an SSH client at `connection`
    /// (ADR 0348): sshd(8)'s, which runs as root, to ask, once
    /// sshd_config(5) lets passwords sign in. The answer to a right one is
    /// `ok` and a ticket, in hexadecimal, which starts the person's
    /// sessions on that connection ([`SshProof::Ticket`]) until sshd says
    /// [`Request::SshEnded`]; a wrong one is answered as `check`'s is,
    /// once the next try may be made, counted for the client's address.
    SshPassword {
        /// The client's address and port, then this machine's, as
        /// `SSH_CONNECTION` says them: `192.0.2.7 51234 192.0.2.1 22`.
        connection: String,
        /// The account.
        name: String,
        /// The password the client sent.
        password: Vec<u8>,
    },
    /// Sign `name` in over SSH and start their session (ADR 0347):
    /// sshd(8)'s, which runs as root, to ask. authd checks `proof`
    /// itself: the signature with the key the signed request names, and
    /// that the key is in the person's `~/.ssh/authorized_keys`; or that
    /// it gave the ticket to this connection for `name`. Then it starts
    /// the session, as a `session` request's, with the descriptors sent:
    /// a terminal, or three for its 0, 1 and 2.
    SshSession {
        /// The client's address and port, then this machine's, as
        /// `SSH_CONNECTION` says them: `192.0.2.7 51234 192.0.2.1 22`.
        connection: String,
        /// The account.
        name: String,
        /// How the person signed in.
        proof: SshProof,
        /// The terminal's type, for `TERM`, on a terminal.
        term: String,
        /// What the client asked to run.
        run: SshRun,
        /// The client's language settings, `LANG`, `LANGUAGE` and
        /// `LC_*`, as `NAME=value`.
        language: Vec<String>,
    },
    /// The connection that an [`SshProof::Ticket`] was given to ended:
    /// sshd(8)'s to say, from the process that asked for it.
    SshEnded {
        /// The ticket.
        ticket: Vec<u8>,
    },
    /// Keep `secret` in the asker's keyring (ADR 0393) as the entry
    /// `name`, in place of one of that name: anyone's to ask, in their own
    /// keyring, while it is open.
    KeyringSet {
        /// The entry's name: at most [`KEYRING_NAME_MAX`] bytes of text,
        /// with no control characters.
        name: String,
        /// Its secret: at most [`KEYRING_SECRET_MAX`] bytes, of any kind.
        secret: Vec<u8>,
    },
    /// The secret of the entry `name` in the asker's keyring: the answer
    /// is `ok` and the secret in hexadecimal.
    KeyringGet {
        /// The entry.
        name: String,
    },
    /// The names of the entries in the asker's keyring: the answer is
    /// `ok` and the names, separated by tabs.
    KeyringList,
    /// Remove the entry `name` from the asker's keyring.
    KeyringRemove {
        /// The entry.
        name: String,
    },
    /// Open the asker's keyring with their password, as a sign-in with
    /// one does: for a session that started without it, such as one
    /// over SSH with a key. A wrong password waits as at a sign-in.
    KeyringUnlock {
        /// The asker's password.
        password: Vec<u8>,
    },
    /// Close the asker's keyring now, until they sign in with their
    /// password again or ask [`Request::KeyringUnlock`].
    KeyringLock,
    /// The Kerberos tickets in the keyring of the person numbered `uid`
    /// (ADR 0399): nfs(1)'s to ask, which runs as root and calls a server
    /// with `sec=krb5` as whoever uses the mount, as Linux's rpc.gssd
    /// reads each person's ticket cache. A person may ask for their own;
    /// an app in a sandbox may not ask at all. The answer is `ok` and the
    /// entries named `krb5 ...`, one to a line, each name and its secret
    /// in hexadecimal with a tab between.
    KerberosTickets {
        /// The person's number.
        uid: u32,
    },
    /// The web console's requests (ADR 0514), in [`console`](crate::console).
    Console(Console),
    /// Run a person's job (ADR 0478): timerd(8)'s, which runs as root, to
    /// ask. authd starts `/bin/sh -c COMMAND` as `owner`, in `folder`,
    /// with the descriptor sent with the request as its standard output
    /// and error, while they are signed in, or while they are not when
    /// they keep their jobs running ([`Request::SetKeepRunning`]); root's
    /// always. The answer is `ok` and the job's process, and the
    /// connection then waits for its end, as a session's does; a job that
    /// does not run is refused with one of the `JOB_` words.
    Job {
        /// Whose it is: the account's name.
        owner: String,
        /// What the log calls it: `crontab alice`, `at 12`.
        label: String,
        /// The folder it starts in.
        folder: String,
        /// The command, which `/bin/sh -c` runs.
        command: String,
        /// Its environment, `NAME=value` each, over a session's.
        environment: Vec<String>,
    },
    /// Whether the asker's jobs run while they are signed out: anyone's
    /// to ask, of their own. The answer is `ok` and [`KeepRunning`]'s
    /// words.
    KeepRunning,
    /// Set whether the asker's jobs run while they are signed out: on,
    /// off, or `None` for the system's default. Anyone's to ask, of their
    /// own; answered as [`Request::KeepRunning`] is.
    SetKeepRunning {
        /// What they choose.
        chosen: Option<bool>,
    },
    /// An SSH connection that tried to sign in, was refused each time,
    /// and ended (ADR 0479): sshd(8)'s to say, which runs as root, once
    /// as the connection ends, when no password of its was checked, as
    /// authd counted those already. authd counts it as a failed sign-in
    /// from the client's address.
    SshFailed {
        /// The connection, as SSH_CONNECTION says it.
        connection: String,
        /// The account it last tried to sign in to, or empty.
        name: String,
    },
    /// The addresses blocked for guessing passwords (ADR 0479): an
    /// administrator's to ask, inside elevate. The answer is `ok` and
    /// each block's [`BlockedEntry::words`], a tab between them.
    Blocked,
    /// Lift the block of `address`, an IPv4 address or an IPv6 network of
    /// 64 bits, and forget its blocks before, so that its next is a
    /// first: an administrator's to ask, inside elevate.
    Unblock {
        /// The address.
        address: String,
    },
    /// Never block `network`, or block it again when `remove`, as
    /// blocking.toml(5)'s `never` lists it: an administrator's to ask,
    /// inside elevate.
    NeverBlock {
        /// An address, or a network as `192.168.1.0/24`.
        network: String,
        /// Whether it is taken out of `never`.
        remove: bool,
    },
    /// Start a help session (ADR 0540) for `minutes`, the asker's, a
    /// person's, with their shell on the terminal sent with it, for a
    /// helper who signs in as [`HELP_NAME`] with the code authd answers
    /// with, or with `key` alone. With `open_ssh`, an administrator's,
    /// sshd is started for the session when it is off, after their
    /// `password`.
    HelpStart {
        /// How long it lasts: [`HELP_MINUTES_MIN`] to
        /// [`HELP_MINUTES_MAX`].
        minutes: u32,
        /// The helper's SSH public key, as its blob, in place of a code.
        key: Option<Vec<u8>>,
        /// Whether sshd is to be started for it alone.
        open_ssh: bool,
        /// The terminal type of the asker's terminal, which shows the
        /// help screen, as TERM says it.
        term: String,
        /// The asker's password, with `open_ssh`; empty otherwise.
        password: Vec<u8>,
    },
    /// End the asker's help session.
    HelpEnd,
    /// The help sessions' transcripts: the asker's own, or every one for
    /// root. With `id`, that one's lines; without, the list, each
    /// [`HelpKept::words`], a tab between them.
    HelpLog {
        /// The session.
        id: Option<String>,
    },
    /// Let elevate run in the asker's help screen for
    /// [`HELP_ELEVATE_MINUTES`], after their `password`: an
    /// administrator's to ask, from outside the help screen.
    HelpAllowElevate {
        /// Their password.
        password: Vec<u8>,
    },
    /// Whether `key` may sign a helper in, before the client signs with
    /// it: sshd's to ask.
    SshHelpKey {
        /// The key, as its blob.
        key: Vec<u8>,
    },
    /// Sign a helper in to the help session `proof` shows, from
    /// `connection`: sshd's to ask.
    SshHelp {
        /// The connection, as SSH_CONNECTION says it.
        connection: String,
        /// What the helper signed in with.
        proof: HelpProof,
    },
    /// The helper's connection to the help session `id` ended: sshd's to
    /// say, from the process that signed them in.
    SshHelpEnded {
        /// The session.
        id: String,
    },
}

/// What a helper signs in to a help session with (ADR 0540).
#[derive(Clone, PartialEq, Eq)]
pub enum HelpProof {
    /// The session's code, as typed for the SSH password.
    Code(String),
    /// The session's key: what the client signed and its signature, as
    /// [`SshProof::Key`].
    Key {
        /// What the client signed.
        signed: Vec<u8>,
        /// The signature.
        signature: Vec<u8>,
    },
}

/// The name a helper signs in with over SSH (ADR 0540): no account's, and
/// one no account may take.
pub const HELP_NAME: &str = "help";

/// The shortest help session, in minutes.
pub const HELP_MINUTES_MIN: u32 = 5;

/// The longest help session, in minutes: 8 hours.
pub const HELP_MINUTES_MAX: u32 = 480;

/// A help session's length unless it is given one, in minutes.
pub const HELP_MINUTES_DEFAULT: u32 = 60;

/// How long elevate runs in a help screen once its owner allows it, in
/// minutes.
pub const HELP_ELEVATE_MINUTES: u32 = 15;

/// The characters of a help session's code: 32, with no I, O, 0 or 1,
/// which look alike, so that each holds 5 bits.
pub const HELP_CODE_LETTERS: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// How many characters a help session's code has: 80 bits.
pub const HELP_CODE_LENGTH: usize = 16;

/// The file that is there while a help session waits for a code, which
/// sshd looks for to take passwords for [`HELP_NAME`].
pub const HELP_WAITING: &str = "/system/live/authd/help-waiting";

/// The file that is there while sshd runs for a help session alone
/// (`--open-ssh`): sshd then takes [`HELP_NAME`]'s sign-ins and no other.
pub const HELP_ONLY: &str = "/system/live/authd/help-only";

/// Where the help sessions' transcripts are kept, `ID.log` each.
pub const HELP_FOLDER: &str = "/system/state/help";

/// Why authd refuses what would ask for a password inside a help screen,
/// where the helper could fake the question and read the answer.
pub const HELP_NO_PASSWORDS: &str = "in a help session, nothing asks for your password: run it \
                                     from another terminal";

/// `typed`, a help session's code as a person types it, without its
/// dashes and spaces and in capitals, if it is one.
#[must_use]
pub fn help_code(typed: &str) -> Option<String> {
    let code: String =
        typed.chars().filter(|c| *c != '-' && *c != ' ').map(|c| c.to_ascii_uppercase()).collect();
    let letters = code.bytes().all(|b| HELP_CODE_LETTERS.contains(&b));
    (code.len() == HELP_CODE_LENGTH && letters).then_some(code)
}

/// `code` as it is shown: in fours, with dashes between them.
#[must_use]
pub fn help_code_shown(code: &str) -> String {
    code.as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// The most a help session's transcript holds, in bytes: sshd ends the
/// helper's connection before a line would take it past this, so that
/// nothing the helper types goes unkept.
pub const HELP_TRANSCRIPT_MAX: u64 = 1 << 20;

/// A help session's transcript line (ADR 0540): `SECONDS KIND TEXT`,
/// with what `text` holds that is not printable shown as
/// [`help_printable`] shows it, so that one line stays one. authd writes
/// the session's steps, and sshd each line the helper types.
#[must_use]
pub fn help_transcript_line(seconds: u64, kind: &str, text: &[u8]) -> String {
    format!("{seconds} {kind} {}\n", help_printable(text))
}

/// `bytes` as a transcript keeps them: control characters as `^X`, other
/// bytes outside printable ASCII as `\xNN`, and a backslash doubled.
#[must_use]
pub fn help_printable(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len());
    for &byte in bytes {
        match byte {
            b'\\' => text.push_str("\\\\"),
            b' '..=b'~' => text.push(char::from(byte)),
            0..=0x1f => {
                text.push('^');
                text.push(char::from(byte + 0x40));
            }
            0x7f => text.push_str("^?"),
            _ => text.push_str(&format!("\\x{byte:02x}")),
        }
    }
    text
}

/// The longest line of a helper's a transcript keeps as one: a longer
/// one, such as a paste with no line end, is kept in lines of this many
/// bytes.
pub const HELP_LINE_MAX: usize = 1024;

/// What a helper types, cut into the lines a help session's transcript
/// keeps (ADR 0540): a carriage return or a line feed ends a line, a line
/// feed right after a carriage return ends none, and a line ends once it
/// has [`HELP_LINE_MAX`] bytes.
#[derive(Default)]
pub struct HelpLines {
    /// What was typed since the end of the last line.
    line: Vec<u8>,
    /// Whether the last byte ended a line with a carriage return.
    after_return: bool,
}

impl HelpLines {
    /// Takes `bytes`, as typed, and gives each line they end.
    pub fn take(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut ended = Vec::new();
        for &byte in bytes {
            match byte {
                b'\n' if self.after_return => self.after_return = false,
                b'\r' | b'\n' => {
                    ended.push(std::mem::take(&mut self.line));
                    self.after_return = byte == b'\r';
                }
                _ => {
                    self.after_return = false;
                    self.line.push(byte);
                    if self.line.len() >= HELP_LINE_MAX {
                        ended.push(std::mem::take(&mut self.line));
                    }
                }
            }
        }
        ended
    }

    /// The line not ended yet.
    #[must_use]
    pub fn rest(&self) -> &[u8] {
        &self.line
    }

    /// The line not ended yet, ended, if anything was typed of it: as the
    /// helper's connection ends.
    pub fn end(&mut self) -> Option<Vec<u8>> {
        (!self.line.is_empty()).then(|| std::mem::take(&mut self.line))
    }
}

/// What authd answers a `help-start` with: `ID UNTIL CODE HOSTKEY`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelpStarted {
    /// The session's ID, a few lowercase letters and digits.
    pub id: String,
    /// When it ends, in seconds since 1970.
    pub until: i64,
    /// The code, or `None` when a key signs the helper in.
    pub code: Option<String>,
    /// The fingerprint of sshd's host key, `SHA256:...`, if it has one.
    pub host_key: Option<String>,
}

impl HelpStarted {
    /// Its words in authd's answer.
    #[must_use]
    pub fn words(&self) -> String {
        format!(
            "{} {} {} {}",
            self.id,
            self.until,
            self.code.as_deref().unwrap_or("-"),
            self.host_key.as_deref().unwrap_or("-")
        )
    }

    /// The answer `words` says, as [`HelpStarted::words`] made them.
    #[must_use]
    pub fn from_words(words: &str) -> Option<HelpStarted> {
        let [id, until, code, host_key] = words.split(' ').collect::<Vec<_>>()[..] else {
            return None;
        };
        let given = |word: &str| (word != "-").then(|| word.to_string());
        Some(HelpStarted {
            id: id.to_string(),
            until: until.parse().ok()?,
            code: given(code),
            host_key: given(host_key),
        })
    }
}

/// Why a help session ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpEnded {
    /// Its owner ended it: Ctrl-], or `help-session end`.
    Owner,
    /// Its time was up.
    Deadline,
    /// The helper's connection closed.
    Helper,
    /// The shell in the help screen ended.
    Session,
    /// Wrong codes spent its code, and no key signs a helper in.
    Spent,
    /// It could not start.
    Refused,
}

impl HelpEnded {
    /// Its word in a line.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            HelpEnded::Owner => "owner",
            HelpEnded::Deadline => "deadline",
            HelpEnded::Helper => "helper",
            HelpEnded::Session => "session",
            HelpEnded::Spent => "spent",
            HelpEnded::Refused => "refused",
        }
    }

    /// The one `word` names.
    #[must_use]
    pub fn from_word(word: &str) -> Option<HelpEnded> {
        [
            HelpEnded::Owner,
            HelpEnded::Deadline,
            HelpEnded::Helper,
            HelpEnded::Session,
            HelpEnded::Spent,
            HelpEnded::Refused,
        ]
        .into_iter()
        .find(|ended| ended.word() == word)
    }
}

/// What authd tells help-session(1) about its session, a line each.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelpEvent {
    /// The helper signed in from `address`, with the key whose
    /// fingerprint is `key`, or `code`.
    Helper {
        /// Their address.
        address: String,
        /// Their key's fingerprint, or `code`.
        key: String,
    },
    /// So many minutes are left.
    Left {
        /// The minutes.
        minutes: u32,
    },
    /// Its owner let elevate run for so many minutes.
    Elevate {
        /// The minutes.
        minutes: u32,
    },
    /// Five wrong codes spent the code.
    Spent,
    /// It ended, and why.
    Ended(HelpEnded),
}

impl HelpEvent {
    /// Its line, with its newline.
    #[must_use]
    pub fn line(&self) -> String {
        match self {
            HelpEvent::Helper { address, key } => format!("helper {address} {key}\n"),
            HelpEvent::Left { minutes } => format!("left {minutes}\n"),
            HelpEvent::Elevate { minutes } => format!("elevate {minutes}\n"),
            HelpEvent::Spent => "spent\n".into(),
            HelpEvent::Ended(why) => format!("ended {}\n", why.word()),
        }
    }

    /// The event `line` says, without its newline.
    #[must_use]
    pub fn parse(line: &str) -> Option<HelpEvent> {
        let words: Vec<&str> = line.trim_end_matches('\n').split(' ').collect();
        match words[..] {
            ["helper", address, key] => {
                Some(HelpEvent::Helper { address: address.into(), key: key.into() })
            }
            ["left", minutes] => Some(HelpEvent::Left { minutes: minutes.parse().ok()? }),
            ["elevate", minutes] => Some(HelpEvent::Elevate { minutes: minutes.parse().ok()? }),
            ["spent"] => Some(HelpEvent::Spent),
            ["ended", why] => HelpEnded::from_word(why).map(HelpEvent::Ended),
            _ => None,
        }
    }
}

/// A help session's transcript, as authd lists them: `ID OWNER STARTED
/// ENDED`, the times in seconds since 1970, ENDED 0 while it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelpKept {
    /// The session's ID.
    pub id: String,
    /// Whose it was.
    pub owner: String,
    /// When it started.
    pub started: i64,
    /// When it ended, or 0.
    pub ended: i64,
}

impl HelpKept {
    /// Its words in authd's answer.
    #[must_use]
    pub fn words(&self) -> String {
        format!("{} {} {} {}", self.id, self.owner, self.started, self.ended)
    }

    /// The transcript `words` says.
    #[must_use]
    pub fn from_words(words: &str) -> Option<HelpKept> {
        let [id, owner, started, ended] = words.split(' ').collect::<Vec<_>>()[..] else {
            return None;
        };
        Some(HelpKept {
            id: id.into(),
            owner: owner.into(),
            started: started.parse().ok()?,
            ended: ended.parse().ok()?,
        })
    }
}

/// Why authd refuses a password from an address it blocked (ADR 0479),
/// unread: the answer begins with it, and says for how long.
pub const BLOCKED: &str = "this address is blocked for guessing passwords";

/// A block, as authd answers [`Request::Blocked`]: `ADDRESS UNTIL TIMES
/// WHY`, as `203.0.113.9 1794652320 2 5 wrong passwords over SSH in 2
/// minutes, for root`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockedEntry {
    /// The address, or the IPv6 network of 64 bits, as
    /// `2001:db8:4:7::/64`.
    pub address: String,
    /// When it ends, in seconds since 1970.
    pub until: i64,
    /// How many blocks of it came one after another, each within a day
    /// of the end of the one before, this one counted.
    pub times: u32,
    /// Why it was blocked.
    pub why: String,
}

impl BlockedEntry {
    /// Its words in authd's answer, kept to one line with no tab.
    #[must_use]
    pub fn words(&self) -> String {
        let why: String = self.why.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        format!("{} {} {} {why}", self.address, self.until, self.times)
    }

    /// The block `words` says, as [`BlockedEntry::words`] made them.
    #[must_use]
    pub fn from_words(words: &str) -> Option<BlockedEntry> {
        let [address, until, times, why] = words.splitn(4, ' ').collect::<Vec<_>>()[..] else {
            return None;
        };
        Some(BlockedEntry {
            address: address.to_string(),
            until: until.parse().ok()?,
            times: times.parse().ok()?,
            why: why.to_string(),
        })
    }

    /// The answer to [`Request::Blocked`] that lists `entries`.
    #[must_use]
    pub fn list(entries: &[BlockedEntry]) -> String {
        entries.iter().map(BlockedEntry::words).collect::<Vec<_>>().join("\t")
    }

    /// The blocks an answer to [`Request::Blocked`] lists, those that
    /// read.
    #[must_use]
    pub fn parse_list(answer: &str) -> Vec<BlockedEntry> {
        answer.split('\t').filter_map(BlockedEntry::from_words).collect()
    }
}

/// Whether a person's jobs run while they are signed out (ADR 0478), as
/// authd answers [`Request::KeepRunning`]: `CHOSEN DEFAULT BAR`, each a
/// word, as `default off allowed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeepRunning {
    /// What they chose, or `None` for the system's default.
    pub chosen: Option<bool>,
    /// The system's default, off unless /system/settings/jobs.toml says
    /// otherwise (the `server` preset turns it on).
    pub default: bool,
    /// Whether an administrator barred it for their account.
    pub barred: bool,
}

impl KeepRunning {
    /// Whether their jobs run while they are signed out.
    #[must_use]
    pub fn on(self) -> bool {
        !self.barred && self.chosen.unwrap_or(self.default)
    }

    /// Its words in authd's answer.
    #[must_use]
    pub fn words(self) -> String {
        let word = |on: bool| if on { "on" } else { "off" };
        format!(
            "{} {} {}",
            self.chosen.map_or("default", word),
            word(self.default),
            if self.barred { "barred" } else { "allowed" }
        )
    }

    /// The setting `words` says, as [`KeepRunning::words`] made them.
    #[must_use]
    pub fn from_words(words: &str) -> Option<KeepRunning> {
        let on = |word: &str| match word {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        };
        match words.split(' ').collect::<Vec<_>>()[..] {
            [chosen, default, bar] => Some(KeepRunning {
                chosen: if chosen == "default" { None } else { Some(on(chosen)?) },
                default: on(default)?,
                barred: match bar {
                    "barred" => true,
                    "allowed" => false,
                    _ => return None,
                },
            }),
            _ => None,
        }
    }
}

/// The most jobs authd runs at once (ADR 0478): timerd starts no more,
/// and holds the rest until one ends.
pub const JOBS_MAX: usize = 16;

/// Why a job did not run: its owner is signed out, and does not keep
/// their jobs running.
pub const JOB_SIGNED_OUT: &str = "its owner is signed out and does not keep their jobs running: crontab --keep-running on \
     turns that on";
/// Why a job did not run: its owner is signed out, and an administrator
/// barred their jobs from running then.
pub const JOB_BARRED: &str =
    "its owner is signed out, and an administrator barred their jobs from running then";
/// Why a job did not run: its owner's account is disabled.
pub const JOB_DISABLED: &str = "its owner's account is disabled";
/// Why a job did not run: its owner is no person's account, nor root's.
pub const JOB_NO_USER: &str = "its owner is no person's account";

/// The longest name of an entry in a keyring, in bytes.
pub const KEYRING_NAME_MAX: usize = 256;

/// The longest secret a keyring keeps, in bytes: room for a token, a
/// passphrase, or a few Kerberos tickets; in hexadecimal it fits in
/// [`REQUEST_MAX`].
pub const KEYRING_SECRET_MAX: usize = 16 * 1024;

/// Whether `name` may name an entry in a keyring: text of at most
/// [`KEYRING_NAME_MAX`] bytes, with no control characters, so that a list
/// of names separated by tabs reads back.
pub fn keyring_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= KEYRING_NAME_MAX && !name.chars().any(char::is_control)
}

/// How a person signed in over SSH, which authd checks again for each
/// session (ADRs 0347 and 0348).
#[derive(Clone, PartialEq, Eq)]
pub enum SshProof {
    /// With a key: what the client signed and its signature.
    Key {
        /// What the client signed (RFC 4252 7): the session's
        /// identifier, then its sign-in request up to the signature.
        signed: Vec<u8>,
        /// The signature: its algorithm's name, then its bytes.
        signature: Vec<u8>,
    },
    /// With a password: the ticket authd answered it with
    /// ([`Request::SshPassword`]).
    Ticket(Vec<u8>),
}

/// What an SSH client asked a session to run (RFC 4254 6.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SshRun {
    /// The person's shell.
    Shell,
    /// A command, which their shell runs.
    Command(Vec<u8>),
    /// The SFTP subsystem, which scp uses too: OpenSSH's sftp-server
    /// (ADR 0348).
    Sftp,
    /// The connection's forwarder, `sshd --forward`, which makes the
    /// connections the client forwards and listens on the ports it asks
    /// for, with the network of the person's session (ADR 0349).
    Forward,
}

/// The length of a ticket authd gives for a password over SSH, in bytes.
pub const SSH_TICKET_LENGTH: usize = 32;

/// What came of a prompt on a console.
#[derive(PartialEq, Eq)]
pub enum Prompted {
    /// The password typed.
    Password(Vec<u8>),
    /// What the person chose at a [`Prompt::Choose`].
    Chose(Choice),
    /// The person closed the prompt, with Esc or Ctrl+C.
    Cancelled,
    /// The terminal is not a console's: the asker asks itself.
    Elsewhere,
}

/// The kind of a [`Request::Choose`] question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Question {
    /// An app asks for a permission: once, always, or don't allow.
    Permission,
    /// The same, for one that reaches what other apps keep: with a
    /// warning.
    Warning,
    /// Giving an app a permission with `bundle grant`: yes or no.
    Grant,
}

impl Question {
    /// Its word in a request and a prompt line.
    pub fn word(self) -> &'static str {
        match self {
            Question::Permission => "permission",
            Question::Warning => "warning",
            Question::Grant => "grant",
        }
    }

    /// The question its word names.
    pub fn from_word(word: &str) -> Option<Question> {
        [Question::Permission, Question::Warning, Question::Grant]
            .into_iter()
            .find(|q| q.word() == word)
    }
}

/// What the person chose at a permission prompt: `bundle grant`'s yes is
/// [`Choice::Always`] and its no [`Choice::Never`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Allow it this time only.
    Once,
    /// Allow it from now on.
    Always,
    /// Don't allow it.
    Never,
}

impl Choice {
    /// Its word in a request and in authd's answer.
    pub fn word(self) -> &'static str {
        match self {
            Choice::Once => "once",
            Choice::Always => "always",
            Choice::Never => "never",
        }
    }

    /// The choice its word names.
    pub fn from_word(word: &str) -> Option<Choice> {
        [Choice::Once, Choice::Always, Choice::Never].into_iter().find(|c| c.word() == word)
    }
}

/// authd's answer to a [`Request::Choose`] the person closed.
pub const CANCELLED: &str = "cancelled";
/// authd's answer to a [`Request::Choose`] no console can ask.
pub const ELSEWHERE: &str = "elsewhere";

/// What an [`Request::Elevate`] runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The command, found in the system's own commands.
    Command,
    /// Root's shell, in the directory asked for (`sudo -s`).
    Shell,
    /// Root's shell as a login shell, in root's home (`sudo -i`).
    Login,
}

impl Mode {
    fn word(self) -> &'static [u8] {
        match self {
            Mode::Command => b"command",
            Mode::Shell => b"shell",
            Mode::Login => b"login",
        }
    }

    fn of(word: &[u8]) -> Option<Mode> {
        match word {
            b"command" => Some(Mode::Command),
            b"shell" => Some(Mode::Shell),
            b"login" => Some(Mode::Login),
            _ => None,
        }
    }
}

/// What `ok` says to an `elevate` or `unlock` request that needs a
/// password and came without one.
pub const NEEDS_PASSWORD: &str = "password";

/// Why a wrong password, or a name there is not, is refused: the same
/// words for both.
pub const WRONG_PASSWORD: &str = "the name or the password is wrong";

/// The fields of an optional password: whether there is one, or whether
/// the console may ask for it, then it.
fn password_fields(password: &Option<Vec<u8>>, ask: bool) -> [&[u8]; 2] {
    match password {
        Some(password) => [b"password", password],
        None if ask => [b"ask", b""],
        None => [b"none", b""],
    }
}

/// The optional password in `kind` and `password`, and whether the
/// console may ask for one, as [`password_fields`] made them.
fn password_of(kind: &[u8], password: &[u8]) -> Option<(Option<Vec<u8>>, bool)> {
    match kind {
        b"password" => Some((Some(password.to_vec()), false)),
        b"none" if password.is_empty() => Some((None, false)),
        b"ask" if password.is_empty() => Some((None, true)),
        _ => None,
    }
}

impl Request {
    /// What is sent.
    pub fn encode(&self) -> Vec<u8> {
        if let Request::Console(console) = self {
            let mut fields = console.fields();
            let mut bytes = b"console\0".to_vec();
            for field in &mut fields {
                bytes.extend_from_slice(field);
                bytes.push(0);
                wipe(field);
            }
            return bytes;
        }
        let id_text;
        let hexed;
        let number;
        // A secret's hexadecimal, wiped once the request is made.
        let mut secret_hex = Vec::new();
        let fields: Vec<&[u8]> = match self {
            Request::Check { name, password } => vec![b"check", name.as_bytes(), password],
            Request::Passwd { name, current, new } => {
                vec![b"passwd", name.as_bytes(), current, new]
            }
            Request::Add { name, full_name, admin, password } => vec![
                b"add",
                name.as_bytes(),
                full_name.as_bytes(),
                if *admin { b"admin" } else { b"user" },
                password,
            ],
            Request::NeedsOwner => vec![b"needs-owner"],
            Request::Owner { name, full_name, password } => {
                vec![b"owner", name.as_bytes(), full_name.as_bytes(), password]
            }
            Request::Session { name, password, term, command } => {
                let mut fields: Vec<&[u8]> =
                    vec![b"session", name.as_bytes(), password, term.as_bytes()];
                fields.extend(command.iter().map(String::as_bytes));
                fields
            }
            Request::Reset { name, password } => vec![b"reset", name.as_bytes(), password],
            Request::Disable { name } => vec![b"disable", name.as_bytes()],
            Request::Enable { name } => vec![b"enable", name.as_bytes()],
            Request::Remove { name } => vec![b"remove", name.as_bytes()],
            Request::Service { name, whose, home } => {
                vec![b"service", name.as_bytes(), whose.as_bytes(), home.as_bytes()]
            }
            Request::SocketGroup { name, whose } => {
                vec![b"socket-group", name.as_bytes(), whose.as_bytes()]
            }
            Request::Rename { name, new } => vec![b"rename", name.as_bytes(), new.as_bytes()],
            Request::Set { name, full_name, admin, without_password, console, jobs_signed_out } => {
                let mut fields: Vec<&[u8]> = vec![b"set", name.as_bytes()];
                if let Some(full_name) = full_name {
                    fields.extend([b"full-name".as_slice(), full_name.as_bytes()]);
                }
                let yes = |yes: bool| -> &[u8] { if yes { b"yes" } else { b"no" } };
                if let Some(admin) = admin {
                    fields.extend([b"admin".as_slice(), yes(*admin)]);
                }
                if let Some(without) = without_password {
                    fields.extend([b"without-password".as_slice(), yes(*without)]);
                }
                if let Some(console) = console {
                    fields.extend([b"console".as_slice(), yes(*console)]);
                }
                if let Some(allowed) = jobs_signed_out {
                    let word: &[u8] = if *allowed { b"allowed" } else { b"never" };
                    fields.extend([b"jobs-signed-out".as_slice(), word]);
                }
                fields
            }
            Request::Elevate { mode, password, ask, term, directory, language, command } => {
                let mut fields: Vec<&[u8]> = vec![b"elevate", mode.word()];
                fields.extend(password_fields(password, *ask));
                fields.extend([term.as_bytes(), directory.as_slice()]);
                fields.extend(language.iter().map(String::as_bytes));
                fields.push(b"--");
                fields.extend(command.iter().map(Vec::as_slice));
                fields
            }
            Request::Unlock { password, ask } => {
                let [kind, password] = password_fields(password, *ask);
                vec![b"unlock", kind, password]
            }
            Request::Forget => vec![b"forget"],
            Request::Prompts => vec![b"prompts"],
            Request::Prompted { id, typed } => {
                id_text = id.to_string();
                let (kind, password): (&[u8], &[u8]) = match typed {
                    Prompted::Password(password) => (b"password", password),
                    Prompted::Chose(choice) => (choice.word().as_bytes(), b""),
                    Prompted::Cancelled => (b"cancelled", b""),
                    Prompted::Elsewhere => (b"elsewhere", b""),
                };
                vec![b"prompted", id_text.as_bytes(), kind, password]
            }
            Request::Choose { terminal, name, question, text } => vec![
                b"choose",
                terminal.as_bytes(),
                name.as_bytes(),
                question.word().as_bytes(),
                text.as_bytes(),
            ],
            Request::SharePassword { name, current, new } => {
                let (kind, new): (&[u8], &[u8]) = match new {
                    Some(new) => (b"set", new),
                    None => (b"clear", b""),
                };
                vec![b"share-password", name.as_bytes(), current, kind, new]
            }
            Request::ShareSignIn { from, negotiate, challenge, authenticate } => {
                hexed = [hex(negotiate), hex(challenge), hex(authenticate)];
                let [negotiate, challenge, authenticate] = &hexed;
                vec![
                    b"share-signin",
                    from.as_bytes(),
                    negotiate.as_bytes(),
                    challenge.as_bytes(),
                    authenticate.as_bytes(),
                ]
            }
            Request::EndShares => vec![b"end-shares"],
            Request::NfsWorker { uid } => {
                number = uid.to_string();
                vec![b"nfs-worker", number.as_bytes()]
            }
            Request::SshKey { name, key } => {
                hexed = [hex(key), String::new(), String::new()];
                vec![b"ssh-key", name.as_bytes(), hexed[0].as_bytes()]
            }
            Request::SshPassword { connection, name, password } => {
                vec![b"ssh-password", connection.as_bytes(), name.as_bytes(), password]
            }
            Request::SshSession { connection, name, proof, term, run, language } => {
                let proven: &[u8] = match proof {
                    SshProof::Key { signed, signature } => {
                        hexed = [hex(signed), hex(signature), String::new()];
                        b"key"
                    }
                    SshProof::Ticket(ticket) => {
                        hexed = [hex(ticket), String::new(), String::new()];
                        b"ticket"
                    }
                };
                let (kind, what): (&[u8], &[u8]) = match run {
                    SshRun::Shell => (b"shell", b""),
                    SshRun::Command(command) => (b"command", command),
                    SshRun::Sftp => (b"subsystem", b"sftp"),
                    SshRun::Forward => (b"forward", b""),
                };
                let mut fields: Vec<&[u8]> = vec![
                    b"ssh-session",
                    connection.as_bytes(),
                    name.as_bytes(),
                    proven,
                    hexed[0].as_bytes(),
                    hexed[1].as_bytes(),
                    term.as_bytes(),
                    kind,
                    what,
                ];
                fields.extend(language.iter().map(String::as_bytes));
                fields
            }
            Request::SshEnded { ticket } => {
                hexed = [hex(ticket), String::new(), String::new()];
                vec![b"ssh-ended", hexed[0].as_bytes()]
            }
            Request::KeyringSet { name, secret } => {
                secret_hex = hex(secret).into_bytes();
                vec![b"keyring-set", name.as_bytes(), &secret_hex]
            }
            Request::KeyringGet { name } => vec![b"keyring-get", name.as_bytes()],
            Request::KeyringList => vec![b"keyring-list"],
            Request::KeyringRemove { name } => vec![b"keyring-remove", name.as_bytes()],
            Request::KeyringUnlock { password } => vec![b"keyring-unlock", password],
            Request::KeyringLock => vec![b"keyring-lock"],
            Request::KerberosTickets { uid } => {
                number = uid.to_string();
                vec![b"kerberos-tickets", number.as_bytes()]
            }
            Request::Console(_) => unreachable!("sent above"),
            Request::Job { owner, label, folder, command, environment } => {
                let mut fields: Vec<&[u8]> = vec![
                    b"job",
                    owner.as_bytes(),
                    label.as_bytes(),
                    folder.as_bytes(),
                    command.as_bytes(),
                ];
                fields.extend(environment.iter().map(String::as_bytes));
                fields
            }
            Request::KeepRunning => vec![b"keep-running"],
            Request::SetKeepRunning { chosen } => {
                let word: &[u8] = match chosen {
                    Some(true) => b"on",
                    Some(false) => b"off",
                    None => b"default",
                };
                vec![b"keep-running", word]
            }
            Request::SshFailed { connection, name } => {
                vec![b"ssh-failed", connection.as_bytes(), name.as_bytes()]
            }
            Request::Blocked => vec![b"blocked"],
            Request::Unblock { address } => vec![b"unblock", address.as_bytes()],
            Request::NeverBlock { network, remove: false } => {
                vec![b"never-block", network.as_bytes()]
            }
            Request::NeverBlock { network, remove: true } => {
                vec![b"never-block", network.as_bytes(), b"remove"]
            }
            Request::HelpStart { minutes, key, open_ssh, term, password } => {
                number = minutes.to_string();
                hexed =
                    [key.as_deref().map_or_else(String::new, hex), String::new(), String::new()];
                let open: &[u8] = if *open_ssh { b"open-ssh" } else { b"" };
                vec![
                    b"help-start",
                    number.as_bytes(),
                    hexed[0].as_bytes(),
                    open,
                    term.as_bytes(),
                    password,
                ]
            }
            Request::HelpEnd => vec![b"help-end"],
            Request::HelpLog { id: None } => vec![b"help-log"],
            Request::HelpLog { id: Some(id) } => vec![b"help-log", id.as_bytes()],
            Request::HelpAllowElevate { password } => vec![b"help-allow-elevate", password],
            Request::SshHelpKey { key } => {
                hexed = [hex(key), String::new(), String::new()];
                vec![b"ssh-help-key", hexed[0].as_bytes()]
            }
            Request::SshHelp { connection, proof: HelpProof::Code(code) } => {
                vec![b"ssh-help", connection.as_bytes(), b"code", code.as_bytes()]
            }
            Request::SshHelpEnded { id } => vec![b"ssh-help-ended", id.as_bytes()],
            Request::SshHelp { connection, proof: HelpProof::Key { signed, signature } } => {
                hexed = [hex(signed), hex(signature), String::new()];
                vec![
                    b"ssh-help",
                    connection.as_bytes(),
                    b"key",
                    hexed[0].as_bytes(),
                    hexed[1].as_bytes(),
                ]
            }
        };
        let mut bytes = Vec::new();
        for field in fields {
            bytes.extend_from_slice(field);
            bytes.push(0);
        }
        wipe(&mut secret_hex);
        bytes
    }

    /// The request in `bytes`, all of what came.
    pub fn decode(bytes: &[u8]) -> Result<Request, String> {
        let malformed = || "a request authd does not know".to_string();
        if bytes.len() > REQUEST_MAX {
            return Err("a request longer than authd reads".into());
        }
        let body = bytes.strip_suffix(&[0]).ok_or_else(malformed)?;
        let fields: Vec<&[u8]> = body.split(|&b| b == 0).collect();
        let text = |field: &[u8]| String::from_utf8(field.to_vec()).map_err(|_| malformed());
        match fields[..] {
            [b"check", name, password] => {
                Ok(Request::Check { name: text(name)?, password: password.to_vec() })
            }
            [b"passwd", name, current, new] => Ok(Request::Passwd {
                name: text(name)?,
                current: current.to_vec(),
                new: new.to_vec(),
            }),
            [b"add", name, full_name, kind @ (b"admin" | b"user"), password] => Ok(Request::Add {
                name: text(name)?,
                full_name: text(full_name)?,
                admin: kind == b"admin",
                password: password.to_vec(),
            }),
            [b"needs-owner"] => Ok(Request::NeedsOwner),
            [b"reset", name, password] => {
                Ok(Request::Reset { name: text(name)?, password: password.to_vec() })
            }
            [b"disable", name] => Ok(Request::Disable { name: text(name)? }),
            [b"enable", name] => Ok(Request::Enable { name: text(name)? }),
            [b"remove", name] => Ok(Request::Remove { name: text(name)? }),
            [b"service", name, whose, home] => {
                Ok(Request::Service { name: text(name)?, whose: text(whose)?, home: text(home)? })
            }
            [b"socket-group", name, whose] => {
                Ok(Request::SocketGroup { name: text(name)?, whose: text(whose)? })
            }
            [b"rename", name, new] => Ok(Request::Rename { name: text(name)?, new: text(new)? }),
            [b"set", name, ref settings @ ..] => {
                let (mut full_name, mut admin, mut without_password) = (None, None, None);
                let mut console = None;
                let mut jobs_signed_out = None;
                let (pairs, rest) = settings.as_chunks::<2>();
                if !rest.is_empty() || pairs.is_empty() {
                    return Err(malformed());
                }
                for [key, value] in pairs {
                    match (*key, *value) {
                        (b"full-name", value) if full_name.is_none() => {
                            full_name = Some(text(value)?);
                        }
                        (b"admin", value @ (b"yes" | b"no")) if admin.is_none() => {
                            admin = Some(value == b"yes");
                        }
                        (b"without-password", value @ (b"yes" | b"no"))
                            if without_password.is_none() =>
                        {
                            without_password = Some(value == b"yes");
                        }
                        (b"console", value @ (b"yes" | b"no")) if console.is_none() => {
                            console = Some(value == b"yes");
                        }
                        (b"jobs-signed-out", value @ (b"allowed" | b"never"))
                            if jobs_signed_out.is_none() =>
                        {
                            jobs_signed_out = Some(value == b"allowed");
                        }
                        _ => return Err(malformed()),
                    }
                }
                Ok(Request::Set {
                    name: text(name)?,
                    full_name,
                    admin,
                    without_password,
                    console,
                    jobs_signed_out,
                })
            }
            [b"owner", name, full_name, password] => Ok(Request::Owner {
                name: text(name)?,
                full_name: text(full_name)?,
                password: password.to_vec(),
            }),
            [b"session", name, password, term, ref command @ ..] => {
                // Older sign-in programs send no command, and a session
                // runs one command at most.
                let command = match command {
                    [] => None,
                    [command] => match text(command)? {
                        command if command.is_empty() => return Err(malformed()),
                        command => Some(command),
                    },
                    _ => return Err(malformed()),
                };
                Ok(Request::Session {
                    name: text(name)?,
                    password: password.to_vec(),
                    term: text(term)?,
                    command,
                })
            }
            [b"elevate", mode, kind, password, term, directory, ref rest @ ..] => {
                let mode = Mode::of(mode).ok_or_else(malformed)?;
                let (password, ask) = password_of(kind, password).ok_or_else(malformed)?;
                // The language settings, up to the first `--`; the command
                // after it.
                let split = rest.iter().position(|f| *f == b"--").ok_or_else(malformed)?;
                let language = rest[..split]
                    .iter()
                    .map(|f| text(f).ok().filter(|t| t.contains('=')).ok_or_else(malformed))
                    .collect::<Result<Vec<_>, _>>()?;
                let command: Vec<Vec<u8>> = rest[split + 1..].iter().map(|f| f.to_vec()).collect();
                if mode == Mode::Command && command.is_empty() {
                    return Err(malformed());
                }
                Ok(Request::Elevate {
                    mode,
                    password,
                    ask,
                    term: text(term)?,
                    directory: directory.to_vec(),
                    language,
                    command,
                })
            }
            [b"unlock", kind, password] => {
                let (password, ask) = password_of(kind, password).ok_or_else(malformed)?;
                Ok(Request::Unlock { password, ask })
            }
            [b"forget"] => Ok(Request::Forget),
            [b"prompts"] => Ok(Request::Prompts),
            [b"prompted", id, kind, password] => {
                let id = text(id)?.parse().map_err(|_| malformed())?;
                let typed = match (kind, password) {
                    (b"password", password) => Prompted::Password(password.to_vec()),
                    (b"cancelled", b"") => Prompted::Cancelled,
                    (b"elsewhere", b"") => Prompted::Elsewhere,
                    (kind, b"") => {
                        let choice = std::str::from_utf8(kind).ok().and_then(Choice::from_word);
                        Prompted::Chose(choice.ok_or_else(malformed)?)
                    }
                    _ => return Err(malformed()),
                };
                Ok(Request::Prompted { id, typed })
            }
            [b"choose", terminal, name, question, words] => {
                let question = Question::from_word(&text(question)?).ok_or_else(malformed)?;
                let (terminal, name, words) = (text(terminal)?, text(name)?, text(words)?);
                let one_line = |t: &str| !t.is_empty() && !t.chars().any(char::is_control);
                if !one_line(&terminal) || !one_line(&name) || !one_line(&words) {
                    return Err(malformed());
                }
                Ok(Request::Choose { terminal, name, question, text: words })
            }
            [b"share-password", name, current, kind, new] => {
                let new = match (kind, new) {
                    (b"set", new) => Some(new.to_vec()),
                    (b"clear", b"") => None,
                    _ => return Err(malformed()),
                };
                Ok(Request::SharePassword { name: text(name)?, current: current.to_vec(), new })
            }
            [b"share-signin", from, negotiate, challenge, authenticate] => {
                let bytes = |field: &[u8]| {
                    std::str::from_utf8(field).ok().and_then(unhex).ok_or_else(malformed)
                };
                Ok(Request::ShareSignIn {
                    from: text(from)?,
                    negotiate: bytes(negotiate)?,
                    challenge: bytes(challenge)?,
                    authenticate: bytes(authenticate)?,
                })
            }
            [b"end-shares"] => Ok(Request::EndShares),
            [b"nfs-worker", uid] => {
                let uid = std::str::from_utf8(uid).ok().and_then(|u| u.parse().ok());
                Ok(Request::NfsWorker { uid: uid.ok_or_else(malformed)? })
            }
            [b"ssh-key", name, key] => {
                let key = std::str::from_utf8(key).ok().and_then(unhex).ok_or_else(malformed)?;
                Ok(Request::SshKey { name: text(name)?, key })
            }
            [b"ssh-password", connection, name, password] => Ok(Request::SshPassword {
                connection: text(connection)?,
                name: text(name)?,
                password: password.to_vec(),
            }),
            [
                b"ssh-session",
                connection,
                name,
                proven,
                first,
                second,
                term,
                kind,
                what,
                ref rest @ ..,
            ] => {
                let bytes = |field: &[u8]| {
                    std::str::from_utf8(field).ok().and_then(unhex).ok_or_else(malformed)
                };
                let proof = match proven {
                    b"key" => SshProof::Key { signed: bytes(first)?, signature: bytes(second)? },
                    b"ticket" if second.is_empty() => {
                        let ticket = bytes(first)?;
                        if ticket.len() != SSH_TICKET_LENGTH {
                            return Err(malformed());
                        }
                        SshProof::Ticket(ticket)
                    }
                    _ => return Err(malformed()),
                };
                let run = match (kind, what) {
                    (b"shell", b"") => SshRun::Shell,
                    (b"command", command) if command.len() <= SSH_COMMAND_MAX => {
                        SshRun::Command(command.to_vec())
                    }
                    (b"subsystem", b"sftp") => SshRun::Sftp,
                    (b"forward", b"") => SshRun::Forward,
                    _ => return Err(malformed()),
                };
                let language = rest
                    .iter()
                    .map(|f| text(f).ok().filter(|t| t.contains('=')).ok_or_else(malformed))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Request::SshSession {
                    connection: text(connection)?,
                    name: text(name)?,
                    proof,
                    term: text(term)?,
                    run,
                    language,
                })
            }
            [b"ssh-ended", ticket] => {
                let ticket = std::str::from_utf8(ticket).ok().and_then(unhex);
                match ticket.filter(|t| t.len() == SSH_TICKET_LENGTH) {
                    Some(ticket) => Ok(Request::SshEnded { ticket }),
                    None => Err(malformed()),
                }
            }
            [b"keyring-set", name, secret] => {
                let secret = std::str::from_utf8(secret).ok().and_then(unhex);
                match secret.filter(|s| s.len() <= KEYRING_SECRET_MAX) {
                    Some(secret) => Ok(Request::KeyringSet { name: text(name)?, secret }),
                    None => Err(malformed()),
                }
            }
            [b"keyring-get", name] => Ok(Request::KeyringGet { name: text(name)? }),
            [b"keyring-list"] => Ok(Request::KeyringList),
            [b"keyring-remove", name] => Ok(Request::KeyringRemove { name: text(name)? }),
            [b"keyring-unlock", password] => {
                Ok(Request::KeyringUnlock { password: password.to_vec() })
            }
            [b"keyring-lock"] => Ok(Request::KeyringLock),
            [b"kerberos-tickets", uid] => {
                let uid = std::str::from_utf8(uid).ok().and_then(|u| u.parse().ok());
                Ok(Request::KerberosTickets { uid: uid.ok_or_else(malformed)? })
            }
            [b"console", ref rest @ ..] => {
                Console::decode(rest).map(Request::Console).ok_or_else(malformed)
            }
            [b"job", owner, label, folder, command, ref environment @ ..] => {
                let environment = environment
                    .iter()
                    .map(|f| {
                        text(f)
                            .ok()
                            .filter(|t| t.find('=').is_some_and(|at| at > 0))
                            .ok_or_else(malformed)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let size = command.len() + environment.iter().map(String::len).sum::<usize>();
                let (owner, label, folder) = (text(owner)?, text(label)?, text(folder)?);
                let one_line = |t: &str| !t.is_empty() && !t.chars().any(char::is_control);
                if command.is_empty()
                    || size > JOB_TEXT_MAX
                    || !one_line(&owner)
                    || !one_line(&label)
                    || !folder.starts_with('/')
                {
                    return Err(malformed());
                }
                Ok(Request::Job { owner, label, folder, command: text(command)?, environment })
            }
            [b"keep-running"] => Ok(Request::KeepRunning),
            [b"keep-running", word] => {
                let chosen = match word {
                    b"on" => Some(true),
                    b"off" => Some(false),
                    b"default" => None,
                    _ => return Err(malformed()),
                };
                Ok(Request::SetKeepRunning { chosen })
            }
            [b"ssh-failed", connection, name] => {
                Ok(Request::SshFailed { connection: text(connection)?, name: text(name)? })
            }
            [b"blocked"] => Ok(Request::Blocked),
            [b"unblock", address] => Ok(Request::Unblock { address: text(address)? }),
            [b"never-block", network] => {
                Ok(Request::NeverBlock { network: text(network)?, remove: false })
            }
            [b"never-block", network, b"remove"] => {
                Ok(Request::NeverBlock { network: text(network)?, remove: true })
            }
            [b"help-start", minutes, key, open, term, password] => {
                let minutes = std::str::from_utf8(minutes).ok().and_then(|m| m.parse().ok());
                let key = match key {
                    [] => None,
                    key => {
                        Some(std::str::from_utf8(key).ok().and_then(unhex).ok_or_else(malformed)?)
                    }
                };
                let open_ssh = match open {
                    b"open-ssh" => true,
                    b"" => false,
                    _ => return Err(malformed()),
                };
                Ok(Request::HelpStart {
                    minutes: minutes.ok_or_else(malformed)?,
                    key,
                    open_ssh,
                    term: text(term)?,
                    password: password.to_vec(),
                })
            }
            [b"help-end"] => Ok(Request::HelpEnd),
            [b"ssh-help-ended", id] => Ok(Request::SshHelpEnded { id: text(id)? }),
            [b"help-log"] => Ok(Request::HelpLog { id: None }),
            [b"help-log", id] => Ok(Request::HelpLog { id: Some(text(id)?) }),
            [b"help-allow-elevate", password] => {
                Ok(Request::HelpAllowElevate { password: password.to_vec() })
            }
            [b"ssh-help-key", key] => {
                let key = std::str::from_utf8(key).ok().and_then(unhex).ok_or_else(malformed)?;
                Ok(Request::SshHelpKey { key })
            }
            [b"ssh-help", connection, b"code", code] => Ok(Request::SshHelp {
                connection: text(connection)?,
                proof: HelpProof::Code(text(code)?),
            }),
            [b"ssh-help", connection, b"key", signed, signature] => {
                let unhexed = |f: &[u8]| std::str::from_utf8(f).ok().and_then(unhex);
                let (Some(signed), Some(signature)) = (unhexed(signed), unhexed(signature)) else {
                    return Err(malformed());
                };
                Ok(Request::SshHelp {
                    connection: text(connection)?,
                    proof: HelpProof::Key { signed, signature },
                })
            }
            _ => Err(malformed()),
        }
    }

    /// Overwrites the passwords it holds.
    pub fn wipe(&mut self) {
        match self {
            Request::Check { password, .. }
            | Request::Add { password, .. }
            | Request::Owner { password, .. }
            | Request::Session { password, .. }
            | Request::Reset { password, .. } => wipe(password),
            Request::Passwd { current, new, .. } => {
                wipe(current);
                wipe(new);
            }
            Request::Elevate { password, .. } | Request::Unlock { password, .. } => {
                if let Some(password) = password {
                    wipe(password);
                }
            }
            Request::Prompted { typed: Prompted::Password(password), .. } => wipe(password),
            Request::SharePassword { current, new, .. } => {
                wipe(current);
                if let Some(new) = new {
                    wipe(new);
                }
            }
            Request::SshPassword { password, .. } | Request::KeyringUnlock { password } => {
                wipe(password);
            }
            Request::SshSession { proof: SshProof::Ticket(ticket), .. }
            | Request::SshEnded { ticket } => wipe(ticket),
            Request::KeyringSet { secret, .. } => wipe(secret),
            Request::HelpStart { password, .. } | Request::HelpAllowElevate { password } => {
                wipe(password);
            }
            Request::SshHelp { proof: HelpProof::Code(code), .. } => {
                let mut bytes = std::mem::take(code).into_bytes();
                wipe(&mut bytes);
            }
            Request::NeedsOwner
            | Request::Forget
            | Request::KeyringGet { .. }
            | Request::KeyringList
            | Request::KeyringRemove { .. }
            | Request::KeyringLock
            | Request::KerberosTickets { .. }
            // Its own Drop wipes it.
            | Request::Console(_)
            | Request::Prompts
            | Request::Prompted { .. }
            | Request::Choose { .. }
            | Request::Disable { .. }
            | Request::Enable { .. }
            | Request::Remove { .. }
            | Request::Service { .. }
            | Request::SocketGroup { .. }
            | Request::Rename { .. }
            | Request::Set { .. }
            | Request::ShareSignIn { .. }
            | Request::EndShares
            | Request::NfsWorker { .. }
            | Request::SshKey { .. }
            | Request::SshSession { proof: SshProof::Key { .. }, .. }
            | Request::Job { .. }
            | Request::KeepRunning
            | Request::SetKeepRunning { .. }
            | Request::SshFailed { .. }
            | Request::Blocked
            | Request::Unblock { .. }
            | Request::NeverBlock { .. }
            | Request::HelpEnd
            | Request::HelpLog { .. }
            | Request::SshHelpKey { .. }
            | Request::SshHelpEnded { .. }
            | Request::SshHelp { proof: HelpProof::Key { .. }, .. } => {}
        }
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        self.wipe();
    }
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Request::Check { name, .. } => write!(f, "Check {{ name: {name:?} }}"),
            Request::Passwd { name, .. } => write!(f, "Passwd {{ name: {name:?} }}"),
            Request::Add { name, full_name, admin, .. } => {
                write!(f, "Add {{ name: {name:?}, full_name: {full_name:?}, admin: {admin} }}")
            }
            Request::NeedsOwner => write!(f, "NeedsOwner"),
            Request::Reset { name, .. } => write!(f, "Reset {{ name: {name:?} }}"),
            Request::Disable { name } => write!(f, "Disable {{ name: {name:?} }}"),
            Request::Enable { name } => write!(f, "Enable {{ name: {name:?} }}"),
            Request::Remove { name } => write!(f, "Remove {{ name: {name:?} }}"),
            Request::Service { name, whose, home } => {
                write!(f, "Service {{ name: {name:?}, whose: {whose:?}, home: {home:?} }}")
            }
            Request::SocketGroup { name, whose } => {
                write!(f, "SocketGroup {{ name: {name:?}, whose: {whose:?} }}")
            }
            Request::Rename { name, new } => {
                write!(f, "Rename {{ name: {name:?}, new: {new:?} }}")
            }
            Request::Set { name, full_name, admin, without_password, console, jobs_signed_out } => {
                write!(
                    f,
                    "Set {{ name: {name:?}, full_name: {full_name:?}, admin: {admin:?}, \
                     without_password: {without_password:?}, console: {console:?}, \
                     jobs_signed_out: {jobs_signed_out:?} }}"
                )
            }
            Request::Owner { name, full_name, .. } => {
                write!(f, "Owner {{ name: {name:?}, full_name: {full_name:?} }}")
            }
            Request::Session { name, term, command, .. } => {
                write!(f, "Session {{ name: {name:?}, term: {term:?}, command: {command:?} }}")
            }
            Request::Elevate { mode, password, ask, term, command, .. } => {
                let typed = password.is_some();
                let command: Vec<_> = command.iter().map(|w| String::from_utf8_lossy(w)).collect();
                write!(
                    f,
                    "Elevate {{ mode: {mode:?}, password: {typed}, ask: {ask}, term: {term:?}, \
                     command: {command:?} }}"
                )
            }
            Request::Unlock { password, ask } => {
                write!(f, "Unlock {{ password: {}, ask: {ask} }}", password.is_some())
            }
            Request::Forget => write!(f, "Forget"),
            Request::Prompts => write!(f, "Prompts"),
            Request::Prompted { id, typed } => {
                let typed = match typed {
                    Prompted::Password(_) => "Password",
                    Prompted::Chose(choice) => choice.word(),
                    Prompted::Cancelled => "Cancelled",
                    Prompted::Elsewhere => "Elsewhere",
                };
                write!(f, "Prompted {{ id: {id}, typed: {typed} }}")
            }
            Request::Choose { terminal, name, question, text } => write!(
                f,
                "Choose {{ terminal: {terminal:?}, name: {name:?}, question: {question:?}, text: \
                 {text:?} }}"
            ),
            Request::SharePassword { name, new, .. } => {
                write!(f, "SharePassword {{ name: {name:?}, clear: {} }}", new.is_none())
            }
            Request::ShareSignIn { from, .. } => write!(f, "ShareSignIn {{ from: {from:?} }}"),
            Request::EndShares => write!(f, "EndShares"),
            Request::NfsWorker { uid } => write!(f, "NfsWorker {{ uid: {uid} }}"),
            Request::SshKey { name, .. } => write!(f, "SshKey {{ name: {name:?} }}"),
            Request::SshPassword { connection, name, .. } => {
                write!(f, "SshPassword {{ connection: {connection:?}, name: {name:?} }}")
            }
            Request::SshSession { connection, name, proof, term, run, .. } => {
                let proof = match proof {
                    SshProof::Key { .. } => "Key",
                    SshProof::Ticket(_) => "Ticket",
                };
                let run = match run {
                    SshRun::Shell => "Shell",
                    SshRun::Command(_) => "Command",
                    SshRun::Sftp => "Sftp",
                    SshRun::Forward => "Forward",
                };
                write!(
                    f,
                    "SshSession {{ connection: {connection:?}, name: {name:?}, proof: {proof}, \
                     term: {term:?}, run: {run} }}"
                )
            }
            Request::SshEnded { .. } => write!(f, "SshEnded"),
            Request::KeyringSet { name, .. } => write!(f, "KeyringSet {{ name: {name:?} }}"),
            Request::KeyringGet { name } => write!(f, "KeyringGet {{ name: {name:?} }}"),
            Request::KeyringList => write!(f, "KeyringList"),
            Request::KeyringRemove { name } => write!(f, "KeyringRemove {{ name: {name:?} }}"),
            Request::KeyringUnlock { .. } => write!(f, "KeyringUnlock"),
            Request::KeyringLock => write!(f, "KeyringLock"),
            Request::KerberosTickets { uid } => write!(f, "KerberosTickets {{ uid: {uid} }}"),
            Request::Console(console) => write!(f, "{console:?}"),
            Request::Job { owner, label, folder, .. } => {
                write!(f, "Job {{ owner: {owner:?}, label: {label:?}, folder: {folder:?} }}")
            }
            Request::KeepRunning => write!(f, "KeepRunning"),
            Request::SetKeepRunning { chosen } => {
                write!(f, "SetKeepRunning {{ chosen: {chosen:?} }}")
            }
            Request::SshFailed { connection, name } => {
                write!(f, "SshFailed {{ connection: {connection:?}, name: {name:?} }}")
            }
            Request::Blocked => write!(f, "Blocked"),
            Request::Unblock { address } => write!(f, "Unblock {{ address: {address:?} }}"),
            Request::NeverBlock { network, remove } => {
                write!(f, "NeverBlock {{ network: {network:?}, remove: {remove} }}")
            }
            Request::HelpStart { minutes, key, open_ssh, term, .. } => write!(
                f,
                "HelpStart {{ minutes: {minutes}, key: {}, open_ssh: {open_ssh}, term: {term:?} }}",
                key.is_some()
            ),
            Request::HelpEnd => write!(f, "HelpEnd"),
            Request::HelpLog { id } => write!(f, "HelpLog {{ id: {id:?} }}"),
            Request::HelpAllowElevate { .. } => write!(f, "HelpAllowElevate"),
            Request::SshHelpKey { .. } => write!(f, "SshHelpKey"),
            Request::SshHelpEnded { id } => write!(f, "SshHelpEnded {{ id: {id:?} }}"),
            Request::SshHelp { connection, proof } => {
                let proof = match proof {
                    HelpProof::Code(_) => "code",
                    HelpProof::Key { .. } => "key",
                };
                write!(f, "SshHelp {{ connection: {connection:?}, proof: {proof} }}")
            }
        }
    }
}

/// authd's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Done, with what there is to say.
    Done(String),
    /// Not done, and why, in words for the person who asked.
    Refused(String),
}

impl Answer {
    /// Its line.
    pub fn encode(&self) -> String {
        let one_line = |text: &str| text.replace(['\n', '\r'], " ");
        match self {
            Answer::Done(text) if text.is_empty() => "ok\n".into(),
            Answer::Done(text) => format!("ok {}\n", one_line(text)),
            Answer::Refused(why) => format!("no {}\n", one_line(why)),
        }
    }

    /// The answer in `line`; one authd would not give is a refusal that
    /// says so.
    pub fn decode(line: &str) -> Answer {
        let line = line.trim_end_matches('\n');
        match line.split_once(' ').unwrap_or((line, "")) {
            ("ok", text) => Answer::Done(text.into()),
            ("no", why) if !why.is_empty() => Answer::Refused(why.into()),
            _ => Answer::Refused(format!("authd answered `{line}`")),
        }
    }
}

/// The line authd writes on a `session` connection when the session's
/// first process ends with wait status `status`.
pub fn ended(status: i32) -> String {
    format!("ended {status}\n")
}

/// The wait status in a line [`ended`] made.
pub fn ended_status(line: &str) -> Option<i32> {
    line.trim_end_matches('\n').strip_prefix("ended ")?.parse().ok()
}

/// The line authd writes on an `elevate` connection when the command
/// stops with signal `signal`, as Ctrl+Z stops it.
pub fn stopped(signal: i32) -> String {
    format!("stopped {signal}\n")
}

/// The signal in a line [`stopped`] made.
pub fn stopped_signal(line: &str) -> Option<i32> {
    line.trim_end_matches('\n').strip_prefix("stopped ")?.parse().ok()
}

/// The line elevate writes to its pipe to authd to have it send the
/// command `signal`, as a terminal would.
pub fn signal(signal: i32) -> String {
    format!("signal {signal}\n")
}

/// The line elevate writes to its pipe to authd to have the stopped
/// command go on, as `fg` has elevate do.
pub const CONTINUE: &str = "continue\n";

/// What a line elevate wrote to its pipe asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relay {
    /// Send the command this signal.
    Signal(i32),
    /// Have the stopped command go on.
    Continue,
}

impl Relay {
    /// The line `line`, without its newline, asks for, if it is one of
    /// [`signal`]'s or [`CONTINUE`].
    pub fn decode(line: &str) -> Option<Relay> {
        if line == CONTINUE.trim_end() {
            return Some(Relay::Continue);
        }
        let number: i32 = line.strip_prefix("signal ")?.parse().ok()?;
        (1..=64).contains(&number).then_some(Relay::Signal(number))
    }
}

/// The longest command line a [`Prompt::Ask`] shows, in characters.
pub const PROMPT_SHOWN_MAX: usize = 256;

/// A line authd writes on consoled's `prompts` connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    /// Ask `name` for their password on the console of `terminal`, as
    /// `pts/N`, to run `shown`, or to unlock elevate there if it is empty.
    Ask {
        /// The prompt's number, for the answer.
        id: u64,
        /// The terminal, as /processes names it.
        terminal: String,
        /// Whose password it is.
        name: String,
        /// The command line, one line of at most [`PROMPT_SHOWN_MAX`]
        /// characters; empty for an unlock.
        shown: String,
    },
    /// Ask `name` the question `text` on the console of `terminal`, as
    /// `pts/N`, with the keys and colours of `question`.
    Choose {
        /// The prompt's number, for the answer.
        id: u64,
        /// The terminal, as /processes names it.
        terminal: String,
        /// Whose question it is.
        name: String,
        /// Its kind.
        question: Question,
        /// Its words, one line of at most [`PROMPT_SHOWN_MAX`] characters.
        text: String,
    },
    /// Prompt `id` is no longer wanted: its asker went away.
    Done {
        /// The prompt's number.
        id: u64,
    },
}

impl Prompt {
    /// Its line. A field with a space or a control character in it would
    /// not come back the same, so a terminal's or name's are left as they
    /// are only if they have none; the command line takes the rest.
    pub fn encode(&self) -> String {
        let word = |text: &str| -> String {
            text.chars()
                .map(|c| if c.is_whitespace() || c.is_control() { '?' } else { c })
                .collect()
        };
        match self {
            Prompt::Ask { id, terminal, name, shown } => {
                let shown: String = shown
                    .chars()
                    .map(|c| if c.is_control() { '?' } else { c })
                    .take(PROMPT_SHOWN_MAX)
                    .collect();
                format!("ask {id} {} {} {shown}\n", word(terminal), word(name))
            }
            Prompt::Choose { id, terminal, name, question, text } => {
                let text: String = text
                    .chars()
                    .map(|c| if c.is_control() { '?' } else { c })
                    .take(PROMPT_SHOWN_MAX)
                    .collect();
                let (terminal, name, question) = (word(terminal), word(name), question.word());
                format!("choose {id} {terminal} {name} {question} {text}\n")
            }
            Prompt::Done { id } => format!("done {id}\n"),
        }
    }

    /// The prompt line in `line`, without or with its newline.
    pub fn decode(line: &str) -> Option<Prompt> {
        let line = line.strip_suffix('\n').unwrap_or(line);
        let (verb, rest) = line.split_once(' ')?;
        match verb {
            "ask" => {
                let mut fields = rest.splitn(4, ' ');
                let id = fields.next()?.parse().ok()?;
                let terminal = fields.next().filter(|t| !t.is_empty())?.to_string();
                let name = fields.next().filter(|n| !n.is_empty())?.to_string();
                let shown = fields.next()?.to_string();
                Some(Prompt::Ask { id, terminal, name, shown })
            }
            "choose" => {
                let mut fields = rest.splitn(5, ' ');
                let id = fields.next()?.parse().ok()?;
                let terminal = fields.next().filter(|t| !t.is_empty())?.to_string();
                let name = fields.next().filter(|n| !n.is_empty())?.to_string();
                let question = Question::from_word(fields.next()?)?;
                let text = fields.next().filter(|t| !t.is_empty())?.to_string();
                Some(Prompt::Choose { id, terminal, name, question, text })
            }
            "done" => Some(Prompt::Done { id: rest.parse().ok()? }),
            _ => None,
        }
    }
}
