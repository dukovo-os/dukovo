// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::{format, vec};

use crate::legacy;
use crate::service::{self, Oom, PciId, Place, Ports, Restart};

/// The service files Dukovo ships, and those of the boot tests.
const SHIPPED: &[(&str, &str)] = &[
    ("consoled", include_str!("../../../defaults/spark/services/consoled.toml")),
    ("diskhealth", include_str!("../../../defaults/spark/services/diskhealth.toml")),
    ("logd", include_str!("../../../defaults/spark/services/logd.toml")),
    ("notifyd", include_str!("../../../defaults/spark/services/notifyd.toml")),
    ("volumed", include_str!("../../../defaults/spark/services/volumed.toml")),
    ("hog", include_str!("../../../spark/tests/services/hog.toml")),
    ("granted", include_str!("../../../spark/tests/services/network/granted.toml")),
    ("none", include_str!("../../../spark/tests/services/network/none.toml")),
    ("driver", include_str!("../../../spark/tests/services/stop/driver.toml")),
    ("leaver", include_str!("../../../spark/tests/services/stop/leaver.toml")),
    ("stubborn", include_str!("../../../spark/tests/services/stop/stubborn.toml")),
    ("e1000d", include_str!("../../../defaults/spark/services/e1000d.toml")),
    ("e1000d", include_str!("../../../spark/tests/services/e1000d/e1000d.toml")),
    ("netd", include_str!("../../../spark/tests/services/e1000d/netd.toml")),
    ("authd", include_str!("../../../defaults/spark/services/authd.toml")),
    ("firewall", include_str!("../../../defaults/spark/services/firewall.toml")),
    ("sysupdate", include_str!("../../../defaults/spark/services/sysupdate.toml")),
    ("snap", include_str!("../../../defaults/spark/services/snap.toml")),
    ("updated", include_str!("../../../defaults/spark/services/updated.toml")),
    ("logforward", include_str!("../../../defaults/spark/services/logforward.toml")),
    ("dumps", include_str!("../../../defaults/spark/services/dumps.toml")),
    ("netd", include_str!("../../../spark/tests/services/serve/netd.toml")),
    ("timed", include_str!("../../../spark/tests/services/serve/timed.toml")),
    ("metricsd", include_str!("../../../defaults/spark/services/metricsd.toml")),
    ("alertd", include_str!("../../../defaults/spark/services/alertd.toml")),
    ("netd", include_str!("../../../defaults/spark/services/netd.toml")),
    ("netd", include_str!("../../../spark/tests/services/dhcp/netd.toml")),
];

/// `text` with the first line of service_version added.
fn file(text: &str) -> String {
    format!("service_version = 1\nprogram = \"/system/commands/true\"\n{text}")
}

/// The error reading `text` gives, as service check says it.
fn error(text: &str) -> String {
    service::from_toml(text).unwrap_err().to_string()
}

#[test]
fn every_shipped_file_reads() {
    for (name, text) in SHIPPED {
        service::check_name(name).unwrap();
        if let Err(err) = service::from_toml(text) {
            panic!("{name}: {err}");
        }
    }
    let logd = service::from_toml(SHIPPED[2].1).unwrap();
    assert_eq!(logd.program, "/system/commands/logd");
    assert_eq!((logd.order, logd.restart), (5, Restart::Always));
    assert_eq!(logd.limits.memory, Some(64 << 20));
    let consoled = service::from_toml(SHIPPED[0].1).unwrap();
    assert_eq!((consoled.limits.cpu_weight, consoled.limits.memory), (200, None));
    assert_eq!(consoled.restart, Restart::OnFailure);
    let granted = service::from_toml(SHIPPED[6].1).unwrap();
    assert!(granted.permissions.network.outbound);
    let listen = &granted.permissions.network.listen;
    assert_eq!(listen, &[Ports { first: 8000, last: 8009 }, Ports { first: 9000, last: 9000 }]);
    assert_eq!(granted.network_grants(), 3);
    // Ports 67, 68 and 546 on each card, servers anywhere, and setting
    // the cards up.
    let netd = service::from_toml(SHIPPED[SHIPPED.len() - 2].1).unwrap();
    assert_eq!(netd.network_grants(), 5);
    assert!(netd.permissions.admin.network && !netd.permissions.runs_with_your_access);
    assert_eq!(
        service::from_toml(SHIPPED[SHIPPED.len() - 1].1).unwrap().permissions,
        netd.permissions
    );
}

#[test]
fn dumps_reads_the_device_of_the_dump_and_changes_its_folder() {
    let (_, text) = SHIPPED.iter().find(|(name, _)| *name == "dumps").unwrap();
    let dumps = service::from_toml(text).unwrap();
    assert_eq!(dumps.program, "/system/commands/dumps");
    assert_eq!(dumps.arguments, ["save"]);
    // After logd, which is 5, so that the line it logs is kept.
    assert_eq!((dumps.order, dumps.restart), (6, Restart::Never));
    let place = |path: &str, write| Place { path: path.into(), write };
    assert_eq!(
        dumps.permissions.places(),
        [place("/devices/crashdump", false), place("/system/state/crash", true)]
    );
    assert!(!dumps.permissions.runs_with_your_access);
}

#[test]
fn only_authd_holds_the_names_capability() {
    // The names capability lets its holder act for anyone: authd needs it
    // to start the sessions of those who sign in (ADR 0250), and no other
    // shipped service may ask for it.
    for (name, text) in SHIPPED {
        let service = service::from_toml(text).unwrap();
        assert_eq!(service.permissions.names, *name == "authd", "{name}");
    }
    assert!(!service::from_toml(&file("")).unwrap().permissions.names);
    let asked = service::from_toml(&file("[permissions]\nnames = true\n")).unwrap();
    assert!(asked.permissions.names);
}

#[test]
fn devices_name_the_cards_a_driver_drives() {
    // Of the shipped services only e1000d drives cards (ADR 0198).
    for (name, text) in SHIPPED {
        let service = service::from_toml(text).unwrap();
        assert_eq!(!service.permissions.devices.pci.is_empty(), *name == "e1000d", "{name}");
    }
    let intel = PciId { vendor: 0x8086, device: None };
    let (_, e1000d) = SHIPPED.iter().find(|(name, _)| *name == "e1000d").unwrap();
    assert_eq!(service::from_toml(e1000d).unwrap().permissions.devices.pci, [intel]);
    assert!(intel.matches(0x8086, 0x100e) && intel.matches(0x8086, 0x10d3));
    assert!(!intel.matches(0x10ec, 0x8136));
    let text = file("[permissions]\ndevices.pci = [\"8086:10D3\", \"10ec:*\"]\n");
    let two = service::from_toml(&text).unwrap().permissions.devices.pci;
    let e1000e = PciId { vendor: 0x8086, device: Some(0x10d3) };
    assert_eq!(two, [e1000e, PciId { vendor: 0x10ec, device: None }]);
    assert!(e1000e.matches(0x8086, 0x10d3) && !e1000e.matches(0x8086, 0x100e));
    assert_eq!((two[0].to_string(), two[1].to_string()), ("8086:10d3".into(), "10ec:*".into()));
    for bad in ["8086", "8086:100", "808g:100e", "*:100e", "8086:100e:0", "8086:+10e"] {
        let text = file(&format!("[permissions]\ndevices.pci = [\"{bad}\"]\n"));
        assert!(error(&text).contains("devices.pci takes a card's vendor and device IDs"), "{bad}");
    }
    let twice = file("[permissions]\ndevices.pci = [\"8086:100e\", \"8086:100E\"]\n");
    assert!(error(&twice).contains("devices.pci names 8086:100e twice"));
    assert!(service::from_toml(&file("")).unwrap().permissions.devices.pci.is_empty());
}

#[test]
fn the_template_reads_as_a_service_not_started_at_boot() {
    let text = service::template("backup");
    assert!(text.starts_with("# The backup service."));
    assert!(text.contains("`service check backup`"));
    let service = service::from_toml(&text).unwrap();
    assert_eq!(service.program, "/system/commands/true");
    assert!(!service.enabled);
    assert_eq!(service.restart, Restart::OnFailure);
    assert_eq!(service.network_grants(), 0);
    // It names each setting the man page documents: as `key =`, as a
    // table, or as `name =` under the table `[table]` of `table.name`.
    let in_table = |key: &str| {
        let Some((table, name)) = key.split_once('.') else { return false };
        let Some((_, after)) = text.split_once(&format!("\n[{table}]\n")) else { return false };
        let body = after.split("\n[").next().unwrap_or_default();
        body.lines().any(|line| line.starts_with(&format!("{name} =")))
    };
    for key in service::KEYS {
        assert!(
            text.contains(&format!("{key} ="))
                || text.contains(&format!("[{key}]"))
                || in_table(key),
            "{key}"
        );
    }
    assert_eq!(service.backup, service::Backup::default());
}

#[test]
fn what_a_file_leaves_out() {
    let service = service::from_toml(&file("")).unwrap();
    assert_eq!(service.description, "");
    assert!(service.arguments.is_empty() && service.environment.is_empty());
    assert_eq!((service.order, service.enabled, service.restart), (50, true, Restart::Always));
    assert_eq!((service.limits.cpu_weight, service.limits.memory), (100, None));
    assert_eq!(service.limits.oom, Oom::Service);
    assert_eq!(service.network_grants(), 0);
}

#[test]
fn every_setting() {
    let text = file(
        r#"description = "A \"quoted\" one"
arguments = ["-c", "echo two words"]
order = 7
enabled = false
restart = "never"

[environment]
TZ = "UTC"
"LC ALL" = "C"

[limits]
cpu_weight = 10000
memory = 4096
oom = "app"

[permissions]
network.outbound = true
network.listen = [80, "443", "8000-8009"]
"#,
    );
    let service = service::from_toml(&text).unwrap();
    assert_eq!(service.description, "A \"quoted\" one");
    assert_eq!(service.arguments, ["-c", "echo two words"]);
    assert_eq!((service.order, service.enabled, service.restart), (7, false, Restart::Never));
    let environment = vec![("LC ALL".to_string(), "C".to_string()), ("TZ".into(), "UTC".into())];
    assert_eq!(service.environment, environment);
    assert_eq!((service.limits.cpu_weight, service.limits.memory), (10_000, Some(4096)));
    assert_eq!(service.limits.oom, Oom::App);
    assert_eq!(service.network_grants(), 4);
    // The same as tables of their own.
    let tables = file("[permissions.network]\noutbound = true\nlisten = [80]\n");
    assert_eq!(service::from_toml(&tables).unwrap().network_grants(), 2);
}

#[test]
fn a_service_may_run_as_an_account() {
    assert_eq!(service::from_toml(&file("")).unwrap().account, None);
    let service = service::from_toml(&file("account = \"web-cache\"\n")).unwrap();
    assert_eq!(service.account.as_deref(), Some("web-cache"));
    assert!(error(&file("account = \"root\"\n")).contains("leave it out"));
    assert!(error(&file("account = \"Web\"\n")).contains("is not an account's name"));
    let whole = file("account = \"web\"\n[permissions]\nruns-with-your-access = true\n");
    assert!(error(&whole).contains("cannot run with your access"));

    let passwd = "root:x:0:0:root:/system/state/root:/system/commands/nukesh\n\
                  web:x:100:100:A service of org.example.web:/shared/services/org.example.web:\
                  /system/commands/false\nbroken:x:1x:2\n";
    assert_eq!(service::account_numbers(passwd, "web"), Some((100, 100)));
    assert_eq!(service::account_numbers(passwd, "root"), Some((0, 0)));
    assert_eq!(service::account_numbers(passwd, "we"), None);
    assert_eq!(service::account_numbers(passwd, "broken"), None);
}

#[test]
fn an_override_sets_keys_over_the_file() {
    let base = file(
        "enabled = true\n[environment]\nMODE = \"slow\"\nKEEP = \"1\"\n[permissions]\n\
         network.listen = [80]\nfiles.read = [\"/a\"]\n",
    );
    let over = service::Override::from_toml(
        "restart = \"never\"\n[environment]\nMODE = \"fast\"\n[limits]\nmemory = \"64M\"\n\
         [permissions]\nfiles.read = [\"/b\"]\n",
    )
    .unwrap();
    let service = service::from_toml_over(&base, Some(&over)).unwrap();
    assert_eq!(service.restart, service::Restart::Never);
    // A table's keys one by one; any other value in place of the file's.
    assert!(service.environment.iter().any(|(k, v)| k == "MODE" && v == "fast"));
    assert!(service.environment.iter().any(|(k, v)| k == "KEEP" && v == "1"));
    assert_eq!(service.limits.memory, Some(64 << 20));
    assert_eq!(service.permissions.network.listen.len(), 1);
    assert_eq!(service.permissions.files.read, ["/b"]);
    assert!(service.enabled);
    // Without one, the file as it is; with one that breaks it, why.
    assert_eq!(service::from_toml_over(&base, None), service::from_toml(&base));
    let bad = service::Override::from_toml("restart = \"sometimes\"\n").unwrap();
    assert!(service::from_toml_over(&base, Some(&bad)).is_err());
    assert!(service::Override::from_toml("restart = \n").is_err());
    let newer = base.replace("[\"/a\"]", "[\"/a\", \"/c\"]").replace("\"slow\"", "\"fast\"");
    assert_eq!(
        service::changed_keys(&base, &newer).unwrap(),
        ["environment.MODE", "permissions.files.read"]
    );
    assert!(service::changed_keys(&base, &base).unwrap().is_empty());
}

#[test]
fn what_a_view_holds_and_the_secrets() {
    let text = file(
        "secrets = [\"api-token\", \"db_password\"]\n\
         [permissions]\n\
         files.read = [\"/devices\", \"/system/settings\"]\n\
         files.read-write = [\"/system/state/web\", \"/system/live/notify\"]\n\
         files.mount = [\"/volumes\"]\n\
         files.shared = [\"media\"]\n",
    );
    let service = service::from_toml(&text).unwrap();
    assert_eq!(service.secrets, ["api-token", "db_password"]);
    assert!(!service.permissions.runs_with_your_access);
    // In the order Spark attaches them: what it reads, then what it
    // changes. A place every service reads may be named, to change it, and
    // so may one that holds such a place.
    let place = |path: &str, write| Place { path: path.into(), write };
    assert_eq!(
        service.permissions.places(),
        [
            place("/devices", false),
            place("/system/settings", false),
            place("/system/state/web", true),
            place("/system/live/notify", true),
            place("/volumes", true),
            place("/shared/media", true),
        ]
    );
    assert_eq!(service::secrets_dir("web"), "/system/state/spark/secrets/web");
    let whole = service::from_toml(&file("[permissions]\nruns-with-your-access = true")).unwrap();
    assert!(whole.permissions.runs_with_your_access && whole.permissions.places().is_empty());
    // Nothing named, nothing beyond what every service reads.
    assert!(service::from_toml(&file("")).unwrap().permissions.places().is_empty());
    let mounts = file("[permissions]\nfiles.mount = [\"/a\", \"/b\", \"/c\", \"/d\", \"/e\"]");
    assert_eq!(error(&mounts), "line 3: files.mount names at most 4 places");
    for (secret, fits) in [("api-token", true), ("_x.1", true), ("", false), ("a/b", false)] {
        assert_eq!(service::check_secret(secret).is_ok(), fits, "{secret}");
    }
}

#[test]
fn errors_name_their_line_in_plain_words() {
    let cases: &[(&str, &str)] = &[
        (
            "restart = \"sometimes\"",
            "line 3: restart must be always, on-failure or never, not `sometimes`",
        ),
        (
            "restart = 5",
            "line 3: invalid type: integer `5`, expected always, on-failure or never for restart",
        ),
        ("oom = \"app\"", "line 3: `oom` is not a setting here; service.toml(5) lists them"),
        ("[limits]\noom = \"kernel\"", "line 4: oom must be service or app, not `kernel`"),
        ("[limits]\ncpu_weight = 0", "line 4: cpu_weight is from 1 to 10000, not 0"),
        ("[limits]\ncpu_weight = -3", "line 4: cpu_weight is not below 0"),
        (
            "[limits]\nmemory = \"lots\"",
            "line 4: memory is a size such as \"64M\", \"512K\" or \"2G\", or \"none\", not `lots`",
        ),
        ("[limits]\nmemory = 0", "line 4: memory is more than 0 bytes; \"none\" sets no limit"),
        ("order = \"first\"", "line 3: invalid type: string \"first\", expected a whole number"),
        ("enabled = \"yes\"", "line 3: invalid type: string \"yes\", expected true or false"),
        (
            "arguments = \"-c\"",
            "line 3: invalid type: string \"-c\", expected a list such as [\"a\", \"b\"]",
        ),
        ("[environment]\n\"A=B\" = \"c\"", "line 4: `A=B` cannot be a variable's name"),
        (
            "[environment]\nTZ = 1",
            "line 4: invalid type: integer `1`, expected text in double quotes",
        ),
        (
            "[permissions]\nnetwork.listen = [70000]",
            "line 4: network.listen takes ports from 1 to 65535 and ranges such as \"8000-8009\", not `70000`",
        ),
        (
            "[permissions]\nnetwork.listen = [\"90-80\"]",
            "line 4: the range \"90-80\" ends before it starts",
        ),
        (
            "[permissions]\nnetwork.listen = [80, \"70-90\"]",
            "line 4: network.listen: 70-90 overlaps 80",
        ),
        (
            "[permissions]\nsensors.camera = true",
            "line 4: `sensors` is not a setting here; service.toml(5) lists them",
        ),
        (
            "[permissions]\ndevices.camera = true",
            "line 4: `camera` is not a setting here; service.toml(5) lists them",
        ),
        (
            "[permissions]\nfiles.read = [\"/\"]",
            "line 4: files holds places beneath /, not / itself: runs-with-your-access gives a \
             service the whole tree",
        ),
        (
            "[permissions]\nfiles.read-write = [\"/system/state/../log\"]",
            "line 4: files takes absolute paths without `.`, `..` or `//`, such as \
             /system/state/log, not `/system/state/../log`",
        ),
        (
            "[permissions]\nfiles.read = [\"/devices\"]\nfiles.read-write = [\"/devices/sda\"]",
            "line 3: files names /devices/sda and /devices, one within the other: name one",
        ),
        (
            "[permissions]\nfiles.read-write = [\"/system/settings/web\"]",
            "line 3: every service reads /system/settings already, so its view cannot hold \
             /system/settings/web apart; name /system/settings with files.read-write to change \
             what is in it",
        ),
        (
            "[permissions]\nfiles.read = [\"/system/state\"]",
            "line 3: no service holds /system/state/spark, where Spark keeps its files and the \
             services' secrets, so it cannot hold /system/state",
        ),
        (
            "[permissions]\nruns-with-your-access = true\nfiles.read = [\"/devices\"]",
            "line 3: runs-with-your-access holds every file already: leave out files.*",
        ),
        (
            "[permissions]\nfiles.shared = [\"../users\"]",
            "line 4: files.shared takes names of folders in /shared, such as \"media\", not \
             `../users`",
        ),
        ("secrets = [\"api-token\", \"api-token\"]", "line 3: secrets names api-token twice"),
        (
            "secrets = [\"../key\"]",
            "line 3: `../key` is not a secret's name: it has 1 to 23 letters, digits, `-`, `_` \
             and `.`, and starts with a letter, a digit or `_`",
        ),
        (
            "[permissions]\nnetwork.inbound = true",
            "line 4: `inbound` is not a setting here; service.toml(5) lists them",
        ),
        ("description = \"unended", "line 3: invalid basic string, expected `\"`"),
    ];
    for (text, expected) in cases {
        assert_eq!(&error(&file(text)), expected, "{text}");
    }
    let many =
        file("[permissions]\nnetwork.outbound = true\nnetwork.listen = [1, 2, 3, 4, 5, 6, 7, 8]");
    assert!(
        error(&many).starts_with("line 4: it asks for 9 network capabilities"),
        "{}",
        error(&many)
    );
    // Setting the interfaces up takes a capability of its own.
    let eight = "[permissions]\nnetwork.outbound = true\nnetwork.listen = [1, 2, 3, 4, 5, 6]\n";
    assert!(service::from_toml(&file(&format!("{eight}admin.network = true"))).is_ok());
    let nine = "[permissions]\nnetwork.outbound = true\nnetwork.listen = [1, 2, 3, 4, 5, 6, 7]\n\
                admin.network = true";
    assert!(
        error(&file(nine)).contains("asks for 9 network capabilities"),
        "{}",
        error(&file(nine))
    );
}

#[test]
fn a_file_needs_its_version_and_program() {
    assert_eq!(
        error("program = \"/system/commands/true\""),
        "it has no service_version: add `service_version = 1`"
    );
    assert_eq!(
        error("service_version = 1"),
        "it names no program: add a line such as `program = \"/system/commands/logd\"`"
    );
    assert_eq!(
        error("program = \"/x\"\nrequires = [\"db\"]"),
        "it has no service_version: add `service_version = 2`"
    );
    assert_eq!(
        error("service_version = 3\nprogram = \"/x\""),
        "line 1: this file is for service_version 3, and this Dukovo reads version 2 and \
         earlier: update Dukovo to use it"
    );
    assert_eq!(error("service_version = 0\nprogram = \"/x\""), "line 1: service_version is 1 or 2");
    assert_eq!(
        error("service_version = 1\nprogram = \"bin/logd\""),
        "line 2: program is an absolute path, such as /system/commands/logd, not `bin/logd`"
    );
}

#[test]
fn names() {
    for good in ["logd", "a", "net-d_2.x", "_hidden", &"x".repeat(23)] {
        assert!(service::check_name(good).is_ok(), "{good}");
    }
    for bad in ["", ".logd", "-x", "a/b", "a b", "é", &"x".repeat(24)] {
        assert!(service::check_name(bad).is_err(), "{bad}");
    }
}

/// The old files Dukovo shipped until Phase 13 (ADR 0101).
const OLD_CONSOLED: &str = "\
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# The virtual consoles on the screen and keyboard: consoled(8). Each
# console runs root's shell; Alt+F1 to Alt+F6 switch between them.

description = Virtual consoles
program = /system/commands/consoled
# First up, last down: the consoles show the other services stopping.
order = 10
# A machine with no screen has no consoles: consoled exits with 0 there,
# and stays stopped.
restart = on-failure
# The shells and all they run share its domain, so it has no memory
# limit; twice the default CPU weight keeps typing quick when services
# are busy.
weight = 200
memory = none
# Root's shells may use the network, listen on any port and set the
# interfaces up with ip(8), and set the fans, the LEDs and the ARGB
# header with ectool(8), as root may on Linux; from Phase 13 an
# administrator's do, once elevated.
network.outbound = yes
network.listen = 1-65535
admin.network = yes
admin.hardware = yes
";

#[test]
fn an_old_file_converts_with_its_comments() {
    let converted = legacy::convert(OLD_CONSOLED).unwrap();
    assert_eq!(
        converted,
        "\
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# The virtual consoles on the screen and keyboard: consoled(8). Each
# console runs root's shell; Alt+F1 to Alt+F6 switch between them.

service_version = 1
description = \"Virtual consoles\"
program = \"/system/commands/consoled\"
# First up, last down: the consoles show the other services stopping.
order = 10
# A machine with no screen has no consoles: consoled exits with 0 there,
# and stays stopped.
restart = \"on-failure\"

[limits]
# The shells and all they run share its domain, so it has no memory
# limit; twice the default CPU weight keeps typing quick when services
# are busy.
cpu_weight = 200
memory = \"none\"

[permissions]
# Root's shells may use the network, listen on any port and set the
# interfaces up with ip(8), and set the fans, the LEDs and the ARGB
# header with ectool(8), as root may on Linux; from Phase 13 an
# administrator's do, once elevated.
network.outbound = true
network.listen = [\"1-65535\"]
admin.network = true
admin.hardware = true
# Before Phase 13 a service held the whole tree, and this one still does;
# narrow it to what it needs with files.* instead (service.toml(5)).
runs-with-your-access = true
"
    );
    // The shipped file has since asked for the clock too (ADR 0194),
    // which no file of before Phase 13 could.
    let mut shipped = service::from_toml(SHIPPED[0].1).unwrap();
    assert!(shipped.permissions.admin.time);
    shipped.permissions.admin.time = false;
    assert_eq!(service::from_toml(&converted).unwrap(), shipped);
}

#[test]
fn every_old_setting_converts() {
    let old = "\
description = Asks for \"the\" network
program = /system/tests/posix/netgrant
arguments = granted \"two words\" back\\slash
# Before the others.
order = 3
enabled = no
restart = never
# Comments move with their settings.
network.outbound = yes
network.listen = 8000-8009, 9000
admin.network = yes
admin.hardware = yes
memory = 512K
oom = app
environment = TZ=UTC LANG=C.UTF-8 \"GREETING=hello there\"
weight = 50
# The end.
";
    let converted = legacy::convert(old).unwrap();
    let service = service::from_toml(&converted).unwrap();
    assert_eq!(service.description, "Asks for \"the\" network");
    assert_eq!(service.arguments, ["granted", "two words", "back\\slash"]);
    assert_eq!((service.order, service.enabled, service.restart), (3, false, Restart::Never));
    assert_eq!((service.limits.cpu_weight, service.limits.memory), (50, Some(512 << 10)));
    assert_eq!(service.limits.oom, Oom::App);
    let names: Vec<&str> = service.environment.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["GREETING", "LANG", "TZ"]);
    assert_eq!(service.environment[0].1, "hello there");
    assert_eq!(service.network_grants(), 4);
    assert!(service.permissions.admin.network && service.permissions.admin.hardware);
    // It keeps the whole tree it held.
    assert!(service.permissions.runs_with_your_access);
    assert!(converted.contains(
        "[permissions]\n# Comments move with their settings.\nnetwork.outbound = true\n"
    ));
    assert!(converted.ends_with("\n\n# The end.\n"), "{converted}");
    assert!(converted.starts_with("service_version = 1\ndescription = "), "{converted}");
}

#[test]
fn a_key_set_twice_keeps_its_last_value() {
    let old = "program = /a\nenvironment = A=1 B=2\nrestart = never\nenvironment = C=3\n";
    let service = service::from_toml(&legacy::convert(old).unwrap()).unwrap();
    assert_eq!(service.environment, [("C".to_string(), "3".to_string())]);
    assert_eq!(service.restart, Restart::Never);
}

#[test]
fn an_old_file_with_an_error_is_not_converted() {
    let cases: &[(&str, &str)] = &[
        ("program = /a\nrestart = sometimes\n", "line 2: restart is always, on-failure or never"),
        ("program = /a\nlisten\n", "line 2: `listen` is not `key = value`"),
        ("program = /a\ncolour = blue\n", "line 2: unknown key `colour`"),
        ("program = a\n", "line 1: the program `a` is not an absolute path"),
        ("program = /a\nmemory = lots\n", "line 2: `lots` is not a size such as 64M"),
        (
            "description = none\n",
            "it names no program: add a line such as `program = \"/system/commands/logd\"`",
        ),
    ];
    for (old, expected) in cases {
        assert_eq!(legacy::convert(old).unwrap_err().to_string(), *expected, "{old}");
    }
}

/// No file, however broken, makes the reader panic: each shipped file
/// with each byte taken out, or a quote, a bracket or a newline put in.
#[test]
fn broken_files_never_panic() {
    for (_, text) in SHIPPED {
        let bytes = text.as_bytes();
        for at in 0..bytes.len() {
            for change in [None, Some(b'"'), Some(b'['), Some(b'\n'), Some(b'=')] {
                let mut broken = bytes[..at].to_vec();
                broken.extend(change);
                broken.extend_from_slice(&bytes[at + usize::from(change.is_none())..]);
                if let Ok(text) = core::str::from_utf8(&broken) {
                    let _ = service::from_toml(text);
                }
            }
        }
    }
}

#[test]
fn enabled_is_set_in_place_or_after_program() {
    let file = "# smbd\nservice_version = 1\nprogram = \"/system/commands/smbd\"\nenabled = false\n\n[permissions]\nenabled = 1\n";
    let on = service::with_enabled(file, true);
    assert!(on.contains("\nenabled = true\n\n[permissions]\nenabled = 1\n"), "{on}");
    assert_eq!(service::with_enabled(&on, false), file);
    let without = "service_version = 1\nprogram = \"/x\"\norder = 40\n";
    assert_eq!(
        service::with_enabled(without, true),
        "service_version = 1\nprogram = \"/x\"\nenabled = true\norder = 40\n"
    );
}

/// `text` after the lines of a version 2 file that listens on port 8080.
fn file_2(text: &str) -> String {
    format!(
        "service_version = 2\nprogram = \"/system/commands/true\"\n{text}\n\
         [permissions]\nnetwork.listen = [8080, \"9000-9001\"]\n"
    )
}

#[test]
fn a_version_2_file_names_what_it_waits_for() {
    let service = service::from_toml(&file_2(
        "requires = [\"db\"]\nwants = [\"cache\", \"mail\"]\nafter = [\"netd\"]\n\
         give_up_after = 5",
    ))
    .unwrap();
    assert_eq!(service.requires, ["db"]);
    assert_eq!(service.wants, ["cache", "mail"]);
    assert_eq!(service.after, ["netd"]);
    assert_eq!(service.give_up_after, 5);
    assert_eq!(service.ready, service::Ready::default());
    assert_eq!(service.ready.timeout, 90);
    assert_eq!(service.health, None);
    assert!(service.check_links("web").is_ok());
    assert_eq!(service.check_links("db"), Err("requires names db, the service itself".into()));
    // What a file leaves out waits for nothing, and Spark never gives up.
    let plain = service::from_toml(&file("")).unwrap();
    assert!(plain.requires.is_empty() && plain.wants.is_empty() && plain.after.is_empty());
    assert_eq!(plain.give_up_after, 0);
}

#[test]
fn a_version_1_file_may_not_use_version_2_s_keys() {
    for (text, key) in [
        ("requires = []", "requires"),
        ("after = [\"netd\"]", "after"),
        ("give_up_after = 3", "give_up_after"),
        ("[ready]\nwhen = \"started\"", "ready"),
        ("[health]\ncommand = [\"/x\"]", "health"),
    ] {
        assert_eq!(
            error(&file(text)),
            format!("{key} needs service_version = 2: change the file's service_version to 2")
        );
    }
}

#[test]
fn each_way_of_being_ready() {
    use service::ReadyWhen;
    let ready = |text: &str| service::from_toml(&file_2(text)).map(|s| s.ready.when);
    assert_eq!(ready("[ready]\nwhen = \"listening\""), Ok(ReadyWhen::Listening(None)));
    assert_eq!(
        ready("[ready]\nwhen = \"listening\"\nport = 9001\ntimeout = 30"),
        Ok(ReadyWhen::Listening(Some(9001)))
    );
    assert_eq!(
        ready("[ready]\nwhen = \"socket\"\npath = \"/system/live/services/web/web.sock\""),
        Ok(ReadyWhen::Socket("/system/live/services/web/web.sock".into()))
    );
    assert_eq!(ready("restart = \"never\"\n[ready]\nwhen = \"ended\""), Ok(ReadyWhen::Ended));
    assert_eq!(
        ready("[ready]\nwhen = \"healthy\"\n[health]\ncommand = [\"/x\", \"ping\"]"),
        Ok(ReadyWhen::Healthy)
    );
    let service =
        service::from_toml(&file_2("ready.when = \"started\"\nready.timeout = 5")).unwrap();
    assert_eq!(service.ready, service::Ready { when: ReadyWhen::Started, timeout: 5 });
}

#[test]
fn a_health_check_and_its_defaults() {
    let service = service::from_toml(&file_2("[health]\ncommand = [\"/x\", \"ping\"]")).unwrap();
    let health = service.health.unwrap();
    assert_eq!(health.command, ["/x", "ping"]);
    assert_eq!((health.every, health.timeout, health.failures), (30, 10, 3));
    let service = service::from_toml(&file_2(
        "[health]\ncommand = [\"/x\"]\nevery = 5\ntimeout = 5\nfailures = 1",
    ))
    .unwrap();
    let health = service.health.unwrap();
    assert_eq!((health.every, health.timeout, health.failures), (5, 5, 1));
}

#[test]
fn version_2_s_mistakes_are_named_in_plain_words() {
    let cases: &[(&str, &str)] = &[
        ("requires = [\"db\", \"db\"]", "line 3: requires names db twice"),
        ("wants = [\"-x\"]", "line 3: wants: `-x` is not a service's name"),
        (
            "requires = [\"db\"]\nafter = [\"db\"]",
            "db is in requires and after: requires says it already, so leave it out of after",
        ),
        ("give_up_after = 1001", "line 3: give_up_after is from 0 to 1000 failed starts, not 1001"),
        (
            "[ready]\nwhen = \"soon\"",
            "line 4: ready.when must be started, listening, socket, healthy or ended, not `soon`",
        ),
        (
            "[ready]\nwhen = \"listening\"\nport = 80",
            "ready.port 80 is none of network.listen's ports",
        ),
        ("[ready]\nport = 8080", "ready.port is for ready.when = \"listening\" alone"),
        ("[ready]\nport = 70000", "line 4: ready.port is from 1 to 65535, not 70000"),
        (
            "[ready]\nwhen = \"socket\"",
            "ready.when = \"socket\" needs the socket's path in ready.path",
        ),
        (
            "[ready]\nwhen = \"socket\"\npath = \"web.sock\"",
            "ready.path is an absolute path, such as /system/live/services/web/web.sock, not \
             `web.sock`",
        ),
        ("[ready]\npath = \"/a\"", "ready.path is for ready.when = \"socket\" alone, not `/a`"),
        ("[ready]\nwhen = \"healthy\"", "ready.when = \"healthy\" needs a [health] check"),
        (
            "[ready]\nwhen = \"ended\"",
            "ready.when = \"ended\" is for a service that ends, so its restart is on-failure or \
             never, not always",
        ),
        ("[ready]\ntimeout = 0", "line 4: ready.timeout is from 1 to 3600 seconds, not 0"),
        (
            "[ready]\nwhen = \"started\"\nsoon = true",
            "line 5: `soon` is not a setting here; service.toml(5) lists them",
        ),
        (
            "[health]\ncommand = []",
            "line 4: health.command names a program, such as [\"/system/commands/true\"]",
        ),
        (
            "[health]\ncommand = [\"ping\"]",
            "line 4: health.command starts with a program's absolute path, not `ping`",
        ),
        (
            "[health]\ncommand = [\"/x\"]\nevery = 5\ntimeout = 6",
            "health.timeout is at most health.every, 5 seconds, not 6",
        ),
        (
            "[health]\ncommand = [\"/x\"]\nfailures = 0",
            "line 5: health.failures is from 1 to 100 failed checks, not 0",
        ),
        ("[health]\nevery = 5", "missing field `command`"),
    ];
    for (text, said) in cases {
        let got = error(&file_2(text));
        assert!(got.contains(said), "{text}: {got}");
    }
    // Listening needs ports to listen on.
    assert_eq!(
        error("service_version = 2\nprogram = \"/x\"\n[ready]\nwhen = \"listening\""),
        "ready.when = \"listening\" needs the ports it listens on in network.listen"
    );
}
