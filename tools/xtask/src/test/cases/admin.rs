// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The web console, Dukovo Admin, answers over HTTPS (ADR 0514). The
//! machine boots as it does for a person, with QEMU's user network, and the
//! host's curl reaches the console's port 9090 through a port QEMU
//! forwards. curl trusts the certificate the machine served at first and
//! no other, and the guest's _cert show this-machine_ and
//! _/system/live/admind/issue_ both name that certificate's fingerprint.
//! consoled shows the console's address and that fingerprint on its
//! sign-in screen, which alice sees by signing out. Health answers with the
//! headers every answer has and no HSTS, which a certificate that is no
//! authority's must not ask for; another name in Host is ADM-114; TLS 1.1
//! is refused and 1.2 served. A silent connection is cut off in about
//! ten seconds, a request head of 20 KiB is 431, 32 connections kept
//! alive from one address leave a 33rd refused and said in the log, and
//! 600 connections at once leave admind running and health answered
//! after.
//!
//! Alice signs in: the cookie is __Host-dukovo with Secure, HttpOnly and
//! SameSite=Strict and nothing else, and the session names her. A wrong
//! password and a name no one has are ADM-101 alike, and bob, whom alice
//! makes through elevate, is ADM-104, which authd's log says too, since he
//! is neither an administrator nor in group console. Changes with no
//! Origin, another origin, another site or no token are ADM-109, a sign-in
//! among them, and change nothing: an unlocked session stays unlocked
//! until alice locks it. Alice adds an authenticator app: the host
//! computes its codes from the otpauth URI (RFC 6238) and the app's
//! recovery codes come back. Signed in again, the console asks the second
//! step; a wrong code is ADM-102, the right one signs in, the same code
//! again is refused as used, and a recovery code signs in once and not
//! twice. Carol, in group console and no administrator, signs in and has
//! nothing to unlock: authd refuses it, ADM-110. A locked session cannot
//! change second steps without the password; unlocked with the app's code
//! it can, shows the seconds left and locks again. The route that lab builds add, _POST
//! /api/1/test/elevate_, is not in this image, which is 404: no case can
//! ask for the lab build, so what an unlocked session runs as an
//! administrator is the lab test web-console/unlock's. Stopping and
//! starting admind ends the sessions, so the old cookie is ADM-107; the
//! console serves the same certificate and signs people in again, and
//! consoled draws tty2's sign-in screen, which nobody has typed on, again
//! when admind removes its file and when it writes it. The power button
//! ends it.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::*;

/// The owner's password, which alice gives at every sign-in.
const ALICE_PASSWORD: &str = "queen of hearts";

/// The password of bob and carol, whom alice makes: bob may not use the
/// console, carol may look at it.
const BOB_PASSWORD: &str = "looking-glass";

/// The console's port in the guest.
const PORT: u16 = 9090;

/// The console as a browser opens it: by `localhost`, a name that the
/// certificate holds and the console takes for its own, on the guest's
/// port, whatever port the host forwards (the console answers a Host
/// header that names another port with ADM-114).
const ORIGIN: &str = "https://localhost:9090";

/// The session's cookie.
const SESSION_COOKIE: &str = "__Host-dukovo";

/// The header that carries the session's token.
const CSRF_HEADER: &str = "X-Dukovo-CSRF";

/// How many connections at once the test opens: more than the console
/// holds from one address.
const CROWD: usize = 600;

/// The connections the console holds from one address at once, as
/// admind's limits say.
const PLACES: usize = 32;

/// The test.
pub const CASE: Case = Case {
    name: "admin",
    about: "The web console, Dukovo Admin, answers the host's curl over HTTPS through a port QEMU forwards, with the machine's own certificate, whose fingerprint consoled shows at sign-in: its headers, another name refused, TLS 1.1 refused; sign-in with the cookie's attributes, a wrong password, an account that may not use the console; changes refused from another origin, another site or without the token; an authenticator app added from its otpauth URI, its code asked at sign-in, a wrong code, a replayed code, a recovery code used once; unlock and lock; 32 connections from one address held and a 33rd refused, a silent connection cut off, a 20 KiB head refused, 600 connections at once survived; a restart that ends the sessions (ADR 0514)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, admind among them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: admin,
    disks: &[],
    second: None,
};

fn admin(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    // netd gives the card its address once QEMU's DHCP answers; admind,
    // which waits for authd and certd, says where it answers once it
    // listens.
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    vm.expect_anywhere("admind: answers at ")?;
    let console = Console::open(vm)?;
    limits(vm, &console)?;

    // alice's password once for this pane, then bob, who is neither an
    // administrator nor in group console, and carol, who is in it.
    elevated(vm)?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate sh -c 'echo {BOB_PASSWORD} | user add bob; echo {BOB_PASSWORD} | user \
             add carol; user set --yes --console carol'"
        ),
    )?;
    // Each account's hash takes a few seconds without KVM: the log says
    // when the last change is done, and the screen follows.
    vm.expect("authd: changed the account carol")?;
    vm.expect_screen(&["Made bob", "Made carol"])?;

    let address = issue_file(vm, &console)?;
    sign_in_screen(vm, &address, &console.fingerprint)?;
    machine(vm, &console)?;
    let mut alice = Browser::default();
    signing_in(vm, &console, &mut alice)?;
    foreign_requests(&console, &mut alice)?;
    let mut app = second_step(&console, &mut alice)?;
    unlocking(&console, &mut alice, &mut app)?;
    restart(vm, &console, &mut alice, &address)?;
    crowd(vm, &console)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    // Once for the restart and once for the power button.
    if vm.log().matches("admind: stops: it ends ").count() < 2 {
        return Err("admind did not say it stopped at the restart and at the power button".into());
    }
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Alice's password, once, for this pane: elevate asks on the console's
/// bottom line, and the commands after it within 15 minutes do not.
fn elevated(vm: &mut Vm) -> Result<(), String> {
    // `-k` locks the pane first, so that the password is asked even where
    // an earlier sign-in of alice's left it unlocked.
    answer_prompt(vm, "$", "clear; elevate -k; elevate true; echo \"elevated $?\"")?;
    answer_prompt(vm, "to run /system/commands/true:", ALICE_PASSWORD)?;
    vm.expect_screen(&["elevated 0"])
}

/// The console's limits on one address. 32 connections, each answered once
/// and kept alive, hold all the places the console gives an address, so a
/// 33rd is refused at once, which its log says: this comes first, so that
/// no refusal from this address has been said in the last minute (the log
/// says the first, and then once a minute, admind(8)). A request head of
/// 20 KiB is 431.
///
/// The limit is shown with connections that stay, not with a crowd, as a
/// crowd reaches the guest only as fast as QEMU's network hands it on:
/// without KVM, slower than the ten seconds each silent one is held.
fn limits(vm: &mut Vm, console: &Console) -> Result<(), String> {
    let held = hold_every_place(console)?;
    let mut extra = TcpStream::connect(("127.0.0.1", console.port))
        .map_err(|err| format!("the 33rd connection to the console: {err}"))?;
    extra
        .set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|err| format!("the 33rd connection's timeout: {err}"))?;
    // The console closes it before a byte is sent: QEMU then ends the
    // host's side, with an end or a reset.
    match extra.read(&mut [0u8; 1]) {
        Ok(0) => {}
        Err(err) if err.kind() == std::io::ErrorKind::ConnectionReset => {}
        Ok(_) => return Err("the console sent the 33rd connection a byte".into()),
        Err(err) => return Err(format!("the 33rd connection was not closed in 60 s: {err}")),
    }
    vm.expect_anywhere(
        "admind: refused a connection from 10.0.2.2: there are as many connections as it takes",
    )?;
    drop(held);

    // The console answers a head of 20 KiB 431 and hangs up on the rest of
    // it. curl may be told so before it has sent all, or find the end of
    // the connection where it looks for more, and then says it failed
    // though the answer came: what it printed counts, and it is asked
    // again when it printed nothing.
    let head = [("X-Padding".to_string(), "a".repeat(20 << 10))];
    let mut last = String::new();
    for _ in 0..3 {
        let out = console.run("GET", "/api/1/health", &head, None)?;
        match parse_reply(&out.stdout) {
            Ok(reply) if reply.status == 431 => return Ok(()),
            Ok(reply) => return Err(format!("a head of 20 KiB was answered {}", reply.status)),
            Err(_) => last = String::from_utf8_lossy(&out.stderr).trim().to_string(),
        }
    }
    Err(format!("a head of 20 KiB got no answer: {last}"))
}

/// What a crowd does to the console: one connection that sends nothing is
/// cut off after about ten seconds, while 600 are made at once; held for a
/// few seconds and let go, they leave it answering. This comes last: QEMU
/// hands the guest the crowd's connections for minutes after they are let
/// go when there is no KVM, and the steps before would wait on them.
fn crowd(vm: &mut Vm, console: &Console) -> Result<(), String> {
    let silent = silent_connection(console.port)?;
    let crowd = (0..CROWD)
        .map(|n| {
            let address = SocketAddr::from(([127, 0, 0, 1], console.port));
            TcpStream::connect_timeout(&address, Duration::from_secs(20))
                .map_err(|err| format!("connection {n} of {CROWD} to the console: {err}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    std::thread::sleep(Duration::from_secs(5));
    drop(crowd);
    // The places the crowd held are given back as the guest sees them
    // close.
    let mut browser = Browser::default();
    let deadline = Instant::now() + Duration::from_secs(60);
    let health = loop {
        match console.get(&mut browser, "/api/1/health") {
            Ok(reply) if reply.status == 200 => break reply,
            Ok(reply) => {
                return Err(format!("health after the crowd: {} {}", reply.status, reply.body));
            }
            Err(why) if Instant::now() >= deadline => {
                return Err(format!("health after the crowd: {why}"));
            }
            Err(_) => std::thread::sleep(Duration::from_secs(1)),
        }
    };
    if health.json()? != json!({"ok": true}) {
        return Err(format!("health after the crowd said {}", health.body));
    }
    if vm.log().contains("starting admind again") {
        return Err("Spark started admind again: the crowd ended it".into());
    }

    let cut = silent
        .join()
        .map_err(|_| "the thread of the silent connection ended badly".to_string())??;
    if !(Duration::from_secs(9)..=Duration::from_secs(30)).contains(&cut) {
        return Err(format!("a connection that sent nothing was cut off after {cut:?}, not 10 s"));
    }
    Ok(())
}

/// The issue file and the certificate agree: the fingerprint the host took
/// from the handshake is in both _/system/live/admind/issue_ and *cert
/// show this-machine*. Returns the address the file gives.
fn issue_file(vm: &mut Vm, console: &Console) -> Result<String, String> {
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; F={}; echo \"issue $(grep -c $F /system/live/admind/issue) cert $(cert show \
             this-machine | grep -c $F) address $(head -n 1 /system/live/admind/issue)\"",
            console.fingerprint
        ),
    )?;
    vm.expect_screen(&["issue 1 cert 1 address https://"])?;
    let screen = vm.screen()?;
    let address = screen
        .iter()
        .rev()
        .find_map(|line| line.split_once("issue 1 cert 1 address ").map(|(_, rest)| rest.trim()))
        .ok_or("the screen does not show the issue file's address")?;
    if !address.ends_with(&format!(":{PORT}")) {
        return Err(format!("the issue file's address is {address}, not on port {PORT}"));
    }
    Ok(address.to_string())
}

/// consoled's sign-in screen shows where the console is and its
/// certificate's fingerprint, in whole bytes on each line, all of them in
/// order. The screen tty1 showed first was drawn before admind wrote the
/// file, and alice has typed on it since, so consoled leaves it as it is
/// (a screen nobody has typed on is drawn again): she signs out to see the
/// next, and signs in again.
fn sign_in_screen(vm: &mut Vm, address: &str, fingerprint: &str) -> Result<(), String> {
    answer_prompt(vm, "$", "exit")?;
    vm.expect("authd: alice signed out on pts/")?;
    vm.expect_screen(&["Web console:", address, "Certificate SHA-256 fingerprint:", "Name:"])?;
    let shown = fingerprint_on_screen(&vm.screen()?)
        .ok_or("the sign-in screen does not show the fingerprint's 32 bytes under its heading")?;
    if shown != fingerprint {
        return Err(format!(
            "the sign-in screen shows {shown}, not the certificate's {fingerprint}"
        ));
    }
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Password:", ALICE_PASSWORD)?;
    vm.expect("authd: alice signed in on pts/")
}

/// What every answer carries, the name a request may use, and the
/// versions of TLS.
fn machine(vm: &mut Vm, console: &Console) -> Result<(), String> {
    let health = console.send("GET", "/api/1/health", &[], None)?.must("health", 200)?;
    if health.json()? != json!({"ok": true}) {
        return Err(format!("health said {}", health.body));
    }
    let policy = health.header("content-security-policy").unwrap_or_default();
    if !policy.starts_with("default-src 'self'")
        || !policy.contains("frame-ancestors 'none'")
        || policy.contains("unsafe")
    {
        return Err(format!("the content security policy is {policy:?}"));
    }
    for (name, want) in [
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("cross-origin-opener-policy", "same-origin"),
        ("cache-control", "no-store"),
        ("content-type", "application/json"),
    ] {
        if health.header(name) != Some(want) {
            return Err(format!("{name} is {:?}, not {want}", health.header(name)));
        }
    }
    // It names no software, and asks for no HSTS: that would keep a
    // browser from passing the warning of a certificate that is no
    // authority's.
    for name in ["server", "strict-transport-security"] {
        if let Some(value) = health.header(name) {
            return Err(format!("health carries {name}: {value}"));
        }
    }
    let named = console.send("GET", "/api/1/fingerprint", &[], None)?.must("fingerprint", 200)?;
    if named.json()?["fingerprint"].as_str() != Some(console.fingerprint.as_str()) {
        return Err(format!("the console gives {}, not the certificate's fingerprint", named.body));
    }
    // A name that is not this machine's, and a port no browser can have
    // opened, are the console's refusal; the log says the first. The
    // machine's name on another port is answered, as a port forward or an
    // SSH tunnel sends it.
    for host in ["evil.example:9090", "localhost:0"] {
        console.send("GET", "/api/1/health", &[("Host".into(), host.into())], None)?.refused(
            &format!("a request for {host}"),
            421,
            "ADM-114",
        )?;
    }
    console
        .send("GET", "/api/1/health", &[("Host".into(), "localhost:8443".into())], None)?
        .must("health through a forwarded port", 200)?;
    vm.expect_anywhere("admind: refused a request from 10.0.2.2: ADM-114")?;
    console.tls_refused()?;
    console.tls_served("1.2")?;
    console.tls_served("1.3")
}

/// Signing in as alice, as a wrong password, a name no one has and bob.
fn signing_in(vm: &mut Vm, console: &Console, alice: &mut Browser) -> Result<(), String> {
    let mut stranger = Browser::default();
    for (name, password) in [("alice", "cheshire cat"), ("nobody", ALICE_PASSWORD)] {
        let body = json!({"name": name, "password": password});
        let reply = console.post(&mut stranger, "/api/1/sign-in", &body)?.refused(
            &format!("a sign-in of {name}"),
            401,
            "ADM-101",
        )?;
        if reply.header("set-cookie").is_some() {
            return Err(format!("a refused sign-in of {name} set a cookie"));
        }
    }
    let body = json!({"name": "bob", "password": BOB_PASSWORD});
    let refused = console.post(&mut stranger, "/api/1/sign-in", &body)?.refused(
        "bob's sign-in",
        403,
        "ADM-104",
    )?;
    if !refused.detail().contains("may not use the web console") {
        return Err(format!("bob was refused for {:?}", refused.detail()));
    }
    vm.expect_anywhere("authd: refused bob the web console from 10.0.2.2")?;

    // carol, in group console and no administrator, signs in and looks,
    // but has nothing to unlock: authd refuses it, since what an unlocked
    // session is for is what only an administrator may run.
    let mut carol = Browser::default();
    console.sign_in(&mut carol, "carol", BOB_PASSWORD)?;
    let session = console.read_session(&mut carol)?;
    if session["name"] != json!("carol") || session["admin"] != json!(false) {
        return Err(format!("carol's session is {session}"));
    }
    let unlock = json!({"kind": "password", "password": BOB_PASSWORD});
    let refused = console.post(&mut carol, "/api/1/unlock", &unlock)?.refused(
        "carol's unlock",
        403,
        "ADM-110",
    )?;
    if !refused.detail().contains("is not an administrator") {
        return Err(format!("carol's unlock was refused for {:?}", refused.detail()));
    }
    console.post(&mut carol, "/api/1/sign-out", &json!({}))?.must("carol's sign-out", 200)?;

    let signed = console.sign_in(alice, "alice", ALICE_PASSWORD)?;
    if signed.body != r#"{"done":true}"# {
        return Err(format!("alice's sign-in said {}", signed.body));
    }
    let cookies: Vec<&str> = signed.headers_named("set-cookie").collect();
    let [text] = cookies[..] else {
        return Err(format!("alice's sign-in set {} cookies, not one", cookies.len()));
    };
    let cookie = parse_cookie(text).ok_or_else(|| format!("the cookie {text:?} does not read"))?;
    // 32 random bytes in URL-safe base64, which only the console's own
    // origin gets back, never to a script, never from another site.
    let value_is_random = cookie.value.len() == 43
        && cookie.value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if cookie.name != SESSION_COOKIE
        || !value_is_random
        || cookie.attributes != ["Path=/", "Secure", "HttpOnly", "SameSite=Strict"]
    {
        return Err(format!("the session's cookie is {text:?}"));
    }
    // The session also gives the token that her page sends back with each
    // change.
    let session = console.read_session(alice)?;
    for (field, want) in [
        ("name", json!("alice")),
        ("full_name", json!("Alice Liddell")),
        ("admin", json!(true)),
        ("needs_step", json!(false)),
        ("unlock_seconds", Value::Null),
        ("kinds", json!(["password"])),
    ] {
        if session[field] != want {
            return Err(format!("alice's session has {field}: {}, not {want}", session[field]));
        }
    }
    if alice.token.as_ref().is_none_or(|token| token.len() != 43) {
        return Err(format!("the session's token is {:?}, not 32 bytes in base64", alice.token));
    }
    // Without the cookie, or with another, there is no session.
    console.get(&mut Browser::default(), "/api/1/session")?.refused(
        "no session",
        401,
        "ADM-107",
    )?;
    Ok(())
}

/// What a request from another page cannot do. The session is unlocked
/// with the password, and `/api/1/lock` is asked as another site would
/// ask it: with no Origin, with another one, from another site, with no
/// token and with a token that is not hers. Each is ADM-109, and she is
/// unlocked still. The sign-in, which has no token, takes no request from
/// another origin or site either. Then she locks it herself.
fn foreign_requests(console: &Console, alice: &mut Browser) -> Result<(), String> {
    let unlock = json!({"kind": "password", "password": ALICE_PASSWORD});
    let unlocked = console.post(alice, "/api/1/unlock", &unlock)?.must("unlock", 200)?.json()?;
    let seconds = unlocked["unlock_seconds"].as_u64().unwrap_or_default();
    if !(800..=900).contains(&seconds) {
        return Err(format!("the unlock lasts {seconds} s, not 15 minutes"));
    }

    let page = alice.page_headers();
    let without = |name: &str| -> Vec<(String, String)> {
        page.iter().filter(|(have, _)| !have.eq_ignore_ascii_case(name)).cloned().collect()
    };
    let with = |name: &str, value: &str| -> Vec<(String, String)> {
        let mut headers = without(name);
        headers.push((name.into(), value.into()));
        headers
    };
    let forged = [
        ("no Origin", without("Origin")),
        ("another origin", with("Origin", "https://evil.example")),
        ("the same name over http", with("Origin", "http://localhost:9090")),
        ("the same name on another port", with("Origin", "https://localhost:8443")),
        ("another site", with("Sec-Fetch-Site", "cross-site")),
        ("a sibling site", with("Sec-Fetch-Site", "same-site")),
        ("a navigation", with("Sec-Fetch-Site", "none")),
        ("no token", without(CSRF_HEADER)),
        ("a token that is not hers", with(CSRF_HEADER, &"A".repeat(43))),
    ];
    for (how, headers) in forged {
        console.send("POST", "/api/1/lock", &headers, Some("{}"))?.refused(
            &format!("a lock with {how}"),
            403,
            "ADM-109",
        )?;
    }
    let still = console.get(alice, "/api/1/session")?.must("her session", 200)?.json()?;
    if still["unlock_seconds"].as_u64().is_none_or(|seconds| seconds == 0) {
        return Err(format!("a refused lock changed her session: {}", still["unlock_seconds"]));
    }

    // Signing in from another page: the right name and password, and no
    // session, no cookie in the answer.
    let sign_in = json!({"name": "alice", "password": ALICE_PASSWORD}).to_string();
    let json_only = [("Content-Type".to_string(), "application/json".to_string())];
    let elsewhere = ("Origin".to_string(), "https://evil.example".to_string());
    let cross_site = ("Sec-Fetch-Site".to_string(), "cross-site".to_string());
    let own_origin = ("Origin".to_string(), ORIGIN.to_string());
    for (how, headers) in [
        ("no Origin", json_only.to_vec()),
        ("another origin", vec![json_only[0].clone(), elsewhere]),
        ("another site", vec![json_only[0].clone(), own_origin, cross_site]),
    ] {
        let reply = console.send("POST", "/api/1/sign-in", &headers, Some(&sign_in))?.refused(
            &format!("a sign-in with {how}"),
            403,
            "ADM-109",
        )?;
        if reply.header("set-cookie").is_some() {
            return Err(format!("a sign-in with {how} was refused and set a cookie"));
        }
    }

    let locked = console.post(alice, "/api/1/lock", &json!({}))?.must("lock", 200)?.json()?;
    if locked != json!({"unlock_seconds": null}) {
        return Err(format!("lock said {locked}"));
    }
    let session = console.get(alice, "/api/1/session")?.must("her session", 200)?.json()?;
    if !session["unlock_seconds"].is_null() {
        return Err(format!("her session is unlocked after lock: {session}"));
    }
    Ok(())
}

/// What the host keeps of the authenticator app it made up: its secret, and
/// the last step of its codes that alice used, which authd refuses to
/// take again.
struct App {
    secret: Vec<u8>,
    last: Option<u64>,
}

/// An authenticator app and recovery codes: alice begins one with her
/// password and confirms it with its code, and the console asks for the
/// step at the next sign-in. A wrong code is refused. The right one signs
/// in; the same code again is refused as used, and a recovery code signs
/// in once. The step is asked of each sign-in, and each answer is the
/// same whichever kind was wrong.
fn second_step(console: &Console, alice: &mut Browser) -> Result<App, String> {
    // Locked as she is, her password is the proof.
    let begin = json!({"proof": ALICE_PASSWORD});
    let begun = console.post(alice, "/api/1/account/second-step/app/begin", &begin)?;
    let uri = begun.must("beginning an app", 200)?.json()?["uri"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    for part in ["otpauth://totp/", ":alice?secret=", "algorithm=SHA1", "digits=6", "period=30"] {
        if !uri.contains(part) {
            return Err(format!("the app's URI {uri:?} lacks {part}"));
        }
    }
    let mut app = App { secret: secret_of(&uri)?, last: None };

    // A code of the app's is confirmed with the guest's own clock, which
    // is read from the console's answers: the machine's time need not be
    // the host's.
    let wrong = wrong_code(&app.secret, console.guest_unix()?);
    console
        .post(alice, "/api/1/account/second-step/app/confirm", &json!({"code": wrong}))?
        .refused("confirming a wrong code", 401, "ADM-102")?;
    let code = console.next_code(&mut app)?;
    let confirmed = console
        .post(alice, "/api/1/account/second-step/app/confirm", &json!({"code": code}))?
        .must("confirming the app", 200)?
        .json()?;
    let recovery: Vec<&str> =
        confirmed["codes"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    if recovery.len() != 10 || !recovery.iter().all(|code| is_recovery_code(code)) {
        return Err(format!("the app's recovery codes are {confirmed}"));
    }
    let (first, second) = (recovery[0].to_string(), recovery[1].to_string());

    // Signed out, the old cookie is no session.
    let old = alice.cookie.clone().unwrap_or_default();
    console.post(alice, "/api/1/sign-out", &json!({}))?.must("sign-out", 200)?;
    if alice.cookie.is_some() {
        return Err("the answer to sign-out did not clear the cookie".into());
    }
    let remembered = [("Cookie".to_string(), format!("{SESSION_COOKIE}={old}"))];
    console.send("GET", "/api/1/session", &remembered, None)?.refused(
        "the session that signed out",
        401,
        "ADM-107",
    )?;

    // Signed in again, the password is not enough: the console asks for
    // the app's code or a recovery code, and sets no cookie yet.
    let asked = console.sign_in(alice, "alice", ALICE_PASSWORD)?;
    let (ticket, kinds) = step_asked(&asked)?;
    if !kinds.contains(&"app".to_string()) || !kinds.contains(&"recovery".to_string()) {
        return Err(format!("the second step can be {kinds:?}, not an app and a recovery code"));
    }
    if alice.cookie.is_some() {
        return Err("a sign-in that waits for its second step set a cookie".into());
    }
    let wrong = wrong_code(&app.secret, console.guest_unix()?);
    let code_of = |ticket: &str, code: &str| json!({"ticket": ticket, "code": code});
    let refused = console.post(alice, "/api/1/sign-in/code", &code_of(&ticket, &wrong))?.refused(
        "a wrong code",
        401,
        "ADM-102",
    )?;
    if !refused.detail().contains("not right") {
        return Err(format!("a wrong code was refused for {:?}", refused.detail()));
    }
    let code = console.next_code(&mut app)?;
    let signed = console
        .post(alice, "/api/1/sign-in/code", &code_of(&ticket, &code))?
        .must("the right code", 200)?;
    if signed.body != r#"{"done":true}"# || alice.cookie.is_none() {
        return Err(format!(
            "the right code signed in with {} and cookie {:?}",
            signed.body, alice.cookie
        ));
    }
    console.read_session(alice)?;
    console.post(alice, "/api/1/sign-out", &json!({}))?.must("sign-out", 200)?;

    // The same code again is a replay, and it is said so; a recovery code
    // takes its place, on the same sign-in, once.
    let asked = console.sign_in(alice, "alice", ALICE_PASSWORD)?;
    let (ticket, _) = step_asked(&asked)?;
    let replay = console.post(alice, "/api/1/sign-in/code", &code_of(&ticket, &code))?.refused(
        "the same code again",
        401,
        "ADM-102",
    )?;
    if !replay.detail().contains("used already") {
        return Err(format!("a replayed code was refused for {:?}", replay.detail()));
    }
    console
        .post(alice, "/api/1/sign-in/code", &code_of(&ticket, &first))?
        .must("a recovery code", 200)?;
    console.read_session(alice)?;
    console.post(alice, "/api/1/sign-out", &json!({}))?.must("sign-out", 200)?;

    // Once: the same recovery code does not sign in again, though another
    // does.
    let asked = console.sign_in(alice, "alice", ALICE_PASSWORD)?;
    let (ticket, _) = step_asked(&asked)?;
    console.post(alice, "/api/1/sign-in/code", &code_of(&ticket, &first))?.refused(
        "a recovery code used already",
        401,
        "ADM-102",
    )?;
    console
        .post(alice, "/api/1/sign-in/code", &code_of(&ticket, &second.to_ascii_lowercase()))?
        .must("another recovery code, typed in lower case", 200)?;
    console.read_session(alice)?;
    Ok(app)
}

/// The ticket and the kinds of second step that an answer to a sign-in
/// asks for.
fn step_asked(reply: &Reply) -> Result<(String, Vec<String>), String> {
    let body = reply.json()?;
    let ticket = body["step"]["ticket"].as_str().filter(|ticket| !ticket.is_empty());
    let kinds: Vec<String> = body["step"]["kinds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    match ticket {
        Some(ticket) if reply.status == 200 => Ok((ticket.to_string(), kinds)),
        _ => {
            Err(format!("the sign-in asked {} {}, not for a second step", reply.status, reply.body))
        }
    }
}

/// Unlocking with an app's code and what the unlock is for. Locked, a
/// change to her second steps wants her password; unlocked with the
/// app's next code it does not. The session says how long is left, and
/// locking ends it. The lab route that runs a command as an administrator
/// is not in this image.
fn unlocking(console: &Console, alice: &mut Browser, app: &mut App) -> Result<(), String> {
    let needs_password = json!({"proof": ""});
    let codes = "/api/1/account/second-step/codes";
    let refused = console.post(alice, codes, &needs_password)?.refused(
        "new recovery codes with no proof, locked",
        403,
        "ADM-110",
    )?;
    if !refused.detail().contains("unlock the console first") {
        return Err(format!("a change with no proof was refused for {:?}", refused.detail()));
    }

    let code = console.next_code(app)?;
    let unlock = json!({"kind": "code", "code": code});
    let unlocked =
        console.post(alice, "/api/1/unlock", &unlock)?.must("unlock with a code", 200)?;
    let seconds = unlocked.json()?["unlock_seconds"].as_u64().unwrap_or_default();
    if !(800..=900).contains(&seconds) {
        return Err(format!("the unlock lasts {seconds} s, not 15 minutes"));
    }
    let session = console.get(alice, "/api/1/session")?.must("her session", 200)?.json()?;
    if !session["unlock_seconds"].as_u64().is_some_and(|left| (1..=900).contains(&left)) {
        return Err(format!("the unlocked session says {}", session["unlock_seconds"]));
    }
    let made = console.post(alice, codes, &needs_password)?.must("new codes when unlocked", 200)?;
    if made.json()?["codes"].as_array().map(Vec::len) != Some(10) {
        return Err(format!("new recovery codes came as {}", made.body));
    }

    console.post(alice, "/api/1/lock", &json!({}))?.must("lock", 200)?;
    console.post(alice, codes, &needs_password)?.refused(
        "new codes with no proof, locked again",
        403,
        "ADM-110",
    )?;
    // Only a build for the lab has the route that runs a command for an
    // unlocked session, and the test image is a build for people.
    console.post(alice, "/api/1/test/elevate", &json!({"command": "id"}))?.refused(
        "the lab route",
        404,
        "ADM-116",
    )?;
    Ok(())
}

/// A restart of admind, stopped and started again, ends the sessions: the
/// cookie that was good is ADM-107, and the answer clears it. The
/// certificate is the same one, the address in the issue file is back,
/// and alice signs in again. tty2's sign-in screen, drawn before the stop
/// and typed on by nobody, is drawn again when admind removes its file as
/// it stops, and again when it writes it as it starts, so that it shows
/// the address again.
fn restart(
    vm: &mut Vm,
    console: &Console,
    alice: &mut Browser,
    address: &str,
) -> Result<(), String> {
    let old = alice.cookie.clone();
    elevated(vm)?;
    vm.press(&["alt-f2"])?;
    vm.expect_screen(&["Web console:", address])?;
    vm.press(&["alt-f1"])?;
    let redrawn = "consoled: tty2: drawing the sign-in screen again for the web console's address";
    let (before, drawn) =
        (vm.log().matches("admind: answers at ").count(), vm.log().matches(redrawn).count());
    answer_prompt(vm, "$", "clear; elevate service admind stop; echo \"stop $?\"")?;
    vm.expect_screen(&["stop 0"])?;
    wait_for_lines(vm, redrawn, drawn + 1, 30)?;
    answer_prompt(vm, "$", "clear; elevate service admind start; echo \"start $?\"")?;
    vm.expect_screen(&["start 0"])?;
    wait_for_lines(vm, "admind: answers at ", before + 1, 60)?;
    wait_for_lines(vm, redrawn, drawn + 2, 30)?;
    vm.press(&["alt-f2"])?;
    vm.expect_screen(&["Web console:", address])?;
    vm.press(&["alt-f1"])?;
    // The new admind is listening: it answers once the port's forwarding
    // reaches it.
    let deadline = Instant::now() + Duration::from_secs(30);
    while console.get(&mut Browser::default(), "/api/1/health").is_err() {
        if Instant::now() >= deadline {
            return Err("admind does not answer 30 s after its restart".into());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    let ended = console.get(alice, "/api/1/session")?.refused(
        "the session after a restart",
        401,
        "ADM-107",
    )?;
    let cleared = ended.headers_named("set-cookie").filter_map(parse_cookie).any(|cookie| {
        cookie.name == SESSION_COOKIE
            && cookie.value.is_empty()
            && cookie.attributes.contains(&"Max-Age=0".to_string())
    });
    if old.is_none() || !cleared || alice.cookie.is_some() {
        return Err("the answer to a session that ended did not clear the cookie".into());
    }
    // The same certificate: curl trusts that one alone.
    let named = console.send("GET", "/api/1/fingerprint", &[], None)?.must("fingerprint", 200)?;
    if named.json()?["fingerprint"].as_str() != Some(console.fingerprint.as_str()) {
        return Err(format!("the certificate changed at the restart: {}", named.body));
    }
    answer_prompt(vm, "$", "clear; head -n 1 /system/live/admind/issue")?;
    vm.expect_screen(&[address])?;
    // authd answers the new admind: the password is right, and the step
    // is asked as before.
    let asked = console.sign_in(alice, "alice", ALICE_PASSWORD)?;
    step_asked(&asked)?;
    Ok(())
}

/// Waits until the log holds `needle` at least `times` times.
fn wait_for_lines(vm: &mut Vm, needle: &str, times: usize, seconds: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while vm.log().matches(needle).count() < times {
        if Instant::now() >= deadline {
            return Err(format!("the log does not say \"{needle}\" {times} times in {seconds} s"));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}

/// A connection that sends nothing, held by a thread of its own: how long
/// until the console closes it.
fn silent_connection(
    port: u16,
) -> Result<std::thread::JoinHandle<Result<Duration, String>>, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .map_err(|err| format!("a silent connection to the console: {err}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|err| format!("a silent connection's timeout: {err}"))?;
    let started = Instant::now();
    Ok(std::thread::spawn(move || {
        let mut byte = [0u8; 1];
        match stream.read(&mut byte) {
            // QEMU closes the host's end when the guest closes its own,
            // with an end or with a reset.
            Ok(0) => Ok(started.elapsed()),
            Err(err) if err.kind() == std::io::ErrorKind::ConnectionReset => Ok(started.elapsed()),
            Ok(_) => Err("the console sent a connection that said nothing a byte".into()),
            Err(err) => Err(format!("a silent connection was not closed in 60 s: {err}")),
        }
    }))
}

/// HTTPS connections that each hold one of the places the console gives an
/// address: the host's openssl, which keeps its connection open, asks each
/// for health once, and the connection is kept alive after the answer
/// for a minute (admind(8)). They end when dropped.
struct Held(Vec<Child>);

impl Drop for Held {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Holds all [`PLACES`] places of the host's address, each connection
/// answered before this returns. They are made at once, so that the first
/// is still kept alive when the last is answered.
fn hold_every_place(console: &Console) -> Result<Held, String> {
    let mut held = Held(Vec::with_capacity(PLACES));
    let (answered, answers) = mpsc::channel();
    let request = format!("GET /api/1/health HTTP/1.1\r\nHost: localhost:{PORT}\r\n\r\n");
    for n in 0..PLACES {
        let mut child = Command::new("openssl")
            .args(["s_client", "-quiet", "-servername", "localhost", "-connect"])
            .arg(format!("127.0.0.1:{}", console.port))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| format!("the admin test needs the host's openssl: {err}"))?;
        let (Some(mut input), Some(mut output)) = (child.stdin.take(), child.stdout.take()) else {
            return Err("openssl s_client has no input or output".into());
        };
        held.0.push(child);
        // -quiet goes on after its input ends, so the input can close.
        input
            .write_all(request.as_bytes())
            .map_err(|err| format!("asking through connection {n}: {err}"))?;
        drop(input);
        let answered = answered.clone();
        std::thread::spawn(move || {
            let mut status = [0u8; 12];
            let read = output.read_exact(&mut status).map(|()| status);
            // The test may have given up on this answer already.
            let _ = answered.send((n, read));
            // The rest of the answer stays unread in the pipe.
            drop(output);
        });
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    for _ in 0..PLACES {
        let wait = deadline.saturating_duration_since(Instant::now());
        let (n, read) = answers
            .recv_timeout(wait)
            .map_err(|_| format!("the {PLACES} held connections were not all answered in 120 s"))?;
        match read {
            Ok(status) if status == *b"HTTP/1.1 200" => {}
            Ok(status) => {
                let status = String::from_utf8_lossy(&status);
                return Err(format!("held connection {n} was answered {status}"));
            }
            Err(err) => return Err(format!("held connection {n} got no answer: {err}")),
        }
    }
    Ok(held)
}

/// The console as the host reaches it: port 9090 of the guest, through the
/// port QEMU forwards on the host's loopback.
struct Console {
    /// The host's port.
    port: u16,
    /// The certificate the machine served at first, which curl trusts.
    pem: PathBuf,
    /// Its SHA-256 fingerprint, `AB:CD:...`.
    fingerprint: String,
}

impl Console {
    /// Forwards a port to the console, and takes the certificate it serves:
    /// the first handshake trusts what it is given, and the test compares
    /// that certificate with the guest's own before it relies on it.
    fn open(vm: &mut Vm) -> Result<Console, String> {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .map_err(|err| format!("no free port: {err}"))?
            .port();
        vm.monitor(&format!("hostfwd_add n0 tcp:127.0.0.1:{port}-:{PORT}"))?;
        let mut console =
            Console { port, pem: vm.dir().join("admind.pem"), fingerprint: String::new() };
        let url = format!("{ORIGIN}/api/1/health");
        let deadline = Instant::now() + Duration::from_secs(30);
        let learned = loop {
            let out = console.curl(&[
                "--insecure",
                "--output",
                "/dev/null",
                "--write-out",
                "%{certs}",
                &url,
            ])?;
            if out.status.success() {
                break String::from_utf8_lossy(&out.stdout).into_owned();
            }
            if Instant::now() >= deadline {
                let why = String::from_utf8_lossy(&out.stderr);
                return Err(format!("the console does not answer on port {port}: {}", why.trim()));
            }
            std::thread::sleep(Duration::from_secs(1));
        };
        let (pem, der) = first_certificate(&learned)
            .ok_or_else(|| format!("curl did not print the console's certificate: {learned:?}"))?;
        std::fs::write(&console.pem, pem)
            .map_err(|err| format!("writing {}: {err}", console.pem.display()))?;
        console.fingerprint = fingerprint_of(&der);
        Ok(console)
    }

    /// Runs the host's curl, aimed at the guest's port 9090 as `localhost`,
    /// with `args` after the options every request has: no settings file,
    /// no proxy, HTTP/1.1, a limit on the time.
    fn curl(&self, args: &[&str]) -> Result<Output, String> {
        Command::new("curl")
            .args(["--disable", "--silent", "--show-error", "--http1.1", "--max-time", "90"])
            .args(["--noproxy", "*", "--connect-to"])
            .arg(format!("localhost:{PORT}:127.0.0.1:{}", self.port))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|err| format!("the admin test needs the host's curl: {err}"))
    }

    /// One request, trusting the certificate taken at first, with exactly
    /// the headers given besides Host and curl's own: what curl printed,
    /// whether or not it succeeded.
    fn run(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<&str>,
    ) -> Result<Output, String> {
        let url = format!("{ORIGIN}{path}");
        let pem = self.pem.display().to_string();
        let mut args = vec!["--include", "--cacert", &pem, "--request", method];
        let lines: Vec<String> =
            headers.iter().map(|(name, value)| format!("{name}: {value}")).collect();
        for line in &lines {
            args.extend(["--header", line]);
        }
        if let Some(body) = body {
            args.extend(["--data-binary", body]);
        }
        args.push(&url);
        self.curl(&args)
    }

    /// [`Console::run`], for a request that curl must have carried out.
    fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<&str>,
    ) -> Result<Reply, String> {
        let out = self.run(method, path, headers, body)?;
        if !out.status.success() {
            let why = String::from_utf8_lossy(&out.stderr);
            return Err(format!("curl {method} {path} ended with {}: {}", out.status, why.trim()));
        }
        parse_reply(&out.stdout)
    }

    /// A read, as the page makes it. The browser keeps the cookie the
    /// answer sets or clears.
    fn get(&self, browser: &mut Browser, path: &str) -> Result<Reply, String> {
        let headers: Vec<_> =
            browser.page_headers().into_iter().filter(|(name, _)| name == "Cookie").collect();
        let reply = self.send("GET", path, &headers, None)?;
        browser.keep(&reply);
        Ok(reply)
    }

    /// A change, as the page makes it, with `body`. A sign-in route wants
    /// no more than ten tries from an address in a minute, and answers
    /// ADM-105 with the seconds to wait; the test waits and asks again.
    fn post(&self, browser: &mut Browser, path: &str, body: &Value) -> Result<Reply, String> {
        let body = body.to_string();
        for _ in 0..4 {
            let reply = self.send("POST", path, &browser.page_headers(), Some(&body))?;
            if reply.status != 429 {
                browser.keep(&reply);
                return Ok(reply);
            }
            let seconds = reply.header("retry-after").and_then(|text| text.parse::<u64>().ok());
            std::thread::sleep(Duration::from_secs(seconds.unwrap_or(10).min(70) + 1));
        }
        Err(format!("{path} was still answered 429 after four waits"))
    }

    /// Signs `name` in, and returns the console's answer, which the
    /// browser has taken its cookie from.
    fn sign_in(&self, browser: &mut Browser, name: &str, password: &str) -> Result<Reply, String> {
        let body = json!({"name": name, "password": password});
        self.post(browser, "/api/1/sign-in", &body)?.must(&format!("{name}'s sign-in"), 200)
    }

    /// Reads the session of a browser that signed in, and keeps the token
    /// its page would.
    fn read_session(&self, browser: &mut Browser) -> Result<Value, String> {
        let session = self.get(browser, "/api/1/session")?.must("the session", 200)?.json()?;
        let token = session["csrf"].as_str().ok_or("the session has no token")?;
        browser.token = Some(token.to_string());
        Ok(session)
    }

    /// Unix seconds by the guest's clock, which the console's own `Date`
    /// header gives: the app's codes follow the clock of the machine that
    /// checks them, which need not be the host's to the second.
    fn guest_unix(&self) -> Result<u64, String> {
        let reply = self.send("GET", "/api/1/health", &[], None)?;
        match reply.header("date").and_then(http_date) {
            Some(seconds) => Ok(seconds),
            None => {
                Ok(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs()))
            }
        }
    }

    /// The app's next code, which authd has not taken: the current step's,
    /// or the next one's, which it takes too for a clock a little off. When
    /// that is a step ahead of what it takes, this waits for the guest's
    /// clock to come to it.
    fn next_code(&self, app: &mut App) -> Result<String, String> {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            if let Some(step) = step_to_give(self.guest_unix()?, app.last) {
                app.last = Some(step);
                return Ok(app_code(&app.secret, step));
            }
            if Instant::now() >= deadline {
                return Err("the guest's clock did not come to the app's next step in 90 s".into());
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    /// TLS 1.1 is refused by the console itself: curl, allowed nothing
    /// above 1.1 and the old ciphers a library would refuse to offer, ends
    /// with an alert the server sent, and not with its own refusal to try.
    /// The alert is for a protocol version, or a handshake failure where
    /// rustls first finds the hello without the signature algorithms that
    /// a TLS 1.1 hello never has.
    fn tls_refused(&self) -> Result<(), String> {
        let pem = self.pem.display().to_string();
        let url = format!("{ORIGIN}/api/1/health");
        let out = self.curl(&[
            "--cacert",
            &pem,
            "--tlsv1.1",
            "--tls-max",
            "1.1",
            "--ciphers",
            "DEFAULT:@SECLEVEL=0",
            &url,
        ])?;
        let why = String::from_utf8_lossy(&out.stderr).to_ascii_lowercase();
        if out.status.success() {
            return Err("TLS 1.1 was served".into());
        }
        let alerted = ["alert protocol version", "alert handshake failure"];
        if !alerted.iter().any(|alert| why.contains(alert)) {
            return Err(format!(
                "TLS 1.1 was refused, but not by the console's alert: {}",
                why.trim()
            ));
        }
        Ok(())
    }

    /// Health over exactly TLS `version`.
    fn tls_served(&self, version: &str) -> Result<(), String> {
        let pem = self.pem.display().to_string();
        let url = format!("{ORIGIN}/api/1/health");
        let flag = format!("--tlsv{version}");
        let out = self.curl(&["--cacert", &pem, &flag, "--tls-max", version, &url])?;
        let body = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || body != r#"{"ok":true}"# {
            let why = String::from_utf8_lossy(&out.stderr);
            return Err(format!("TLS {version} was not served: {} {}", body.trim(), why.trim()));
        }
        Ok(())
    }
}

/// What a browser keeps of the console: the session's cookie and the token
/// its page reads from the session.
#[derive(Default)]
struct Browser {
    cookie: Option<String>,
    token: Option<String>,
}

impl Browser {
    /// The headers that the console's own page sends with a change.
    fn page_headers(&self) -> Vec<(String, String)> {
        let mut headers = vec![
            ("Origin".into(), ORIGIN.into()),
            ("Content-Type".into(), "application/json".into()),
        ];
        if let Some(cookie) = &self.cookie {
            headers.push(("Cookie".into(), format!("{SESSION_COOKIE}={cookie}")));
        }
        if let Some(token) = &self.token {
            headers.push((CSRF_HEADER.into(), token.clone()));
        }
        headers
    }

    /// Takes what `reply` tells a browser to keep: the session's cookie, or
    /// to forget it.
    fn keep(&mut self, reply: &Reply) {
        for cookie in reply.headers_named("set-cookie").filter_map(parse_cookie) {
            if cookie.name == SESSION_COOKIE {
                self.cookie = Some(cookie.value).filter(|value| !value.is_empty());
                if self.cookie.is_none() {
                    self.token = None;
                }
            }
        }
    }
}

/// What the console answered.
struct Reply {
    status: u16,
    /// Names in lower case, in the order they came.
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    /// The first header called `name`.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(have, _)| have == name).map(|(_, value)| value.as_str())
    }

    /// Every header called `name`.
    fn headers_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> {
        self.headers.iter().filter(move |(have, _)| have == name).map(|(_, value)| value.as_str())
    }

    /// The body as JSON.
    fn json(&self) -> Result<Value, String> {
        serde_json::from_str(&self.body)
            .map_err(|err| format!("the answer {} {:?} is not JSON: {err}", self.status, self.body))
    }

    /// The error's own words, if the body is the console's error.
    fn detail(&self) -> String {
        self.json()
            .ok()
            .and_then(|body| body["error"]["detail"].as_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// This reply if it has `status`; otherwise an error that says what
    /// `what` got.
    fn must(self, what: &str, status: u16) -> Result<Reply, String> {
        if self.status == status {
            return Ok(self);
        }
        Err(format!("{what} was answered {} {}, not {status}", self.status, self.body))
    }

    /// This reply if it is the console's error `id` with `status`.
    fn refused(self, what: &str, status: u16, id: &str) -> Result<Reply, String> {
        let said =
            self.json().ok().and_then(|body| body["error"]["id"].as_str().map(str::to_string));
        if self.status == status && said.as_deref() == Some(id) {
            return Ok(self);
        }
        Err(format!("{what} was answered {} {}, not {id} with {status}", self.status, self.body))
    }
}

// Helpers that read and compute, with no machine to ask: the unit tests
// below run them alone.

/// SHA-1 (FIPS 180-4), which RFC 6238's codes are made with by default and
/// authd checks them with.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut state: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&(data.len() as u64 * 8).to_be_bytes());
    let (blocks, _) = padded.as_chunks::<64>();
    for block in blocks {
        let mut words = [0u32; 80];
        for (word, bytes) in words.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*bytes);
        }
        for at in 16..80 {
            words[at] =
                (words[at - 3] ^ words[at - 8] ^ words[at - 14] ^ words[at - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = state;
        for (at, word) in words.into_iter().enumerate() {
            let (mix, constant) = match at {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(mix)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(word);
            (a, b, c, d, e) = (next, a, b.rotate_left(30), c, d);
        }
        for (kept, new) in state.iter_mut().zip([a, b, c, d, e]) {
            *kept = kept.wrapping_add(new);
        }
    }
    let mut hash = [0u8; 20];
    for (bytes, word) in hash.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *bytes = word.to_be_bytes();
    }
    hash
}

/// HMAC-SHA1 (RFC 2104).
fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    let mut block = [0u8; 64];
    if key.len() > block.len() {
        block[..20].copy_from_slice(&sha1(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = block.iter().map(|byte| byte ^ 0x36).collect();
    inner.extend_from_slice(data);
    let mut outer: Vec<u8> = block.iter().map(|byte| byte ^ 0x5c).collect();
    outer.extend_from_slice(&sha1(&inner));
    sha1(&outer)
}

/// The app's six digits for 30-second `step` since 1970 (RFC 6238 with
/// HMAC-SHA1, truncated as RFC 4226 says).
fn app_code(secret: &[u8], step: u64) -> String {
    let mac = hmac_sha1(secret, &step.to_be_bytes());
    let at = usize::from(mac[19] & 0x0f);
    let number = u32::from_be_bytes([mac[at], mac[at + 1], mac[at + 2], mac[at + 3]]) & 0x7fff_ffff;
    format!("{:06}", number % 1_000_000)
}

/// The step of the app's code to give at guest time `now` when `after` is
/// the last step authd took a code for: this one, or the next when this
/// was taken already, which authd takes as well for a clock a little off.
/// `None` when even that is not one it takes yet: the clock has to come to
/// it.
fn step_to_give(now: u64, after: Option<u64>) -> Option<u64> {
    let current = now / 30;
    let step = after.map_or(current, |after| current.max(after + 1));
    (step <= current + 1).then_some(step)
}

/// Six digits that are none of the app's codes for the steps around `now`.
fn wrong_code(secret: &[u8], now: u64) -> String {
    let current = now / 30;
    let right: Vec<String> =
        (current.saturating_sub(2)..=current + 2).map(|step| app_code(secret, step)).collect();
    (1u32..)
        .map(|n| format!("{:06}", n * 123_457 % 1_000_000))
        .find(|code| !right.contains(code))
        .unwrap_or_default()
}

/// The bytes of RFC 4648's base32 `text`, with or without padding.
fn from_base32(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for letter in text.trim_end_matches('=').bytes() {
        let value = match letter {
            b'A'..=b'Z' => letter - b'A',
            b'2'..=b'7' => letter - b'2' + 26,
            _ => return None,
        };
        buffer = buffer << 5 | u32::from(value);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(bytes)
}

/// The bytes of standard base64 `text`, which may have padding and
/// whitespace.
fn from_base64(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for letter in text.bytes().filter(|&byte| !byte.is_ascii_whitespace() && byte != b'=') {
        let value = match letter {
            b'A'..=b'Z' => letter - b'A',
            b'a'..=b'z' => letter - b'a' + 26,
            b'0'..=b'9' => letter - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        buffer = buffer << 6 | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(bytes)
}

/// The secret in an `otpauth://` URI.
fn secret_of(uri: &str) -> Result<Vec<u8>, String> {
    let query = uri.split_once('?').map_or("", |(_, query)| query);
    let text = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("secret="))
        .ok_or_else(|| format!("the app's URI {uri:?} has no secret"))?;
    from_base32(text)
        .filter(|secret| secret.len() >= 10)
        .ok_or_else(|| format!("the secret {text:?} does not read"))
}

/// Whether `text` is a recovery code: four groups of four letters and
/// digits of base32, as `ABCD-EFGH-IJKL-MNOP`.
fn is_recovery_code(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 4
        && groups.iter().all(|group| {
            group.len() == 4
                && group.bytes().all(|b| b.is_ascii_uppercase() || (b'2'..=b'7').contains(&b))
        })
}

/// The first certificate in `text`, the way curl's `%{certs}` prints a
/// chain with details around the PEM: the PEM as every program reads it,
/// and its DER.
fn first_certificate(text: &str) -> Option<(String, Vec<u8>)> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let from = text.find(BEGIN)? + BEGIN.len();
    let body = &text[from..from + text[from..].find(END)?];
    let der = from_base64(body)?;
    let compact: String = body.split_whitespace().collect();
    let mut pem = format!("{BEGIN}\n");
    for line in compact.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(line).ok()?);
        pem.push('\n');
    }
    pem.push_str(END);
    pem.push('\n');
    Some((pem, der))
}

/// A certificate's SHA-256 fingerprint as `cert show` and the console
/// write it: upper-case bytes with colons.
fn fingerprint_of(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|byte| format!("{byte:02X}")).collect::<Vec<_>>().join(":")
}

/// The fingerprint that a screen shows under its heading, whole bytes on
/// each line however many: the 32 bytes of the lines after the heading
/// that hold bytes and nothing else.
fn fingerprint_on_screen(screen: &[String]) -> Option<String> {
    let heading =
        screen.iter().position(|line| line.contains("Certificate SHA-256 fingerprint:"))?;
    let mut bytes: Vec<String> = Vec::new();
    for line in &screen[heading + 1..] {
        let pairs: Vec<&str> = line.trim().split(':').collect();
        if !pairs.iter().all(|pair| pair.len() == 2 && pair.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            break;
        }
        bytes.extend(pairs.iter().map(|pair| pair.to_ascii_uppercase()));
    }
    (bytes.len() == 32).then(|| bytes.join(":"))
}

/// A `Set-Cookie` value: the cookie and its attributes as written.
struct Cookie {
    name: String,
    value: String,
    attributes: Vec<String>,
}

fn parse_cookie(text: &str) -> Option<Cookie> {
    let mut parts = text.split("; ");
    let (name, value) = parts.next()?.split_once('=')?;
    Some(Cookie {
        name: name.to_string(),
        value: value.to_string(),
        attributes: parts.map(str::to_string).collect(),
    })
}

/// A reply as `curl --include` prints it: the status, the headers with
/// lower-case names, and the body. An interim reply (1xx) before it is
/// passed over.
fn parse_reply(raw: &[u8]) -> Result<Reply, String> {
    let mut rest = raw;
    loop {
        let end = rest
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or_else(|| format!("no end of the head in {:?}", String::from_utf8_lossy(raw)))?;
        let head = String::from_utf8_lossy(&rest[..end]).into_owned();
        rest = &rest[end + 4..];
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .and_then(|line| line.split(' ').nth(1))
            .and_then(|code| code.parse::<u16>().ok())
            .ok_or_else(|| format!("no status in {head:?}"))?;
        if !(100..200).contains(&status) {
            let headers = lines
                .filter_map(|line| line.split_once(':'))
                .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
                .collect();
            return Ok(Reply { status, headers, body: String::from_utf8_lossy(rest).into_owned() });
        }
    }
}

/// Unix seconds of an HTTP date, `Sun, 06 Nov 1994 08:49:37 GMT`.
fn http_date(text: &str) -> Option<u64> {
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let words: Vec<&str> = text.split_whitespace().collect();
    let [_, day, month, year, time, "GMT"] = words[..] else { return None };
    let month = i64::try_from(MONTHS.iter().position(|&name| name == month)?).ok()? + 1;
    let (day, year): (i64, i64) = (day.parse().ok()?, year.parse().ok()?);
    let clock: Vec<u64> = time.split(':').map(str::parse).collect::<Result<_, _>>().ok()?;
    let [hours, minutes, seconds] = clock[..] else { return None };
    // Days since 1970 by the calendar of the proleptic Gregorian years,
    // counted from March, as Howard Hinnant writes it.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year.rem_euclid(400);
    let of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let days = era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468;
    u64::try_from(days)
        .ok()?
        .checked_mul(86_400)?
        .checked_add(hours * 3600 + minutes * 60 + seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().fold(String::new(), |text, byte| text + &format!("{byte:02x}"))
    }

    #[test]
    fn sha1_has_the_standard_answers_across_block_ends() {
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        let two_blocks = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(hex(&sha1(two_blocks)), "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
        // 55, 56 and 64 bytes: where the padding and the length change blocks.
        assert_eq!(hex(&sha1(&[b'a'; 55])), "c1c8bbdc22796e28c0e15163d20899b65621d65a");
        assert_eq!(hex(&sha1(&[b'a'; 56])), "c2db330f6083854c99d4b5bfb6e8f29f201be699");
        assert_eq!(hex(&sha1(&[b'a'; 64])), "0098ba824b5c16427bd7a1122a5a442a25ec644d");
        assert_eq!(hex(&sha1(&vec![b'a'; 1_000_000])), "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
    }

    #[test]
    fn hmac_sha1_has_the_answers_of_rfc_2202() {
        assert_eq!(
            hex(&hmac_sha1(&[0x0b; 20], b"Hi There")),
            "b617318655057264e28bc0b6fb378c8ef146be00"
        );
        assert_eq!(
            hex(&hmac_sha1(b"Jefe", b"what do ya want for nothing?")),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
        // A key longer than a block is hashed first.
        assert_eq!(
            hex(&hmac_sha1(&[0xaa; 80], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "aa4ae5e15272d00e95705637ce8a3b55ed402112"
        );
    }

    #[test]
    fn the_apps_codes_are_those_of_rfc_6238() {
        // The RFC's SHA-1 secret, and its 8-digit codes cut to the last 6.
        let secret = b"12345678901234567890";
        for (time, code) in [
            (59u64, "287082"),
            (1_111_111_109, "081804"),
            (1_111_111_111, "050471"),
            (1_234_567_890, "005924"),
            (2_000_000_000, "279037"),
            (20_000_000_000, "353130"),
        ] {
            assert_eq!(app_code(secret, time / 30), code, "at {time}");
        }
    }

    #[test]
    fn a_code_is_given_for_this_step_or_the_next_and_otherwise_waits() {
        // 1000 s is in step 33, which runs from 990 to 1019.
        assert_eq!(step_to_give(1000, None), Some(33));
        assert_eq!(step_to_give(1000, Some(10)), Some(33));
        assert_eq!(step_to_give(1000, Some(32)), Some(33));
        // Taken already: the next, which authd takes for a clock a little off.
        assert_eq!(step_to_give(1000, Some(33)), Some(34));
        // The next was taken too: wait for the clock.
        assert_eq!(step_to_give(1000, Some(34)), None);
        assert_eq!(step_to_give(1020, Some(34)), Some(35));
    }

    #[test]
    fn a_wrong_code_is_none_of_the_codes_around_now() {
        let secret = b"12345678901234567890";
        for now in [0, 59, 1_111_111_109, 2_000_000_000] {
            let wrong = wrong_code(secret, now);
            assert_eq!(wrong.len(), 6);
            for step in (now / 30).saturating_sub(2)..=now / 30 + 2 {
                assert_ne!(wrong, app_code(secret, step), "now {now}, step {step}");
            }
        }
    }

    #[test]
    fn base32_and_base64_read_the_rfc_4648_vectors_with_or_without_padding() {
        for (text, want) in [
            ("", ""),
            ("MY======", "f"),
            ("MZXQ====", "fo"),
            ("MZXW6", "foo"),
            ("MZXW6YQ=", "foob"),
            ("MZXW6YTB", "fooba"),
            ("MZXW6YTBOI======", "foobar"),
        ] {
            assert_eq!(from_base32(text).as_deref(), Some(want.as_bytes()), "{text}");
        }
        assert_eq!(from_base32("mzxw6"), None);
        assert_eq!(from_base32("MZXW1"), None);
        for (text, want) in [
            ("", ""),
            ("Zg==", "f"),
            ("Zm8=", "fo"),
            ("Zm9v", "foo"),
            ("Zm9vYg==", "foob"),
            ("Zm9v\nYmE=\n", "fooba"),
            ("Zm9vYmFy", "foobar"),
        ] {
            assert_eq!(from_base64(text).as_deref(), Some(want.as_bytes()), "{text:?}");
        }
        assert_eq!(from_base64("Zm9v!"), None);
    }

    #[test]
    fn the_secret_is_read_from_the_uri_as_authd_writes_it() {
        let uri = "otpauth://totp/Dukovo%20(duke-nas):alice?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ\
                   &issuer=Dukovo%20(duke-nas)&algorithm=SHA1&digits=6&period=30";
        assert_eq!(secret_of(uri).unwrap(), b"12345678901234567890");
        assert!(secret_of("otpauth://totp/x?issuer=y").is_err());
        assert!(secret_of("otpauth://totp/x?secret=!!!").is_err());
        // Too short to be an app's secret.
        assert!(secret_of("otpauth://totp/x?secret=MZXW6").is_err());
    }

    #[test]
    fn recovery_codes_are_four_groups_of_base32() {
        assert!(is_recovery_code("ABCD-EFGH-IJKL-MNOP"));
        assert!(is_recovery_code("A2B3-C4D5-E6F7-GHIJ"));
        for wrong in
            ["", "ABCD-EFGH-IJKL", "ABCDEFGHIJKLMNOP", "abcd-efgh-ijkl-mnop", "ABC1-EFGH-IJKL-MNOP"]
        {
            assert!(!is_recovery_code(wrong), "{wrong}");
        }
    }

    #[test]
    fn the_certificate_is_taken_from_what_curl_prints_and_fingerprinted_as_cert_show_does() {
        // Bytes 1, 2, 3 are no certificate, and need not be: only the
        // base64 and the digest are read.
        let printed = "Subject:CN=duke-nas\nSignature Algorithm:ecdsa\n\
                       Cert:-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n\
                       Subject:CN=another\nCert:-----BEGIN CERTIFICATE-----\nBAUG\n\
                       -----END CERTIFICATE-----\n";
        let (pem, der) = first_certificate(printed).unwrap();
        assert_eq!(pem, "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n");
        assert_eq!(der, [1, 2, 3]);
        assert_eq!(
            fingerprint_of(&der),
            "03:90:58:C6:F2:C0:CB:49:2C:53:3B:0A:4D:14:EF:77:CC:0F:78:AB:CC:CE:D5:28:7D:84:A1:A2:01:1C:FB:81"
        );
        // A body of more than 64 characters is written again in lines of 64.
        let long = format!(
            "-----BEGIN CERTIFICATE-----\n{}\n{}\n-----END CERTIFICATE-----",
            "A".repeat(80),
            "A".repeat(8)
        );
        let (pem, der) = first_certificate(&long).unwrap();
        let lines: Vec<&str> = pem.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!((lines[1].len(), lines[2].len()), (64, 24));
        assert_eq!(der.len(), 66);
        assert!(first_certificate("no certificate here").is_none());
        assert!(first_certificate("-----BEGIN CERTIFICATE-----\nAQID").is_none());
    }

    #[test]
    fn the_fingerprint_is_read_from_the_screen_however_consoled_wrapped_it() {
        let bytes: Vec<String> = (0..32).map(|n| format!("{:02X}", n * 7 + 3)).collect();
        let want = bytes.join(":");
        let screen = |width: usize| -> Vec<String> {
            let mut lines = vec![
                "Dukovo 0.15.0".to_string(),
                " Web console: https://duke-nas.local:9090".into(),
                " Certificate SHA-256 fingerprint:".into(),
            ];
            lines.extend(bytes.chunks(width).map(|line| format!(" {}", line.join(":"))));
            lines.extend([String::new(), "Name:".into()]);
            lines
        };
        // 16 and 16 on 80 columns, and 11, 11 and 10 on 40.
        for width in [16, 11, 8, 1] {
            assert_eq!(
                fingerprint_on_screen(&screen(width)).as_deref(),
                Some(want.as_str()),
                "{width}"
            );
        }
        let mut short = screen(16);
        short.remove(4);
        assert_eq!(fingerprint_on_screen(&short), None);
        assert_eq!(fingerprint_on_screen(&["no heading".to_string()]), None);
        // Lines of the screen that are not bytes end the fingerprint.
        let mut broken = screen(16);
        broken.insert(4, " 0A:0B:0C".into());
        assert_eq!(fingerprint_on_screen(&broken), None);
    }

    #[test]
    fn a_reply_is_read_from_curls_output_past_an_interim_one() {
        let raw = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\n\
                    Set-Cookie: a=1; Path=/\r\nset-cookie: b=; Max-Age=0\r\nRetry-After: 7\r\n\r\n\
                    {\"error\":{\"id\":\"ADM-109\",\"detail\":\"no\\r\\n\\r\\nmore\"}}";
        let reply = parse_reply(raw).unwrap();
        assert_eq!(reply.status, 403);
        assert_eq!(reply.header("content-type"), Some("application/json"));
        assert_eq!(reply.header("retry-after"), Some("7"));
        assert_eq!(
            reply.headers_named("set-cookie").collect::<Vec<_>>(),
            ["a=1; Path=/", "b=; Max-Age=0"]
        );
        assert_eq!(reply.header("server"), None);
        assert_eq!(reply.json().unwrap()["error"]["id"], "ADM-109");
        assert_eq!(reply.detail(), "no\r\n\r\nmore");
        assert!(reply.refused("x", 403, "ADM-109").is_ok());
        let reply = parse_reply(b"HTTP/1.1 200 OK\r\n\r\nbody\r\n\r\nwith a blank line").unwrap();
        assert_eq!(reply.body, "body\r\n\r\nwith a blank line");
        assert!(reply.must("x", 200).is_ok());
        assert!(parse_reply(b"HTTP/1.1 200 OK\r\nno end").is_err());
        assert!(parse_reply(b"not http\r\n\r\n").is_err());
        assert!(parse_reply(b"").is_err());
    }

    #[test]
    fn a_reply_with_another_id_or_status_is_not_the_refusal_asked_for() {
        let raw = b"HTTP/1.1 401 Unauthorized\r\n\r\n{\"error\":{\"id\":\"ADM-101\"}}";
        assert!(parse_reply(raw).unwrap().refused("x", 401, "ADM-102").is_err());
        assert!(parse_reply(raw).unwrap().refused("x", 403, "ADM-101").is_err());
        assert!(parse_reply(raw).unwrap().must("x", 200).is_err());
        let not_json = parse_reply(b"HTTP/1.1 401 Unauthorized\r\n\r\nplain").unwrap();
        assert!(not_json.refused("x", 401, "ADM-101").is_err());
    }

    #[test]
    fn the_sessions_cookie_reads_with_its_attributes_as_written() {
        let cookie =
            parse_cookie("__Host-dukovo=abc-_9; Path=/; Secure; HttpOnly; SameSite=Strict")
                .unwrap();
        assert_eq!((cookie.name.as_str(), cookie.value.as_str()), ("__Host-dukovo", "abc-_9"));
        assert_eq!(cookie.attributes, ["Path=/", "Secure", "HttpOnly", "SameSite=Strict"]);
        let cleared =
            parse_cookie("__Host-dukovo=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0")
                .unwrap();
        assert!(cleared.value.is_empty());
        assert_eq!(cleared.attributes.last().map(String::as_str), Some("Max-Age=0"));
        assert!(parse_cookie("no equals sign").is_none());
    }

    #[test]
    fn a_browser_keeps_the_sessions_cookie_until_it_is_cleared() {
        let reply = |cookie: &str| Reply {
            status: 200,
            headers: vec![("set-cookie".into(), cookie.into())],
            body: String::new(),
        };
        let mut browser = Browser::default();
        browser.keep(&reply("__Host-dukovo=one; Path=/; Secure"));
        browser.token = Some("token".into());
        assert_eq!(browser.cookie.as_deref(), Some("one"));
        // Another cookie of the console's is not the session's.
        browser.keep(&reply("__Host-dukovo-trust=two; Path=/"));
        assert_eq!(browser.cookie.as_deref(), Some("one"));
        browser.keep(&reply("__Host-dukovo=; Path=/; Max-Age=0"));
        assert_eq!((browser.cookie, browser.token), (None, None));
    }

    #[test]
    fn http_dates_are_read_to_the_second() {
        assert_eq!(http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(http_date("Sun, 06 Nov 1994 08:49:37 GMT"), Some(784_111_777));
        // A leap day, and the day after.
        assert_eq!(http_date("Tue, 29 Feb 2000 23:59:59 GMT"), Some(951_868_799));
        assert_eq!(http_date("Wed, 01 Mar 2000 00:00:00 GMT"), Some(951_868_800));
        assert_eq!(http_date("Sat, 14 Nov 2026 10:00:00 GMT"), Some(1_794_650_400));
        for wrong in [
            "",
            "yesterday",
            "Sun, 06 Nov 1994 08:49:37",
            "Sun, 06 Foo 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 08:49 GMT",
        ] {
            assert_eq!(http_date(wrong), None, "{wrong}");
        }
    }
}
