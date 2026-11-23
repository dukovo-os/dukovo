// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::io;

use super::*;
use crate::ipmi::sim::{self, Sim};
use crate::json;

fn args(items: &[&str]) -> Args {
    Args::new("bmc", items.iter().map(OsString::from).collect())
}

fn parsed(items: &[&str]) -> Options {
    parse(&mut args(items)).unwrap()
}

fn refused(items: &[&str]) -> String {
    parse(&mut args(items)).unwrap_err().message
}

/// A BMC with an inlet temperature, a discrete power supply sensor, a
/// sensor of another controller, a non-linear fan, one whose scanning is
/// off and one that is unavailable.
fn bmc_with_sensors() -> Sim {
    let mut bmc = Sim::new();
    let mut other_owner = sim::full_record(3, 5, "Exhaust Temp");
    other_owner[5] = 0x30;
    let mut fan = sim::full_record(4, 6, "Fan Curve");
    fan[23] = 0x70;
    fan[12] = 0x04;
    fan[21] = 18;
    bmc.sdr = vec![
        sim::full_record(1, 4, "Inlet Temp"),
        sim::compact_record(2, 9, "PS Status", 1),
        other_owner,
        fan,
        sim::full_record(5, 7, "Idle Temp"),
        sim::full_record(6, 8, "Gone Temp"),
    ];
    // Raw 100 is 30 degrees with no threshold crossed.
    bmc.readings.insert(4, vec![100, 0x40, 0x00]);
    bmc.readings.insert(9, vec![0, 0xC0, 0x03, 0x00]);
    bmc.readings.insert(6, vec![50, 0x40, 0x08]);
    bmc.readings.insert(7, vec![0, 0x00, 0x00]);
    bmc.readings.insert(8, vec![0, 0x20, 0x00]);
    // The fan's constants at its reading: 2 * 50 + 10 * 10 = 200 RPM.
    bmc.factors.insert((6, 50), [0x02, 0x00, 0x0A, 0x00, 0x00, 0x01]);
    bmc
}

fn json_of(bmc: &mut Sim, command: &Command) -> String {
    run(bmc, command).unwrap().json().unwrap()
}

#[test]
fn a_bare_bmc_is_the_summary() {
    assert_eq!(parsed(&[]), Options { command: Command::Summary, json: false });
    assert_eq!(parsed(&["--json"]), Options { command: Command::Summary, json: true });
}

#[test]
fn the_commands_are_read_with_their_options() {
    assert_eq!(parsed(&["sensors"]).command, Command::Sensors);
    assert_eq!(parsed(&["--json", "sensors"]), Options { command: Command::Sensors, json: true });
    assert_eq!(parsed(&["events"]).command, Command::Events { clear: false });
    assert_eq!(parsed(&["events", "--clear"]).command, Command::Events { clear: true });
    assert!(parsed(&["fru", "--json"]).json);
    assert_eq!(parsed(&["lan"]).command, Command::Lan);
}

#[test]
fn identify_takes_seconds_on_or_off() {
    assert_eq!(parsed(&["identify"]).command, Command::Identify(Identify::Seconds(15)));
    assert_eq!(parsed(&["identify", "30"]).command, Command::Identify(Identify::Seconds(30)));
    assert_eq!(parsed(&["identify", "255"]).command, Command::Identify(Identify::Seconds(255)));
    assert_eq!(parsed(&["identify", "on"]).command, Command::Identify(Identify::ForceOn));
    assert_eq!(parsed(&["identify", "off"]).command, Command::Identify(Identify::Off));
    assert!(refused(&["identify", "0"]).contains("from 1 to 255"));
    assert!(refused(&["identify", "256"]).contains("from 1 to 255"));
    assert!(refused(&["identify", "soon"]).contains("'soon'"));
    assert!(refused(&["identify", "5", "6"]).contains("unexpected argument '6'"));
}

#[test]
fn lan_set_takes_an_address_with_its_prefix_and_a_gateway_or_dhcp() {
    let expected = Command::LanStatic {
        address: Ipv4Addr::new(10, 0, 0, 5),
        mask: Ipv4Addr::new(255, 255, 255, 0),
        gateway: None,
    };
    assert_eq!(parsed(&["lan", "set", "address", "10.0.0.5/24"]).command, expected);
    let with_gateway = parsed(&["lan", "set", "address", "10.0.0.5/24", "gateway", "10.0.0.1"]);
    assert_eq!(
        with_gateway.command,
        Command::LanStatic {
            address: Ipv4Addr::new(10, 0, 0, 5),
            mask: Ipv4Addr::new(255, 255, 255, 0),
            gateway: Some(Ipv4Addr::new(10, 0, 0, 1)),
        }
    );
    assert_eq!(parsed(&["lan", "set", "dhcp"]).command, Command::LanDhcp);
    let wide = parsed(&["lan", "set", "address", "172.16.0.9/12"]);
    let Command::LanStatic { mask, .. } = wide.command else { panic!("a static address") };
    assert_eq!(mask, Ipv4Addr::new(255, 240, 0, 0));
}

#[test]
fn a_mistake_in_the_command_line_is_named() {
    assert!(refused(&["lan", "set", "address", "10.0.0.5"]).contains("prefix length"));
    assert!(refused(&["lan", "set", "address", "10.0.0.5/0"]).contains("prefix length"));
    assert!(refused(&["lan", "set", "address", "10.0.0.5/33"]).contains("prefix length"));
    assert!(refused(&["lan", "set", "address", "ten/8"]).contains("prefix length"));
    assert!(refused(&["lan", "set", "address", "10.0.0.5/8", "gateway", "x"]).contains("'x'"));
    assert!(refused(&["lan", "set"]).contains("address ADDRESS/PREFIX or dhcp"));
    assert!(refused(&["lan", "dhcp"]).contains("address ADDRESS/PREFIX or dhcp"));
    assert!(refused(&["sensors", "all"]).contains("unexpected argument 'all'"));
    assert!(refused(&["power"]).contains("no command named power"));
    assert!(refused(&["sensors", "--clear"]).contains("--clear is for bmc events"));
    assert!(refused(&["--all"]).contains("unrecognized option"));
}

#[test]
fn prefixes_are_read_from_netmasks() {
    assert_eq!(views::prefix_of(Ipv4Addr::new(255, 255, 255, 0)), Some(24));
    assert_eq!(views::prefix_of(Ipv4Addr::new(255, 255, 255, 255)), Some(32));
    assert_eq!(views::prefix_of(Ipv4Addr::new(0, 0, 0, 0)), Some(0));
    assert_eq!(views::prefix_of(Ipv4Addr::new(255, 0, 255, 0)), None);
}

#[test]
fn a_machine_without_a_bmc_and_a_user_who_may_not_open_it_are_told_so() {
    let sensors = Command::Sensors;
    let missing = open_failure(&io::Error::from(io::ErrorKind::NotFound), &sensors);
    assert_eq!(missing.message, "this machine has no BMC that Dukovo reaches");
    assert_eq!(missing.code, 1);
    let denied = open_failure(&io::Error::from(io::ErrorKind::PermissionDenied), &sensors);
    assert_eq!(denied.message, "the BMC is for an administrator: run elevate bmc sensors");
    let clearing = Command::Events { clear: true };
    let denied = open_failure(&io::Error::from(io::ErrorKind::PermissionDenied), &clearing);
    assert!(denied.message.ends_with("elevate bmc events --clear"), "{}", denied.message);
}

#[test]
fn the_summary_says_what_the_bmc_is() {
    let mut bmc = Sim::new();
    let line = json_of(&mut bmc, &Command::Summary);
    json::assert_golden(&line, include_str!("../../tests/json/bmc-summary.json"));
    let text = run(&mut bmc, &Command::Summary).unwrap().text();
    assert_eq!(
        text,
        "Maker      Dell (674)\n\
         Product    0x0100\n\
         Firmware   3.20\n\
         IPMI       2.0\n\
         GUID       00112233-4455-6677-8899-aabbccddeeff\n\
         Functions  sensor device, SDR repository, SEL, FRU inventory\n\
         LAN        10.1.2.3/24 (static), MAC 00:11:22:33:44:55\n"
    );
}

#[test]
fn a_bmc_with_no_lan_and_no_guid_says_none() {
    let mut bmc = Sim::new();
    bmc.channels.clear();
    bmc.guid = None;
    let text = run(&mut bmc, &Command::Summary).unwrap().text();
    assert!(text.contains("GUID       none\n"), "{text}");
    assert!(text.ends_with("LAN        none\n"), "{text}");
    let line = json_of(&mut bmc, &Command::Summary);
    assert!(line.contains(r#""guid":null"#) && line.ends_with(r#""lan":null}"#), "{line}");
    let error = run(&mut bmc, &Command::Lan).err().unwrap();
    assert_eq!(error.message, "this BMC has no LAN channel");
}

#[test]
fn the_sensors_are_listed_with_their_readings_and_states() {
    let mut bmc = bmc_with_sensors();
    let line = json_of(&mut bmc, &Command::Sensors);
    json::assert_golden(&line, include_str!("../../tests/json/bmc-sensors.json"));
    let text = run(&mut bmc, &Command::Sensors).unwrap().text();
    assert_eq!(
        text,
        "NAME          TYPE          READING  STATE\n\
         Inlet Temp    temperature   30 °C    ok\n\
         PS Status     power supply  -        presence detected, failure detected\n\
         Exhaust Temp  temperature   -        not reachable\n\
         Fan Curve     fan           200 RPM  upper non-critical\n\
         Idle Temp     temperature   -        scanning disabled\n\
         Gone Temp     temperature   -        unavailable\n"
    );
}

#[test]
fn a_bmc_with_no_sensors_says_so() {
    let mut bmc = Sim::new();
    assert_eq!(run(&mut bmc, &Command::Sensors).unwrap().text(), "The BMC has no sensors.\n");
}

fn bmc_with_events() -> Sim {
    let mut bmc = bmc_with_sensors();
    bmc.sel = vec![
        // The inlet crossed its upper critical threshold: raw 120 is 40
        // degrees against 110, which is 35.
        sim::sel_entry(1, 100, 0x01, 4, 0x01, [0x59, 120, 110]),
        sim::sel_entry(2, 1_794_000_000, 0x08, 9, 0x6F, [0x01, 0, 0]),
        sim::sel_entry(3, 1_794_000_100, 0x23, 0, 0xEF, [0xC1, 0x04, 0]),
        sim::sel_entry(4, 1_794_000_200, 0x0C, 0, 0x6F, [0x01, 0, 0]),
    ];
    let mut oem = sim::sel_entry(5, 1_794_000_300, 0, 0, 0, [0; 3]);
    oem[2] = 0xC1;
    bmc.sel.push(oem);
    bmc
}

#[test]
fn the_event_log_is_read_with_the_sensors_names_and_values() {
    let mut bmc = bmc_with_events();
    let line = json_of(&mut bmc, &Command::Events { clear: false });
    json::assert_golden(&line, include_str!("../../tests/json/bmc-events.json"));
    assert_eq!(bmc.sel.len(), 5, "reading the log leaves it");
}

#[test]
fn the_text_of_an_event_has_its_time_sensor_and_words() {
    let mut bmc = bmc_with_events();
    bmc.sel.truncate(1);
    let text = run(&mut bmc, &Command::Events { clear: false }).unwrap().text();
    assert_eq!(
        text,
        "ID  TIME             SENSOR           EVENT\n\
         1   start +00:01:40  Inlet Temp (#4)  upper critical going high, asserted; \
         reading 40 °C, threshold 35 °C\n"
    );
}

#[test]
fn clearing_prints_the_log_and_empties_it() {
    let mut bmc = bmc_with_events();
    let report = run(&mut bmc, &Command::Events { clear: true }).unwrap();
    assert!(bmc.sel.is_empty());
    assert_eq!(bmc.erase_requests, 1);
    let line = report.json().unwrap();
    assert!(line.contains(r#""cleared":true"#), "{line}");
    assert_eq!(line.matches(r#""id":"#).count(), 5);
    assert!(report.text().ends_with("The event log is cleared.\n"));
    let again = run(&mut bmc, &Command::Events { clear: false }).unwrap();
    assert_eq!(again.text(), "The event log is empty.\n");
    assert_eq!(again.json().unwrap(), r#"{"format":1,"events":[],"cleared":false}"#);
}

#[test]
fn the_inventory_is_shown_by_area() {
    let mut bmc = Sim::new();
    bmc.fru = crate::ipmi::sim::fru_image();
    let line = json_of(&mut bmc, &Command::Fru);
    json::assert_golden(&line, include_str!("../../tests/json/bmc-fru.json"));
    let text = run(&mut bmc, &Command::Fru).unwrap().text();
    assert_eq!(
        text,
        "Chassis\n  Type           rack-mount chassis\n  Part number    CH-PART-1\n  \
         Serial number  CH-SERIAL\n  Custom         rack 12\n\n\
         Board\n  Manufactured   2022-03-01 10:00\n  Maker          Contoso\n  \
         Product        MB 4000\n  Serial number  123-45\n  Part number    DEAD\n\n\
         Product\n  Maker          Contoso Systems\n  Name           Rack Server 1U\n  \
         Part number    RS-1U\n  Version        v2\n  Serial number  SN-0001\n  \
         Asset tag      ASSET-7\n  FRU file ID    file.fru\n  Custom         extra one\n"
    );
}

#[test]
fn identify_runs_and_says_what_it_did() {
    let mut bmc = Sim::new();
    let line = json_of(&mut bmc, &Command::Identify(Identify::Seconds(30)));
    assert_eq!(line, r#"{"format":1,"mode":"seconds","seconds":30}"#);
    let line = json_of(&mut bmc, &Command::Identify(Identify::ForceOn));
    assert_eq!(line, r#"{"format":1,"mode":"on","seconds":null}"#);
    let off = run(&mut bmc, &Command::Identify(Identify::Off)).unwrap();
    assert_eq!(off.text(), "The identify light is off.\n");
    assert_eq!(bmc.identifies, [vec![30], vec![0, 1], vec![0]]);
    let one = run(&mut bmc, &Command::Identify(Identify::Seconds(1))).unwrap();
    assert_eq!(one.text(), "The identify light is on for 1 second.\n");
    bmc.identify_refused = true;
    let error = run(&mut bmc, &Command::Identify(Identify::Off)).err().unwrap();
    assert!(error.message.contains("invalid command"), "{}", error.message);
}

#[test]
fn the_lan_settings_are_shown_and_changed() {
    let mut bmc = Sim::new();
    let line = json_of(&mut bmc, &Command::Lan);
    json::assert_golden(&line, include_str!("../../tests/json/bmc-lan.json"));
    let text = run(&mut bmc, &Command::Lan).unwrap().text();
    assert_eq!(
        text,
        "Channel  1\nSource   static\nAddress  10.1.2.3\nNetmask  255.255.255.0 (/24)\n\
         Gateway  10.1.2.1\nMAC      00:11:22:33:44:55\nVLAN     291\n"
    );
    let set = Command::LanStatic {
        address: Ipv4Addr::new(192, 168, 7, 9),
        mask: Ipv4Addr::new(255, 255, 0, 0),
        gateway: Some(Ipv4Addr::new(192, 168, 7, 1)),
    };
    let changed = run(&mut bmc, &set).unwrap().text();
    assert!(changed.contains("Address  192.168.7.9\n"), "{changed}");
    assert!(changed.contains("Netmask  255.255.0.0 (/16)\n"), "{changed}");
    let dhcp = run(&mut bmc, &Command::LanDhcp).unwrap().text();
    assert!(dhcp.contains("Source   dhcp\n"), "{dhcp}");
}

#[test]
fn a_refusal_of_the_bmc_is_the_commands_failure() {
    let mut bmc = Sim::new();
    bmc.refuse_lan_parameter = Some(4);
    let error = run(&mut bmc, &Command::LanDhcp).err().unwrap();
    assert_eq!(error.code, 1);
    assert_eq!(error.message, "the BMC refused the command: invalid data field in request (0xCC)");
}
