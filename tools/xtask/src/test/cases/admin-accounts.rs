// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The web console's Accounts, Dukovo Admin: what alice, the owner, does to
//! other people's accounts through the routes of the accounts' actions (ADR
//! 0515, ADR 0561, ADR 0562), and what each change does at the machine. The
//! reader lab-reader, in group console and no administrator, makes the
//! contrasts; lab-made is the account alice makes and takes apart.
//!
//! The list. accounts.list is what user list --json prints, the same for
//! the reader as for alice. For alice, unlocked, accounts.list-all is what
//! user list --json prints as root, with whether each account is disabled
//! and has a password; accounts.last is what lastlog --json prints as root,
//! and accounts.show of the reader what user show --json prints as root.
//! The reader is ADM-122 and alice locked is ADM-106.
//!
//! An account made. accounts.add with a password on the command's input
//! makes lab-made, no administrator and not yet allowed to use the console:
//! its sign-in is ADM-104 until accounts.set lets it, and then it signs in.
//! A name in use is ACC-105.
//!
//! A password. accounts.password gives lab-made a new one and says it
//! changed something: the old one is ADM-101, the new one signs in, and
//! neither is in admind's log, authd's audit, logd, the journal or the
//! tasks.
//!
//! A disabled account. accounts.disable ends lab-made's session, whose next
//! call is ADM-107, and its sign-in is ADM-104 with the words that it is
//! disabled; accounts.enable lets it sign in again.
//!
//! Keys. A key that ssh-keygen made in the guest is added with
//! accounts.key-add: the line is in authorized_keys, which lab-made owns
//! with mode 600 in a folder of mode 700, and user show lists it with the
//! fingerprint ssh-keygen gives; accounts.key-remove takes the line away,
//! and a second removal is ACC-104.
//!
//! A token. lab-made makes an API token of its own; user show lists it and
//! it reads; accounts.token-revoke takes it away, and its next call is
//! ADM-115. A second revoke is ACC-104.
//!
//! A second step. lab-made adds an authenticator app, with the code that
//! console_client computes from the secret as admin does; the next sign-in
//! asks for the step; accounts.steps-reset takes it away and the sign-in
//! asks for none.
//!
//! A session at the machine. sshd is on, and the guest's own client signs in
//! as lab-made with a key on a pseudo-terminal: user show lists the session
//! as ssh, accounts.end ends it, and it is gone from the list.
//!
//! A console session of another account. lab-made has a session with an
//! event stream open and a terminal. alice, unlocked, lists them with
//! GET /api/1/accounts/lab-made/sessions, ends the terminal with POST
//! /api/1/terminals/ID/end and the session with POST
//! /api/1/accounts/lab-made/sessions/ID/end: the stream is told ended,
//! by-administrator, the next call is ADM-107 with the words that an
//! administrator ended the session, and a second end is ADM-139. The reader
//! is ADM-122 and alice locked ADM-106; alice's own session in use is
//! ADM-136, which Sign out is for. admind's audit says who ended what.
//!
//! The own account. alice disabling, removing, demoting or ending a session
//! of her own account is ADM-136 from her session and from an API token of
//! hers, and nothing runs. At the serial console the plain elevate user
//! disable --yes alice is refused by user itself, and with SUDO_USER left
//! out, as a command run by no one in particular, authd refuses it as
//! ACC-102 because alice is the only administrator. ACC-102 cannot be
//! reached by a route: the caller of every route is an enabled administrator,
//! so the account is never the last one unless it is the caller's, which
//! ADM-136 refuses first. accounts.remove then takes lab-made and its home
//! away.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::console_client::{
    ALICE_PASSWORD, App, Browser, Console, Pane, elevated, secret_of, tail, wait_for,
};
use super::super::*;
use super::admin_backups::{READER, READER_PASSWORD, Test, Who, places_of_the_machine};

/// The account alice makes and takes apart.
const MADE: &str = "lab-made";

/// Its password when it is made.
const FIRST: &str = "lab-pass-1";

/// Its password once alice has changed it.
const SECOND: &str = "lab-pass-2";

/// The key that ssh-keygen makes in the guest, from the top of its tree.
const KEY: &str = "/tmp/lab-key";

/// The size of a terminal as a page opens one, in columns and rows.
const SIZE: (u32, u32) = (80, 24);

/// The test.
pub const CASE: Case = Case {
    name: "admin-accounts",
    about: "The web console's Accounts: accounts.list is user list --json for alice and for lab-reader, and accounts.list-all, accounts.last and accounts.show are user list, lastlog and user show as root, ADM-122 for him and ADM-106 for alice locked; accounts.add with its password on the input makes lab-made, whose sign-in is ADM-104 until accounts.set lets it use the console, and a name in use is ACC-105; accounts.password says it changed something, makes the old password ADM-101 and the new one sign in, with neither in admind's log, authd's audit, logd, the journal or the tasks; accounts.disable ends its session (ADM-107), is in accounts.list-all, and refuses its sign-in until accounts.enable; accounts.key-add puts a key's line in authorized_keys, owned by lab-made with modes 600 and 700, listed by user show with ssh-keygen's fingerprint, and accounts.key-remove takes it away, a second removal being ACC-104; accounts.token-revoke makes a token's next call ADM-115; an authenticator app made lab-made's sign-in ask for a step until accounts.steps-reset; an ssh session of lab-made from inside the machine is listed by accounts.show, recorded as a sign-in over SSH, and ended by accounts.end; its console session and terminal are listed by the sessions route and ended by alice, whose stream is told by-administrator and whose next call is ADM-107, with ADM-139 for a second end and the audit saying who ended what; alice's own account is ADM-136 to disable, remove, demote or end by her session and by her token, the serial console's elevate user disable is refused by user itself and, with no SUDO_USER, by authd as ACC-102; accounts.remove takes lab-made and its home away (ADR 0515, ADR 0561, ADR 0562)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, admind and authd among
    // them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: admin_accounts,
    disks: &[],
    second: None,
};

fn admin_accounts(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    vm.expect_anywhere("admind: answers at ")?;
    let mut test = Test {
        console: Console::open(vm)?,
        pane: Pane::open()?,
        alice: Browser::default(),
        reader: Browser::default(),
    };

    // alice's password once for this pane, then the reader.
    elevated(vm)?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate sh -c 'echo {READER_PASSWORD} | user add {READER}; user set --yes \
             --console {READER}'"
        ),
    )?;
    vm.expect(&format!("authd: changed the account {READER}"))?;
    vm.expect_screen(&[&format!("Made {READER}")])?;
    test.sign_in()?;

    // lab-made's browser, which each step that signs it in again replaces.
    let mut made = Browser::default();
    the_list(vm, &mut test)?;
    an_account_added(vm, &mut test, &mut made)?;
    a_password_changed(vm, &mut test, &mut made)?;
    an_account_disabled(&mut test, &mut made)?;
    keys_added_and_removed(vm, &mut test)?;
    a_token_revoked(&mut test, &mut made)?;
    a_second_step_reset(&mut test, &mut made)?;
    a_session_at_the_machine(vm, &mut test)?;
    a_console_session_of_another(vm, &mut test, &mut made)?;
    the_own_account(vm, &mut test)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The row of the list `list`, which `user list --json` prints, for the
/// account `name`.
fn account_in<'a>(list: &'a Value, name: &str) -> Result<&'a Value, String> {
    list.as_array()
        .and_then(|rows| rows.iter().find(|row| row["name"] == name))
        .ok_or_else(|| format!("the list has no account {name}: {list}"))
}

/// The ID of the first session of the kind `kind` that `shown`, the answer
/// of accounts.show, lists.
fn session_of_kind(shown: &Value, kind: &str) -> Option<String> {
    let sessions = shown["sessions"].as_array()?;
    let found = sessions.iter().find(|session| session["kind"] == kind)?;
    found["id"].as_str().map(String::from)
}

/// The fingerprint in a line of `ssh-keygen -l`, which reads `256
/// SHA256:... comment (ED25519)`.
fn fingerprint_in(line: &str) -> Option<String> {
    line.split_whitespace().find(|word| word.starts_with("SHA256:")).map(String::from)
}

/// Whether `shown`, the answer of accounts.show, lists a key with
/// `fingerprint`.
fn lists_key(shown: &Value, fingerprint: &str) -> bool {
    let keys = shown["keys"].as_array();
    keys.is_some_and(|all| all.iter().any(|key| key["fingerprint"] == fingerprint))
}

/// A new API token of the account whose session `browser` is, for every
/// scope and a day: `POST /api/1/account/tokens/add` with the account's
/// password as the proof.
fn token_of(
    console: &Console,
    browser: &mut Browser,
    name: &str,
    password: &str,
) -> Result<String, String> {
    let body = json!({"proof": password, "name": name, "scopes": ["*"], "days": 1});
    let made = console.post(browser, "/api/1/account/tokens/add", &body)?;
    let made = made.must(&format!("the token {name}"), 200)?.json()?;
    let token = made["token"].as_str().filter(|token| token.starts_with("dkv_"));
    token.map(String::from).ok_or_else(|| format!("no token {name} was made: {made}"))
}

/// What accounts.show answers for `name` to alice, unlocked.
fn shown(test: &mut Test, name: &str) -> Result<Value, String> {
    test.unlocked()?;
    test.read(Who::Alice, &format!("/api/1/accounts/{name}"))
}

/// Step 1: accounts.list is what `user list --json` prints, the same for
/// alice and for the reader. What needs an administrator is ADM-122 for the
/// reader and ADM-106 for alice locked, and what alice, unlocked, reads of
/// the list, an account and the sign-ins is what the commands print as
/// root.
fn the_list(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let hers = test.read(Who::Alice, "/api/1/accounts")?;
    let printed = test.printed(vm, "user list --json")?;
    if hers != printed {
        return Err(format!("accounts.list is {hers}, and user list --json printed {printed}"));
    }
    let his = test.read(Who::Reader, "/api/1/accounts")?;
    if his != hers {
        return Err(format!("the reader's accounts.list is {his}, alice's is {hers}"));
    }
    for (name, admin) in [("alice", true), (READER, false)] {
        let row = account_in(&hers, name)?;
        if row["admin"] != json!(admin) || row["console"] != json!(true) {
            return Err(format!("the list has {row} for {name}: admin {admin}, console true"));
        }
    }

    // What an administrator reads, as root reads it: the list says which
    // accounts are disabled and which have a password.
    test.unlocked()?;
    let all = test.read(Who::Alice, "/api/1/accounts/list/all")?;
    let printed = test.printed(vm, "elevate user list --json")?;
    if all != printed {
        return Err(format!("accounts.list-all is {all}, and user list --json printed {printed}"));
    }
    let row = account_in(&all, READER)?;
    if row["disabled"] != json!(false) || row["password"] != json!("set") {
        return Err(format!("accounts.list-all has {row} for {READER}: enabled, with a password"));
    }
    let last = test.read(Who::Alice, "/api/1/sign-ins/last")?;
    let printed = test.printed(vm, "elevate lastlog --json")?;
    if last != printed {
        return Err(format!("accounts.last is {last}, and lastlog --json printed {printed}"));
    }
    let reads = shown(test, READER)?;
    let printed = test.printed(vm, &format!("elevate user show {READER} --json"))?;
    if reads != printed {
        return Err(format!("accounts.show is {reads}, and user show --json printed {printed}"));
    }
    let console = reads["account"]["console"] == json!(true);
    if reads["account"]["disabled"] != json!(false) || !console {
        return Err(format!("{READER} is not enabled and a console user as root reads it: {reads}"));
    }

    // The reader is no administrator, and alice locked is not unlocked.
    let own = format!("/api/1/accounts/{READER}");
    let reads = [own.as_str(), "/api/1/accounts/list/all", "/api/1/sign-ins/last"];
    for path in reads {
        test.get(Who::Reader, path)?.refused(path, 403, "ADM-122")?;
    }
    test.locked()?;
    for path in reads {
        test.get(Who::Alice, path)?.refused(path, 403, "ADM-106")?;
    }
    Ok(())
}

/// Step 2: accounts.add with a password makes lab-made. It may not use the
/// console until accounts.set says so (ADM-104), and then it signs in with
/// the password; its folder is there, and a second add of the name is
/// ACC-105. The reader may not add.
fn an_account_added(vm: &mut Vm, test: &mut Test, made: &mut Browser) -> Result<(), String> {
    let add = json!({"name": MADE, "full_name": "Made by the test", "password": FIRST});
    test.post(Who::Reader, "/api/1/accounts/add", &add)?.refused(
        "the reader's accounts.add",
        403,
        "ADM-122",
    )?;
    let answer = test.change("/api/1/accounts/add", &add)?;
    if answer["changed"] != json!(true) {
        return Err(format!("accounts.add says it changed nothing: {answer}"));
    }
    // The command's own refusal carries its ID, ACC-105.
    test.unlocked()?;
    test.post(Who::Alice, "/api/1/accounts/add", &add)?.refused(
        "a name in use",
        422,
        "ACC-105",
    )?;
    test.ends_with(vm, &format!("test -d /users/{MADE}"), 0)?;

    // No administrator and no console yet.
    let list = test.read(Who::Alice, "/api/1/accounts")?;
    let row = account_in(&list, MADE)?;
    let plain = row["admin"] == json!(false) && row["console"] == json!(false);
    if !plain || row["full_name"] != json!("Made by the test") {
        return Err(format!("the list has {row} for {MADE}, not a plain account with no console"));
    }
    let sign_in = json!({"name": MADE, "password": FIRST});
    let refused = test.console.post(made, "/api/1/sign-in", &sign_in)?.refused(
        "the sign-in of an account that may not use the console",
        403,
        "ADM-104",
    )?;
    if !refused.detail().contains("may not use the web console") {
        return Err(format!("{MADE} was refused for {:?}", refused.detail()));
    }

    // accounts.set lets it, and it signs in with its password.
    let set = test.change(&format!("/api/1/accounts/{MADE}/set"), &json!({"console": true}))?;
    if set["changed"] != json!(true) {
        return Err(format!("accounts.set says it changed nothing: {set}"));
    }
    let list = test.read(Who::Alice, "/api/1/accounts")?;
    if account_in(&list, MADE)?["console"] != json!(true) {
        return Err(format!("the list does not let {MADE} use the console: {list}"));
    }
    let printed = test.printed(vm, "user list --json")?;
    if list != printed {
        return Err(format!("accounts.list is {list}, and user list --json printed {printed}"));
    }
    test.console.sign_in(made, MADE, FIRST)?;
    test.console.read_session(made)?;
    let own = test.console.get(made, "/api/1/account/show")?.must("lab-made's own account", 200)?;
    if own.json()?["account"]["name"] != json!(MADE) {
        return Err(format!("lab-made's own page shows {}", own.body));
    }
    Ok(())
}

/// Step 3: accounts.password gives lab-made a new password: the old one is
/// ADM-101 and the new one signs in. Neither was typed in the guest, and
/// neither is in what the machine keeps.
fn a_password_changed(vm: &mut Vm, test: &mut Test, made: &mut Browser) -> Result<(), String> {
    let set = test.change(&format!("/api/1/accounts/{MADE}/password"), &json!({"password": SECOND}))?;
    // authd's file of passwords is not in admind's view, so nothing
    // compares it: the answer says that the change changed something.
    if set["changed"] != json!(true) {
        return Err(format!("accounts.password says it changed nothing: {}", set["changed"]));
    }
    let old = json!({"name": MADE, "password": FIRST});
    let mut other = Browser::default();
    test.console.post(&mut other, "/api/1/sign-in", &old)?.refused(
        "the old password",
        401,
        "ADM-101",
    )?;
    if other.cookie.is_some() {
        return Err("a refused sign-in with the old password set a cookie".into());
    }
    test.console.sign_in(&mut other, MADE, SECOND)?;
    test.console.read_session(&mut other)?;
    *made = other;

    for (what, text) in places_of_the_machine(vm, test)? {
        for password in [FIRST, SECOND] {
            if text.contains(password) {
                return Err(format!("the password {password} is in {what}"));
            }
        }
    }
    Ok(())
}

/// Step 4: accounts.disable ends lab-made's session and refuses its
/// sign-in, which accounts.enable allows again. alice reads it as disabled,
/// in the account and in the list she reads unlocked.
fn an_account_disabled(test: &mut Test, made: &mut Browser) -> Result<(), String> {
    let path = format!("/api/1/accounts/{MADE}");
    test.change(&format!("{path}/disable"), &json!({}))?;
    let reads = shown(test, MADE)?;
    if reads["account"]["disabled"] != json!(true) {
        return Err(format!("user show does not say {MADE} is disabled: {reads}"));
    }
    let all = test.read(Who::Alice, "/api/1/accounts/list/all")?;
    if account_in(&all, MADE)?["disabled"] != json!(true) {
        return Err(format!("accounts.list-all does not say {MADE} is disabled: {all}"));
    }
    // The console learns of it within its round of upkeep.
    let console = &test.console;
    wait_for("lab-made's session ending", 90, || {
        let reply = console.get(made, "/api/1/session")?;
        if reply.status != 401 {
            return Ok(Err(format!("the session was answered {} {}", reply.status, reply.body)));
        }
        reply.refused("the session of a disabled account", 401, "ADM-107").map(Ok)
    })?;
    let sign_in = json!({"name": MADE, "password": SECOND});
    let refused = test.console.post(made, "/api/1/sign-in", &sign_in)?.refused(
        "the sign-in of a disabled account",
        403,
        "ADM-104",
    )?;
    if !refused.detail().contains("is disabled") {
        return Err(format!("{MADE} was refused for {:?}", refused.detail()));
    }

    test.change(&format!("{path}/enable"), &json!({}))?;
    *made = Browser::default();
    test.console.sign_in(made, MADE, SECOND)?;
    test.console.read_session(made)?;
    let reads = shown(test, MADE)?;
    if reads["account"]["disabled"] != json!(false) {
        return Err(format!("user show says {MADE} is disabled after accounts.enable: {reads}"));
    }
    Ok(())
}

/// Step 5: a key that ssh-keygen made in the guest is added, and its line
/// is in authorized_keys, which lab-made owns with mode 600 in a folder of
/// mode 700; user show lists it with ssh-keygen's fingerprint. Removing it
/// takes the line away, and removing it again is ACC-104.
fn keys_added_and_removed(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let path = format!("/api/1/accounts/{MADE}");
    let make = format!("rm -f {KEY} {KEY}.pub; ssh-keygen -q -t ed25519 -N '' -C lab-key -f {KEY}");
    test.ends_with(vm, &make, 0)?;
    let line = test.pane.output(vm, &format!("cat {KEY}.pub"))?.trim().to_string();
    if !line.starts_with("ssh-ed25519 ") || line.contains('\n') {
        return Err(format!("ssh-keygen made the public key {line:?}"));
    }
    let listed = test.pane.output(vm, &format!("ssh-keygen -l -f {KEY}.pub"))?;
    let fingerprint = fingerprint_in(&listed)
        .ok_or_else(|| format!("ssh-keygen -l printed {listed:?}, which has no fingerprint"))?;

    let added = test.change(&format!("{path}/keys/add"), &json!({"line": line}))?;
    if added["changed"] != json!(true) {
        return Err(format!("accounts.key-add says it changed nothing: {added}"));
    }
    let file = test.pane.output(vm, &format!("elevate cat /users/{MADE}/.ssh/authorized_keys"))?;
    if !file.lines().any(|one| one.trim() == line) {
        return Err(format!("authorized_keys does not have the key's line: {file:?}"));
    }
    let stat = format!(
        "elevate stat -c '%U %a' /users/{MADE}/.ssh/authorized_keys /users/{MADE}/.ssh"
    );
    let modes = test.pane.output(vm, &stat)?;
    let wanted = [format!("{MADE} 600"), format!("{MADE} 700")];
    if modes.lines().map(str::trim).collect::<Vec<_>>() != wanted {
        return Err(format!("the key's file and folder are {modes:?}, not {wanted:?}"));
    }
    let reads = shown(test, MADE)?;
    if !lists_key(&reads, &fingerprint) {
        return Err(format!("user show does not list {fingerprint}: {}", reads["keys"]));
    }

    let take = json!({"fingerprint": fingerprint});
    test.change(&format!("{path}/keys/remove"), &take)?;
    let file = test.pane.output(vm, &format!("elevate cat /users/{MADE}/.ssh/authorized_keys"))?;
    if file.contains(&line) {
        return Err(format!("authorized_keys has the key's line after its removal: {file:?}"));
    }
    if lists_key(&shown(test, MADE)?, &fingerprint) {
        return Err("user show lists the key after its removal".into());
    }
    test.unlocked()?;
    test.post(Who::Alice, &format!("{path}/keys/remove"), &take)?.refused(
        "a key that is gone",
        422,
        "ACC-104",
    )?;
    Ok(())
}

/// Step 6: lab-made makes a token of its own, which reads; user show lists
/// it; accounts.token-revoke takes it away, so that its next call is
/// ADM-115, and a second revoke is ACC-104.
fn a_token_revoked(test: &mut Test, made: &mut Browser) -> Result<(), String> {
    let path = format!("/api/1/accounts/{MADE}");
    let token = token_of(&test.console, made, "lab-token", SECOND)?;
    test.console
        .with_token(&token, "GET", "/api/1/account/show", None)?
        .must("the token reading its account", 200)?;
    let reads = shown(test, MADE)?;
    let listed = reads["tokens"].as_array();
    if !listed.is_some_and(|all| all.iter().any(|one| one["name"] == "lab-token")) {
        return Err(format!("user show does not list lab-token: {}", reads["tokens"]));
    }

    let revoke = json!({"token": "lab-token"});
    test.change(&format!("{path}/tokens/remove"), &revoke)?;
    test.console.with_token(&token, "GET", "/api/1/account/show", None)?.refused(
        "a token that was revoked",
        401,
        "ADM-115",
    )?;
    test.unlocked()?;
    test.post(Who::Alice, &format!("{path}/tokens/remove"), &revoke)?.refused(
        "a token that is gone",
        422,
        "ACC-104",
    )?;
    Ok(())
}

/// Step 7: lab-made adds an authenticator app, and its next sign-in asks
/// for a step. accounts.steps-reset takes the step away, and the sign-in
/// after it asks for none.
fn a_second_step_reset(test: &mut Test, made: &mut Browser) -> Result<(), String> {
    let path = format!("/api/1/accounts/{MADE}");
    let begin = json!({"proof": SECOND});
    let begun = test.console.post(made, "/api/1/account/second-step/app/begin", &begin)?;
    let uri = begun.must("beginning an app", 200)?.json()?["uri"].as_str().map(String::from);
    let uri = uri.ok_or("no URI for the app")?;
    let mut app = App { secret: secret_of(&uri)?, last: None };
    let code = test.console.next_code(&mut app)?;
    test.console
        .post(made, "/api/1/account/second-step/app/confirm", &json!({"code": code}))?
        .must("confirming the app", 200)?;
    if shown(test, MADE)?["steps"]["app"] != json!(true) {
        return Err("user show does not say lab-made has an app".into());
    }

    // The password alone is not enough now.
    let mut again = Browser::default();
    let asked = test.console.sign_in(&mut again, MADE, SECOND)?.json()?;
    let ticket = asked["step"]["ticket"].as_str().filter(|ticket| !ticket.is_empty());
    if ticket.is_none() || again.cookie.is_some() {
        return Err(format!("the sign-in with the app on is {asked}, not a step to give"));
    }

    test.change(&format!("{path}/second-step/reset"), &json!({}))?;
    if shown(test, MADE)?["steps"]["app"] != json!(false) {
        return Err("user show says lab-made has an app after accounts.steps-reset".into());
    }
    let mut free = Browser::default();
    let signed = test.console.sign_in(&mut free, MADE, SECOND)?;
    if signed.body != r#"{"done":true}"# || free.cookie.is_none() {
        return Err(format!("the sign-in after the reset is {} with no cookie", signed.body));
    }
    test.console.read_session(&mut free)?;
    // The browser it replaces signs out, so that lab-made has one console
    // session for step 9.
    test.console.post(made, "/api/1/sign-out", &json!({}))?.must("lab-made's sign-out", 200)?;
    *made = free;
    Ok(())
}

/// Step 8: sshd is on, lab-made's key is added, and the guest's own client
/// signs in as lab-made on a pseudo-terminal that it keeps. accounts.show
/// lists the session as ssh, accounts.last says the sign-in came over SSH,
/// and accounts.end ends it, and it is gone.
fn a_session_at_the_machine(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let path = format!("/api/1/accounts/{MADE}");
    test.ends_with(vm, "elevate service sshd enable", 0)?;
    wait_for("sshd running", 60, || {
        let shown = test.read(Who::Alice, "/api/1/ssh")?;
        Ok(if shown["running"] == json!(true) { Ok(()) } else { Err(shown.to_string()) })
    })?;
    let line = test.pane.output(vm, &format!("cat {KEY}.pub"))?.trim().to_string();
    test.change(&format!("{path}/keys/add"), &json!({"line": line}))?;

    // The client runs in the background of the pane, and keeps the session
    // for a quarter of an hour unless it is ended.
    let client = format!(
        "ssh -tt -i {KEY} -o BatchMode=yes -o StrictHostKeyChecking=no -o \
         UserKnownHostsFile=/dev/null {MADE}@127.0.0.1 sleep 900 </dev/null >/dev/null 2>&1 &"
    );
    answer_prompt(vm, "$", &client)?;
    let session = wait_for("lab-made's session over ssh", 60, || {
        let reads = shown(test, MADE)?;
        Ok(session_of_kind(&reads, "ssh").ok_or_else(|| reads["sessions"].to_string()))
    })?;
    // authd records it as a sign-in over SSH, which the Security page's
    // trial of SSH lists, where a console terminal's is not one.
    test.unlocked()?;
    let last = test.read(Who::Alice, "/api/1/sign-ins/last")?;
    let row = last.as_array().into_iter().flatten().find(|row| row["name"] == json!(MADE));
    if row.map(|row| &row["sign_in"]["via"]) != Some(&json!("ssh")) {
        return Err(format!("lab-made's last sign-in is not over SSH: {last}"));
    }

    let end = json!({"session": session});
    test.change(&format!("{path}/end"), &end)?;
    wait_for("lab-made's session ended", 60, || {
        let reads = shown(test, MADE)?;
        Ok(match session_of_kind(&reads, "ssh") {
            None => Ok(()),
            Some(_) => Err(reads["sessions"].to_string()),
        })
    })?;
    test.unlocked()?;
    test.post(Who::Alice, &format!("{path}/end"), &end)?.refused(
        "a session that has ended",
        422,
        "ACC-104",
    )?;
    test.ends_with(vm, "elevate service sshd disable", 0)
}

/// Step 9: lab-made's console session, with an event stream and a terminal,
/// is in the sessions route for alice, unlocked, and for no one else. She
/// ends the terminal and then the session: the stream is told
/// by-administrator, the next call is ADM-107 with the words for it, a
/// second end is ADM-139, and the audit says who ended what.
fn a_console_session_of_another(
    vm: &mut Vm,
    test: &mut Test,
    made: &mut Browser,
) -> Result<(), String> {
    let events = test.console.events(made)?;
    let body = json!({"cols": SIZE.0, "rows": SIZE.1, "admin": false});
    let terminal = test.console.post(made, "/api/1/terminals", &body)?;
    let terminal = terminal.must("lab-made's terminal", 200)?.json()?["terminal"]["id"].clone();
    let Some(terminal) = terminal.as_u64() else {
        return Err("the answer for lab-made's terminal has no id".into());
    };

    let sessions = format!("/api/1/accounts/{MADE}/sessions");
    test.unlocked()?;
    let list = test.read(Who::Alice, &sessions)?;
    let ids: Vec<&str> = list["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|session| session["this"] == json!(false))
        .filter_map(|session| session["id"].as_str())
        .collect();
    let [id] = ids[..] else {
        return Err(format!("lab-made has {} console sessions, not one: {list}", ids.len()));
    };
    let id = id.to_string();
    if list["terminals"][0]["id"] != json!(terminal) || list["terminals"].as_array().map(Vec::len) != Some(1) {
        return Err(format!("lab-made's terminals are {}, not terminal {terminal}", list["terminals"]));
    }

    // alice sees her own as hers, the reader sees none of another's, and
    // alice locked is no one to see them.
    let hers = test.read(Who::Alice, "/api/1/accounts/alice/sessions")?;
    let own = hers["sessions"].as_array().into_iter().flatten().find(|one| one["this"] == json!(true));
    let own = own.and_then(|one| one["id"].as_str()).ok_or("alice's list has no session in use")?;
    let own = own.to_string();
    test.post(Who::Alice, &format!("/api/1/accounts/alice/sessions/{own}/end"), &json!({}))?
        .refused("the session in use", 409, "ADM-136")?;
    test.get(Who::Reader, &sessions)?.refused("the reader's list", 403, "ADM-122")?;
    test.post(Who::Reader, &format!("{sessions}/{id}/end"), &json!({}))?
        .refused("the reader's end", 403, "ADM-122")?;
    test.locked()?;
    test.get(Who::Alice, &sessions)?.refused("the locked list", 403, "ADM-106")?;
    test.post(Who::Alice, &format!("/api/1/terminals/{terminal}/end"), &json!({}))?
        .refused("the locked end of a terminal", 403, "ADM-106")?;

    // The terminal, then the session.
    test.unlocked()?;
    test.post(Who::Alice, &format!("/api/1/terminals/{terminal}/end"), &json!({}))?
        .must("alice ending lab-made's terminal", 200)?;
    let list = test.read(Who::Alice, &sessions)?;
    if list["terminals"] != json!([]) {
        return Err(format!("lab-made has terminals after alice ended it: {}", list["terminals"]));
    }
    let end = format!("{sessions}/{id}/end");
    let ended = test.post(Who::Alice, &end, &json!({}))?.must("alice ending lab-made's session", 200)?;
    if ended.body != r#"{"done":true}"# {
        return Err(format!("the end of the session was answered {}", ended.body));
    }
    let reason = events.ended(30)?;
    if reason != "by-administrator" {
        return Err(format!("the stream of lab-made's session was told {reason:?}"));
    }
    let next = test.console.get(made, "/api/1/session")?;
    let next = next.refused("the session an administrator ended", 401, "ADM-107")?;
    if !next.detail().contains("an administrator ended this session") {
        return Err(format!("the ended session was refused for {:?}", next.detail()));
    }
    test.post(Who::Alice, &end, &json!({}))?.refused("a session that has ended", 404, "ADM-139")?;

    // admind's audit has who ended what, which logd gives a while after.
    audit_has(
        vm,
        test,
        &[&format!("alice ended the console session of {MADE}"), "alice ended the console terminal"],
    )
}

/// Waits up to 90 seconds for the audit, which `log --audit` reads, to have
/// a line with each of `words`: logd takes a while to give a service's lines
/// to the log.
fn audit_has(vm: &mut Vm, test: &Test, words: &[&str]) -> Result<(), String> {
    elevated(vm)?;
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let audit = test.pane.output(vm, "elevate log --audit | grep -e 'ended the console'")?;
        if words.iter().all(|word| audit.contains(word)) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "the audit has no line with each of {words:?}: {:?}",
                tail(&audit, 600)
            ));
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}

/// Step 10: alice's own account is ADM-136 to disable, remove, demote or end
/// a session of, from her session and from a token of hers, and nothing
/// ran. At the serial console user itself refuses the plain command, and
/// authd, which has no one to name, refuses it as ACC-102. Last, accounts.
/// remove takes lab-made away with its home.
fn the_own_account(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    test.unlocked()?;
    let token = token_of(&test.console, &mut test.alice, "lab-own", ALICE_PASSWORD)?;
    let asks = [
        ("disable", json!({})),
        ("remove", json!({"home": "keep"})),
        ("set", json!({"admin": false})),
        ("end", json!({"session": "1-1"})),
    ];
    for (what, body) in &asks {
        let path = format!("/api/1/accounts/alice/{what}");
        test.post(Who::Alice, &path, body)?.refused(&format!("{path} by session"), 409, "ADM-136")?;
        test.console
            .with_token(&token, "POST", &path, Some(body))?
            .refused(&format!("{path} by token"), 409, "ADM-136")?;
    }
    let reads = shown(test, "alice")?;
    if reads["account"]["disabled"] != json!(false) || reads["account"]["admin"] != json!(true) {
        return Err(format!("alice's account was changed by a refused change: {reads}"));
    }
    let own = json!({"name": "lab-own"});
    test.post(Who::Alice, "/api/1/account/tokens/remove", &own)?.must("removing her token", 200)?;

    // At the serial console: user's own refusal first.
    let said = test.pane.output(vm, "elevate user disable --yes alice 2>&1")?;
    if !said.contains("another administrator disables it") {
        return Err(format!("user disable --yes alice was answered {said:?}"));
    }
    let said = test.pane.output(
        vm,
        "elevate sh -c 'unset SUDO_USER; user disable --yes alice' 2>&1",
    )?;
    if !said.contains("(ACC-102)") || !said.contains("no administrator") {
        return Err(format!("authd answered the last administrator's disabling with {said:?}"));
    }
    if shown(test, "alice")?["account"]["disabled"] != json!(false) {
        return Err("alice is disabled after authd refused it".into());
    }

    // lab-made goes, with its home.
    let gone = format!("/api/1/accounts/{MADE}");
    test.change(&format!("{gone}/remove"), &json!({"home": "delete"}))?;
    test.unlocked()?;
    test.get(Who::Alice, &gone)?.refused("a removed account", 422, "ACC-101")?;
    test.ends_with(vm, &format!("elevate test -e /users/{MADE}"), 1)?;
    let list = test.read(Who::Alice, "/api/1/accounts")?;
    if account_in(&list, MADE).is_ok() {
        return Err(format!("the list has {MADE} after accounts.remove: {list}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_is_found_by_its_name_in_the_list() {
        let list = json!([{"name": "alice", "admin": true}, {"name": "lab-made", "admin": false}]);
        assert_eq!(account_in(&list, "lab-made").map(|row| &row["admin"]), Ok(&json!(false)));
        assert!(account_in(&list, "bob").is_err());
        assert!(account_in(&json!({}), "alice").is_err());
    }

    #[test]
    fn a_session_is_found_by_its_kind() {
        let shown = json!({"sessions": [
            {"id": "12-100", "kind": "serial"},
            {"id": "41-2100", "kind": "ssh"},
            {"id": "43-2200", "kind": "ssh"},
        ]});
        assert_eq!(session_of_kind(&shown, "ssh").as_deref(), Some("41-2100"));
        assert_eq!(session_of_kind(&shown, "screen"), None);
        assert_eq!(session_of_kind(&json!({"sessions": null}), "ssh"), None);
        assert_eq!(session_of_kind(&json!({}), "ssh"), None);
    }

    #[test]
    fn the_fingerprint_is_the_word_ssh_keygen_gives() {
        let line = "256 SHA256:ZkAslGjFiUHdGf/WUL8rQvkib4PTvQatUV0OUQSncCA lab-key (ED25519)\n";
        assert_eq!(
            fingerprint_in(line).as_deref(),
            Some("SHA256:ZkAslGjFiUHdGf/WUL8rQvkib4PTvQatUV0OUQSncCA")
        );
        assert_eq!(fingerprint_in("no key here"), None);
        assert_eq!(fingerprint_in(""), None);
    }

    #[test]
    fn a_key_is_listed_by_its_fingerprint() {
        let shown = json!({"keys": [{"fingerprint": "SHA256:a"}, {"fingerprint": "SHA256:b"}]});
        assert!(lists_key(&shown, "SHA256:b"));
        assert!(!lists_key(&shown, "SHA256:c"));
        assert!(!lists_key(&json!({"keys": null}), "SHA256:a"));
        assert!(!lists_key(&json!({}), "SHA256:a"));
    }
}
