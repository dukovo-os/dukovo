// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::{format, vec};

use crate::legacy;
use crate::service::{self, Oom, Override, PciId, Place, Ports, ReadyWhen, Restart};
use crate::{Error, Source};

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
    ("sysupdate-stage", include_str!("../../../defaults/spark/services/sysupdate-stage.toml")),
    ("snap", include_str!("../../../defaults/spark/services/snap.toml")),
    ("updated", include_str!("../../../defaults/spark/services/updated.toml")),
    ("logforward", include_str!("../../../defaults/spark/services/logforward.toml")),
    ("certd", include_str!("../../../defaults/spark/services/certd.toml")),
    ("certtest", include_str!("../../../spark/tests/services/certificates/certtest.toml")),
    ("certtest2", include_str!("../../../spark/tests/services/certificates/certtest2.toml")),
    ("timerd", include_str!("../../../defaults/spark/services/timerd.toml")),
    ("logturn", include_str!("../../../defaults/spark/services/logturn.toml")),
    ("dumps", include_str!("../../../defaults/spark/services/dumps.toml")),
    ("sockets-echo", include_str!("../../../spark/tests/services/sockets/sockets-echo.toml")),
    ("netd", include_str!("../../../spark/tests/services/serve/netd.toml")),
    ("timed", include_str!("../../../spark/tests/services/serve/timed.toml")),
    ("metricsd", include_str!("../../../defaults/spark/services/metricsd.toml")),
    ("alertd", include_str!("../../../defaults/spark/services/alertd.toml")),
    ("admind", include_str!("../../../defaults/spark/services/admind.toml")),
    ("tune", include_str!("../../../defaults/spark/services/tune.toml")),
    ("limits-hog", include_str!("../../../spark/tests/services/limits/limits-hog.toml")),
    ("limits-steady", include_str!("../../../spark/tests/services/limits/limits-steady.toml")),
    ("limits-bomb", include_str!("../../../spark/tests/services/limits/limits-bomb.toml")),
    ("limits-files", include_str!("../../../spark/tests/services/limits/limits-files.toml")),
    ("iow-light", include_str!("../../../spark/tests/services/io-weight/iow-light.toml")),
    ("iow-heavy", include_str!("../../../spark/tests/services/io-weight/iow-heavy.toml")),
    ("iow-rated", include_str!("../../../spark/tests/services/io-weight/iow-rated.toml")),
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

/// The shipped service `name`, read.
fn shipped(name: &str) -> service::Service {
    let (_, text) = SHIPPED.iter().find(|(shipped, _)| *shipped == name).unwrap();
    service::from_toml(text).unwrap()
}

#[test]
fn the_services_that_manage_disks_hold_the_disks_and_nothing_else_of_devices() {
    use service::DeviceAccess::{Read, ReadWrite};
    let volumed = shipped("volumed").permissions;
    assert_eq!(volumed.files.disks, Some(ReadWrite));
    assert_eq!(shipped("diskhealth").permissions.files.disks, Some(Read));
    for permissions in [volumed, shipped("diskhealth").permissions] {
        assert!(!permissions.runs_with_your_access);
        let devices = permissions.places().into_iter().filter(|p| p.path.starts_with("/devices"));
        assert_eq!(devices.count(), 0);
    }
}

#[test]
fn the_updater_holds_the_roots_system_names_and_no_owner_s_files() {
    let health = shipped("sysupdate").permissions;
    let stage = shipped("sysupdate-stage").permissions;
    for permissions in [&health, &stage] {
        assert!(!permissions.runs_with_your_access);
        assert_eq!(permissions.files.efi_system, Some(service::DeviceAccess::ReadWrite));
        assert_eq!(permissions.files.disks, None);
        let names = &permissions.files.root_subvolumes;
        for owner in service::OWNERS_SUBVOLUMES {
            assert!(!names.iter().any(|name| name == owner), "{owner}");
        }
        for slot in ["@system.a", "@system.b", "@system.new", "@firmware"] {
            assert!(names.iter().any(|name| name == slot), "{slot}");
        }
        // The lock's holder may be the other one, in another domain.
        assert!(permissions.files.read.iter().any(|place| place == service::PROCESSES));
    }
    // Only the stage reaches the network and the snapshots it takes for a
    // fallback; the health check removes a half-built slot and no more.
    assert!(stage.network.outbound && !health.network.outbound);
    assert_eq!(health.files.root_subvolumes.len(), 4);
    for name in ["@settings", "@state", "@snapshots"] {
        assert!(stage.files.root_subvolumes.iter().any(|held| held == name), "{name}");
    }
    let stage = shipped("sysupdate-stage");
    assert_eq!(stage.arguments, ["stage", "--request", "/system/live/sysupdate/request"]);
    assert!(!stage.enabled);
    assert_eq!(stage.restart, Restart::Never);
}

#[test]
fn snap_holds_the_snapshots_of_the_root_and_reads_the_places_it_snapshots() {
    let snap = shipped("snap").permissions;
    assert!(!snap.runs_with_your_access);
    assert_eq!(snap.files.root_subvolumes, ["@snapshots"]);
    for place in ["/users", "/shared", "/snapshots"] {
        assert!(snap.files.read.iter().any(|held| held == place), "{place}");
    }
    assert!(snap.files.read_write.is_empty());
}

#[test]
fn updated_stages_through_the_stage_and_keeps_the_whole_tree_for_the_apps() {
    assert!(shipped("updated").permissions.runs_with_your_access);
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
fn tune_holds_the_kernel_settings_and_waits_to_have_ended() {
    let (_, text) = SHIPPED.iter().find(|(name, _)| *name == "tune").unwrap();
    let tune = service::from_toml(text).unwrap();
    assert_eq!(tune.program, "/system/commands/tune");
    assert_eq!(tune.arguments, ["apply"]);
    // After logd, which is 5, and one-shot: `tune set` waits for it to end.
    assert_eq!((tune.order, tune.restart), (7, Restart::Never));
    assert_eq!(tune.ready.when, ReadyWhen::Ended);
    assert!(tune.permissions.admin.tune);
    assert!(!tune.permissions.runs_with_your_access);
    let place = |path: &str, write| Place { path: path.into(), write };
    assert_eq!(
        tune.permissions.places(),
        [
            place("/system/live/sign-in", true),
            place("/system/live/tune", true),
            place("/system/state/tune", true)
        ]
    );
    // No other shipped file asks for the kernel's settings.
    for (name, text) in SHIPPED {
        let held = service::from_toml(text).unwrap().permissions.admin.tune;
        assert_eq!(held, *name == "tune", "{name}");
    }
}

#[test]
fn the_limits_boot_tests_services_say_their_limits() {
    let read = |name: &str| {
        let (_, text) = SHIPPED.iter().find(|(shipped, _)| *shipped == name).unwrap();
        service::from_toml(text).unwrap()
    };
    let hog = read("limits-hog");
    assert_eq!((hog.limits.memory, hog.restart), (Some(64 << 20), Restart::Never));
    let bomb = read("limits-bomb");
    assert_eq!((bomb.limits.threads, bomb.limits.memory), (64, None));
    assert_eq!(read("limits-files").limits.open_files, Some(10_000));
    assert_eq!(read("limits-steady").limits.threads, 4096);
    // Three readers of one disk: the weights 100 and 400, and a rate.
    let (light, heavy, rated) = (read("iow-light"), read("iow-heavy"), read("iow-rated"));
    assert_eq!(
        (light.limits.io_weight, heavy.limits.io_weight, rated.limits.io_weight),
        (100, 400, 100)
    );
    assert_eq!(rated.limits.io_read, Some(10 << 20));
    assert_eq!((light.limits.io_read, heavy.limits.io_read), (None, None));
    for reader in [&light, &heavy, &rated] {
        assert_eq!(reader.arguments, ["/devices/vda", "16", "64"]);
        let place = |path: &str, write| Place { path: path.into(), write };
        assert_eq!(reader.permissions.places(), [place("/devices/vda", false)]);
    }
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
    // The sockets' keys are in a commented `[[sockets]]` table.
    let in_sockets = |key: &str| {
        key.strip_prefix("sockets.").is_some_and(|name| {
            text.contains("# [[sockets]]\n") && text.contains(&format!("\n# {name} = "))
        })
    };
    for key in service::KEYS {
        assert!(
            text.contains(&format!("{key} ="))
                || text.contains(&format!("[{key}]"))
                || in_table(key)
                || in_sockets(key),
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
    let limits = &service.limits;
    assert_eq!((limits.io_weight, limits.threads, limits.open_files), (100, 4096, None));
    assert_eq!((limits.io_read, limits.io_write), (None, None));
    assert_eq!((limits.io_read_ops, limits.io_write_ops), (None, None));
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
fn a_service_may_limit_its_disks_threads_and_files() {
    let text = file(
        "[limits]\nio_weight = 400\nio_read = \"50M\"\nio_write = 1048576\nio_read_ops = 200\n\
         io_write_ops = \"none\"\nthreads = 64\nopen_files = 65536\n",
    );
    let limits = service::from_toml(&text).unwrap().limits;
    assert_eq!(limits.io_weight, 400);
    assert_eq!((limits.io_read, limits.io_write), (Some(50 << 20), Some(1 << 20)));
    assert_eq!((limits.io_read_ops, limits.io_write_ops), (Some(200), None));
    assert_eq!((limits.threads, limits.open_files), (64, Some(65536)));
    // The ends of each range are allowed.
    let ends = file("[limits]\nio_weight = 10000\nthreads = 1048576\nopen_files = 1048576\n");
    let limits = service::from_toml(&ends).unwrap().limits;
    assert_eq!(
        (limits.io_weight, limits.threads, limits.open_files),
        (10_000, 1 << 20, Some(1 << 20))
    );
    let least = file("[limits]\nio_weight = 1\nthreads = 1\nopen_files = 1\nio_read_ops = 1\n");
    let limits = service::from_toml(&least).unwrap().limits;
    assert_eq!((limits.io_weight, limits.threads, limits.open_files), (1, 1, Some(1)));
    assert_eq!(limits.io_read_ops, Some(1));
}

#[test]
fn a_limit_out_of_range_is_placed_on_its_value() {
    let weight = failure(&file("[limits]\nio_weight = 20000"));
    assert_eq!(place(&weight), (Some(4), Some(13), 5, Some("limits.io_weight")));
    assert_eq!(weight.message, "io_weight is from 1 to 10000, not 20000");
    let threads = failure(&file("[limits]\nthreads = 2000000"));
    assert_eq!(place(&threads), (Some(4), Some(11), 7, Some("limits.threads")));
    assert_eq!(threads.message, "threads is from 1 to 1048576, not 2000000");
    let files = failure(&file("[limits]\nopen_files = 0"));
    assert_eq!(place(&files), (Some(4), Some(14), 1, Some("limits.open_files")));
    assert_eq!(files.message, "open_files is from 1 to 1048576, not 0");
    // The second of two limits is placed on its own line.
    let second = failure(&file("[limits]\nthreads = 64\nio_read = \"fast\""));
    assert_eq!(place(&second), (Some(5), Some(11), 6, Some("limits.io_read")));
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
    let group = "root:x:0:\nadmins:x:10:alice\nphp-fpm-socket:x:105:nginx,web\nbroken:x:x:\n";
    assert_eq!(service::group_number(group, "php-fpm-socket"), Some(105));
    assert_eq!(service::group_number(group, "admins"), Some(10));
    assert_eq!(service::group_number(group, "php-fpm"), None);
    assert_eq!(service::group_number(group, "broken"), None);
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
    // Disks by class and the root's subvolumes by name (ADR 0473): kept
    // apart from the places, which Spark attaches as they are.
    let classes = service::from_toml(&file(
        "[permissions]\nfiles.disks = \"read\"\n\
         files.root-subvolumes = [\"@snapshots\", \"@system.new\", \"@system-a\"]",
    ))
    .unwrap();
    let files = &classes.permissions.files;
    assert_eq!(files.disks, Some(service::DeviceAccess::Read));
    assert!(files.efi_system.is_none() && classes.permissions.places().is_empty());
    assert_eq!(files.root_subvolumes, ["@snapshots", "@system.new", "@system-a"]);
    let efi = service::from_toml(&file("[permissions]\nfiles.efi-system = \"read-write\""));
    assert!(efi.unwrap().permissions.files.efi_system.is_some_and(service::DeviceAccess::write));
    let many: Vec<String> = (0..17).map(|n| format!("\"@{n}\"")).collect();
    let many = file(&format!("[permissions]\nfiles.root-subvolumes = [{}]", many.join(", ")));
    assert_eq!(error(&many), "line 4: files.root-subvolumes names at most 16 subvolumes");
    // Every process, by naming the place.
    let every = service::from_toml(&file("[permissions]\nfiles.read = [\"/processes\"]"));
    assert_eq!(every.unwrap().permissions.places(), [place("/processes", false)]);
    let mounts = file("[permissions]\nfiles.mount = [\"/a\", \"/b\", \"/c\", \"/d\", \"/e\"]");
    assert_eq!(error(&mounts), "line 3: files.mount names at most 4 places");
    for (secret, fits) in [("api-token", true), ("_x.1", true), ("", false), ("a/b", false)] {
        assert_eq!(service::check_secret(secret).is_ok(), fits, "{secret}");
    }
}

#[test]
fn the_certificates_a_service_names() {
    let service =
        service::from_toml(&file("certificates = [\"this-machine\", \"example.com\"]")).unwrap();
    assert_eq!(service.certificates, ["this-machine", "example.com"]);
    assert!(service::from_toml(&file("")).unwrap().certificates.is_empty());
    assert_eq!(service::certificates_dir("nginx"), "/system/live/certificates/nginx");
    let long = "a".repeat(64);
    for name in ["this-machine", "example.com", "a", "9", "mail.example.org", long.as_str()] {
        assert!(service::check_certificate(name).is_ok(), "{name}");
    }
    let too_long = "a".repeat(65);
    for name in ["", too_long.as_str(), ".x", "-x", "a..b", "A", "a/b", "a b", "\u{e9}", "a_b"] {
        assert_eq!(
            service::check_certificate(name),
            Err(format!(
                "`{}` is not a certificate's name: it has 1 to 64 lowercase letters, digits, \
                 `-` and `.`, starts with a letter or a digit, and has no `..`",
                name.escape_debug()
            )),
            "{name}"
        );
    }
    // certd makes them, and names none; any other service may.
    let certd = service::from_toml(&file("certificates = [\"this-machine\"]")).unwrap();
    assert_eq!(
        certd.check_certificates("certd"),
        Err("certd keeps the certificates and names none: leave out certificates".into())
    );
    assert!(certd.check_certificates("nginx").is_ok());
    assert!(service::from_toml(&file("")).unwrap().check_certificates("certd").is_ok());
    // A version 2 file names them too.
    assert!(service::from_toml(&file_2("certificates = [\"this-machine\"]")).is_ok());
    // A generation's folder is hidden, and no certificate's name.
    assert_eq!(service::certificate_generation("this-machine", 7), ".this-machine.7");
    assert_eq!(service::generation_of(".this-machine.7"), Some(("this-machine", 7)));
    assert_eq!(service::generation_of(".a.1.12"), Some(("a.1", 12)));
    for entry in [
        "this-machine",
        ".this-machine",
        ".this-machine.",
        ".x.07",
        ".x.0",
        ".x.+7",
        ".x.new",
        ".lock",
        "..x.1",
        ".X.1",
    ] {
        assert_eq!(service::generation_of(entry), None, "{entry}");
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
        ("[limits]\nio_weight = 0", "line 4: io_weight is from 1 to 10000, not 0"),
        ("[limits]\nio_weight = -1", "line 4: io_weight is not below 0"),
        (
            "[limits]\nio_read = \"fast\"",
            "line 4: io_read is a size such as \"64M\", \"512K\" or \"2G\", or \"none\", not `fast`",
        ),
        ("[limits]\nio_write = 0", "line 4: io_write is more than 0 bytes; \"none\" sets no limit"),
        (
            "[limits]\nio_read_ops = 0",
            "line 4: io_read_ops is 1 or more a second; \"none\" sets no limit",
        ),
        (
            "[limits]\nio_write_ops = \"many\"",
            "line 4: io_write_ops is a whole number a second, or \"none\", not `many`",
        ),
        ("[limits]\nthreads = 0", "line 4: threads is from 1 to 1048576, not 0"),
        ("[limits]\nthreads = 1048577", "line 4: threads is from 1 to 1048576, not 1048577"),
        ("[limits]\nopen_files = 1048577", "line 4: open_files is from 1 to 1048576, not 1048577"),
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
            "[permissions]\nfiles.disks = \"write\"",
            "line 4: unknown variant `write`, expected `read` or `read-write`",
        ),
        (
            "[permissions]\nfiles.disks = \"read\"\nfiles.efi-system = \"read\"",
            "line 3: files.disks holds the EFI system partitions already: leave out \
             files.efi-system",
        ),
        (
            "[permissions]\nfiles.efi-system = \"read-write\"\nfiles.read = [\"/devices/sda\"]",
            "line 3: files.disks and files.efi-system make its /devices, so its view cannot hold \
             /devices/sda apart: leave it out",
        ),
        (
            "[permissions]\nruns-with-your-access = true\nfiles.root-subvolumes = [\"@snapshots\"]",
            "line 3: runs-with-your-access holds every file already: leave out files.*",
        ),
        (
            "[permissions]\nfiles.root-subvolumes = [\"@snapshots\", \"@users\"]",
            "line 4: files.root-subvolumes cannot name @users, which holds the owner's files: an \
             update or a snapshot never needs them",
        ),
        (
            "[permissions]\nfiles.root-subvolumes = [\"@shared/media\"]",
            "line 4: files.root-subvolumes takes names at the top of the root, such as \
             \"@snapshots\": letters, digits, `@`, `.`, `-` and `_`, not `@shared/media`",
        ),
        (
            "[permissions]\nfiles.root-subvolumes = [\"..\"]",
            "line 4: files.root-subvolumes takes names at the top of the root, such as \
             \"@snapshots\": letters, digits, `@`, `.`, `-` and `_`, not `..`",
        ),
        (
            "[permissions]\nfiles.root-subvolumes = [\"@state\", \"@state\"]",
            "line 4: files.root-subvolumes names @state twice",
        ),
        (
            "[permissions]\nfiles.read = [\"/processes/1\"]",
            "line 3: a service sees its own processes at /processes already, so its view cannot \
             hold /processes/1 apart; name /processes with files.read to see every process",
        ),
        (
            "[permissions]\nfiles.shared = [\"../users\"]",
            "line 4: files.shared takes names of folders in /shared, such as \"media\", not \
             `../users`",
        ),
        ("secrets = [\"api-token\", \"api-token\"]", "line 3: secrets names api-token twice"),
        (
            "certificates = [\"this-machine\", \"this-machine\"]",
            "line 3: certificates names this-machine twice",
        ),
        (
            "certificates = [\"../key\"]",
            "line 3: `../key` is not a certificate's name: it has 1 to 64 lowercase letters, \
             digits, `-` and `.`, starts with a letter or a digit, and has no `..`",
        ),
        (
            "[permissions]\nfiles.read = [\"/system/live\"]",
            "line 3: no service holds /system/live/certificates, where Spark keeps each \
             service's copies of its certificates' keys, so it cannot hold /system/live",
        ),
        (
            "[permissions]\nfiles.read-write = [\"/system/live/certificates/web\"]",
            "line 3: no service holds /system/live/certificates, where Spark keeps each \
             service's copies of its certificates' keys, so it cannot hold \
             /system/live/certificates/web",
        ),
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

/// The error reading `text` gives, whole.
fn failure(text: &str) -> Error {
    service::from_toml(text).unwrap_err()
}

/// Where `error` is: its line, column, width and key.
fn place(error: &Error) -> (Option<usize>, Option<usize>, usize, Option<&str>) {
    (error.line, error.column, error.width, error.key.as_deref())
}

#[test]
fn an_unknown_key_is_placed_and_the_nearest_is_offered() {
    let wrong = failure(&file("discription = \"x\""));
    assert_eq!(place(&wrong), (Some(3), Some(1), 11, Some("discription")));
    assert_eq!(wrong.message, "`discription` is not a setting here; service.toml(5) lists them");
    assert_eq!(wrong.fix.as_deref(), Some("did you mean `description`?"));
    // What Spark logs says what it always did.
    assert_eq!(
        wrong.to_string(),
        "line 3: `discription` is not a setting here; service.toml(5) lists them"
    );
    // In a table, after what comes before it on its line.
    let dotted = failure(&file("[permissions]\nnetwork.outbond = true"));
    assert_eq!(place(&dotted), (Some(4), Some(9), 7, Some("permissions.network.outbond")));
    assert_eq!(dotted.fix.as_deref(), Some("did you mean `outbound`?"));
    let table = failure(&file("[limits]\nmemmory = \"64M\""));
    assert_eq!(place(&table), (Some(4), Some(1), 7, Some("limits.memmory")));
    assert_eq!(table.fix.as_deref(), Some("did you mean `memory`?"));
    // Nothing is near enough to guess at.
    let far = failure(&file("colour = 1"));
    assert_eq!(place(&far), (Some(3), Some(1), 6, Some("colour")));
    assert_eq!(far.fix, None);
}

#[test]
fn a_value_of_the_wrong_kind_is_placed_and_the_line_service_new_writes_is_offered() {
    let wrong = failure(&file_2("[health]\ncommand = [\"/x\"]\nevery = \"30\""));
    assert_eq!(place(&wrong), (Some(5), Some(9), 4, Some("health.every")));
    assert_eq!(wrong.fix.as_deref(), Some("as `service new` writes it: `health.every = 30`"));
    assert_eq!(
        wrong.to_string(),
        "line 5: invalid type: string \"30\", expected a whole number for health.every"
    );
    let order = failure(&file("order = \"first\""));
    assert_eq!(order.fix.as_deref(), Some("as `service new` writes it: `order = 60`"));
    // A table's key is written under its table in the template.
    let weight = failure(&file("[limits]\ncpu_weight = \"heavy\""));
    assert_eq!(weight.key.as_deref(), Some("limits.cpu_weight"));
    // No line of the template, no fix; an item of a list is no line to copy.
    let variable = failure(&file("[environment]\nTZ = 1"));
    assert_eq!(place(&variable), (Some(4), Some(6), 1, Some("environment.TZ")));
    assert_eq!(variable.fix, None);
    let item = failure(&file("arguments = [1]"));
    assert_eq!(place(&item), (Some(3), Some(14), 1, Some("arguments")));
    assert_eq!(item.fix, None);
}

#[test]
fn a_check_of_several_settings_is_placed_on_the_one_to_change() {
    let port = failure(&file_2("[ready]\nport = 8080"));
    assert_eq!(place(&port), (Some(4), Some(1), 4, Some("ready.port")));
    assert_eq!(port.message, "ready.port is for ready.when = \"listening\" alone");
    assert_eq!(port.fix, None);
    assert_eq!(port.to_string(), "ready.port is for ready.when = \"listening\" alone");
    // The fix after the colon is its own, and Display puts it back.
    let twice = failure(&file_2("requires = [\"db\"]\nafter = [\"db\"]"));
    assert_eq!(place(&twice), (Some(4), Some(1), 5, Some("after")));
    assert_eq!(twice.message, "db is in requires and after");
    assert_eq!(twice.fix.as_deref(), Some("requires says it already, so leave it out of after"));
    assert_eq!(
        twice.to_string(),
        "db is in requires and after: requires says it already, so leave it out of after"
    );
    let version = failure("service_version = 1\nprogram = \"/x\"\nrequires = []\n");
    assert_eq!(place(&version), (Some(1), Some(1), 15, Some("service_version")));
    assert_eq!(version.fix.as_deref(), Some("change the file's service_version to 2"));
    // A setting the file leaves at its default has its table named.
    let timeout = failure(&file_2("[health]\ncommand = [\"/x\"]\nevery = 5"));
    assert_eq!(place(&timeout), (Some(3), Some(2), 6, Some("health.timeout")));
}

#[test]
fn a_file_that_leaves_something_out_is_placed_nowhere() {
    let program = failure("service_version = 1\n");
    assert_eq!(place(&program), (None, None, 0, None));
    assert_eq!(program.message, "it names no program");
    assert_eq!(
        program.fix.as_deref(),
        Some("add a line such as `program = \"/system/commands/logd\"`")
    );
    assert_eq!(
        program.to_string(),
        "it names no program: add a line such as `program = \"/system/commands/logd\"`"
    );
    let version = failure("program = \"/x\"\n");
    assert_eq!(place(&version), (None, None, 0, None));
    assert_eq!(version.fix.as_deref(), Some("add `service_version = 1`"));
}

#[test]
fn an_override_s_mistakes_are_placed_in_the_override() {
    let base = file_2("[ready]\nwhen = \"started\"\n");
    let over = |text: &str| Override::from_toml(text).unwrap();
    let typo =
        service::from_toml_over(&base, Some(&over("# a typo\ndiscription = \"x\"\n"))).unwrap_err();
    assert!(typo.in_override);
    assert_eq!(place(&typo), (Some(2), Some(1), 11, Some("discription")));
    assert_eq!(typo.fix.as_deref(), Some("did you mean `description`?"));
    assert_eq!(
        typo.to_string(),
        "line 2 of its override: `discription` is not a setting here; service.toml(5) lists them"
    );
    let kind =
        service::from_toml_over(&base, Some(&over("[health]\nevery = \"30\"\n"))).unwrap_err();
    assert!(kind.in_override);
    assert_eq!(place(&kind), (Some(2), Some(9), 4, Some("health.every")));
    // A check that an override's key fails is on that key, there.
    let port = service::from_toml_over(&base, Some(&over("\n[ready]\nport = 8080\n"))).unwrap_err();
    assert!(port.in_override);
    assert_eq!(place(&port), (Some(3), Some(1), 4, Some("ready.port")));
    // One that the file's own key fails stays in the file.
    let broken = file_2("[ready]\nport = 8080\n");
    let own = service::from_toml_over(&broken, Some(&over("description = \"web\"\n"))).unwrap_err();
    assert!(!own.in_override);
    assert_eq!(place(&own), (Some(4), Some(1), 4, Some("ready.port")));
    // And a file that an override mends reads.
    assert!(service::from_toml_over(&broken, Some(&over("ready.when = \"listening\"\n"))).is_ok());
}

#[test]
fn a_report_is_a_compilers() {
    let text = file("discription = \"x\"");
    let at = Source { path: "web.toml", text: &text };
    let wrong = failure(&text);
    assert_eq!(
        wrong.report(at, None),
        "web.toml:3:1: `discription` is not a setting here; service.toml(5) lists them\n  \
         |\n3 | discription = \"x\"\n  | ^^^^^^^^^^^\n  = fix: did you mean `description`?"
    );
    assert_eq!(
        wrong.brief(at, None),
        "web.toml:3:1: `discription` is not a setting here; service.toml(5) lists them: did you \
         mean `description`?"
    );
    // A line, and no column in it.
    let line = Error::at(2, "it is wrong").with_fix("do this");
    assert_eq!(
        line.report(at, None),
        "web.toml:2: it is wrong\n  |\n2 | program = \"/system/commands/true\"\n  = fix: do this"
    );
    assert_eq!(line.brief(at, None), "web.toml:2: it is wrong: do this");
    // No place: the fix follows the message.
    let nowhere = failure("service_version = 1\n");
    let at_nowhere = Source { path: "web.toml", text: "service_version = 1\n" };
    assert_eq!(
        nowhere.report(at_nowhere, None),
        "web.toml: it names no program\n  = fix: add a line such as `program = \
         \"/system/commands/logd\"`"
    );
    assert_eq!(Error::new("it is wrong").report(at, None), "web.toml: it is wrong");
    // Tabs stay tabs, so the carets line up under them.
    let indented = file("\tdiscription = \"x\"");
    let tabbed = failure(&indented).report(Source { path: "web.toml", text: &indented }, None);
    assert!(tabbed.contains("\n3 | \tdiscription = \"x\"\n  | \t^^^^^^^^^^^\n"), "{tabbed}");
    assert!(tabbed.starts_with("web.toml:3:2: "), "{tabbed}");
    // The gutter is as wide as the line's number.
    let far = file(&format!("{}discription = \"x\"", "\n".repeat(9)));
    let deep = failure(&far).report(Source { path: "web.toml", text: &far }, None);
    assert!(
        deep.contains("\n   |\n12 | discription = \"x\"\n   | ^^^^^^^^^^^\n   = fix: "),
        "{deep}"
    );
}

#[test]
fn a_report_of_an_override_s_mistake_says_what_it_is_set_over() {
    let base = file_2("[ready]\nwhen = \"started\"\n");
    let text = "\n[ready]\nport = 8080\n";
    let over = Override::from_toml(text).unwrap();
    let error = service::from_toml_over(&base, Some(&over)).unwrap_err();
    let file = Source { path: "/s/web.toml", text: &base };
    let in_over = Source { path: "/s/settings.d/web.toml", text };
    assert_eq!(
        error.report(file, Some(in_over)),
        "/s/settings.d/web.toml:3:1: ready.port is for ready.when = \"listening\" alone\n  \
         |\n3 | port = 8080\n  | ^^^^\n  = note: set over /s/web.toml"
    );
    assert_eq!(
        error.brief(file, Some(in_over)),
        "/s/settings.d/web.toml:3:1: ready.port is for ready.when = \"listening\" alone"
    );
}

#[test]
fn a_key_is_found_in_a_file() {
    let text = "a = 1\n[t]\nb = 2\nc.d = 3\ne = { f = 4 }\n";
    let found = |key: &str| service::key_span(text, key).map(|span| &text[span]);
    assert_eq!(found("a"), Some("a"));
    assert_eq!(found("t.b"), Some("b"));
    assert_eq!(found("t.c.d"), Some("d"));
    assert_eq!(found("t.e.f"), Some("f"));
    assert_eq!(found("t.c"), Some("c"));
    assert_eq!(found("t"), Some("t"));
    assert_eq!(found("t.z"), None);
    assert_eq!(found("a.b"), None);
    assert_eq!(found("nothing"), None);
    assert_eq!(service::key_span("a = ", "a"), None);
    // The span is where the key is in the text, not in its table.
    let span = service::key_span(text, "t.b").unwrap();
    assert_eq!(text[..span.start].matches('\n').count(), 2);
}

#[test]
fn the_template_with_its_settings_set_reads() {
    let text = service::template_with_settings();
    assert!(toml::de::DeTable::parse(&text).is_ok());
    for (key, line) in [
        ("order", Some("order = 60")),
        ("restart", Some("restart = \"on-failure\"")),
        ("limits.cpu_weight", Some("cpu_weight = 100")),
        ("ready.when", Some("ready.when = \"started\"")),
        ("health.every", Some("health.every = 30")),
        ("health.command", Some("health.command = [\"/system/commands/true\"]")),
        ("environment.TZ", None),
        ("nothing", None),
    ] {
        assert_eq!(service::template_line(key).as_deref(), line, "{key}");
    }
}

#[test]
fn the_nearest_key_is_the_closest_within_two_edits() {
    let near = |name: &str, keys: &[&'static str]| crate::nearest(name, keys.iter().copied());
    // Equal, and one, two and three edits away.
    assert_eq!(near("order", &["order"]), Some("order"));
    assert_eq!(near("ordr", &["order"]), Some("order"));
    assert_eq!(near("ordre", &["order"]), Some("order"));
    assert_eq!(near("oder", &["order"]), Some("order"));
    assert_eq!(near("rder", &["order"]), Some("order"));
    assert_eq!(near("mem", &["memory"]), None);
    assert_eq!(near("", &["memory"]), None);
    assert_eq!(near("x", &[]), None);
    // The closest wins, even after one that is near; ties go to the first.
    assert_eq!(near("memry", &["mem", "memory"]), Some("memory"));
    assert_eq!(near("ab", &["aa", "bb"]), Some("aa"));
    assert_eq!(near("ab", &["abcd", "abc"]), Some("abc"));
    // Characters, not bytes.
    assert_eq!(near("caf\u{e9}", &["cafe"]), Some("cafe"));
}

#[test]
fn the_settings_that_differ_are_those_of_the_files_with_their_overrides() {
    let base = file("restart = \"never\"\n[limits]\ncpu_weight = 100\n");
    let heavier = base.replace("100", "200");
    let over = |text: &str| Override::from_toml(text).unwrap();
    let always = over("restart = \"always\"\n");
    let differ = |old: (&str, Option<&Override>), new: (&str, Option<&Override>)| {
        service::changed_settings(old, new).unwrap()
    };
    assert!(differ((&base, None), (&base, None)).is_empty());
    assert_eq!(differ((&base, None), (&heavier, None)), ["limits.cpu_weight"]);
    // An override sets keys over the file's.
    assert_eq!(differ((&base, None), (&base, Some(&always))), ["restart"]);
    assert_eq!(differ((&base, Some(&always)), (&base, None)), ["restart"]);
    // What the override sets, the file no longer decides.
    let other = base.replace("never", "on-failure");
    assert!(differ((&base, Some(&always)), (&other, Some(&always))).is_empty());
    assert_eq!(
        differ((&base, Some(&always)), (&heavier.replace("never", "on-failure"), Some(&always))),
        ["limits.cpu_weight"]
    );
    // A table's keys one by one.
    assert_eq!(
        differ((&base, None), (&base, Some(&over("[limits]\nmemory = \"64M\"\n")))),
        ["limits.memory"]
    );
    // A text that is not TOML, with its line.
    let error = service::changed_settings(("a = \n", None), (&base, None)).unwrap_err();
    assert_eq!(error.line, Some(1));
    // changed_keys compares two texts alone.
    assert_eq!(service::changed_keys(&base, &heavier).unwrap(), ["limits.cpu_weight"]);
}

#[test]
fn a_timer_is_read_with_its_defaults() {
    let service =
        service::from_toml(&file("[timer]\nat = [\"daily\", \"30 2 * * 1-5\"]\n")).unwrap();
    let timer = service.timer.unwrap();
    assert_eq!(timer.at.len(), 2);
    assert_eq!(timer.at[0], crate::schedule::word("daily").unwrap());
    assert_eq!((timer.every, timer.after_boot, timer.spread), (None, None, 0));
    assert!(timer.catch_up);
    assert_eq!(timer.time_zone, None);
    assert!(service::from_toml(&file("")).unwrap().timer.is_none());
}

#[test]
fn every_timer_setting() {
    let text = file(
        "[timer]\nevery = \"6h\"\nafter_boot = \"10m\"\nspread = \"1h30m\"\ncatch_up = false\n\
         time_zone = \"Europe/Berlin\"\nat = [\"hourly\"]\n",
    );
    let timer = service::from_toml(&text).unwrap().timer.unwrap();
    assert_eq!(timer.every, Some(21_600));
    assert_eq!(timer.after_boot, Some(600));
    assert_eq!(timer.spread, 5400);
    assert!(!timer.catch_up);
    assert_eq!(timer.time_zone.as_deref(), Some("Europe/Berlin"));
}

#[test]
fn a_wrong_timer_is_refused_where_it_is_wrong() {
    let at = |body: &str| service::from_toml(&file(body)).unwrap_err();
    let err = at("[timer]\n");
    assert!(err.message.contains("`at` or `every`"), "{err}");
    let err = at("[timer]\nat = [\"daily\"]\nafter_boot = \"1m\"\n");
    assert!(err.message.contains("goes with `every`"), "{err}");
    assert_eq!(err.key.as_deref(), Some("timer.after_boot"));
    let err = at("[timer]\nat = [\"@daily\"]\n");
    assert!(err.message.contains("without the @"), "{err}");
    let err = at("[timer]\nat = [\"daily\", \"61 * * * *\"]\n");
    assert!(err.message.contains("item 2") && err.message.contains("column 1"), "{err}");
    assert_eq!(err.line, Some(4));
    for every in ["\"0s\"", "\"1x\"", "\"367d\"", "6", "\"\""] {
        let err = at(&format!("[timer]\nevery = {every}\n"));
        assert!(
            err.message.contains("timer.every") || err.message.contains("invalid type"),
            "{err}"
        );
    }
    let err = at("[timer]\nevery = \"1h\"\nspread = \"soon\"\n");
    assert!(err.message.contains("timer.spread"), "{err}");
    for zone in ["../etc/passwd", "/UTC", "A//B", "UTC/..", "", "a b"] {
        let err = at(&format!("[timer]\nevery = \"1h\"\ntime_zone = \"{zone}\"\n"));
        assert!(err.message.contains("time zone"), "{zone}: {err}");
    }
    let err = at("[timer]\nevery = \"1h\"\nsometimes = true\n");
    assert!(err.message.contains("sometimes"), "{err}");
}

#[test]
fn a_timer_can_be_written_as_dotted_keys() {
    let service = service::from_toml(&file("timer.every = \"20s\"\n")).unwrap();
    assert_eq!(service.timer.unwrap().every, Some(20));
}

#[test]
fn logs_are_read_with_their_defaults() {
    let service = service::from_toml(&file("[logs]\nfiles = [\"logs/*.log\"]\n")).unwrap();
    let logs = service.logs.unwrap();
    assert_eq!(logs.files, ["logs/*.log"]);
    assert_eq!(logs.turn_over, service::TurnOver::Daily);
    assert_eq!((logs.keep, logs.compress), (14, true));
    assert_eq!(logs.reopen, service::Reopen::Nothing);
}

#[test]
fn every_logs_setting() {
    let text = file(
        "[logs]\nfiles = [\"a/*.log\", \"/var/b.log\"]\nturn_over = \"100M\"\nkeep = 100\n\
         compress = false\nreopen = \"SIGUSR1\"\n",
    );
    let logs = service::from_toml(&text).unwrap().logs.unwrap();
    assert_eq!(logs.files, ["a/*.log", "/var/b.log"]);
    assert_eq!(logs.turn_over, service::TurnOver::Size(100 << 20));
    assert_eq!((logs.keep, logs.compress), (100, false));
    assert_eq!(logs.reopen, service::Reopen::Signal(service::Signal::Usr1));
    for (word, want) in [
        ("weekly", service::TurnOver::Weekly),
        ("64K", service::TurnOver::Size(64 << 10)),
        ("4096", service::TurnOver::Size(4096)),
    ] {
        let text = file(&format!("[logs]\nfiles = [\"x.log\"]\nturn_over = \"{word}\"\n"));
        assert_eq!(service::from_toml(&text).unwrap().logs.unwrap().turn_over, want, "{word}");
    }
    let text = file("[logs]\nfiles = [\"x.log\"]\nturn_over = 8192\nreopen = \"restart\"\n");
    let logs = service::from_toml(&text).unwrap().logs.unwrap();
    assert_eq!(logs.turn_over, service::TurnOver::Size(8192));
    assert_eq!(logs.reopen, service::Reopen::Restart);
}

#[test]
fn wrong_logs_are_refused() {
    let at = |body: &str| service::from_toml(&file(&format!("[logs]\n{body}"))).unwrap_err();
    assert!(at("keep = 3\n").message.contains("files"), "a file with none");
    assert!(at("files = []\n").message.contains("1 to 16"));
    for pattern in
        ["a/*/b.log", "../x.log", "/", "a//b.log", "x.*", "x?.log", "./x.log", "x[1].log"]
    {
        let err = at(&format!("files = [\"{pattern}\"]\n"));
        assert!(err.message.contains("logs.files"), "{pattern}: {err}");
    }
    assert!(at("files = [\"a.log\", \"a.log\"]\n").message.contains("twice"));
    for (key, value) in [
        ("turn_over", "\"hourly\""),
        ("turn_over", "0"),
        ("turn_over", "\"none\""),
        ("keep", "0"),
        ("keep", "101"),
        ("reopen", "\"USR1\""),
        ("reopen", "\"SIGTERM\""),
        ("reopen", "\"SIGQUIT\""),
        ("reopen", "\"later\""),
    ] {
        let err = at(&format!("files = [\"x.log\"]\n{key} = {value}\n"));
        assert!(err.message.contains(&format!("logs.{key}")), "{key} = {value}: {err}");
        assert_eq!(err.line, Some(5), "{key} = {value}");
    }
}

/// A version 1 file that listens on ports 80, 8090 and 9000 to 9009,
/// with `top` before its tables and `tables` after them.
fn socket_file(top: &str, tables: &str) -> String {
    format!(
        "service_version = 1\nprogram = \"/system/commands/true\"\n{top}\n{tables}\n\
         [permissions]\nnetwork.listen = [80, 8090, \"9000-9009\"]\n"
    )
}

#[test]
fn a_service_names_the_sockets_spark_holds() {
    use service::{BACKLOG_DEFAULT, SocketAddress, SocketKind, Start};
    let text = socket_file(
        "start = \"on-connection\"",
        "[[sockets]]\nport = 80\naddress = \"0.0.0.0\"\n\
         [[sockets]]\nport = 80\naddress = \"::\"\nname = \"web6\"\nbacklog = 128\nnon-blocking = true\n\
         [[sockets]]\nport = 9000\nudp = true\naddress = \"192.168.1.5\"\n\
         [[sockets]]\nport = 8090\naddress = \"::1\"\n\
         [[sockets]]\nport = 9001\n\
         [[sockets]]\npath = \"db\"\n",
    );
    let service = service::from_toml(&text).unwrap();
    assert_eq!(service.start, Start::OnConnection);
    let kinds: Vec<&SocketKind> = service.sockets.iter().map(|socket| &socket.kind).collect();
    let mut loopback = [0; 16];
    loopback[15] = 1;
    assert_eq!(
        kinds,
        [
            &SocketKind::Stream { port: 80, address: SocketAddress::AnyV4 },
            &SocketKind::Stream { port: 80, address: SocketAddress::AnyV6 },
            &SocketKind::Datagram { port: 9000, address: SocketAddress::V4([192, 168, 1, 5]) },
            &SocketKind::Stream { port: 8090, address: SocketAddress::V6(loopback) },
            &SocketKind::Stream { port: 9001, address: SocketAddress::Any },
            &SocketKind::Unix { path: "db".into() },
        ]
    );
    let names: Vec<&str> = service.sockets.iter().map(|socket| socket.name.as_str()).collect();
    assert_eq!(names, ["80", "web6", "9000", "8090", "9001", "db"]);
    let backlogs: Vec<u32> = service.sockets.iter().map(|socket| socket.backlog).collect();
    assert_eq!(backlogs, [BACKLOG_DEFAULT, 128, BACKLOG_DEFAULT, BACKLOG_DEFAULT, 4096, 4096]);
    // Blocking unless the file says, as systemd's NonBlocking= is.
    let non_blocking: Vec<bool> =
        service.sockets.iter().map(|socket| socket.non_blocking).collect();
    assert_eq!(non_blocking, [false, true, false, false, false, false]);
}

#[test]
fn a_file_without_sockets_starts_at_boot_with_no_groups() {
    let service = service::from_toml(&file("")).unwrap();
    assert!(service.sockets.is_empty() && service.groups.is_empty());
    assert_eq!(service.start, service::Start::AtBoot);
    let text = file("start = \"at-boot\"\ngroups = [\"web\", \"mail.out\"]");
    let service = service::from_toml(&text).unwrap();
    assert_eq!(service.groups, ["web", "mail.out"]);
}

#[test]
fn a_unix_socket_says_who_connects_besides_its_service() {
    use service::SocketAccess;
    let text = socket_file(
        "",
        "[[sockets]]\npath = \"own\"\n[[sockets]]\npath = \"all\"\nto = \"anyone\"\n\
         [[sockets]]\npath = \"root\"\nto = \"system\"\n\
         [[sockets]]\npath = \"fpm\"\ngroup = \"php-fpm-socket\"\n[[sockets]]\nport = 80\n",
    );
    let service = service::from_toml(&text).unwrap();
    let access: Vec<&SocketAccess> = service.sockets.iter().map(|socket| &socket.access).collect();
    assert_eq!(
        access,
        [
            &SocketAccess::Own,
            &SocketAccess::Anyone,
            &SocketAccess::System,
            &SocketAccess::Group("php-fpm-socket".into()),
            &SocketAccess::Own,
        ]
    );
    assert!(!SocketAccess::Own.offered() && SocketAccess::System.offered());
    // Each mistake on its key, or its line for a value that does not
    // read: a port's reach is the firewall's to say.
    for (tables, key, said) in [
        (
            "[[sockets]]\nport = 80\nto = \"anyone\"",
            "sockets.to",
            "to is for a socket with a path alone",
        ),
        (
            "[[sockets]]\nport = 80\ngroup = \"web\"",
            "sockets.group",
            "group is for a socket with a path alone",
        ),
        (
            "[[sockets]]\npath = \"a\"\nto = \"anyone\"\ngroup = \"web\"",
            "sockets.group",
            "a socket has a `to` or a `group`, not both",
        ),
        (
            "[[sockets]]\npath = \"a\"\nto = \"everyone\"",
            "line 6",
            "a socket's to is \"anyone\" or",
        ),
        ("[[sockets]]\npath = \"a\"\ngroup = \"Web Users\"", "line 6", "a socket's group:"),
    ] {
        let error = failure(&socket_file("", tables));
        assert!(error.message.contains(said), "{tables}: {}", error.message);
        match key.strip_prefix("line ") {
            Some(line) => assert_eq!(error.line, line.parse().ok(), "{tables}"),
            None => assert_eq!(error.key.as_deref(), Some(key), "{tables}"),
        }
    }
}

#[test]
fn a_socket_that_does_not_fit_is_placed_on_its_key() {
    // Each case: the tables, the key's line and column, and what is said.
    let cases: &[(&str, (usize, usize), &str)] = &[
        (
            "[[sockets]]\nport = 80\npath = \"db\"",
            (6, 1),
            "a socket has a port or a path, not both",
        ),
        ("[[sockets]]\nname = \"db\"", (4, 1), "a socket needs a port or a path"),
        ("[[sockets]]\npath = \"db\"\nudp = true", (6, 1), "udp is for a socket with a port alone"),
        (
            "[[sockets]]\npath = \"db\"\naddress = \"::\"",
            (6, 1),
            "address is for a socket with a port alone",
        ),
        (
            "[[sockets]]\nport = 80\n[[sockets]]\nport = 81",
            (7, 1),
            "port 81 is none of network.listen's ports",
        ),
        (
            "[[sockets]]\nport = 80\n[[sockets]]\nport = 80\nudp = true",
            (6, 1),
            "two sockets are named 80",
        ),
        (
            "[[sockets]]\npath = \"x\"\nname = \"x\"\n[[sockets]]\nport = 80\nname = \"x\"",
            (9, 1),
            "two sockets are named x",
        ),
    ];
    for (tables, (line, column), said) in cases {
        let error = failure(&socket_file("", tables));
        assert!(error.message.contains(said), "{tables}: {}", error.message);
        assert_eq!((error.line, error.column), (Some(*line), Some(*column)), "{tables}");
    }
    let listen = failure(&socket_file("", "[[sockets]]\nport = 81"));
    assert_eq!(listen.fix.as_deref(), Some("add 81 to network.listen"));
    assert_eq!(listen.key.as_deref(), Some("sockets.port"));
    // A table with neither is placed on the table.
    let neither = failure(&socket_file("", "[[sockets]]\nname = \"db\""));
    assert_eq!(neither.key.as_deref(), Some("sockets"));
}

#[test]
fn a_socket_s_values_are_checked_where_they_are_written() {
    let cases: &[(&str, &str)] = &[
        ("port = 0", "a socket's port is from 1 to 65535, not 0"),
        ("port = 70000", "a socket's port is from 1 to 65535, not 70000"),
        ("port = 80\naddress = \"localhost\"", "a socket's address is \"0.0.0.0\", \"::\""),
        ("port = 80\naddress = \"1.2.3\"", "not `1.2.3`"),
        ("path = \"a/b\"", "a socket's path is a plain name"),
        ("path = \"..\"", "a socket's path is a plain name"),
        ("path = \".\"", "a socket's path is a plain name"),
        ("path = \"\"", "a socket's path is a plain name"),
        ("port = 80\nname = \"a:b\"", "a socket's name is 1 to 64 letters"),
        ("port = 80\nname = \"\"", "a socket's name is 1 to 64 letters"),
        ("port = 80\nbacklog = 0", "a socket's backlog is from 1 to 65535 connections, not 0"),
        ("port = 80\nbacklog = 65536", "not 65536"),
        ("port = 80\nwarm = true", "`warm` is not a setting here"),
    ];
    for (table, said) in cases {
        let error = failure(&socket_file("", &format!("[[sockets]]\n{table}")));
        assert!(error.message.contains(said), "{table}: {}", error.message);
        // Each is on the line of the value, below the table's own.
        assert!(error.line.is_some_and(|line| line >= 5), "{table}: {:?}", error.line);
    }
    let long = "a".repeat(service::UNIX_NAME_MAX + 1);
    let error = failure(&socket_file("", &format!("[[sockets]]\npath = \"{long}\"")));
    assert!(error.message.contains("a socket's path is a plain name"));
    let longest = "a".repeat(service::UNIX_NAME_MAX);
    service::from_toml(&socket_file("", &format!("[[sockets]]\npath = \"{longest}\""))).unwrap();
}

#[test]
fn a_file_names_at_most_sixteen_sockets() {
    let socket = |n: usize| format!("[[sockets]]\npath = \"s{n}\"\n");
    let sixteen: String = (0..service::SOCKETS_MAX).map(socket).collect();
    assert_eq!(service::from_toml(&socket_file("", &sixteen)).unwrap().sockets.len(), 16);
    let seventeen = format!("{sixteen}{}", socket(16));
    assert!(failure(&socket_file("", &seventeen)).message.contains("at most 16 sockets"));
}

#[test]
fn a_port_of_a_socket_is_one_of_network_listen_for_udp_too() {
    let udp = socket_file("", "[[sockets]]\nport = 9005\nudp = true");
    assert!(service::from_toml(&udp).is_ok());
    let outside = socket_file("", "[[sockets]]\nport = 9010\nudp = true");
    assert!(failure(&outside).message.contains("port 9010 is none of network.listen's ports"));
    // A file that listens on nothing has no socket to make.
    let none = file("[[sockets]]\nport = 80");
    assert!(failure(&none).message.contains("port 80 is none of network.listen's ports"));
}

#[test]
fn starting_on_a_connection_needs_a_socket() {
    let error = failure(&file("start = \"on-connection\""));
    assert_eq!(error.key.as_deref(), Some("start"));
    assert_eq!((error.line, error.column), (Some(3), Some(1)));
    assert!(error.message.contains("waits for a connection"));
    assert_eq!(error.fix.as_deref(), Some("add a [[sockets]] table"));
    let bad = failure(&file("start = \"lazily\""));
    assert!(bad.message.contains("start must be at-boot or on-connection, not `lazily`"));
}

#[test]
fn groups_are_account_style_names_at_most_sixteen_each_once() {
    for (groups, said) in [
        ("[\"Web\"]", "groups: `Web` is not an account's name"),
        ("[\"web\", \"web\"]", "groups names web twice"),
        ("[\"\"]", "groups: ``"),
        ("[\"a:b\"]", "groups: `a:b`"),
    ] {
        let error = failure(&file(&format!("groups = {groups}")));
        assert!(error.message.contains(said), "{groups}: {}", error.message);
        assert_eq!(error.line, Some(3), "{groups}");
    }
    let many: Vec<String> = (0..17).map(|n| format!("\"g{n}\"")).collect();
    let error = failure(&file(&format!("groups = [{}]", many.join(", "))));
    assert!(error.message.contains("groups names at most 16 groups"));
    let sixteen = &many[..16];
    assert!(service::from_toml(&file(&format!("groups = [{}]", sixteen.join(", ")))).is_ok());
}

#[test]
fn an_override_may_replace_a_service_s_sockets() {
    let text = socket_file("", "[[sockets]]\nport = 80");
    let over = Override::from_toml("sockets = [{ port = 8090, name = \"alt\" }]\n").unwrap();
    let service = service::from_toml_over(&text, Some(&over)).unwrap();
    assert_eq!(service.sockets.len(), 1);
    assert_eq!(service.sockets[0].name, "alt");
    let bad = Override::from_toml("sockets = [{ port = 81 }]\n").unwrap();
    let error = service::from_toml_over(&text, Some(&bad)).unwrap_err();
    assert!(error.in_override);
    assert_eq!((error.line, error.key.as_deref()), (Some(1), Some("sockets.port")));
}

/// `text` after the lines of a file that sets nothing else.
fn stop_file(version: u32, text: &str) -> String {
    format!("service_version = {version}\nprogram = \"/system/commands/true\"\n{text}\n")
}

#[test]
fn a_service_stops_with_the_signal_and_time_its_file_says() {
    use service::{STOP_TIMEOUT_DEFAULT, STOP_TIMEOUT_MAX, Signal, Stop};
    assert_eq!(Stop::default(), Stop { signal: Signal::Term, timeout: 5 });
    assert_eq!((STOP_TIMEOUT_DEFAULT, STOP_TIMEOUT_MAX), (5, 3600));
    assert_eq!(service::from_toml(&file("")).unwrap().stop, Stop::default());
    assert_eq!(service::from_toml(&stop_file(2, "")).unwrap().stop, Stop::default());
    for (word, signal) in [
        ("SIGTERM", Signal::Term),
        ("SIGINT", Signal::Int),
        ("SIGQUIT", Signal::Quit),
        ("SIGUSR1", Signal::Usr1),
        ("SIGUSR2", Signal::Usr2),
        ("SIGWINCH", Signal::Winch),
    ] {
        let text = stop_file(2, &format!("[stop]\nsignal = \"{word}\""));
        let stop = service::from_toml(&text).unwrap().stop;
        assert_eq!(stop, Stop { signal, timeout: 5 }, "{word}");
    }
    let timeout = |n: u32| service::from_toml(&stop_file(2, &format!("stop.timeout = {n}")));
    assert_eq!(timeout(1).unwrap().stop.timeout, 1);
    assert_eq!(timeout(3600).unwrap().stop.timeout, 3600);
    for n in [0, 3601] {
        let said = format!("stop.timeout is from 1 to 3600 seconds, not {n}");
        assert!(timeout(n).unwrap_err().message.contains(&said), "{n}");
    }
    let both = stop_file(2, "stop = { signal = \"SIGQUIT\", timeout = 30 }");
    assert_eq!(service::from_toml(&both).unwrap().stop, Stop { signal: Signal::Quit, timeout: 30 });
}

#[test]
fn a_stop_signal_that_is_no_request_is_refused_with_why() {
    let error = |word: &str| failure(&stop_file(2, &format!("[stop]\nsignal = \"{word}\"")));
    let hup = error("SIGHUP");
    assert!(hup.message.contains("`service reload` sends"), "{}", hup.message);
    assert_eq!((hup.line, hup.column), (Some(4), Some(10)));
    for word in ["SIGKILL", "SIGSTOP"] {
        assert!(error(word).message.contains("which a program cannot take as a request"));
    }
    let wrong = error("TERM");
    assert!(wrong.message.contains(
        "stop.signal must be SIGTERM, SIGINT, SIGQUIT, SIGUSR1, SIGUSR2 or SIGWINCH, not `TERM`"
    ));
}

#[test]
fn a_version_1_file_may_not_use_stop() {
    assert_eq!(
        error(&stop_file(1, "[stop]\ntimeout = 9")),
        "stop needs service_version = 2: change the file's service_version to 2"
    );
}

#[test]
fn an_override_changes_the_stop_timeout_alone() {
    let text = stop_file(2, "[stop]\nsignal = \"SIGINT\"\ntimeout = 300");
    let over = Override::from_toml("[stop]\ntimeout = 2\n").unwrap();
    let stop = service::from_toml_over(&text, Some(&over)).unwrap().stop;
    assert_eq!((stop.signal, stop.timeout), (service::Signal::Int, 2));
    let bad = Override::from_toml("[stop]\ntimeout = 0\n").unwrap();
    assert!(service::from_toml_over(&text, Some(&bad)).unwrap_err().in_override);
}
