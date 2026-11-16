// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Updates and Apps pages of the web console, Dukovo Admin: the
//! updates' reads and changes and the apps' (ADR 0515), on an installed
//! system's disk with the test update server of the host as the machine's
//! update server, a test catalog of two versions of an app, and the files
//! of two test bundles, one that offers a socket and one that uses it.
//! alice, the owner, is an administrator; lab-reader may use the console
//! and is none.
//!
//! What a person reads. updates.settings, updates.checks and updates.last
//! are what `update settings --json`, `update checks --json` and `update
//! last --json` print in alice's pane, for alice and for lab-reader. No
//! check is kept yet, so the last one is null, and the command ends with
//! status 3.
//!
//! How the machine updates. updates.set with nothing to change, a time
//! that is none, an `apps` that is no word, and a restart time for a
//! machine that does not update by itself are refused at the check. A
//! restart time on a machine that does not update by itself, which only the
//! settings know, is the command's own words. `check` and `apps` together
//! are written to updates.toml, which keeps its comment and the other
//! settings, and bundled's own file says that everyone's apps update by
//! themselves; asked again, nothing changes; and the undo puts both back.
//! A restart time, `null` for none, and `unattended` follow. updates.channel
//! follows the channel of the one it is told.
//!
//! A check. updates.check is `update check --json`, and offers the
//! stable channel's release for a canary, by way of the bridge release it
//! updates from. A check that root makes keeps its document, which
//! updates.last then answers, and one that a person makes does not. With
//! the server not there, the check says in `unchecked` that the system could
//! not be checked and ends with status 0, and the record of the last check
//! is as it was.
//!
//! What is checked after an update. updates.check-add of a page, with its
//! name in use, an address that is not one, a page and a service together
//! and the reader refused; updates.check-add of a service; updates.essential
//! on and off, each twice, the second changing nothing; and
//! updates.check-remove, which a name that is not there refuses. Each
//! change shows in health.toml, which keeps its comment, and in `update
//! checks --json`.
//!
//! A hold. updates.hold of the system holds it at the release the stable
//! channel offers, which the check then says is held because it was asked
//! for and not because the health check fell back from it, and updates.unhold
//! lets it go; each twice, the second changing nothing.
//!
//! The apps. The test catalog is added and the image's own turned off, so
//! that nothing reaches the network. diary 1.0.0 and the two test bundles
//! are installed for everyone from their files. apps.list is `bundle list
//! --json`; apps.show of the one that offers a socket has its license, its
//! service, the socket it offers and the app that uses it, as `bundle info
//! --json` prints them, and the app that uses it says what it uses. A name
//! that is no app's ID is ADM-125, and an ID that is none installed ADM-123.
//! apps.hold and apps.unhold, each twice, change the list's `held`;
//! apps.update of diary is a task that brings it to 1.1.0, apps.rollback
//! a task that takes it back to 1.0.0 and holds it there, and apps.update-all
//! a task that brings it up again once it is let go. apps.remove with
//! `keep` is a task that takes diary away and leaves its data in alice's
//! folder, and a word that is neither `keep` nor `purge` is ADM-125.
//!
//! An update. updates.apply of the system is a task that stages the bridge
//! release in the other slot, which updates.slots, `sysupdate status
//! --json` as root, shows; updates.unstage leaves the slots as they were.
//!
//! The journal and who may. The rows of the changes made above name
//! updates.toml, health.toml or no file, as each does, and the task of the
//! update has its own row. A reader is ADM-122 and a locked administrator
//! ADM-106 for every action that needs an administrator, before anything
//! runs; what a person reads of their own is open to both.

use std::path::Path;

use serde_json::{Value, json};

use super::super::console_client::{Browser, Console, Pane, elevated};
use super::super::*;
use super::admin_backups::{READER, READER_PASSWORD, Test, Who};
use super::update_online::Server;

/// The test catalog, which Spark mounts as the first disk after the
/// installed system's, and the files of the services' test bundles, as the
/// second.
const DISKS: &[Disk] =
    &[update_site_disk(16 * 1024, "UPDATES"), service_files_disk(8 * 1024, "SERVICES")];

/// Where Spark mounts the catalog's disk.
const CATALOG: &str = "/volumes/vda";

/// Where Spark mounts the bundles' disk.
const SERVICES: &str = "/volumes/vdb";

/// How the machine updates, which `update set` writes.
const SETTINGS: &str = "/system/settings/sysupdate/updates.toml";

/// What the machine checks after an update, which `update checks` writes.
const HEALTH: &str = "/system/settings/sysupdate/health.toml";

/// bundled's list of the places whose apps update by themselves.
const AUTOMATIC: &str = "/system/state/bundled/automatic";

/// The app of the catalog the update steps use.
const DIARY: &str = "org.example.diary";

/// The test.
pub const CASE: Case = Case {
    name: "admin-updates",
    about: "The web console's Updates and Apps pages on an installed system whose update server is the host's test server: updates.settings, updates.checks and updates.last are what their commands print, for alice and lab-reader, with no check kept yet; updates.set is refused for nothing to change, a bad time, a word that is none and a restart time without updating by itself, and with check and apps writes updates.toml with its comment and tells bundled, then changes nothing the second time and is put back by its undo; updates.check offers the release of the stable channel by its bridge, root's check is kept for updates.last and a person's is not, and with the server away unchecked names the system and the status is 0; updates.check-add, updates.essential and updates.check-remove show in health.toml and update checks, each change run twice with the second changing nothing; updates.hold holds the system as asked and updates.unhold lets it go; apps.list, apps.show with its license, service, socket and user, apps.hold, apps.unhold, apps.update, apps.rollback, apps.update-all and apps.remove of an app installed from the test catalog, the tasks finishing and the data staying; updates.apply stages the bridge release as a task and updates.unstage takes it back; the journal's rows name the files; a reader is ADM-122 and a locked administrator ADM-106 for what needs an administrator (ADR 0515)",
    firmware: Firmware::Uefi,
    // The installed image keeps the slots the update is staged in.
    media: Media::Disk,
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: admin_updates,
    disks: DISKS,
    second: None,
};

fn admin_updates(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let dir = Path::new("out/test").join(TEST_SERVER);
    let before_the_pull = dir.join(TEST_SERVER_BEFORE_PULL);
    let server = Server::start(&before_the_pull)?;
    let (bridge, year) = (test_server_version("bridge"), test_server_version("year"));

    // The installed image's start, which has no report of the user test
    // programs that the plain image's has.
    owner_made(vm)?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    vm.expect_anywhere("admind: answers at ")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vda at {CATALOG}"))?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {SERVICES}"))?;
    let mut test = Test {
        console: Console::open(vm)?,
        pane: Pane::open()?,
        alice: Browser::default(),
        reader: Browser::default(),
    };

    // alice's password once for this pane, then the reader, who is in group
    // console and no administrator.
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

    the_machine_set_up(vm, &mut test, &server)?;
    what_a_person_reads(vm, &mut test)?;
    how_the_machine_updates(vm, &mut test)?;
    a_check(vm, &mut test, &server, &dir, (bridge.as_str(), year.as_str()))?;
    what_is_checked_after_an_update(vm, &mut test)?;
    a_hold(vm, &mut test, &year)?;
    the_channel(vm, &mut test)?;
    the_apps(vm, &mut test)?;
    an_update(vm, &mut test, &bridge)?;
    the_journal(&mut test)?;
    what_needs_an_administrator(&mut test)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The text of `file`, as root.
fn text_of(test: &Test, vm: &mut Vm, file: &str) -> Result<String, String> {
    test.pane.output(vm, &format!("elevate cat {file}"))
}

/// The text of `file`, as root, or nothing when there is no such file.
fn text_of_any(test: &Test, vm: &mut Vm, file: &str) -> Result<String, String> {
    test.pane.output(vm, &format!("elevate sh -c 'cat {file} 2>/dev/null; true'"))
}

/// An error naming `what` unless `text` has each of `needles`.
fn has(what: &str, text: &str, needles: &[&str]) -> Result<(), String> {
    match needles.iter().find(|needle| !text.contains(**needle)) {
        Some(needle) => Err(format!("{what} lacks {needle:?}: {text:?}")),
        None => Ok(()),
    }
}

/// An error naming `what` if `text` has any of `needles`.
fn has_none(what: &str, text: &str, needles: &[&str]) -> Result<(), String> {
    match needles.iter().find(|needle| text.contains(**needle)) {
        Some(needle) => Err(format!("{what} still has {needle:?}: {text:?}")),
        None => Ok(()),
    }
}

/// What the route answers and what the command prints in alice's pane are
/// the same JSON: the answer.
fn same_as_command(
    vm: &mut Vm,
    test: &mut Test,
    route: &str,
    command: &str,
) -> Result<Value, String> {
    let said = test.read(Who::Alice, route)?;
    let printed = test.printed(vm, command)?;
    if said != printed {
        return Err(format!("{route} answers {said}, and `{command}` printed {printed}"));
    }
    Ok(said)
}

/// A change at `path` with `body`, as alice, which says whether it changed
/// anything and is `wanted`: the answer.
fn changes(test: &mut Test, path: &str, body: &Value, wanted: bool) -> Result<Value, String> {
    let answer = test.change(path, body)?;
    if answer["changed"] != json!(wanted) {
        return Err(format!("{path} with {body} says changed {}, not {wanted}", answer["changed"]));
    }
    Ok(answer)
}

/// A change that alice asks for and the check or the command refuses, with
/// `id`, `status` and words that include `words`.
fn is_refused(
    test: &mut Test,
    path: &str,
    body: &Value,
    (status, id): (u16, &str),
    words: &str,
) -> Result<(), String> {
    test.unlocked()?;
    let reply = test.post(Who::Alice, path, body)?.refused(path, status, id)?;
    if !reply.detail().contains(words) {
        let said = reply.detail();
        return Err(format!("{path} with {body} is refused with {said:?}, not {words:?}"));
    }
    Ok(())
}

/// The row of `list`, the answer of `bundle list --json`, that is the app
/// `id` of everyone's.
fn app_row(list: &Value, id: &str) -> Option<Value> {
    list["bundles"]
        .as_array()?
        .iter()
        .find(|row| row["id"] == id && row["for_everyone"] == true)
        .cloned()
}

/// The offer `item` of `check`, the answer of `update check --json`.
fn offer<'a>(check: &'a Value, item: &str) -> Option<&'a Value> {
    check["offers"].as_array()?.iter().find(|offer| offer["item"] == item)
}

/// The machine set up for the steps: the host's update server, which a
/// canary takes the stable channel's release from on its first day, a
/// health check that proves an update in 10 seconds, each file with a
/// comment that the changes must keep, and the test catalog in place of
/// the image's own, which would look for apps on the network.
fn the_machine_set_up(vm: &mut Vm, test: &mut Test, server: &Server) -> Result<(), String> {
    elevated(vm)?;
    let port = server.port;
    test.ends_with(
        vm,
        &format!(
            r##"elevate sh -c 'printf "# kept\nserver = \"http://10.0.2.2:{port}/system\"\ncanary = true\n" > {SETTINGS}'"##
        ),
        0,
    )?;
    test.ends_with(
        vm,
        &format!(
            r##"elevate sh -c 'printf "# kept\nessential = [\"logd\", \"authd\"]\nstable = 10\n" > {HEALTH}'"##
        ),
        0,
    )?;
    let add =
        format!("elevate bundle catalog add test file://{CATALOG}/catalog {CATALOG}/root.json");
    test.ends_with(vm, &add, 0)?;
    test.ends_with(vm, "elevate bundle catalog remove dukovo", 0)
}

/// What alice and the reader read of how the machine updates, what it
/// checks after an update and the last check.
fn what_a_person_reads(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    for (route, command) in [
        ("/api/1/updates/settings", "update settings --json"),
        ("/api/1/updates/checks", "update checks --json"),
        ("/api/1/updates", "update last --json"),
    ] {
        let alice = same_as_command(vm, test, route, command)?;
        let reader = test.read(Who::Reader, route)?;
        if reader != alice {
            return Err(format!("{route} is {reader} for the reader, and {alice} for alice"));
        }
    }
    // No check is kept yet: null, and the command's status 3.
    let last = test.read(Who::Reader, "/api/1/updates")?;
    if !last.is_null() {
        return Err(format!("the last check is {last} before any is kept, not null"));
    }
    test.ends_with(vm, "update last --json", 3)?;
    // What the setup wrote, with the defaults that it did not.
    let settings = test.read(Who::Reader, "/api/1/updates/settings")?;
    let wanted = [
        ("check", json!("notify")),
        ("apps", json!("notify")),
        ("metered", json!("wait")),
        ("unattended", json!(false)),
        ("restart", json!(null)),
        ("canary", json!(true)),
        ("apps_automatic", json!(false)),
        ("apps_disagree", json!(false)),
    ];
    for (key, value) in wanted {
        if settings[key] != value {
            let was = &settings[key];
            return Err(format!("{key} of the settings is {was}, not {value}: {settings}"));
        }
    }
    let checks = test.read(Who::Reader, "/api/1/updates/checks")?;
    if checks["essential"] != json!(["logd", "authd"])
        || checks["stable"] != json!(10)
        || checks["checks"] != json!([])
    {
        return Err(format!("the checks after an update are {checks}"));
    }
    Ok(())
}

/// `update set` through its route: refused at the check, refused by the
/// command, then written, again, and put back.
fn how_the_machine_updates(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    elevated(vm)?;
    let path = "/api/1/updates/settings";
    for (body, words) in [
        (json!({}), "Say what to change"),
        (json!({"restart": "25:00"}), "HH:MM"),
        (json!({"restart": "3:30"}), "HH:MM"),
        (json!({"apps": "sometimes"}), "apps is notify or auto"),
        (json!({"unattended": false, "restart": "03:30"}), "unattended is off"),
    ] {
        is_refused(test, path, &body, (400, "ADM-125"), words)?;
    }
    // A restart time needs a machine that updates by itself, which only the
    // settings know: the command says so.
    is_refused(test, path, &json!({"restart": "03:30"}), (422, "ADM-123"), "unattended")?;
    test.get(Who::Reader, path)?.must("the reader's read of the settings", 200)?;
    test.post(Who::Reader, path, &json!({"check": "off"}))?.refused(
        "the reader's change",
        403,
        "ADM-122",
    )?;
    // Nothing was written by any of those.
    let text = text_of(test, vm, SETTINGS)?;
    has_none("updates.toml after the refusals", &text, &["check =", "apps =", "restart ="])?;

    // Both of the setting and bundled's.
    let both = json!({"check": "download", "apps": "auto"});
    changes(test, path, &both, true)?;
    let text = text_of(test, vm, SETTINGS)?;
    has(
        "updates.toml",
        &text,
        &["# kept", "canary = true", "check = \"download\"", "apps = \"auto\""],
    )?;
    let automatic = text_of_any(test, vm, AUTOMATIC)?;
    if !automatic.lines().any(|line| line.starts_with("everyone")) {
        return Err(format!(
            "bundled's {AUTOMATIC} does not say everyone's apps update: {automatic:?}"
        ));
    }
    let settings = same_as_command(vm, test, path, "update settings --json")?;
    if settings["check"] != json!("download")
        || settings["apps"] != json!("auto")
        || settings["apps_automatic"] != json!(true)
        || settings["apps_disagree"] != json!(false)
    {
        return Err(format!("the settings after the change are {settings}"));
    }
    // Asked again, it changes nothing.
    changes(test, path, &both, false)?;
    // The undo puts both back, and the file keeps its comment.
    changes(test, path, &json!({"check": "notify", "apps": "notify"}), true)?;
    let text = text_of(test, vm, SETTINGS)?;
    has("updates.toml", &text, &["# kept", "check = \"notify\"", "apps = \"notify\""])?;
    let automatic = text_of_any(test, vm, AUTOMATIC)?;
    has_none(AUTOMATIC, &automatic, &["everyone"])?;
    same_as_command(vm, test, path, "update settings --json")?;

    // A restart time, none, and the machine that no longer updates by
    // itself, which takes the time with it.
    changes(test, path, &json!({"unattended": true, "restart": "03:30"}), true)?;
    let settings = test.read(Who::Alice, path)?;
    if settings["unattended"] != json!(true) || settings["restart"] != json!("03:30") {
        return Err(format!("the settings after a restart time are {settings}"));
    }
    changes(test, path, &json!({"restart": null}), true)?;
    if test.read(Who::Alice, path)?["restart"] != json!(null) {
        return Err("a null restart did not take the restart time away".into());
    }
    changes(test, path, &json!({"restart": "04:15"}), true)?;
    changes(test, path, &json!({"unattended": false}), true)?;
    let settings = same_as_command(vm, test, path, "update settings --json")?;
    if settings["unattended"] != json!(false) || settings["restart"] != json!(null) {
        return Err(format!("the settings after unattended off are {settings}"));
    }
    let text = text_of(test, vm, SETTINGS)?;
    has("updates.toml", &text, &["# kept", "canary = true"])?;
    has_none("updates.toml", &text, &["restart"])
}

/// The check the console makes, as a person's, and the one root makes,
/// which is kept; then with the server away.
fn a_check(
    vm: &mut Vm,
    test: &mut Test,
    server: &Server,
    dir: &Path,
    (bridge, year): (&str, &str),
) -> Result<(), String> {
    elevated(vm)?;
    let said = test.read(Who::Alice, "/api/1/updates/check")?;
    let printed = test.printed(vm, "update check --json")?;
    if said["offers"] != printed["offers"] {
        let (route, command) = (&said["offers"], &printed["offers"]);
        return Err(format!("updates.check offers {route}, and the command {command}"));
    }
    let system = offer(&said, "system")
        .ok_or_else(|| format!("the check has no offer of the system: {said}"))?;
    let wanted = [
        ("from", json!(VERSION)),
        ("to", json!(year)),
        ("state", json!("update")),
        ("bridge", json!(bridge)),
    ];
    for (key, value) in wanted {
        if system[key] != value {
            let was = &system[key];
            return Err(format!("the system's offer has {key} {was}, not {value}: {system}"));
        }
    }
    let reader = test.read(Who::Reader, "/api/1/updates/check")?;
    if reader["offers"] != said["offers"] {
        let (theirs, hers) = (&reader["offers"], &said["offers"]);
        return Err(format!("the reader's check offers {theirs}, alice's {hers}"));
    }
    // A person's check keeps nothing.
    if !test.read(Who::Alice, "/api/1/updates")?.is_null() {
        return Err("a person's check was kept as the last".into());
    }
    // Root's is kept, with who made it, for everyone to read.
    test.ends_with(vm, "elevate update check --json > /dev/null", 0)?;
    let last = same_as_command(vm, test, "/api/1/updates", "update last --json")?;
    if last["by"] != json!("update check") || last["checked_ms"].as_u64().is_none() {
        return Err(format!("the last check is {last}"));
    }
    if last["updates"]["offers"] != said["offers"] {
        let (kept, now) = (&last["updates"]["offers"], &said["offers"]);
        return Err(format!("the kept check offers {kept}, not {now}"));
    }
    if test.read(Who::Reader, "/api/1/updates")? != last {
        return Err("the reader does not read the last check alice reads".into());
    }

    // With the server away, the system is unchecked, with the reason, and
    // the status is 0; the last check kept is as it was.
    server.serve(&dir.join("not-there"));
    let down = test.read(Who::Alice, "/api/1/updates/check")?;
    let unchecked =
        down["unchecked"].as_array().into_iter().flatten().find(|part| part["item"] == "system");
    if unchecked.is_none_or(|part| part["why"].as_str().is_none_or(str::is_empty)) {
        return Err(format!("the check with the server away does not name the system: {down}"));
    }
    if offer(&down, "system").is_some() {
        return Err(format!("the check with the server away still offers the system: {down}"));
    }
    test.ends_with(vm, "update check --json > /dev/null", 0)?;
    if test.read(Who::Alice, "/api/1/updates")? != last {
        return Err("the last check changed when a person's was made".into());
    }
    server.serve(&dir.join(TEST_SERVER_BEFORE_PULL));
    Ok(())
}

/// What the machine checks after an update, changed through the console.
fn what_is_checked_after_an_update(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    elevated(vm)?;
    let add = "/api/1/updates/checks";
    let page = json!({"name": "web site", "url": "https://localhost/", "required": true});
    for (body, words) in [
        (json!({"name": "x", "url": "ftp://a/"}), "address that starts with http://"),
        (json!({"name": "x", "url": "https://"}), "address that starts with http://"),
        (json!({"name": "x", "url": "https://a/", "service": "nginx"}), "one of the two"),
        (json!({"name": "x"}), "Say what to check"),
        (json!({"name": "-x", "url": "https://a/"}), "starts with a letter or a digit"),
        (json!({"name": "x", "url": "https://a/", "timeout": 601}), "from 1 to 600 seconds"),
    ] {
        is_refused(test, add, &body, (400, "ADM-125"), words)?;
    }
    test.post(Who::Reader, add, &page)?.refused("the reader's add", 403, "ADM-122")?;

    changes(test, add, &page, true)?;
    let text = text_of(test, vm, HEALTH)?;
    has(
        "health.toml",
        &text,
        &["# kept", "stable = 10", "name = \"web site\"", "required = true", "https://localhost/"],
    )?;
    let shown = same_as_command(vm, test, add, "update checks --json")?;
    let row = shown["checks"][0].clone();
    if shown["checks"].as_array().map(Vec::len) != Some(1)
        || row["name"] != json!("web site")
        || row["kind"] != json!("url")
        || row["target"] != json!("https://localhost/")
        || row["required"] != json!(true)
    {
        return Err(format!("the checks after the add are {shown}"));
    }
    // The name is taken: the command's own words.
    is_refused(test, add, &page, (422, "ADM-123"), "a check is called web site already")?;

    // A service, which has no certificate to be insecure about.
    let service = json!({"name": "nginx check", "service": "nginx"});
    changes(test, add, &service, true)?;
    let shown = test.read(Who::Alice, add)?;
    let row = shown["checks"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["name"] == "nginx check"))
        .ok_or_else(|| format!("the checks lack the service check: {shown}"))?;
    if row["kind"] != json!("service")
        || row["target"] != json!("nginx")
        || row["required"] != json!(false)
    {
        return Err(format!("the service check is {row}"));
    }
    is_refused(
        test,
        add,
        &json!({"name": "svc", "service": "nginx", "insecure": true}),
        (400, "ADM-125"),
        "insecure is for a url",
    )?;

    // A service that must run: on and off, each twice.
    let essential = "/api/1/updates/essential";
    let on = json!({"service": "lab-web", "on": true});
    let off = json!({"service": "lab-web", "on": false});
    changes(test, essential, &on, true)?;
    let text = text_of(test, vm, HEALTH)?;
    has("health.toml", &text, &["# kept", "\"lab-web\""])?;
    if test.read(Who::Alice, add)?["essential"] != json!(["logd", "authd", "lab-web"]) {
        return Err("the essential services after the add lack lab-web".into());
    }
    changes(test, essential, &on, false)?;
    changes(test, essential, &off, true)?;
    let text = text_of(test, vm, HEALTH)?;
    has("health.toml", &text, &["# kept", "\"logd\""])?;
    has_none("health.toml", &text, &["\"lab-web\""])?;
    changes(test, essential, &off, false)?;
    is_refused(
        test,
        essential,
        &json!({"service": "../x", "on": true}),
        (400, "ADM-125"),
        "A service's name has 1 to",
    )?;

    // The checks taken away, which a name that is not there refuses.
    for name in ["web%20site", "nginx%20check"] {
        changes(test, &format!("/api/1/updates/checks/{name}/remove"), &json!({}), true)?;
    }
    let text = text_of(test, vm, HEALTH)?;
    has("health.toml", &text, &["# kept", "stable = 10"])?;
    has_none("health.toml", &text, &["web site", "nginx check"])?;
    is_refused(
        test,
        "/api/1/updates/checks/web%20site/remove",
        &json!({}),
        (422, "ADM-123"),
        "no check is called web site",
    )?;
    let shown = same_as_command(vm, test, add, "update checks --json")?;
    if shown["checks"] != json!([]) {
        return Err(format!("the checks after the removals are {shown}"));
    }
    Ok(())
}

/// The system held back at the release on offer, as the person asked, and
/// let go.
fn a_hold(vm: &mut Vm, test: &mut Test, year: &str) -> Result<(), String> {
    elevated(vm)?;
    let hold = "/api/1/updates/system/hold";
    let unhold = "/api/1/updates/system/unhold";
    test.post(Who::Reader, hold, &json!({}))?.refused("the reader's hold", 403, "ADM-122")?;
    changes(test, hold, &json!({}), true)?;
    let text = text_of(test, vm, SETTINGS)?;
    has("updates.toml", &text, &["[hold]", &format!("system = \"{year}\"")])?;
    let settings = test.read(Who::Alice, "/api/1/updates/settings")?;
    if settings["hold"]["system"] != json!(year) || !settings["held"].is_null() {
        return Err(format!("the settings after the hold are {settings}"));
    }
    // The check says it is held because it was asked, not because the
    // health check fell back from it.
    let check = test.read(Who::Alice, "/api/1/updates/check")?;
    let system = offer(&check, "system").ok_or_else(|| format!("no offer of the system: {check}"))?;
    if system["state"] != json!("held")
        || system["held_because"] != json!("asked")
        || system["to"] != json!(year)
        || !system["held_at"].is_null()
    {
        return Err(format!("the system's offer once held is {system}"));
    }
    // The same release again changes nothing; letting it go does, once.
    changes(test, hold, &json!({}), false)?;
    changes(test, unhold, &json!({}), true)?;
    let settings = test.read(Who::Alice, "/api/1/updates/settings")?;
    if !settings["hold"]["system"].is_null() {
        return Err(format!("the system is held after the unhold: {settings}"));
    }
    let check = test.read(Who::Alice, "/api/1/updates/check")?;
    let state = offer(&check, "system").map(|offer| offer["state"].clone());
    if state != Some(json!("update")) {
        return Err(format!("the system's offer after the unhold is {state:?}"));
    }
    changes(test, unhold, &json!({}), false)?;
    // What a hold must not take with it: the server and the canary.
    let text = text_of(test, vm, SETTINGS)?;
    has("updates.toml", &text, &["canary = true", "server = \"http://10.0.2.2:"])
}

/// The channel the machine follows, changed and put back; it writes the
/// settings whole, as `sysupdate channel` does.
fn the_channel(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let path = "/api/1/updates/channel";
    for (body, words) in [
        (json!({"channel": "alpha"}), "channel is stable, beta or nightly"),
        (json!({"channel": "Beta"}), "channel is stable, beta or nightly"),
    ] {
        is_refused(test, path, &body, (400, "ADM-125"), words)?;
    }
    test.post(Who::Reader, path, &json!({"channel": "beta"}))?.refused(
        "the reader's channel",
        403,
        "ADM-122",
    )?;
    changes(test, path, &json!({"channel": "beta"}), true)?;
    let settings = same_as_command(vm, test, "/api/1/updates/settings", "update settings --json")?;
    if settings["channel"] != json!("beta") {
        return Err(format!("the channel after the change is {}", settings["channel"]));
    }
    changes(test, path, &json!({"channel": "beta"}), false)?;
    changes(test, path, &json!({"channel": "stable"}), true)?;
    if test.read(Who::Alice, "/api/1/updates/settings")?["channel"] != json!("stable") {
        return Err("the channel did not go back to stable".into());
    }
    Ok(())
}

/// A task as alice, which ends done, and the answer of the list of the
/// apps after it.
fn app_task(test: &mut Test, path: &str, body: &Value) -> Result<Value, String> {
    test.task(path, body, 300)?;
    test.read(Who::Alice, "/api/1/apps")
}

/// The apps: installed from their files, listed, shown, held, updated,
/// taken back, updated again and removed.
fn the_apps(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    elevated(vm)?;
    for (folder, file) in [
        (CATALOG, "org.example.diary-1.0.0.dkv"),
        (SERVICES, "org.example.echoprov-1.0.0.dkv"),
        (SERVICES, "org.example.echouser-1.0.0.dkv"),
    ] {
        test.ends_with(
            vm,
            &format!("cd {folder} && elevate bundle install --for-everyone --yes {file} && cd"),
            0,
        )?;
    }
    // The list is what bundle list --json prints; diary has its update.
    let list = same_as_command(vm, test, "/api/1/apps", "bundle list --json")?;
    if test.read(Who::Reader, "/api/1/apps")? != list {
        return Err("the reader's list of the apps is not alice's".into());
    }
    let diary = app_row(&list, DIARY).ok_or_else(|| format!("the list has no {DIARY}: {list}"))?;
    if diary["version"] != json!("1.0.0")
        || diary["held"] != json!(false)
        || diary["update"] != json!("1.1.0")
    {
        return Err(format!("{DIARY} in the list is {diary}"));
    }

    // The sheet of the app that offers a socket, which the other uses.
    let provider = "org.example.echoprov";
    let sheet = same_as_command(
        vm,
        test,
        &format!("/api/1/apps/{provider}"),
        &format!("bundle info {provider} --json"),
    )?;
    if sheet.get("license").is_none() {
        return Err(format!("the sheet has no license member: {sheet}"));
    }
    let services = sheet["service_list"].as_array().cloned().unwrap_or_default();
    let provided = sheet["provides"].as_array().cloned().unwrap_or_default();
    let used_by = sheet["used_by"].as_array().cloned().unwrap_or_default();
    if !services.iter().any(|service| service["name"] == "echoprov")
        || !provided.iter().any(|socket| socket["socket"] == "echo")
        || !used_by.iter().any(|user| user["id"] == "org.example.echouser")
    {
        return Err(format!(
            "the sheet of {provider} does not show its service, socket and user: {sheet}"
        ));
    }
    let user = test.read(Who::Reader, "/api/1/apps/org.example.echouser")?;
    let uses = user["uses"].as_array().cloned().unwrap_or_default();
    if !uses.iter().any(|used| used["socket"] == "org.example.echoprov/echo") {
        return Err(format!("the sheet of the app that uses the socket says no such use: {user}"));
    }
    test.get(Who::Alice, "/api/1/apps/Notes")?.refused("an ID that is none", 400, "ADM-125")?;
    test.get(Who::Alice, "/api/1/apps/org.example.none")?.refused(
        "an app that is not installed",
        422,
        "ADM-123",
    )?;

    // Held and let go, each twice, and what the list says of it.
    let held = |list: &Value| app_row(list, DIARY).map(|row| row["held"].clone());
    let hold = format!("/api/1/apps/{DIARY}/hold");
    let unhold = format!("/api/1/apps/{DIARY}/unhold");
    test.post(Who::Reader, &hold, &json!({}))?.refused("the reader's hold", 403, "ADM-122")?;
    changes(test, &hold, &json!({}), true)?;
    if held(&test.read(Who::Alice, "/api/1/apps")?) != Some(json!(true)) {
        return Err(format!("{DIARY} is not held after apps.hold"));
    }
    changes(test, &hold, &json!({}), false)?;
    changes(test, &unhold, &json!({}), true)?;
    if held(&test.read(Who::Alice, "/api/1/apps")?) != Some(json!(false)) {
        return Err(format!("{DIARY} is held after apps.unhold"));
    }
    changes(test, &unhold, &json!({}), false)?;

    // An update, a way back and an update of every app, each a task.
    let list = app_task(test, &format!("/api/1/apps/{DIARY}/update"), &json!({"yes": true}))?;
    let diary = app_row(&list, DIARY).ok_or("diary is gone after its update")?;
    if diary["version"] != json!("1.1.0") || !diary["update"].is_null() {
        return Err(format!("{DIARY} after its update is {diary}"));
    }
    let list = app_task(test, &format!("/api/1/apps/{DIARY}/rollback"), &json!({}))?;
    let diary = app_row(&list, DIARY).ok_or("diary is gone after its way back")?;
    if diary["version"] != json!("1.0.0") || diary["held"] != json!(true) {
        return Err(format!("{DIARY} after its way back is {diary}, not 1.0.0 and held"));
    }
    changes(test, &unhold, &json!({}), true)?;
    let list = app_task(test, "/api/1/apps/update", &json!({}))?;
    let diary = app_row(&list, DIARY).ok_or("diary is gone after the update of every app")?;
    if diary["version"] != json!("1.1.0") {
        return Err(format!("{DIARY} after the update of every app is {diary}"));
    }

    // Removed, its data kept: the folder and what is in it stay.
    let data = format!("/users/alice/app-data/{DIARY}");
    test.ends_with(vm, &format!("mkdir -p {data} && echo kept > {data}/days"), 0)?;
    let remove = format!("/api/1/apps/{DIARY}/remove");
    test.unlocked()?;
    test.post(Who::Alice, &remove, &json!({}))?.refused("a removal without data", 400, "ADM-108")?;
    for body in [json!({"data": "some"}), json!({"data": "KEEP"})] {
        test.post(Who::Alice, &remove, &body)?.refused(
            "a removal that is neither keep nor purge",
            400,
            "ADM-125",
        )?;
    }
    test.post(Who::Reader, &remove, &json!({"data": "keep"}))?.refused(
        "the reader's removal",
        403,
        "ADM-122",
    )?;
    let list = app_task(test, &remove, &json!({"data": "keep"}))?;
    if app_row(&list, DIARY).is_some() {
        return Err(format!("{DIARY} is in the list after apps.remove: {list}"));
    }
    let kept = text_of(test, vm, &format!("{data}/days"))?;
    if kept.trim() != "kept" {
        return Err(format!("the data of {DIARY} after its removal is {kept:?}, not what was kept"));
    }
    Ok(())
}

/// The system's update staged as a task, and not tried.
fn an_update(vm: &mut Vm, test: &mut Test, bridge: &str) -> Result<(), String> {
    elevated(vm)?;
    let before = test.read(Who::Alice, "/api/1/updates/slots")?;
    if !before["staged"].is_null() {
        return Err(format!("an update is staged before the test stages one: {before}"));
    }
    let (task, _) = test.task("/api/1/updates/system/apply", &json!({}), 900)?;
    if task["action"] != json!("updates.apply") {
        return Err(format!("the task of the update is {task}"));
    }
    // The other slot holds the bridge release, which starts at the next
    // start unless it is unstaged: sysupdate status says so, as root.
    let slots = status_as_root(vm, test)?;
    let staged = &slots["staged"];
    if staged.is_null() || slots["next"] == slots["default"] {
        return Err(format!("the slots after the update show nothing staged: {slots}"));
    }
    let in_a_slot = |slot: &Value| slot["version"] == json!(bridge);
    if !slots["slots"].as_array().into_iter().flatten().any(in_a_slot) {
        return Err(format!("no slot holds the bridge release {bridge}: {slots}"));
    }
    // Not tried: the default is the slot that runs.
    changes(test, "/api/1/updates/unstage", &json!({}), true)?;
    let slots = status_as_root(vm, test)?;
    if !slots["staged"].is_null() || slots["next"] != slots["running"] {
        return Err(format!("the slots after the unstage are {slots}"));
    }
    Ok(())
}

/// The slots, which updates.slots answers to an administrator and `sysupdate
/// status --json` prints as root.
fn status_as_root(vm: &mut Vm, test: &mut Test) -> Result<Value, String> {
    test.unlocked()?;
    same_as_command(vm, test, "/api/1/updates/slots", "elevate sysupdate status --json")
}

/// The rows the changes above made, which name their files, and the task
/// of the update, which has a row of its own.
fn the_journal(test: &mut Test) -> Result<(), String> {
    test.unlocked()?;
    let list = test.read(Who::Alice, "/api/1/changes?limit=200")?;
    let rows = list["changes"].as_array().cloned().unwrap_or_default();
    let of = |action: &str| -> Vec<Value> {
        rows.iter().filter(|row| row["action"] == action).cloned().collect()
    };
    for (action, file) in [
        ("updates.set", Some(SETTINGS)),
        ("updates.channel", Some(SETTINGS)),
        ("updates.hold", Some(SETTINGS)),
        ("updates.check-add", Some(HEALTH)),
        ("updates.essential", Some(HEALTH)),
        ("updates.check-remove", Some(HEALTH)),
        ("apps.hold", None),
        ("apps.update", None),
        ("updates.unstage", None),
    ] {
        let rows = of(action);
        if rows.is_empty() {
            return Err(format!("the journal has no row of {action}"));
        }
        for row in &rows {
            if row["who"] != json!("alice") || row["how"] != json!("console") {
                return Err(format!("the row of {action} is {row}"));
            }
        }
        // A change that moved something names its file, in a row of its own
        // that says it ended well.
        let names = |row: &Value, file: &str| {
            row["files"].as_array().is_some_and(|files| files.iter().any(|f| f["path"] == file))
        };
        match file {
            Some(file) => {
                if !rows.iter().any(|row| row["status"] == json!(0) && names(row, file)) {
                    return Err(format!(
                        "no row of {action} that ended well names {file}: {rows:?}"
                    ));
                }
            }
            None => {
                let some = |row: &Value| row["files"].as_array().is_some_and(|f| !f.is_empty());
                if rows.iter().any(some) {
                    return Err(format!("a row of {action} names files, as it has none: {rows:?}"));
                }
            }
        }
    }
    // The diff of the settings change says what it did.
    let set = of("updates.set")
        .into_iter()
        .find(|row| row["status"] == json!(0))
        .and_then(|row| row["id"].as_i64())
        .ok_or("no row of updates.set that ended well")?;
    let detail = test.read(Who::Alice, &format!("/api/1/changes/{set}"))?;
    let diff = detail["files"][0]["diff"].as_str().unwrap_or_default();
    if !diff.contains("+check = ") && !diff.contains("-check = ") {
        return Err(format!("the diff of updates.set is {diff:?}, which should have the check"));
    }
    // The update is a task, and its row says which.
    let apply = of("updates.apply");
    if apply.is_empty() || apply.iter().any(|row| row["task"].is_null()) {
        return Err(format!("the row of updates.apply is no task's: {apply:?}"));
    }
    Ok(())
}

/// Every action that needs an administrator, each with arguments that
/// would work if the asker were one, is ADM-122 for the reader and ADM-106
/// for alice with her session locked, before anything runs. What a person
/// reads of their own is open to both.
fn what_needs_an_administrator(test: &mut Test) -> Result<(), String> {
    // Whether it is a POST, its address, and its arguments.
    let asks = [
        (false, "/api/1/updates/slots", json!({})),
        (true, "/api/1/updates/settings", json!({"check": "off"})),
        (true, "/api/1/updates/channel", json!({"channel": "beta"})),
        (true, "/api/1/updates/checks", json!({"name": "x", "url": "https://localhost/"})),
        (true, "/api/1/updates/checks/x/remove", json!({})),
        (true, "/api/1/updates/essential", json!({"service": "nginx", "on": true})),
        (true, "/api/1/updates/system/apply", json!({})),
        (true, "/api/1/updates/unstage", json!({})),
        (true, "/api/1/updates/system/undo", json!({"settings": "keep"})),
        (true, "/api/1/updates/system/hold", json!({})),
        (true, "/api/1/updates/system/unhold", json!({})),
        (true, "/api/1/apps/update", json!({})),
        (true, "/api/1/apps/org.example.diary/update", json!({})),
        (true, "/api/1/apps/org.example.diary/rollback", json!({})),
        (true, "/api/1/apps/org.example.diary/hold", json!({})),
        (true, "/api/1/apps/org.example.diary/unhold", json!({})),
        (true, "/api/1/apps/org.example.diary/remove", json!({"data": "keep"})),
    ];
    test.locked()?;
    for (post, path, body) in &asks {
        for (who, id) in [(Who::Reader, "ADM-122"), (Who::Alice, "ADM-106")] {
            let reply = if *post { test.post(who, path, body)? } else { test.get(who, path)? };
            reply.refused(path, 403, id)?;
        }
    }
    // What a person reads of their own is open to both.
    for path in [
        "/api/1/updates",
        "/api/1/updates/check",
        "/api/1/updates/history",
        "/api/1/updates/advisories",
        "/api/1/updates/settings",
        "/api/1/updates/checks",
        "/api/1/updates/failed",
        "/api/1/apps",
        "/api/1/apps/org.example.echoprov",
    ] {
        test.read(Who::Reader, path)?;
        test.read(Who::Alice, path)?;
    }
    Ok(())
}
