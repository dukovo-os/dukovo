// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! login(1): signing in at a console (ADR 0250).
//!
//! consoled(8) runs `login` as root on each console's terminal. It asks
//! for a name and a password and sends them to authd(8) with the
//! terminal; authd checks them and starts the person's session there,
//! acting for them, and tells login when it ends. login then ends too,
//! and consoled shows the sign-in again. On a machine no account can be
//! signed in to yet, login first makes its owner's, an administrator's.
//!
//! In a session, authd runs `login --session` first, as the person: it
//! makes the terminal the session's controlling terminal, goes to their
//! home and runs their shell as a login shell, as login(1) does on Linux
//! after its checks. An SSH session (ADR 0347) may run on pipes rather
//! than a terminal, and with `--command` runs that command with the
//! shell's `-c`, as Linux's sshd runs one, instead of a login shell.
//!
//! While the failure notice of a start that failed its health check is
//! shown (ADR 0501), login offers three keys after it, read one at a time
//! (ADR 0532): `Q` draws the QR code of the notice's support page, for a
//! phone to read; `R` asks for an administrator's name and password and
//! signs them in with the command `report failure`, so that their session
//! is the report and ends with it; Enter signs in as usual, and any other
//! printable key starts the name. A terminal too narrow or too low for the
//! code gets the address alone.
//!
//! A computer started from its boot menu's startup-trouble entry, with
//! the kernel flag `startup-trouble`, shows the startup-trouble menu
//! before the sign-in (ADR 0253): it resets an administrator's password
//! through authd, which tells every administrator at their next sign-in,
//! or restarts or powers off the computer.

use std::ffi::{CString, OsString};
use std::io::{self, BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use accounts::request::{Answer, Request};
use accounts::wipe;

use crate::account::Input;
use crate::args::{Arg, Args};
use crate::reports::qr;
use crate::util::{self, Fail, fail};
use crate::{authd, prompt, signin};

/// How long login waits for authd to answer at all, as at boot, when
/// the consoles may come up before it.
const AUTHD_WAIT: Duration = Duration::from_secs(30);

/// Connects to authd, waiting for it for up to [`AUTHD_WAIT`].
fn connect() -> Result<UnixStream, Fail> {
    let start = Instant::now();
    let mut told = false;
    loop {
        match UnixStream::connect(accounts::SOCKET) {
            Ok(stream) => return Ok(stream),
            Err(err) if start.elapsed() >= AUTHD_WAIT => {
                return Err(fail(format!(
                    "cannot reach authd at {}: {}",
                    accounts::SOCKET,
                    util::strip_os_error(&err)
                )));
            }
            Err(_) => {
                if !told {
                    eprintln!("Waiting for authd...");
                    told = true;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

/// Asks authd `request`, sending the terminal in descriptor 0 with it if
/// `terminal`; the connection, its request sent, to read the answer from.
fn send(request: &Request, terminal: bool) -> Result<UnixStream, Fail> {
    let stream = connect()?;
    let mut bytes = request.encode();
    let sent = if terminal {
        signin::send_with_fds(&stream, &bytes, &[0])
    } else {
        (&stream).write_all(&bytes)
    };
    wipe(&mut bytes);
    sent.and_then(|()| stream.shutdown(Shutdown::Write))
        .map_err(|err| fail(format!("cannot ask authd: {}", util::strip_os_error(&err))))?;
    Ok(stream)
}

/// Reads authd's first line on `stream`.
fn answer(reader: &mut BufReader<UnixStream>) -> Result<Answer, Fail> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|err| fail(format!("authd did not answer: {}", util::strip_os_error(&err))))?;
    Ok(Answer::decode(&line))
}

/// Asks authd `request` and reads its answer.
fn ask(request: &Request) -> Result<Answer, Fail> {
    answer(&mut BufReader::new(send(request, false)?))
}

/// A line typed at the terminal after `prompt`, shown as typed; none if
/// the input ended or a signal stopped it.
fn line(prompt_text: &str) -> Result<Option<String>, Fail> {
    line_after(prompt_text, None)
}

/// A line typed after `prompt_text`, as [`line`] gives it, that starts
/// with `first`, a key already pressed.
fn line_after(prompt_text: &str, first: Option<char>) -> Result<Option<String>, Fail> {
    let start = first.map_or_default(String::from);
    match prompt::ask(&format!("{prompt_text}{start}"), true) {
        Ok(prompt::Answer::Line(line)) => {
            let rest = String::from_utf8_lossy(&line);
            Ok(Some(format!("{start}{rest}").trim().to_string()))
        }
        Ok(prompt::Answer::Stopped) => Ok(None),
        Err(err) => Err(util::failed(&"the terminal", err)),
    }
}

/// Makes the owner's account at first boot; its name and password, to
/// sign in with, or none if the input ended.
fn make_owner() -> Result<Option<(String, Vec<u8>)>, Fail> {
    println!(
        "Welcome to Dukovo. This computer has no account yet: make yours, the owner's,\n\
         which administers it. Nothing but a name and a password is needed, and the\n\
         password may be empty.\n"
    );
    loop {
        let Some(name) = line("Name: ")? else { return Ok(None) };
        if name.is_empty() {
            continue;
        }
        if let Err(why) = accounts::check_name(&name) {
            println!("{why}\n");
            continue;
        }
        let Some(full_name) = line("Full name (may be empty): ")? else { return Ok(None) };
        if let Err(why) = accounts::check_full_name(&full_name) {
            println!("{why}\n");
            continue;
        }
        let password = match Input::Terminal.new_password(&name) {
            Ok(password) => password,
            Err(why) => {
                println!("{}\n", why.message);
                continue;
            }
        };
        let request = Request::Owner { name: name.clone(), full_name, password: password.clone() };
        match ask(&request)? {
            Answer::Done(text) => {
                println!("{}\n", capitalized(&text));
                return Ok(Some((name, password)));
            }
            Answer::Refused(why) => {
                let mut password = password;
                wipe(&mut password);
                println!("{why}\n");
                // Someone made an account on another console meanwhile.
                if needs_owner()? {
                    continue;
                }
                return Ok(None);
            }
        }
    }
}

/// `text` with a capital first letter.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

/// Whether authd says no account can be signed in to yet.
fn needs_owner() -> Result<bool, Fail> {
    match ask(&Request::NeedsOwner)? {
        Answer::Done(text) => Ok(text == "yes"),
        Answer::Refused(why) => Err(fail(why)),
    }
}

/// Has authd start `name`'s session on this terminal, if `password` is
/// theirs, and waits for it to end: a shell, or `command` if there is one.
/// Whether it started; why not if not.
fn sign_in(
    name: String,
    password: Vec<u8>,
    command: Option<&str>,
) -> Result<Result<(), String>, Fail> {
    let term = std::env::var("TERM").unwrap_or_else(|_| "dumb".into());
    let command = command.map(String::from);
    let stream = send(&Request::Session { name, password, term, command }, true)?;
    let mut reader = BufReader::new(stream);
    match answer(&mut reader)? {
        Answer::Done(_) => {}
        Answer::Refused(why) => return Ok(Err(why)),
    }
    // The terminal is the session's now: login keeps none of it open, so
    // that it hangs up once the session's programs are done with it.
    let null = c"/devices/null";
    // SAFETY: opening /devices/null and putting it in place of the
    // terminal on 0 to 2 touches only our descriptors.
    unsafe {
        let fd = libc::open(null.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC);
        if fd >= 0 {
            for target in 0..3 {
                libc::dup2(fd, target);
            }
            libc::close(fd);
        }
    }
    // Then it waits for authd's word that the session ended, or for authd
    // to go, which ends it as far as this console is concerned.
    let mut line = String::new();
    let _ = reader.read_line(&mut line);
    Ok(Ok(()))
}

/// The administrators' names, as the account files list them, each
/// disabled one marked so: resetting its password enables it too (ADR
/// 0220).
fn administrators() -> Vec<String> {
    let read = |path: &str| std::fs::read_to_string(path).unwrap_or_default();
    let users = accounts::users(&read(accounts::PASSWD));
    let groups = accounts::groups(&read(accounts::GROUP));
    let passwords = accounts::passwords(&read(accounts::PASSWORDS));
    let disabled = |name: &str| passwords.entries().any(|p| p.name == name && p.is_disabled());
    users
        .entries()
        .filter(|u| u.is_person() && accounts::is_admin(u, &groups))
        .map(|u| if disabled(&u.name) { format!("{} (disabled)", u.name) } else { u.name.clone() })
        .collect()
}

/// The startup-trouble menu (ADR 0253): whether to go on to the sign-in;
/// not if the input ended.
fn trouble() -> Result<bool, Fail> {
    loop {
        println!(
            "Startup trouble: this computer was started from its boot menu's\n\
             startup-trouble entry.\n\n  \
             1  Reset an administrator's password\n  \
             2  Sign in\n  \
             3  Restart\n  \
             4  Power off\n"
        );
        let Some(choice) = line("Choose 1 to 4: ")? else { return Ok(false) };
        println!();
        match choice.as_str() {
            "1" => reset()?,
            "2" | "" => return Ok(true),
            "3" => power("reboot"),
            "4" => power("poweroff"),
            _ => println!("Type 1, 2, 3 or 4.\n"),
        }
    }
}

/// Resets an administrator's password, as authd allows only while the
/// computer runs for startup trouble.
fn reset() -> Result<(), Fail> {
    let admins = administrators();
    if admins.is_empty() {
        println!("No administrator's account yet: signing in makes the owner's.\n");
        return Ok(());
    }
    println!("Administrators: {}", admins.join(", "));
    let Some(name) = line("Whose password? ")? else { return Ok(()) };
    if name.is_empty() {
        println!();
        return Ok(());
    }
    let password = match Input::Terminal.new_password(&name) {
        Ok(password) => password,
        Err(why) => {
            println!("{}\n", capitalized(&why.message));
            return Ok(());
        }
    };
    match ask(&Request::Reset { name: name.clone(), password })? {
        Answer::Done(_) => println!(
            "The password of {name} is reset. Every administrator is told at their\n\
             next sign-in, and the audit log keeps it.\n"
        ),
        Answer::Refused(why) => println!("{}.\n", capitalized(&why)),
    }
    Ok(())
}

/// Restarts or powers off the computer with `command`, reboot(8) or
/// poweroff(8), which ask Spark; the menu comes back only if it refused.
fn power(command: &str) {
    let path = format!("/system/commands/{command}");
    match std::process::Command::new(&path).status() {
        // Spark ends the sign-in with the rest.
        Ok(status) if status.success() => loop {
            std::thread::sleep(Duration::from_secs(60));
        },
        Ok(_) => println!(),
        Err(err) => println!("{path}: {}\n", util::strip_os_error(&err)),
    }
}

/// The command the failure notice's `R` signs in with: the report of the
/// failed start, which only an administrator may make.
const REPORT_COMMAND: &str = "report failure";

/// The start of the support pages' addresses, as the failure notice
/// gives one.
const SUPPORT: &str = "https://dukovo.org/support/";

/// What the failure notice offers.
#[derive(Debug, PartialEq, Eq)]
struct Failure {
    /// The address of the notice's support page, if it names one.
    page: Option<String>,
}

impl Failure {
    /// The failure notice of this start, if there is one.
    fn read() -> Option<Failure> {
        let notice = std::fs::read_to_string(crate::sysupdate::health::NOTICE).ok()?;
        Some(Failure { page: support_page(&notice) })
    }
}

/// The address of the support page in the failure notice `notice`: its
/// first word that starts as a support address does.
fn support_page(notice: &str) -> Option<String> {
    notice.split_whitespace().find(|word| word.starts_with(SUPPORT)).map(String::from)
}

/// What a key pressed at the failure notice's choices does.
#[derive(Debug, PartialEq, Eq)]
enum Choice {
    /// `Q`: show the support page's QR code.
    Support,
    /// `R`: sign in as an administrator to report the failure.
    Report,
    /// Enter: sign in as usual.
    SignIn,
    /// Another printable key: sign in as usual, with it the name's first
    /// letter.
    Name(char),
    /// Nothing: not a key a name starts with.
    Ignore,
}

/// What `key` does when the notice has a support page, `page`, or not.
/// A name may start with any visible ASCII character; `Q` starts one when
/// there is no page to show.
fn choose(key: u8, page: bool) -> Choice {
    match key {
        b'\n' | b'\r' => Choice::SignIn,
        b'r' | b'R' => Choice::Report,
        b'q' | b'Q' if page => Choice::Support,
        0x21..=0x7e => Choice::Name(char::from(key)),
        _ => Choice::Ignore,
    }
}

/// The line that says which keys do what.
fn choices_line(page: bool) -> &'static str {
    if page {
        "Press R to report this failure, Q for its support page as a QR code, or Enter to sign in."
    } else {
        "Press R to report this failure, or Enter to sign in."
    }
}

/// The lines that show the support page at `address` on a terminal of
/// `columns` and `rows`: its QR code, when it fits with the lines under
/// it, and the address.
fn page_lines(address: &str, columns: usize, rows: usize) -> Vec<String> {
    // Under the code: a blank line, the address, a blank line and the
    // line that says how to go back.
    let code =
        qr::encode(address).and_then(|code| qr::lines(&code, columns, rows.saturating_sub(4)));
    let mut lines = code.unwrap_or_default();
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push(address.to_string());
    lines
}

/// Shows the support page's QR code and address, and waits for a key.
fn show_page(address: &str) {
    let (columns, rows) = util::terminal_size(1).unwrap_or((80, 24));
    println!();
    for line in page_lines(address, columns, rows) {
        println!("{line}");
    }
    println!();
    // Any key, or none: the input ending or Ctrl+C goes back as well.
    let _ = prompt::key("Press a key to go back.");
    println!();
}

/// Where the failure notice's choices lead.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// The usual sign-in, with the name's first letter if a key began it.
    SignIn(Option<char>),
    /// An administrator's sign-in that reports the failure.
    Report,
}

/// Shows the failure notice's choices and reads keys until one leads
/// somewhere; none if the input ended.
fn offer(failure: &Failure) -> Result<Option<Next>, Fail> {
    println!("{}", choices_line(failure.page.is_some()));
    loop {
        let key = match prompt::key("") {
            Ok(prompt::Pressed::Key(key)) => key,
            Ok(prompt::Pressed::Stopped) => return Ok(None),
            Err(err) => return Err(util::failed(&"the terminal", err)),
        };
        match (choose(key, failure.page.is_some()), &failure.page) {
            (Choice::Support, Some(address)) => {
                show_page(address);
                println!("{}", choices_line(true));
            }
            (Choice::Report, _) => return Ok(Some(Next::Report)),
            (Choice::SignIn, _) => return Ok(Some(Next::SignIn(None))),
            (Choice::Name(letter), _) => return Ok(Some(Next::SignIn(Some(letter)))),
            _ => {}
        }
    }
}

/// The sign-in at a console: name and password until a session starts
/// and ends.
fn console() -> Result<i32, Fail> {
    if util::own_user() != 0 {
        return Err(fail("only root signs people in: consoled(8) runs login".into()));
    }
    if util::kernel_flag(authd::STARTUP_TROUBLE) && !trouble()? {
        return Ok(1);
    }
    let failure = Failure::read();
    // Whether the failure notice's choices come next: first, and again
    // after a report that did not start.
    let mut offered = failure.is_some();
    loop {
        if needs_owner()? {
            match make_owner()? {
                Some((name, password)) => match sign_in(name, password, None)? {
                    Ok(()) => return Ok(0),
                    Err(why) => println!("{why}\n"),
                },
                None => return Ok(1),
            }
            continue;
        }
        let (mut first, mut command) = (None, None);
        if let Some(failure) = failure.as_ref().filter(|_| offered) {
            match offer(failure)? {
                None => return Ok(1),
                Some(Next::SignIn(letter)) => first = letter,
                Some(Next::Report) => {
                    println!("Sign in as an administrator to report it.\n");
                    command = Some(REPORT_COMMAND);
                }
            }
        }
        let Some(name) = line_after("Name: ", first)? else { return Ok(1) };
        if name.is_empty() {
            offered = command.is_some();
            continue;
        }
        let Ok(password) = Input::Terminal.password("Password: ") else { return Ok(1) };
        match sign_in(name, password, command)? {
            Ok(()) => return Ok(0),
            Err(why) => {
                println!("{}.\n", capitalized(&why));
                offered = command.is_some();
            }
        }
    }
}

/// The first program of a session, as its user: the terminal, if it
/// has one, becomes the session's, then the shell runs as a login shell
/// in the home, or runs `command` if there is one.
fn session(command: Option<OsString>) -> Result<i32, Fail> {
    // SAFETY: setsid and the terminal request change only this process's
    // session; authd made it in its own process group, so setsid works.
    unsafe {
        if libc::setsid() < 0 {
            return Err(util::failed(&"setsid", io::Error::last_os_error()));
        }
        if libc::isatty(0) == 1 && libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
            return Err(util::failed(&"the terminal", io::Error::last_os_error()));
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    if std::env::set_current_dir(&home).is_err() {
        println!("No home {home}: starting in /.");
        let _ = std::env::set_current_dir("/");
        // SAFETY: one thread; nothing else reads the environment now.
        unsafe { std::env::set_var("HOME", "/") };
    }
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty());
    let shell = shell.unwrap_or_else(|| accounts::SHELL.into());
    let base = shell.rsplit('/').next().unwrap_or("sh");
    let nul = || fail("a shell path or command with a NUL".into());
    let path = CString::new(shell.clone()).map_err(|_| nul())?;
    let mut args = Vec::new();
    match command {
        // A command runs with the shell's -c, the shell named as it is,
        // so that it reads no login files, as with Linux's sshd.
        Some(command) => {
            args.push(CString::new(base).map_err(|_| nul())?);
            args.push(c"-c".to_owned());
            args.push(CString::new(command.into_vec()).map_err(|_| nul())?);
        }
        // A login shell's name starts with a dash (sh(1)).
        None => args.push(CString::new(format!("-{base}")).map_err(|_| nul())?),
    }
    let mut argv: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
    argv.push(std::ptr::null());
    // SAFETY: execv takes NUL-terminated strings and a null-ended list,
    // which live until it returns, if it does.
    unsafe { libc::execv(path.as_ptr(), argv.as_ptr()) };
    Err(util::failed(&shell, io::Error::last_os_error()))
}

/// login(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut in_session = false;
    let mut command = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "session" => in_session = true,
            Arg::Long(name) if name == "command" => command = Some(args.value()?),
            other => return Err(args.unknown(&other)),
        }
    }
    if command.is_some() && !in_session {
        return Err(fail("--command runs only in a session (--session)".into()));
    }
    let _ = io::stdout().flush();
    if in_session { session(command) } else { console() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_signs_in_and_r_reports() {
        for page in [true, false] {
            assert_eq!(choose(b'\n', page), Choice::SignIn);
            assert_eq!(choose(b'\r', page), Choice::SignIn);
            assert_eq!(choose(b'r', page), Choice::Report);
            assert_eq!(choose(b'R', page), Choice::Report);
        }
    }

    #[test]
    fn q_shows_the_page_only_when_the_notice_has_one() {
        assert_eq!(choose(b'q', true), Choice::Support);
        assert_eq!(choose(b'Q', true), Choice::Support);
        // Without a page, Q is a name's first letter, as for quincy.
        assert_eq!(choose(b'q', false), Choice::Name('q'));
        assert_eq!(choose(b'Q', false), Choice::Name('Q'));
    }

    #[test]
    fn any_other_visible_key_starts_the_name() {
        for &key in b"aZ0_.~!" {
            assert_eq!(choose(key, true), Choice::Name(char::from(key)));
        }
    }

    #[test]
    fn keys_that_no_name_starts_with_are_ignored() {
        // A space, Escape and the arrow keys' first byte, Backspace, Tab,
        // Ctrl+D, a delete and a byte of a longer character.
        for key in [b' ', 0x1b, 0x08, b'\t', 0x04, 0x7f, 0xc3, 0x00] {
            assert_eq!(choose(key, true), Choice::Ignore, "{key:#x}");
        }
    }

    #[test]
    fn the_choices_name_the_keys_that_work() {
        assert_eq!(
            choices_line(true),
            "Press R to report this failure, Q for its support page as a QR code, or Enter to sign in."
        );
        assert_eq!(choices_line(false), "Press R to report this failure, or Enter to sign in.");
    }

    const NOTICE: &str = "This start of Dukovo 0.16.0 failed its health check, and goes on.\n\
        The service web did not become healthy.\n\
        More: changes --failed, and log -b -1 for the failed start's log.\n\
        Help: https://dukovo.org/support/0.16.0/errors/NET-132.html\n";

    #[test]
    fn the_support_page_is_the_address_in_the_notice() {
        assert_eq!(
            support_page(NOTICE).as_deref(),
            Some("https://dukovo.org/support/0.16.0/errors/NET-132.html")
        );
        assert_eq!(support_page("The start failed.\nMore: changes --failed\n"), None);
        assert_eq!(support_page("See https://example.org/support/x"), None);
        assert_eq!(support_page(""), None);
    }

    #[test]
    fn the_page_is_a_code_over_its_address_where_there_is_room() {
        let address = "https://dukovo.org/support/0.16.0/errors/NET-132.html";
        let lines = page_lines(address, 80, 25);
        assert_eq!(lines.last().map(String::as_str), Some(address));
        let code = &lines[..lines.len() - 2];
        // A code of 33 modules with a quiet zone of four: 41 columns, and
        // 21 lines of two modules.
        assert_eq!((code.len(), code[0].chars().count()), (21, 41));
        assert_eq!(lines[lines.len() - 2], "");
        assert!(code.iter().all(|line| line.chars().all(|c| " ▀▄█".contains(c))));
    }

    #[test]
    fn a_narrow_or_low_terminal_gets_the_address_only() {
        let address = "https://dukovo.org/support/0.16.0/errors/NET-132.html";
        assert_eq!(page_lines(address, 36, 40), [address]);
        // Rows enough for the code with its narrow quiet zone, 19, and the
        // four lines under it: 23.
        assert_eq!(page_lines(address, 80, 22), [address]);
        let narrow_quiet = page_lines(address, 80, 23);
        assert_eq!((narrow_quiet.len(), narrow_quiet[0].chars().count()), (19 + 2, 37));
    }
}
