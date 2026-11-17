// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What is typed, what is printed, and the exit status: the command line
//! as it is read, and each command run on a machine in a folder.

use std::ffi::OsString;
use std::fs;

use serde_json::{Value, json};

use super::fake::{Fake, person};
use super::*;

/// The command `items` say.
fn parsed(items: &[&str]) -> Result<Command, Fail> {
    parse(&mut Args::new("website", items.iter().map(OsString::from).collect()))
}

/// Why `items` are refused, which must be a mistake in the command line.
fn mistake(items: &[&str]) -> String {
    let fail = parsed(items).expect_err(&format!("{items:?} are refused"));
    assert!(fail.usage, "{items:?}: {}", fail.message);
    fail.message
}

/// Runs the command `items` say on `fake`, and what it printed.
fn run(fake: &Fake, items: &[&str]) -> (Result<i32, Fail>, String) {
    let command = parsed(items).expect("the command line reads");
    let mut out = Vec::new();
    let result = execute(command, fake.paths(), fake, &mut out);
    (result, String::from_utf8(out).unwrap())
}

/// Runs `items`, which must end well, and gives what it printed as JSON.
fn json_of(fake: &Fake, items: &[&str], code: i32) -> Value {
    let (result, out) = run(fake, items);
    assert_eq!(result.expect("it ends"), code, "{out}");
    serde_json::from_str(&out).unwrap_or_else(|error| panic!("{error}: {out}"))
}

/// The keys of the object `value`, sorted.
fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys
}

const SHOP: &str = "shop.example.org";
const WWW_SHOP: &str = "www.shop.example.org";
const SHOP_FOLDER: &str = "/shared/www/sites/shop.example.org";

#[test]
fn no_words_and_list_are_the_list_and_check_is_the_check() {
    assert_eq!(parsed(&[]).unwrap(), Command::List { json: false });
    assert_eq!(parsed(&["list"]).unwrap(), Command::List { json: false });
    assert_eq!(parsed(&["--json"]).unwrap(), Command::List { json: true });
    assert_eq!(parsed(&["list", "--json"]).unwrap(), Command::List { json: true });
    assert_eq!(parsed(&["check", "--json"]).unwrap(), Command::Check { json: true });
    assert_eq!(parsed(&["check"]).unwrap(), Command::Check { json: false });
    assert_eq!(parsed(&["--help"]).unwrap(), Command::Help);
    assert_eq!(parsed(&["check", "--help"]).unwrap(), Command::Help);
    assert!(mistake(&["frobnicate"]).starts_with("no such command: frobnicate"));
    assert!(mistake(&["list", "--force"]).contains("--force"));
    assert!(mistake(&["list", "extra"]).contains("extra"));
}

#[test]
fn only_root_changes_and_asks_nginx_and_anyone_lists() {
    for command in ["list", "--help"] {
        assert!(!parsed(&[command]).unwrap().needs_root(), "{command}");
    }
    for items in [
        &["check"][..],
        &["add", SHOP, "--owner", "alice"],
        &["https", SHOP, "--off"],
        &["remove", SHOP],
    ] {
        assert!(parsed(items).unwrap().needs_root(), "{items:?}");
    }
}

#[test]
fn an_add_says_its_names_kind_folder_owner_and_certificate() {
    let Command::Add { request, dry_run, json, progress } =
        parsed(&["add", SHOP, "--owner", "alice"]).unwrap()
    else {
        panic!("an add")
    };
    assert_eq!(
        request,
        AddRequest {
            names: vec![SHOP.into()],
            kind: Kind::Static,
            folder: SHOP_FOLDER.into(),
            owner: "alice".into(),
            certificate: None,
        }
    );
    assert!(!dry_run && !json && !progress);
    let Command::Add { request, progress, .. } = parsed(&[
        "add",
        SHOP,
        "--name",
        WWW_SHOP,
        "--kind",
        "php",
        "--folder",
        "/shared/www/sites/clients/shop",
        "--owner",
        "alice",
        "--certificate",
        SHOP,
        "--progress",
    ])
    .unwrap() else {
        panic!("an add")
    };
    assert_eq!(request.names, [SHOP, WWW_SHOP]);
    assert_eq!(request.kind, Kind::Php);
    assert_eq!(request.folder, "/shared/www/sites/clients/shop");
    assert_eq!(request.certificate.as_deref(), Some(SHOP));
    assert!(progress);
    let Command::Add { dry_run, json, .. } =
        parsed(&["add", SHOP, "--owner", "alice", "--dry-run", "--json"]).unwrap()
    else {
        panic!("an add")
    };
    assert!(dry_run && json);
}

#[test]
fn an_add_that_is_not_whole_or_is_contradicted_is_refused() {
    for (items, said) in [
        (&["add"][..], "needs the site's name"),
        (&["add", SHOP], "needs --owner"),
        (&["add", SHOP, "--owner", "alice", "--kind", "ruby"], "static or php, not ruby"),
        (&["add", SHOP, "--owner", "alice", "--json"], "--json goes with --dry-run"),
        (&["add", SHOP, "--owner", "alice", "--dry-run", "--progress"], "leave out --progress"),
        (&["add", SHOP, "--owner", "alice", "--name", SHOP], "shop.example.org is named twice"),
        (&["add", SHOP, "--owner", "alice", "--force"], "--force"),
        (&["add", SHOP, "other.example.org", "--owner", "alice"], "other.example.org"),
        (&["add", SHOP, "--owner", "alice", "--folder", "/etc"], "/etc"),
        (&["add", SHOP, "--owner", "Alice"], "not an account's name"),
        (&["add", SHOP, "--owner", "alice", "--certificate", "A_B"], "not a certificate's name"),
    ] {
        let words = mistake(items);
        assert!(words.contains(said), "{items:?}: {words}");
    }
    let many: Vec<String> = (0..=NAMES_MAX).map(|n| format!("n{n}.example.org")).collect();
    let mut items = vec!["add", "n0.example.org", "--owner", "alice"];
    for name in &many[1..] {
        items.extend(["--name", name]);
    }
    assert!(mistake(&items).contains("at most 20 names"));
}

#[test]
fn https_takes_one_certificate_or_off_and_remove_takes_files() {
    assert_eq!(
        parsed(&["https", SHOP, "--certificate", "c", "--progress"]).unwrap(),
        Command::Https { name: SHOP.into(), how: Https::With("c".into()), progress: true }
    );
    assert_eq!(
        parsed(&["https", SHOP, "--off"]).unwrap(),
        Command::Https { name: SHOP.into(), how: Https::Off, progress: false }
    );
    for items in [
        &["https", SHOP][..],
        &["https", SHOP, "--off", "--certificate", "c"],
        &["https", "--off"],
        &["https", SHOP, "--certificate", "A_B"],
    ] {
        mistake(items);
    }
    assert_eq!(
        parsed(&["remove", SHOP, "--files", "--progress"]).unwrap(),
        Command::Remove { name: SHOP.into(), files: true, progress: true }
    );
    assert_eq!(
        parsed(&["remove", SHOP]).unwrap(),
        Command::Remove { name: SHOP.into(), files: false, progress: false }
    );
    mistake(&["remove"]);
    mistake(&["remove", SHOP, "extra"]);
}

/// What a person may type that must never reach a file: nginx's syntax,
/// the shell's, a path's and the log's.
const HOSTILE: [&str; 26] = [
    "a.example.org;",
    "a.example.org; }",
    "a b.example.org",
    "a.example.org\nserver {",
    "a.example.org\r\n",
    "a$host.example.org",
    "${jndi}.example.org",
    "a.example.org}",
    "{a}.example.org",
    "`id`.example.org",
    "$(id).example.org",
    "a\"b.example.org",
    "a'b.example.org",
    "a#b.example.org",
    "../a.example.org",
    "a/b.example.org",
    "a\\b.example.org",
    "a\0b.example.org",
    "A.example.org",
    "*.example.org",
    "127.0.0.1",
    "::1",
    "-a.example.org",
    "a..example.org",
    ".example.org",
    "",
];

#[test]
fn a_name_that_is_not_a_plain_name_is_refused_wherever_it_is_typed() {
    for bad in HOSTILE {
        assert!(check_name(bad).is_err(), "{bad:?}");
        assert!(check_site_name(bad).is_err(), "{bad:?}");
        for items in [
            vec!["add", bad, "--owner", "alice"],
            vec!["add", "ok.example.org", "--name", bad, "--owner", "alice"],
            vec!["https", bad, "--off"],
            vec!["remove", bad],
        ] {
            // A name that starts with a dash is an option, which is as
            // well refused.
            let refused = parsed(&items);
            assert!(refused.is_err(), "{items:?}");
        }
    }
}

#[test]
fn a_folder_an_owner_and_a_certificate_that_are_not_plain_are_refused() {
    for bad in [
        "/shared/www/sites/a;b",
        "/shared/www/sites/a b",
        "/shared/www/sites/$x",
        "/shared/www/sites/../etc",
        "/shared/www/sites/a\"",
        "relative",
        "/shared/www/sites/{x}",
    ] {
        mistake(&["add", SHOP, "--owner", "alice", "--folder", bad]);
    }
    for bad in ["a;b", "a b", "$x", "../x", "A", "a\nb", "-a", ""] {
        mistake(&["add", SHOP, "--owner", bad]);
    }
    for bad in ["a;b", "a b", "$x", "../x", "A", "a\nb", "-a", "a..b", "a/b"] {
        mistake(&["add", SHOP, "--owner", "alice", "--certificate", bad]);
        mistake(&["https", SHOP, "--certificate", bad]);
    }
}

#[test]
fn a_name_is_lowercase_letters_digits_and_dashes_in_labels_with_dots_and_a_site_s_is_not_too_long()
{
    for good in ["a", "a.b", "a-b.c1.org", "xn--bcher-kva.example", "1.2.3.example"] {
        assert!(check_name(good).is_ok(), "{good}");
    }
    assert!(check_name(&format!("{}.org", "a".repeat(63))).is_ok());
    assert!(check_name(&format!("{}.org", "a".repeat(64))).is_err());
    let long = vec!["a"; 126].join(".");
    assert_eq!(long.len(), 251);
    assert!(check_name(&long).is_ok());
    let why = check_site_name(&long).unwrap_err();
    assert!(why.contains("too long") && why.contains("250"), "{why}");
    assert!(check_site_name(&vec!["a"; 125].join(".")).is_ok());
    let wild = check_name("*.example.org").unwrap_err();
    assert!(wild.contains("wildcard"), "{wild}");
    let number = check_name("192.0.2.1").unwrap_err();
    assert!(number.contains("an address by number"), "{number}");
    // What the words say of a name does not carry its control characters.
    let words = check_name("a\u{1b}[2J.example.org").unwrap_err();
    assert!(!words.contains('\u{1b}'), "{words:?}");
}

#[test]
fn an_error_ends_with_its_id_after_all_it_says() {
    let fail = change::Refusal::new("WEB-103", "nginx refuses it").into_fail();
    assert_eq!(
        (fail.message.as_str(), fail.usage, fail.code),
        ("nginx refuses it (WEB-103)", false, 1)
    );
    let left = change::Refusal::new("WEB-104", "It did not answer")
        .and_left(&["nginx did not restart".into()])
        .into_fail();
    assert_eq!(
        left.message,
        "It did not answer\nIt could not be taken back: nginx did not restart (WEB-104)"
    );
    let usage = change::Refusal::usage("no such account").into_fail();
    assert_eq!((usage.message.as_str(), usage.usage), ("no such account", true));
    assert_eq!(change::Refusal::plain("x").and_left(&[]).words, "x");
}

#[test]
fn help_prints_the_usage_and_ends_well() {
    let fake = Fake::new();
    let (result, out) = run(&fake, &["--help"]);
    assert_eq!(result.unwrap(), 0);
    assert!(out.starts_with("usage: website [list] [--json]\n"), "{out}");
    assert!(!fake.changed_services());
}

#[test]
fn an_add_through_the_command_prints_what_it_made_and_a_refused_one_ends_with_its_id() {
    let fake = Fake::new();
    let (result, out) = run(&fake, &["add", SHOP, "--owner", "alice"]);
    assert_eq!(
        (result.unwrap(), out.as_str()),
        (0, "Added shop.example.org: http://shop.example.org/\n")
    );
    let (result, out) = run(&fake, &["add", SHOP, "--owner", "alice"]);
    let fail = result.unwrap_err();
    assert!(out.is_empty());
    assert!(fail.message.ends_with("(WEB-101)") && !fail.usage, "{}", fail.message);
    let (result, _) = run(&fake, &["add", "b.example.org", "--owner", "root"]);
    let fail = result.unwrap_err();
    assert!(
        fail.usage && fail.message.contains("root is not the account of a person"),
        "{}",
        fail.message
    );
    let (result, out) = run(&fake, &["https", SHOP, "--certificate", SHOP]);
    assert_eq!(
        (result.unwrap(), out.as_str()),
        (0, "shop.example.org is served at https://shop.example.org/\n")
    );
    let (result, out) = run(&fake, &["remove", SHOP, "--files"]);
    assert_eq!(result.unwrap(), 0);
    assert_eq!(
        out,
        "Removed shop.example.org\nRemoved its folder /shared/www/sites/shop.example.org and what was in it\n\
         The certificate shop.example.org is kept; `elevate cert remove shop.example.org` removes it.\n"
    );
}

#[test]
fn a_change_with_its_steps_told_goes_through_the_same_way() {
    let fake = Fake::new();
    let (result, out) = run(&fake, &["add", SHOP, "--owner", "alice", "--progress"]);
    assert_eq!(result.unwrap(), 0);
    assert!(out.ends_with("http://shop.example.org/\n"), "{out}");
    let (result, _) = run(&fake, &["https", SHOP, "--off", "--progress"]);
    assert_eq!(result.unwrap(), 0);
    let (result, _) = run(&fake, &["remove", SHOP, "--files", "--progress"]);
    assert_eq!(result.unwrap(), 0);
}

#[test]
fn the_list_of_no_websites_and_of_websites_nginx_does_not_serve() {
    let fake = Fake::new();
    assert_eq!(run(&fake, &["list"]).1, "No websites.\n");
    let fake = Fake::new().without_nginx();
    assert_eq!(run(&fake, &[]).1, "nginx is not installed.\nNo websites.\n");
    let fake = Fake::new().nginx_off();
    assert_eq!(run(&fake, &["list"]).1, "nginx is off: no site answers.\nNo websites.\n");
}

#[test]
fn the_list_in_words_is_a_table_and_what_is_wrong_under_it() {
    let fake = Fake::new();
    fake.added(&[SHOP, WWW_SHOP], Kind::Static, None);
    assert_eq!(
        run(&fake, &["list"]).1,
        "NAME              KIND    HTTPS  FOLDER\n\
         shop.example.org  static  no     /shared/www/sites/shop.example.org\n"
    );
    fake.added(&["blog.example.org"], Kind::Php, Some("wild.example.org"));
    fs::remove_dir_all(fake.real("/shared/www/sites/blog.example.org")).unwrap();
    fake.php.set(Some(host::Svc { enabled: false, running: false }));
    let out = run(&fake, &["list"]).1;
    assert!(
        out.contains(
            "blog.example.org  php     wild.example.org  /shared/www/sites/blog.example.org\n"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "blog.example.org: Its folder /shared/www/sites/blog.example.org is not there\n"
        ),
        "{out}"
    );
    assert!(out.contains("blog.example.org: PHP-FPM is off: its PHP pages fail\n"), "{out}");
}

#[test]
fn a_bundles_service_that_never_ran_is_installed_and_off() {
    // Spark reads such a service's file as it first starts.
    let fake = Fake::new().without_php();
    assert_eq!(json_of(&fake, &["list", "--json"], 0)["php_fpm"]["installed"], false);
    fake.put("/system/settings/spark/services/php-fpm.toml", "");
    let said = json_of(&fake, &["list", "--json"], 0);
    assert_eq!(said["php_fpm"], json!({"installed": true, "enabled": false, "running": false}));
}

#[test]
fn the_list_as_json_has_the_members_a_page_reads() {
    let fake = Fake::new();
    fake.added(&[SHOP, WWW_SHOP], Kind::Static, None);
    let said = json_of(&fake, &["list", "--json"], 0);
    assert_eq!(keys(&said), ["format", "nginx", "php_fpm", "websites"]);
    assert_eq!(said["format"], 1);
    assert_eq!(said["nginx"], json!({"installed": true, "enabled": true, "running": true}));
    assert_eq!(said["php_fpm"], json!({"installed": true, "enabled": false, "running": false}));
    let site = &said["websites"][0];
    assert_eq!(
        keys(site),
        [
            "added_by_hand",
            "by_hand",
            "certificate",
            "certificate_path",
            "file",
            "folder",
            "https",
            "kind",
            "name",
            "names",
            "owner",
            "problems",
            "redirects_http",
            "text"
        ]
    );
    let file = fake.text("/system/settings/websites/shop.example.org.conf").unwrap();
    assert_eq!(
        *site,
        json!({
            "name": SHOP,
            "names": [SHOP, WWW_SHOP],
            "file": "/system/settings/websites/shop.example.org.conf",
            "kind": "static",
            "folder": SHOP_FOLDER,
            "owner": "alice",
            "https": false,
            "certificate": null,
            "certificate_path": null,
            "redirects_http": false,
            "by_hand": false,
            "added_by_hand": false,
            "problems": [],
            "text": file,
        })
    );
    assert!(!fake.changed_services());
}

#[test]
fn the_list_tells_a_site_a_person_changed_from_one_website_did_not_write_and_a_file_that_does_not_read()
 {
    let fake = Fake::new();
    fake.added(&[SHOP], Kind::Static, Some(SHOP));
    let view = "/system/settings/websites/shop.example.org.conf";
    let text = fake.text(view).unwrap();
    fake.put(view, &format!("{text}# a person's note\n"));
    fake.put(
        "/system/settings/websites/hand.conf",
        "server { listen 80; server_name hand.example.org; root /shared/www/sites/hand; }\n",
    );
    fake.put("/system/settings/websites/broken.conf", "server { listen 80;\n");
    let said = json_of(&fake, &["list", "--json"], 0);
    let sites = said["websites"].as_array().unwrap();
    let find = |name: &str| sites.iter().find(|site| site["name"] == name).unwrap();
    let changed = find(SHOP);
    assert_eq!(
        (changed["by_hand"].clone(), changed["added_by_hand"].clone()),
        (json!(true), json!(false))
    );
    assert_eq!(
        (changed["https"].clone(), changed["certificate"].clone()),
        (json!(true), json!(SHOP))
    );
    assert_eq!(changed["redirects_http"], true);
    let hand = find("hand");
    assert_eq!(
        (hand["by_hand"].clone(), hand["added_by_hand"].clone()),
        (json!(true), json!(true))
    );
    assert_eq!(hand["problems"][0]["words"], "Its folder /shared/www/sites/hand is not there");
    assert_eq!(hand["problems"][0]["id"], Value::Null);
    let broken = find("broken");
    assert_eq!(
        (broken["kind"].clone(), broken["names"].clone(), broken["folder"].clone()),
        (json!("other"), json!([]), Value::Null)
    );
    assert!(
        broken["problems"][0]["words"]
            .as_str()
            .unwrap()
            .starts_with("Its file does not read as nginx's settings")
    );
}

#[test]
fn the_list_says_what_nginx_would_not_find_of_a_site() {
    let fake = Fake::new();
    // An HTTPS site whose certificate nginx's service file does not name,
    // and a PHP site with PHP-FPM not installed.
    fake.put(
        "/system/settings/websites/secure.conf",
        &plan::site_text(&plan::Spec {
            name: "secure".into(),
            names: vec!["secure.example.org".into()],
            kind: Kind::Static,
            folder: "/shared/www/sites/secure".into(),
            certificate: Some("shop.example.org".into()),
        }),
    );
    fs::create_dir_all(fake.real("/shared/www/sites/secure")).unwrap();
    let said = json_of(&fake, &["list", "--json"], 0);
    let words = said["websites"][0]["problems"][0]["words"].as_str().unwrap();
    assert!(words.contains("nginx's service file does not name shop.example.org"), "{words}");
    let php = Fake::new().without_php();
    php.added(&["blog.example.org"], Kind::Static, None);
    php.put(
        "/system/settings/websites/blog.example.org.conf",
        &plan::site_text(&plan::Spec {
            name: "blog.example.org".into(),
            names: vec!["blog.example.org".into()],
            kind: Kind::Php,
            folder: "/shared/www/sites/blog.example.org".into(),
            certificate: None,
        }),
    );
    let said = json_of(&php, &["list", "--json"], 0);
    assert_eq!(said["php_fpm"]["installed"], false);
    assert_eq!(
        said["websites"][0]["problems"][0]["words"],
        "PHP-FPM is not installed: its PHP pages fail"
    );
}

#[test]
fn the_check_as_json_says_nginx_conf_nginx_s_answer_and_each_site() {
    let fake = Fake::new();
    fake.added(&[SHOP], Kind::Static, None);
    let said = json_of(&fake, &["check", "--json"], 0);
    assert_eq!(keys(&said), ["check", "format", "nginx_conf", "websites"]);
    assert_eq!(
        said,
        json!({
            "format": 1,
            "nginx_conf": {
                "path": "/shared/services/org.nginx.nginx/nginx/conf/nginx.conf",
                "includes": true,
                "hides_sites": true
            },
            "check": {"passed": true, "line": null},
            "websites": [{"name": SHOP, "problems": []}]
        })
    );
    assert_eq!(fake.calls(), ["nginx_check live"]);
    let (result, out) = run(&fake, &["check"]);
    assert_eq!(result.unwrap(), 0);
    assert!(
        out.contains("nginx's check: passed\n")
            && out.contains("include the websites' folder: yes\n"),
        "{out}"
    );
}

#[test]
fn the_check_names_the_site_nginx_refuses_and_ends_with_status_one() {
    let fake = Fake::new();
    fake.added(&[SHOP], Kind::Static, None);
    fake.added(&["blog.example.org"], Kind::Static, None);
    fake.refuses("blog.example.org");
    let said = json_of(&fake, &["check", "--json"], 1);
    assert_eq!(said["check"]["passed"], false);
    assert_eq!(
        said["check"]["line"],
        "nginx: [emerg] unknown directive \"refused\" in /system/settings/websites/blog.example.org.conf:7"
    );
    let sites = said["websites"].as_array().unwrap();
    assert_eq!(sites.iter().find(|site| site["name"] == SHOP).unwrap()["problems"], json!([]));
    let blog = sites.iter().find(|site| site["name"] == "blog.example.org").unwrap();
    assert_eq!(blog["problems"][0]["id"], "WEB-103");
    assert_eq!(
        blog["problems"][0]["words"],
        "nginx refuses its file: unknown directive \"refused\""
    );
    let (result, out) = run(&fake, &["check"]);
    assert_eq!(result.unwrap(), 1);
    assert!(
        out.contains("nginx's check: failed: nginx: [emerg]")
            && out.contains("blog.example.org: nginx refuses its file"),
        "{out}"
    );
}

#[test]
fn the_check_fails_for_settings_that_lack_a_line_even_when_nginx_passes_them() {
    let fake = Fake::new();
    let bare = super::fake::NGINX_CONF.replace(plan::INCLUDE_LINE, "");
    fake.put(&stage::conf_view(), &bare);
    let said = json_of(&fake, &["check", "--json"], 1);
    assert_eq!(said["nginx_conf"]["includes"], false);
    assert_eq!(said["nginx_conf"]["hides_sites"], true);
    assert_eq!(said["check"]["passed"], true);
}

#[test]
fn the_check_of_nginx_that_never_started_and_of_none_says_so_without_asking_nginx() {
    let fake = Fake::new().never_started();
    let said = json_of(&fake, &["check", "--json"], 1);
    assert_eq!(said["check"]["passed"], false);
    assert!(said["check"]["line"].as_str().unwrap().contains("has not started yet"));
    assert_eq!(
        (said["nginx_conf"]["includes"].clone(), said["nginx_conf"]["hides_sites"].clone()),
        (json!(false), json!(false))
    );
    let fake = Fake::new().never_started().without_nginx();
    let said = json_of(&fake, &["check", "--json"], 1);
    assert_eq!(said["check"]["line"], "nginx is not installed");
    assert!(fake.calls().is_empty());
}

#[test]
fn a_check_that_could_not_be_run_is_a_failure_with_the_reason() {
    let fake = Fake::new();
    fake.fail_calls("nginx_check");
    let (result, _) = run(&fake, &["check"]);
    let fail = result.unwrap_err();
    assert!(fail.message.contains("nginx's check could not be run"), "{}", fail.message);
}

#[test]
fn the_plan_of_a_static_add_says_all_it_would_do_and_does_none_of_it() {
    let fake = Fake::new();
    let before = fake.snapshot();
    let said = json_of(
        &fake,
        &["add", SHOP, "--name", WWW_SHOP, "--owner", "alice", "--dry-run", "--json"],
        0,
    );
    assert_eq!(
        keys(&said),
        [
            "checks",
            "files",
            "firewall",
            "folder",
            "format",
            "nginx_check",
            "nginx_line",
            "restarts_nginx",
            "steps",
            "turns_on"
        ]
    );
    assert_eq!(said["format"], 1);
    let results: Vec<&str> =
        said["checks"].as_array().unwrap().iter().map(|c| c["result"].as_str().unwrap()).collect();
    assert_eq!(results, ["passed"; 7]);
    assert_eq!(keys(&said["checks"][0]), ["id", "result", "words"]);
    assert_eq!(
        said["folder"],
        json!({"path": SHOP_FOLDER, "exists": false, "entries": 0, "owner": null})
    );
    let files = said["files"].as_array().unwrap();
    assert_eq!(files.len(), 3);
    assert_eq!(keys(&files[0]), ["new", "path", "text"]);
    assert_eq!(files[0]["path"], "/system/settings/websites/shop.example.org.conf");
    assert_eq!(files[0]["new"], true);
    assert!(
        files[0]["text"]
            .as_str()
            .unwrap()
            .contains("server_name shop.example.org www.shop.example.org;")
    );
    assert_eq!(files[1]["path"], "/system/settings/spark/services/settings.d/nginx.toml");
    assert_eq!(files[1]["new"], true);
    assert_eq!(files[2]["path"], format!("{SHOP_FOLDER}/index.html"));
    // The first site gives the service file the folder certd answers from.
    assert_eq!(said["restarts_nginx"], true);
    assert_eq!(said["turns_on"], json!([]));
    assert_eq!(said["firewall"], json!([{"port": 80, "allowed": true, "rule": null, "text": ""}]));
    assert_eq!(
        (said["nginx_check"].clone(), said["nginx_line"].clone()),
        (json!("passed"), Value::Null)
    );
    let step = &said["steps"][0];
    assert_eq!(keys(step), ["argv", "words"]);
    assert_eq!(step["words"], format!("Make the folder {SHOP_FOLDER}"));
    assert_eq!(fake.snapshot(), before, "a plan changes nothing that stays");
    assert!(!fake.changed_services());
    assert!(
        fake.calls()
            .iter()
            .all(|call| call.starts_with("nginx_check") || call.starts_with("certificate_names"))
    );
    assert!(fake.staged_left().is_empty());
}

#[test]
fn the_plan_of_a_php_site_over_https_restarts_nginx_turns_php_on_and_asks_both_ports() {
    let fake = Fake::new().blocks(443);
    let said = json_of(
        &fake,
        &[
            "add",
            SHOP,
            "--kind",
            "php",
            "--owner",
            "alice",
            "--certificate",
            SHOP,
            "--dry-run",
            "--json",
        ],
        0,
    );
    assert_eq!(said["restarts_nginx"], true);
    assert_eq!(said["turns_on"], json!(["php-fpm"]));
    assert_eq!(
        said["nginx_check"], "later",
        "a certificate nginx has no copy of yet is seen once it restarts"
    );
    assert_eq!(
        said["firewall"],
        json!([
            {"port": 80, "allowed": true, "rule": null, "text": ""},
            {"port": 443, "allowed": false, "rule": 3, "text": "3: deny tcp 443"}
        ])
    );
    let paths: Vec<&str> =
        said["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(
        paths,
        [
            "/system/settings/websites/_default.conf",
            "/system/settings/websites/shop.example.org.conf",
            "/system/settings/spark/services/settings.d/nginx.toml",
            "/shared/www/sites/shop.example.org/index.php"
        ]
    );
    let restart = said["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["words"] == "Restart nginx")
        .unwrap();
    assert_eq!(restart["argv"], json!(["service", "nginx", "restart"]));
    assert_eq!(said["checks"].as_array().unwrap().len(), 8);
}

#[test]
fn the_plan_of_an_add_with_nginx_off_turns_it_on() {
    let fake = Fake::new().nginx_off();
    let said = json_of(&fake, &["add", SHOP, "--owner", "alice", "--dry-run", "--json"], 0);
    assert_eq!(said["turns_on"], json!(["nginx"]));
    assert_eq!(said["restarts_nginx"], false);
    let turn = said["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["words"] == "Turn nginx on")
        .unwrap();
    assert_eq!(turn["argv"], json!(["service", "nginx", "enable"]));
    assert!(!fake.changed_services());
}

#[test]
fn a_plan_that_fails_says_each_check_that_fails_and_ends_with_status_one() {
    let fake = Fake::new().without_php();
    fake.added(&[SHOP], Kind::Static, None);
    fake.clear();
    let said = json_of(
        &fake,
        &[
            "add",
            "blog.example.org",
            "--name",
            SHOP,
            "--kind",
            "php",
            "--owner",
            "nobody",
            "--certificate",
            "nosuch",
            "--dry-run",
            "--json",
        ],
        1,
    );
    let checks: Vec<(Option<&str>, &str)> = said["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|check| (check["id"].as_str(), check["result"].as_str().unwrap()))
        .collect();
    assert_eq!(
        checks,
        [
            (Some("WEB-101"), "failed"),
            (None, "passed"),
            (Some("WEB-105"), "failed"),
            (None, "passed"),
            (None, "passed"),
            (None, "failed"),
            (None, "passed"),
            (Some("WEB-106"), "failed"),
        ]
    );
    let (result, out) =
        run(&fake, &["add", "blog.example.org", "--name", SHOP, "--owner", "alice", "--dry-run"]);
    assert_eq!(result.unwrap(), 1);
    assert!(
        out.contains("failed: A website answers to shop.example.org already")
            && out.contains("(WEB-101)"),
        "{out}"
    );
    assert!(!fake.changed_services());
}

#[test]
fn the_plan_of_a_set_nginx_refuses_says_nginx_s_line_with_the_file_the_site_would_be() {
    let fake = Fake::new();
    fake.refuses("blog.example.org");
    let said =
        json_of(&fake, &["add", "blog.example.org", "--owner", "alice", "--dry-run", "--json"], 1);
    assert_eq!(said["nginx_check"], "failed");
    assert_eq!(
        said["nginx_line"],
        "nginx: [emerg] unknown directive \"refused\" in /system/settings/websites/blog.example.org.conf:7"
    );
    assert!(said["checks"].as_array().unwrap().iter().any(|check| check["id"] == "WEB-103"));
}

#[test]
fn the_plan_of_nginx_that_never_started_reads_the_bundle_s_settings_and_leaves_its_check_for_later()
{
    let fake = Fake::new().never_started();
    let said = json_of(&fake, &["add", SHOP, "--owner", "alice", "--dry-run", "--json"], 0);
    assert_eq!(said["nginx_check"], "later");
    let results: Vec<&str> =
        said["checks"].as_array().unwrap().iter().map(|c| c["result"].as_str().unwrap()).collect();
    assert!(results.contains(&"warning"), "{results:?}");
    assert!(!fake.changed_services(), "{:?}", fake.calls());
    assert!(fake.text(&stage::conf_view()).is_none(), "nothing was started to write them");
}

#[test]
fn the_plan_in_words_says_each_check_each_file_and_each_step() {
    let fake = Fake::new();
    let (result, out) = run(&fake, &["add", SHOP, "--owner", "alice", "--dry-run"]);
    assert_eq!(result.unwrap(), 0);
    assert!(out.starts_with("passed: No website answers to shop.example.org\n"), "{out}");
    assert!(out.contains("file /system/settings/websites/shop.example.org.conf: new\n"), "{out}");
    assert!(out.contains("step: Make the folder /shared/www/sites/shop.example.org\n"), "{out}");
    assert!(out.ends_with("nginx's check of the new set: passed\n"), "{out}");
}

#[test]
fn a_name_the_rule_accepts_is_written_as_it_is_and_one_it_refuses_is_never_written() {
    let fake = Fake::new();
    let ok = ["a-b.example.org", "xn--bcher-kva.example", "1.2.3.example"];
    for name in ok {
        let (result, _) = run(&fake, &["add", name, "--owner", "alice"]);
        assert_eq!(result.unwrap(), 0, "{name}");
        let text = fake.text(&format!("/system/settings/websites/{name}.conf")).unwrap();
        assert!(text.contains(&format!("server_name {name};")), "{text}");
    }
    let before = fake.snapshot();
    for bad in HOSTILE {
        assert!(parsed(&["add", bad, "--owner", "alice"]).is_err(), "{bad:?}");
    }
    assert_eq!(fake.snapshot(), before);
}

#[test]
fn what_is_made_for_a_person_is_theirs_and_the_folders_above_are_root_s() {
    let fake = Fake::new();
    let (result, _) = run(&fake, &["add", SHOP, "--owner", "alice"]);
    assert_eq!(result.unwrap(), 0);
    assert_eq!(fake.owner_of(SHOP_FOLDER), person());
    assert_eq!(fake.owner_of(&format!("{SHOP_FOLDER}/index.html")), person());
    // SAFETY: geteuid has no failure and touches no memory.
    let root = unsafe { libc::geteuid() };
    assert_eq!(fake.owner_of("/shared/www/sites"), root);
    assert_eq!(fake.owner_of("/system/settings/websites/shop.example.org.conf"), root);
}
