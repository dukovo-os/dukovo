// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Tests of cloud-setup's reading: YAML, the user data, the meta-data and
//! the network configuration, as clouds and their users write them.

use super::config::{self, Account, Command, MetaData, UserData, UserEntry, WriteFile};
use super::network::network_file;
use super::yaml::{Value, parse};
use super::{Cloud, cloud_of, parse_answer};

/// A scalar.
fn t(text: &str) -> Value {
    Value::Text(text.into())
}

/// A mapping of `entries`.
fn map(entries: &[(&str, Value)]) -> Value {
    Value::Map(entries.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect())
}

#[test]
fn yaml_reads_mappings_and_lists_by_their_indentation() {
    let text = "#cloud-config\n\
                --- \n\
                hostname: web01   # the name\n\
                users:\n\
                - default\n\
                - name: ann\n  \
                  groups: [sudo, users]\n  \
                  keys:\n    \
                    - ssh-ed25519 AAAA ann@laptop\n\
                empty:\n\
                nested:\n  \
                  - - a\n    \
                    - b\n  \
                  -\n    c: d\n\
                url: http://example.org:8080/x\n";
    let value = parse(text).unwrap();
    let ann = map(&[
        ("name", t("ann")),
        ("groups", Value::List(vec![t("sudo"), t("users")])),
        ("keys", Value::List(vec![t("ssh-ed25519 AAAA ann@laptop")])),
    ]);
    assert_eq!(
        value,
        map(&[
            ("hostname", t("web01")),
            ("users", Value::List(vec![t("default"), ann])),
            ("empty", Value::Null),
            (
                "nested",
                Value::List(vec![Value::List(vec![t("a"), t("b")]), map(&[("c", t("d"))]),])
            ),
            ("url", t("http://example.org:8080/x")),
        ])
    );
    assert_eq!(value.get("users").map(|u| u.texts()), Some(vec!["default"]));
}

#[test]
fn yaml_reads_quotes_and_their_escapes() {
    let value = parse(
        "a: \"tab\\there \\\"q\\\" \\u00e9 # not a comment\"\n\
         b: 'it''s # still text'\n\
         'c d': \"two\n  lines\"\n\
         e: ~\n\
         f: !!str 0644\n",
    )
    .unwrap();
    assert_eq!(value.get("a"), Some(&t("tab\there \"q\" é # not a comment")));
    assert_eq!(value.get("b"), Some(&t("it's # still text")));
    assert_eq!(value.get("c d"), Some(&t("two lines")));
    assert_eq!(value.get("e"), None);
    assert_eq!(value.get("f"), Some(&t("0644")));
}

#[test]
fn yaml_reads_block_scalars_with_their_chomping() {
    let value = parse(
        "literal: |\n  line one\n    indented\n\n  line three\n\n\
         stripped: |-\n  no newline\n\
         kept: |+\n  kept\n\n\
         folded: >\n  one\n  two\n\n  three\n\
         list:\n  - |\n    in a list\n  - after\n\
         last: x\n",
    )
    .unwrap();
    assert_eq!(value.get("literal"), Some(&t("line one\n  indented\n\nline three\n")));
    assert_eq!(value.get("stripped"), Some(&t("no newline")));
    assert_eq!(value.get("kept"), Some(&t("kept\n\n")));
    assert_eq!(value.get("folded"), Some(&t("one two\nthree\n")));
    assert_eq!(value.get("list"), Some(&Value::List(vec![t("in a list\n"), t("after")])));
    assert_eq!(value.get("last"), Some(&t("x")));
}

#[test]
fn yaml_reads_flow_collections_over_lines() {
    let value = parse(
        "runcmd:\n  - [sh, -c, \"echo a, b\"]\n  - [ls,\n     -l]   # a list\n\
         match: {macaddress: '52:54:00:12:34:56', name: eth0, none: }\n",
    )
    .unwrap();
    assert_eq!(
        value.get("runcmd"),
        Some(&Value::List(vec![
            Value::List(vec![t("sh"), t("-c"), t("echo a, b")]),
            Value::List(vec![t("ls"), t("-l")]),
        ]))
    );
    assert_eq!(
        value.get("match"),
        Some(&map(&[
            ("macaddress", t("52:54:00:12:34:56")),
            ("name", t("eth0")),
            ("none", Value::Null)
        ]))
    );
}

#[test]
fn yaml_says_what_it_does_not_read_and_where() {
    let error = |text: &str| parse(text).unwrap_err();
    assert_eq!(error("a: 1\n\tb: 2\n").line, 2);
    assert!(error("a: &x 1\nb: *x\n").what.contains("anchors"));
    assert!(error("a: 1\na: 2\n").what.contains("twice"));
    assert!(error("a: \"open\n").what.contains("quote"));
    assert!(error("a: [1, 2\n").what.contains("[ or {"));
    assert!(error("a: 1\n---\nb: 2\n").what.contains("one document"));
    assert_eq!(error("a:\n  b: 1\n c: 2\n").line, 3);
    assert_eq!(parse("# only a comment\n"), Ok(Value::Null));
}

#[test]
fn user_data_is_told_apart_by_its_first_line() {
    assert_eq!(config::user_data(b"  \n"), Ok(UserData::Empty));
    assert_eq!(
        config::user_data(b"#!/bin/sh\necho hi\n"),
        Ok(UserData::Script(b"#!/bin/sh\necho hi\n".to_vec()))
    );
    assert_eq!(config::user_data(&[0x1f, 0x8b, 8]), Ok(UserData::Other("compressed with gzip")));
    assert_eq!(
        config::user_data(b"#include\nhttp://x\n"),
        Ok(UserData::Other("an #include of other files"))
    );
    assert!(matches!(config::user_data(b"plain words"), Ok(UserData::Other(_))));
    assert!(matches!(config::user_data(b"#cloud-config\n"), Ok(UserData::Config(_))));
    assert!(config::user_data(b"#cloud-config\na: [\n").is_err());
}

/// A `#cloud-config` file as a cloud's documentation shows one.
const CONFIG: &str = "#cloud-config
hostname: web01
fqdn: web01.example.org
users:
  - default
  - name: ann
    gecos: Ann Admin
    groups: users, sudo
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    ssh_authorized_keys:
      - ssh-ed25519 AAAAann ann@laptop
  - name: bob
    lock_passwd: false
    plain_text_passwd: hunter22
  - name: cy
    passwd: $6$salt$hash
    plain_text_passwd: kept-locked
ssh_authorized_keys:
  - ssh-ed25519 AAAAextra extra@desk
write_files:
  - path: /system/settings/motd
    content: |
      Welcome.
  - path: /users/ann/hello
    encoding: b64
    content: aGVsbG8=
    permissions: '0600'
    owner: ann:ann
    append: true
runcmd:
  - echo first > /tmp/first
  - [service, nginx, enable]
growpart:
  mode: 'off'
packages: [nginx]
snap: {}
";

#[test]
fn a_cloud_config_is_read_into_what_is_to_be_done() {
    let Ok(UserData::Config(config)) = config::user_data(CONFIG.as_bytes()) else {
        panic!("not read")
    };
    let meta = MetaData {
        instance: "i-1".into(),
        hostname: Some("ip-10-0-0-5.ec2.internal".into()),
        keys: vec!["ssh-ed25519 AAAAcloud owner".into(), "ssh-ed25519 AAAAextra extra@desk".into()],
    };
    assert_eq!(config.hostname(&meta), Some("web01".into()));
    let accounts = config.accounts(&meta);
    assert_eq!(
        accounts[0],
        Account {
            name: config::DEFAULT_ACCOUNT.into(),
            full_name: String::new(),
            admin: true,
            without_password: true,
            // The cloud's key, then the data's, each once.
            keys: vec![
                "ssh-ed25519 AAAAcloud owner".into(),
                "ssh-ed25519 AAAAextra extra@desk".into()
            ],
            password: None,
        }
    );
    assert_eq!(
        accounts[1],
        Account {
            name: "ann".into(),
            full_name: "Ann Admin".into(),
            admin: true,
            without_password: true,
            keys: vec!["ssh-ed25519 AAAAann ann@laptop".into()],
            password: None,
        }
    );
    assert_eq!((accounts[2].admin, accounts[2].password.as_deref()), (false, Some("hunter22")));
    // A password locked, as by default, is not given.
    assert_eq!(accounts[3].password, None);
    assert_eq!(
        config.files,
        vec![
            WriteFile {
                path: "/system/settings/motd".into(),
                content: b"Welcome.\n".to_vec(),
                mode: 0o644,
                owner: None,
                append: false,
            },
            WriteFile {
                path: "/users/ann/hello".into(),
                content: b"hello".to_vec(),
                mode: 0o600,
                owner: Some("ann:ann".into()),
                append: true,
            },
        ]
    );
    assert_eq!(
        config.commands,
        vec![
            Command::Shell("echo first > /tmp/first".into()),
            Command::Words(vec!["service".into(), "nginx".into(), "enable".into()]),
        ]
    );
    assert!(!config.grow_partition && config.grow_filesystem);
    assert_eq!(
        config.left_out,
        vec![
            "users: ann's shell: cloud-setup does not read it".to_string(),
            "users: cy's passwd: a crypt(3) hash, which Dukovo's Argon2id passwords cannot take; \
             give plain_text_passwd, or set one with passwd"
                .into(),
            "packages: Dukovo installs apps with bundle(1)".into(),
            "snap: cloud-setup does not read it".into(),
        ]
    );
}

#[test]
fn the_default_account_is_made_when_the_data_names_none() {
    let Ok(UserData::Config(config)) = config::user_data(b"#cloud-config\npassword: pw\n") else {
        panic!("not read")
    };
    assert_eq!(config.users, vec![UserEntry::Default]);
    let meta = MetaData {
        instance: "x".into(),
        hostname: Some("droplet-1".into()),
        keys: vec!["k".into()],
    };
    let accounts = config.accounts(&meta);
    assert_eq!(
        (accounts[0].name.as_str(), accounts[0].password.as_deref()),
        ("dukovo", Some("pw"))
    );
    assert_eq!(accounts[0].keys, ["k"]);
    // The cloud's name, short.
    assert_eq!(config.hostname(&meta), Some("droplet-1".into()));
    let Ok(UserData::Config(kept)) = config::user_data(b"#cloud-config\npreserve_hostname: true\n")
    else {
        panic!("not read")
    };
    assert_eq!(kept.hostname(&meta), None);
    let Ok(UserData::Config(named)) = config::user_data(b"#cloud-config\nusers: [ann]\n") else {
        panic!("not read")
    };
    assert_eq!(named.accounts(&meta)[0].name, "ann");
    assert!(named.accounts(&meta)[0].keys.is_empty());
}

#[test]
fn proxmox_names_the_default_account_with_user() {
    // As Proxmox's cloud-init drive writes the user data.
    let text = "#cloud-config
hostname: vm100
manage_etc_hosts: true
fqdn: vm100.lan
user: chase
password: $5$abc$hashed
ssh_authorized_keys:
  - ssh-ed25519 AAAA proxmox
chpasswd:
  expire: False
users:
  - default
package_upgrade: true
";
    let Ok(UserData::Config(config)) = config::user_data(text.as_bytes()) else {
        panic!("not read")
    };
    let meta = MetaData { instance: "x".into(), hostname: None, keys: Vec::new() };
    let accounts = config.accounts(&meta);
    assert_eq!(accounts.len(), 1);
    assert_eq!((accounts[0].name.as_str(), accounts[0].admin), ("chase", true));
    assert_eq!(accounts[0].keys, ["ssh-ed25519 AAAA proxmox"]);
    assert_eq!(config.hostname(&meta), Some("vm100".into()));
    // Its password is a hash, which no Argon2id password can be made of.
    assert_eq!(accounts[0].password, None);
    assert_eq!(config.left_out.len(), 2);
    assert!(config.left_out[0].starts_with("password: a crypt(3) hash"));
    assert_eq!(config.left_out[1], "package_upgrade: Dukovo installs apps with bundle(1)");
    let Ok(UserData::Config(mapped)) =
        config::user_data(b"#cloud-config\nuser:\n  name: ann\n  shell: /bin/bash\n")
    else {
        panic!("not read")
    };
    assert_eq!(mapped.accounts(&meta)[0].name, "ann");
    assert_eq!(mapped.left_out, ["user: shell: only the default account's name is read"]);
}

#[test]
fn a_cloud_config_with_a_mistake_says_where() {
    let bad = |text: &str| match config::user_data(text.as_bytes()) {
        Err(why) => why,
        other => panic!("{other:?}"),
    };
    assert!(bad("#cloud-config\nwrite_files:\n  - content: x\n").contains("entry 1 has no path"));
    assert!(bad("#cloud-config\nwrite_files:\n  - path: rel\n").contains("not absolute"));
    assert!(
        bad("#cloud-config\nwrite_files:\n  - path: /a\n    permissions: rw\n").contains("octal")
    );
    assert!(bad("#cloud-config\nusers:\n  - groups: x\n").contains("has no name"));
    assert!(bad("#cloud-config\nruncmd:\n  - [a, [b]]\n").contains("list of words"));
    // A colon and a space make a mapping of a line, as in cloud-init.
    assert!(bad("#cloud-config\nruncmd:\n  - echo \"a: b\"\n").contains("quote a line"));
    assert!(bad("#cloud-config\n- a list\n").contains("mapping"));
}

#[test]
fn meta_data_comes_from_nocloud_and_digitalocean() {
    let meta = MetaData::nocloud(
        "instance-id: iid-local01\nlocal-hostname: cloudy.example\npublic-keys:\n  - ssh-rsa AAAA a@b\n",
    )
    .unwrap();
    assert_eq!(
        meta,
        MetaData {
            instance: "iid-local01".into(),
            hostname: Some("cloudy.example".into()),
            keys: vec!["ssh-rsa AAAA a@b".into()],
        }
    );
    // JSON, as some write it, and keys by name, as OpenStack does.
    let json =
        MetaData::nocloud(r#"{"instance-id": "i-2", "public-keys": {"one": "k1\nk2"}}"#).unwrap();
    assert_eq!((json.instance.as_str(), json.keys.len()), ("i-2", 2));
    assert!(MetaData::nocloud("local-hostname: x\n").unwrap_err().contains("instance-id"));
    let (meta, user_data) = MetaData::digitalocean(
        br#"{"droplet_id": 2756294, "hostname": "sample-droplet",
             "public_keys": ["ssh-ed25519 AAAAdo me"], "user_data": "write me"}"#,
    )
    .unwrap();
    assert_eq!(
        (meta.instance.as_str(), meta.hostname.as_deref(), meta.keys.as_slice()),
        ("2756294", Some("sample-droplet"), ["ssh-ed25519 AAAAdo me".to_string()].as_slice())
    );
    assert_eq!(user_data.as_deref(), Some(b"write me".as_slice()));
}

#[test]
fn a_version_1_network_config_as_proxmox_writes_it() {
    let value = parse(
        "version: 1
config:
    - type: physical
      name: eth0
      mac_address: 'BC:24:11:AA:BB:CC'
      subnets:
      - type: static
        address: '192.168.1.50'
        netmask: '255.255.255.0'
        gateway: '192.168.1.1'
      - type: dhcp6
    - type: physical
      name: eth1
      subnets:
      - type: dhcp4
    - type: nameserver
      address:
      - '1.1.1.1'
      search:
      - 'example.org'
",
    )
    .unwrap();
    let (file, left_out) = network_file(&value).unwrap();
    assert_eq!(
        file.unwrap(),
        "# Written by cloud-setup(8) from the cloud's network configuration (ADR 0328).\n\
         # A card not named here asks DHCP (network(5)).\n\
         \n[bc:24:11:aa:bb:cc]\naddress = 192.168.1.50/24\nrouter = 192.168.1.1\ndns = 1.1.1.1\n"
    );
    assert!(left_out.is_empty());
}

#[test]
fn a_version_2_network_config_as_netplan_writes_it() {
    let value = parse(
        "network:
  version: 2
  ethernets:
    eth0:
      dhcp4: false
      addresses: [10.0.2.15/24, 'fd00::15/64']
      routes:
        - to: default
          via: 10.0.2.2
        - to: 10.9.0.0/16
          via: 10.0.2.9
      nameservers:
        addresses: [10.0.2.3]
    lan:
      match:
        macaddress: '52:54:00:12:34:57'
      addresses: [192.168.5.2]
      gateway4: 192.168.5.1
      dhcp4: true
    other:
      dhcp4: true
  bridges:
    br0: {}
",
    )
    .unwrap();
    let (file, left_out) = network_file(&value).unwrap();
    let file = file.unwrap();
    assert!(file.contains(
        "\n[net0]\naddress = 10.0.2.15/24\naddress = fd00::15/64\nrouter = 10.0.2.2\ndns = 10.0.2.3\n"
    ));
    assert!(file.contains(
        "\n[52:54:00:12:34:57]\naddress = 192.168.5.2/24\nrouter = 192.168.5.1\ndhcp = yes\n"
    ));
    assert!(!file.contains("[other]"));
    assert_eq!(
        left_out,
        [
            "network-config: bridges; ip(8) makes them by hand",
            "network-config: the route to 10.9.0.0/16 of eth0"
        ]
    );
    // DHCP alone needs no file, and a disabled one changes nothing.
    let dhcp = parse("version: 2\nethernets:\n  eth0:\n    dhcp4: true\n").unwrap();
    assert_eq!(network_file(&dhcp).unwrap(), (None, Vec::new()));
    assert_eq!(network_file(&parse("config: disabled\n").unwrap()).unwrap(), (None, Vec::new()));
    assert!(network_file(&parse("version: 3\n").unwrap()).is_err());
}

#[test]
fn the_cloud_comes_from_the_firmwares_tables() {
    assert_eq!(cloud_of("DigitalOcean\n", "DigitalOcean", "20171212"), Some(Cloud::DigitalOcean));
    assert_eq!(cloud_of("Amazon EC2\n", "Amazon EC2", "1.0"), Some(Cloud::Aws));
    assert_eq!(cloud_of("Xen", "Xen", "4.11.amazon"), Some(Cloud::Aws));
    assert_eq!(cloud_of("QEMU", "SeaBIOS", "1.16.3"), None);
    assert_eq!(cloud_of("", "", ""), None);
}

#[test]
fn an_http_answer_gives_its_status_and_body() {
    let answer =
        b"HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\ncontent-length: 5\r\n\r\ni-123extra";
    assert_eq!(parse_answer(answer), Some((200, b"i-123".to_vec())));
    assert_eq!(parse_answer(b"HTTP/1.1 404 Not Found\r\n\r\n"), Some((404, Vec::new())));
    assert_eq!(parse_answer(b"garbage"), None);
}

#[test]
fn keys_are_lines_without_blanks_and_comments() {
    assert_eq!(
        config::key_lines("# mine\nssh-ed25519 A a\n\n  ssh-rsa B b  \n"),
        ["ssh-ed25519 A a", "ssh-rsa B b"]
    );
    assert_eq!(config::short_name("web01.example.org"), Some("web01".into()));
    assert_eq!(config::short_name(" "), None);
}
