// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The changes on a machine in a folder: what each makes, in what order,
//! what each refuses with nothing changed, and that a call that fails at
//! any step leaves the files and the services as they were.

use super::*;
use crate::website::fake::{Fake, NGINX_CONF, NGINX_SERVICE, REFUSED, person, request};
use crate::website::plan::{INCLUDE_LINE, LOG_FORMAT_LINE};
use crate::website::remove::remove;

const SHOP: &str = "shop.example.org";
const WWW_SHOP: &str = "www.shop.example.org";
const SHOP_FOLDER: &str = "/shared/www/sites/shop.example.org";

/// The shop, answering to its two names.
fn shop(kind: Kind, certificate: Option<&str>) -> AddRequest {
    request(&[SHOP, WWW_SHOP], kind, certificate)
}

/// Where the site `name`'s file is.
fn site_view(name: &str) -> String {
    format!("{SETTINGS}/{name}.conf")
}

/// The certificates nginx's service file names, with its override.
fn named(fake: &Fake) -> Vec<String> {
    named_certificates(NGINX_SERVICE, fake.text(OVERRIDE).as_deref()).unwrap()
}

/// A machine with the shop added, over HTTPS with its certificate or not.
fn with_shop(certificate: Option<&str>) -> Fake {
    let fake = Fake::new();
    fake.added(&[SHOP, WWW_SHOP], Kind::Static, certificate);
    fake
}

/// An add that must be refused: what it was refused with, when nothing
/// was changed.
fn refused_add(fake: &Fake, request: &AddRequest) -> Refusal {
    fake.clear();
    let before = fake.snapshot();
    let refusal = add(&fake.ctx(), request).unwrap_err();
    assert_eq!(fake.snapshot(), before, "a refused add changes no file: {}", refusal.words);
    assert!(!fake.changed_services(), "{:?}", fake.calls());
    assert!(fake.staged_left().is_empty());
    refusal
}

/// Runs `run` on a machine from `make` once, to count the calls it makes
/// that can fail, and then once for each of them with that call failing.
/// Every run that has a call fail must end in a refusal that says nothing
/// was left, and leave the files, the services and what nginx serves as
/// they were.
fn sweep(make: impl Fn() -> Fake, run: impl Fn(&Fake) -> Result<Vec<String>, Refusal>) {
    let clean = make();
    run(&clean).expect("the change goes through when nothing fails");
    let calls = clean.calls();
    assert!(calls.len() >= 2, "{calls:?}");
    for number in 1..=calls.len() {
        let fake = make();
        let before = fake.snapshot();
        let (nginx, php, serving) = (fake.nginx.get(), fake.php.get(), fake.serving());
        fake.fail_at(number);
        let call = &calls[number - 1];
        let refusal = run(&fake).expect_err(&format!("call {number}, {call}, failed"));
        assert!(
            !refusal.words.contains("could not be taken back"),
            "after {call} failed: {}",
            refusal.words
        );
        assert_eq!(fake.snapshot(), before, "after {call} failed: {}", refusal.words);
        assert_eq!((fake.nginx.get(), fake.php.get()), (nginx, php), "after {call} failed");
        assert_eq!(fake.serving(), serving, "nginx serves what it did after {call} failed");
        assert!(fake.staged_left().is_empty(), "after {call} failed");
    }
}

#[test]
fn a_static_site_is_added_with_its_folder_its_file_and_nginx_restarted() {
    let fake = Fake::new();
    let lines = add(&fake.ctx(), &shop(Kind::Static, None)).unwrap();
    assert_eq!(lines, ["Added shop.example.org: http://shop.example.org/"]);
    assert_eq!(fake.text(&site_view(SHOP)).unwrap(), site_text(&shop(Kind::Static, None).spec()));
    assert_eq!(fake.names(SETTINGS), ["shop.example.org.conf"], "no default server without HTTPS");
    // Over HTTP the service file needs the folder certd answers from, and
    // no certificate and no socket for 443.
    let over = fake.text(OVERRIDE).unwrap();
    assert!(over.contains("/system/live/certd"), "{over}");
    assert!(!over.contains("certificates") && !over.contains("443"), "{over}");
    assert!(named(&fake).is_empty());
    assert_eq!(fake.names(SHOP_FOLDER), ["index.html"]);
    assert!(fake.text(&format!("{SHOP_FOLDER}/index.html")).unwrap().contains(SHOP));
    assert_eq!(fake.owner_of(SHOP_FOLDER), person());
    assert_eq!(
        fake.calls(),
        [
            "nginx_check live",
            "service_check nginx",
            "restart nginx",
            "nginx_check staged",
            "restart nginx",
            "fetch 80 shop.example.org file",
            "fetch 80 www.shop.example.org file"
        ]
    );
    assert_eq!(
        fake.logged(),
        ["added shop.example.org (static, /shared/www/sites/shop.example.org, HTTP only)"]
    );
    assert!(fake.staged_left().is_empty());
    assert_eq!(fake.serving(), [SHOP, WWW_SHOP]);
}

#[test]
fn the_new_set_is_checked_staged_before_any_of_it_is_live() {
    let fake = Fake::new();
    add(&fake.ctx(), &shop(Kind::Static, None)).unwrap();
    let seen = fake.checks();
    assert_eq!(seen.len(), 2);
    assert!(!seen[0].staged && seen[0].set.is_empty() && seen[0].live.is_empty());
    assert!(seen[1].staged);
    assert_eq!(seen[1].set, ["shop.example.org.conf"]);
    assert!(seen[1].live.is_empty(), "nothing of the new set was live when nginx checked it");
}

#[test]
fn a_php_site_over_https_gets_its_service_file_before_the_check_and_php_after_it() {
    let fake = Fake::new();
    let request = shop(Kind::Php, Some(SHOP));
    let lines = add(&fake.ctx(), &request).unwrap();
    assert_eq!(lines, ["Added shop.example.org: https://shop.example.org/"]);
    assert_eq!(fake.names(SETTINGS), ["_default.conf", "shop.example.org.conf"]);
    assert_eq!(fake.text(&site_view("_default")).unwrap(), default_text());
    assert_eq!(fake.text(&site_view(SHOP)).unwrap(), site_text(&request.spec()));
    assert_eq!(named(&fake), [SHOP]);
    let over = fake.text(OVERRIDE).unwrap();
    assert!(over.contains("443"), "the sockets for 443 are in it: {over}");
    assert_eq!(
        fake.calls(),
        [
            "nginx_check live",
            "certificate_names shop.example.org",
            "service_check nginx",
            "restart nginx",
            "nginx_check staged",
            "restart nginx",
            "turn php-fpm on",
            "fetch 80 shop.example.org file",
            "fetch 443 shop.example.org file",
            "fetch 80 www.shop.example.org file",
            "fetch 443 www.shop.example.org file",
            "fetch 443 shop.example.org php",
        ]
    );
    assert_eq!(
        fake.restarted_with(),
        [Some(over.clone()), Some(over)],
        "nginx restarted with the new service file, and again with the new sites"
    );
    assert_eq!(fake.php.get(), Some(Svc { enabled: true, running: true }));
    assert_eq!(fake.names(SHOP_FOLDER), ["index.php", "uploads"]);
    let php = fake.accounts().into_iter().find(|user| user.name == "php").unwrap();
    assert_eq!(fake.owner_of(&format!("{SHOP_FOLDER}/uploads")), php.uid);
    assert_eq!(fake.owner_of(SHOP_FOLDER), person());
    assert_eq!(
        fake.logged(),
        [
            "added shop.example.org (php, /shared/www/sites/shop.example.org, HTTPS with shop.example.org)"
        ]
    );
}

#[test]
fn a_second_https_site_adds_its_certificate_and_a_last_one_gone_takes_the_default_server_away() {
    let fake = Fake::new();
    add(&fake.ctx(), &request(&[SHOP], Kind::Static, Some(SHOP))).unwrap();
    add(&fake.ctx(), &request(&["other.example.net"], Kind::Static, Some("other.example.net")))
        .unwrap();
    assert_eq!(named(&fake), [SHOP, "other.example.net"]);
    assert_eq!(
        fake.names(SETTINGS),
        ["_default.conf", "other.example.net.conf", "shop.example.org.conf"]
    );
    remove(&fake.ctx(), "other.example.net", false).unwrap();
    assert_eq!(named(&fake), [SHOP], "a certificate no site needs is given back");
    assert!(fake.names(SETTINGS).contains(&"_default.conf".to_string()));
    remove(&fake.ctx(), SHOP, false).unwrap();
    assert_eq!(fake.names(SETTINGS), Vec::<String>::new());
    assert!(named(&fake).is_empty());
    assert!(!fake.text(OVERRIDE).unwrap_or_default().contains("443"));
    assert_eq!(fake.serving(), Vec::<String>::new());
}

#[test]
fn an_https_site_with_nginx_off_is_checked_by_its_start_and_has_no_restart() {
    let fake = Fake::new().nginx_off();
    add(&fake.ctx(), &shop(Kind::Static, Some(SHOP))).unwrap();
    assert_eq!(
        fake.calls(),
        [
            "nginx_check live",
            "certificate_names shop.example.org",
            "service_check nginx",
            "turn nginx on",
            "fetch 80 shop.example.org file",
            "fetch 443 shop.example.org file",
            "fetch 80 www.shop.example.org file",
            "fetch 443 www.shop.example.org file",
        ]
    );
    assert_eq!(fake.nginx.get(), Some(Svc { enabled: true, running: true }));
}

#[test]
fn nginx_that_never_started_is_turned_on_first_to_write_its_settings() {
    let fake = Fake::new().never_started();
    add(&fake.ctx(), &shop(Kind::Static, None)).unwrap();
    assert_eq!(
        fake.calls(),
        [
            "turn nginx on",
            "nginx_check live",
            "service_check nginx",
            "restart nginx",
            "nginx_check staged",
            "restart nginx",
            "fetch 80 shop.example.org file",
            "fetch 80 www.shop.example.org file"
        ]
    );
    assert!(fake.text(&conf_view()).is_some());
}

#[test]
fn nginx_that_writes_no_settings_when_it_starts_is_turned_off_again() {
    let fake = Fake::new().never_started();
    fake.writes_conf.set(false);
    let before = fake.snapshot();
    let refusal = add(&fake.ctx(), &shop(Kind::Static, None)).unwrap_err();
    assert!(refusal.words.contains("did not write its settings"), "{}", refusal.words);
    assert_eq!(fake.nginx.get(), Some(Svc { enabled: false, running: false }));
    assert_eq!(fake.snapshot(), before);
}

#[test]
fn nginx_that_cannot_be_turned_on_to_write_its_settings_is_told_and_left_off() {
    let fake = Fake::new().never_started();
    fake.fail_calls("turn nginx on");
    let refusal = add(&fake.ctx(), &shop(Kind::Static, None)).unwrap_err();
    assert!(
        refusal.words.contains("could not be turned on to write its settings"),
        "{}",
        refusal.words
    );
    assert_eq!(fake.nginx.get(), Some(Svc { enabled: false, running: false }));
}

#[test]
fn a_site_is_refused_for_a_name_another_site_or_nginx_conf_answers_to_or_a_file_that_is_there() {
    let fake = with_shop(None);
    let taken = refused_add(&fake, &request(&["blog.example.org", WWW_SHOP], Kind::Static, None));
    assert_eq!(taken.id, Some("WEB-101"));
    assert!(
        taken.words.contains(WWW_SHOP) && taken.words.contains("shop.example.org.conf"),
        "{}",
        taken.words
    );
    assert_eq!(refused_add(&fake, &request(&[SHOP], Kind::Static, None)).id, Some("WEB-101"));
    // A file by the site's name that is not a site: its name is taken.
    fake.put(&site_view("blog.example.org"), "# notes\n");
    let file = refused_add(&fake, &request(&["blog.example.org"], Kind::Static, None));
    assert_eq!(file.id, Some("WEB-101"));
    assert!(
        file.words.contains("A file of the site blog.example.org is there already"),
        "{}",
        file.words
    );
    let fake = Fake::new();
    fake.put(
        &conf_view(),
        &NGINX_CONF.replace("server_name _;", "server_name _ taken.example.org;"),
    );
    let main = refused_add(&fake, &request(&["taken.example.org"], Kind::Static, None));
    assert_eq!(main.id, Some("WEB-101"));
    assert!(main.words.contains("nginx.conf"), "{}", main.words);
}

#[test]
fn a_site_is_refused_when_nginx_or_php_is_not_installed() {
    let fake = Fake::new().without_nginx();
    let refusal = refused_add(&fake, &shop(Kind::Static, None));
    assert_eq!((refusal.id, refusal.words.as_str()), (Some("WEB-105"), "nginx is not installed"));
    let fake = Fake::new().without_php();
    let refusal = refused_add(&fake, &shop(Kind::Php, None));
    assert_eq!((refusal.id, refusal.words.as_str()), (Some("WEB-105"), "PHP-FPM is not installed"));
    assert!(add(&fake.ctx(), &shop(Kind::Static, None)).is_ok(), "a static site needs no PHP");
}

#[test]
fn a_site_is_refused_when_nginx_conf_lacks_what_the_websites_need() {
    for (text, missing) in [
        (NGINX_CONF.replace(INCLUDE_LINE, ""), "does not include the websites' folder"),
        (
            NGINX_CONF.replace("location ^~ /sites/", "location /x/"),
            "does not answer 404 under /sites/",
        ),
        (
            NGINX_CONF.replace(LOG_FORMAT_LINE, ""),
            "does not define the log format of the sites' own logs",
        ),
    ] {
        let fake = Fake::new();
        fake.put(&conf_view(), &text);
        let refusal = refused_add(&fake, &shop(Kind::Static, None));
        assert_eq!(refusal.id, Some("WEB-102"));
        assert!(
            refusal.words.contains(missing) && refusal.words.contains(&conf_view()),
            "{}",
            refusal.words
        );
    }
    let fake = Fake::new();
    fake.put(&conf_view(), "http { server {");
    assert_eq!(refused_add(&fake, &shop(Kind::Static, None)).id, Some("WEB-102"));
}

#[test]
fn a_site_is_refused_when_nginx_refuses_its_own_settings_before_any_change() {
    let fake = Fake::new();
    fake.put(
        &site_view("old.example.org"),
        &format!("server {{ listen 80; server_name old.example.org; }} # {REFUSED}\n"),
    );
    let refusal = refused_add(&fake, &shop(Kind::Static, None));
    assert_eq!(refusal.id, Some("WEB-103"));
    assert!(
        refusal.words.contains("before any change")
            && refusal.words.contains("in /system/settings/websites/old.example.org.conf:7"),
        "{}",
        refusal.words
    );
    let fake = Fake::new();
    fake.fail_calls("nginx_check");
    let refusal = refused_add(&fake, &shop(Kind::Static, None));
    assert_eq!(refusal.id, None);
    assert!(refusal.words.contains("nginx's check could not be run"), "{}", refusal.words);
}

#[test]
fn a_site_is_refused_for_an_owner_who_is_not_a_person_and_for_an_account_that_is_not_there() {
    let fake = Fake::new();
    // Not php: when the tests do not run as root, the fake's php is the
    // process's own account, which may be a person's.
    for (owner, said) in [
        ("root", "root is not the account of a person"),
        ("daemon", "daemon is not the account of a person"),
        ("nobody", "There is no account nobody"),
    ] {
        let mut asked = shop(Kind::Static, None);
        asked.owner = owner.to_string();
        let refusal = refused_add(&fake, &asked);
        assert!(refusal.usage && refusal.id.is_none(), "{owner}");
        assert!(refusal.words.contains(said), "{}", refusal.words);
    }
}

#[test]
fn a_site_is_refused_for_a_folder_that_is_a_link_or_is_another_site_s_or_in_it() {
    let fake = with_shop(None);
    fs::create_dir_all(fake.real("/elsewhere")).unwrap();
    std::os::unix::fs::symlink(
        fake.real("/elsewhere"),
        fake.real("/shared/www/sites/blog.example.org"),
    )
    .unwrap();
    let linked = refused_add(&fake, &request(&["blog.example.org"], Kind::Static, None));
    assert!(linked.usage && linked.words.contains("link"), "{}", linked.words);
    let mut inside = request(&["blog.example.org"], Kind::Static, None);
    inside.folder = format!("{SHOP_FOLDER}/blog");
    let refusal = refused_add(&fake, &inside);
    assert!(
        refusal.usage && refusal.words.contains("in the folder of the site shop.example.org"),
        "{}",
        refusal.words
    );
    let mut above = request(&["blog.example.org"], Kind::Static, None);
    above.folder = "/shared/www/sites".to_string();
    assert!(refused_add(&fake, &above).usage);
}

#[test]
fn a_site_over_https_is_refused_for_a_certificate_that_is_not_there_or_does_not_name_it() {
    let fake = Fake::new();
    let missing = refused_add(&fake, &request(&[SHOP], Kind::Static, Some("nosuch")));
    assert_eq!(missing.id, Some("WEB-106"));
    assert!(missing.words.contains("nosuch"), "{}", missing.words);
    let other = refused_add(&fake, &request(&["blog.example.org"], Kind::Static, Some(SHOP)));
    assert_eq!(other.id, Some("WEB-106"));
    assert!(other.words.contains("does not name blog.example.org"), "{}", other.words);
    // A wildcard covers one label.
    assert!(
        add(&fake.ctx(), &request(&["blog.example.org"], Kind::Static, Some("wild.example.org")))
            .is_ok()
    );
    let deep =
        refused_add(&fake, &request(&["a.b.example.org"], Kind::Static, Some("wild.example.org")));
    assert_eq!(deep.id, Some("WEB-106"));
}

#[test]
fn the_names_a_certificate_covers_are_itself_and_one_label_under_a_wildcard() {
    let have = |names: &[&str]| names.iter().map(ToString::to_string).collect::<Vec<_>>();
    let wanted = have(&["a.example.org", "example.org", "a.b.example.org", ".example.org"]);
    assert_eq!(
        missing_names(&have(&["*.example.org"]), &wanted),
        ["example.org", "a.b.example.org", ".example.org"]
    );
    assert!(
        missing_names(
            &have(&["a.example.org", "example.org"]),
            &have(&["example.org", "a.example.org"])
        )
        .is_empty()
    );
    assert_eq!(missing_names(&[], &have(&["a.example.org"])), ["a.example.org"]);
}

#[test]
fn a_set_nginx_refuses_is_told_with_its_line_and_nothing_is_changed() {
    let fake = Fake::new();
    fake.refuses(SHOP);
    let before = fake.snapshot();
    let refusal = add(&fake.ctx(), &shop(Kind::Static, None)).unwrap_err();
    assert_eq!(refusal.id, Some("WEB-103"));
    assert_eq!(fake.snapshot(), before);
    assert_eq!(fake.nginx.get(), Some(Svc { enabled: true, running: true }));
    assert!(fake.staged_left().is_empty());
    assert!(
        refusal.words.contains("nginx refuses the new set of websites")
            && refusal.words.contains(
                "unknown directive \"refused\" in /system/settings/websites/shop.example.org.conf:7"
            ),
        "the line names the file the site will be, not the staged one: {}",
        refusal.words
    );
}

#[test]
fn a_set_nginx_refuses_after_its_service_file_changed_puts_the_service_file_and_nginx_back() {
    let fake = Fake::new();
    fake.refuses(SHOP);
    let before = fake.snapshot();
    let refusal = add(&fake.ctx(), &shop(Kind::Static, Some(SHOP))).unwrap_err();
    assert_eq!(refusal.id, Some("WEB-103"));
    assert_eq!(fake.snapshot(), before);
    // Restarted with the new file, and again with the old one back.
    let with = fake.restarted_with();
    assert_eq!(with.len(), 2);
    assert!(with[0].as_deref().is_some_and(|over| over.contains("443")));
    assert_eq!(with[1], None);
    assert_eq!(fake.nginx.get(), Some(Svc { enabled: true, running: true }));
    assert_eq!(fake.serving(), Vec::<String>::new());
}

#[test]
fn a_site_that_does_not_answer_is_told_and_taken_back() {
    let fake = Fake::new();
    fake.fail_calls("fetch 80");
    let before = fake.snapshot();
    let refusal = add(&fake.ctx(), &shop(Kind::Static, None)).unwrap_err();
    assert_eq!(refusal.id, Some("WEB-104"));
    assert!(
        refusal
            .words
            .contains("The site did not answer: port 80 for shop.example.org did not answer"),
        "{}",
        refusal.words
    );
    assert_eq!(fake.snapshot(), before);
    assert_eq!(
        fake.serving(),
        Vec::<String>::new(),
        "nginx was told to read the old settings again"
    );
    // It restarted with the new service file and the new sites, so it
    // restarts again with the old.
    assert_eq!(fake.calls().last().map(String::as_str), Some("restart nginx"));
    assert_eq!(fake.calls().iter().filter(|call| call.starts_with("restart")).count(), 3);
}

#[test]
fn a_static_add_is_taken_back_from_the_failure_of_any_call() {
    sweep(Fake::new, |fake| add(&fake.ctx(), &shop(Kind::Static, None)));
}

#[test]
fn a_php_add_over_https_is_taken_back_from_the_failure_of_any_call() {
    sweep(Fake::new, |fake| add(&fake.ctx(), &shop(Kind::Php, Some(SHOP))));
}

#[test]
fn an_https_add_with_nginx_off_is_taken_back_from_the_failure_of_any_call() {
    sweep(
        || Fake::new().nginx_off().php_on(),
        |fake| add(&fake.ctx(), &shop(Kind::Php, Some(SHOP))),
    );
}

#[test]
fn a_second_add_is_taken_back_from_the_failure_of_any_call_and_the_first_stays() {
    let make = || {
        let fake = Fake::new();
        fake.added(&["blog.example.org"], Kind::Static, Some("wild.example.org"));
        fake
    };
    sweep(make, |fake| {
        add(&fake.ctx(), &request(&["other.example.net"], Kind::Static, Some("other.example.net")))
    });
}

#[test]
fn a_call_that_fails_while_a_change_is_taken_back_is_told_as_left() {
    let clean = Fake::new();
    add(&clean.ctx(), &shop(Kind::Static, Some(SHOP))).unwrap();
    let first_fetch = clean.calls().iter().position(|call| call.starts_with("fetch")).unwrap() + 1;
    let fake = Fake::new();
    fake.fail_from(first_fetch);
    let refusal = add(&fake.ctx(), &shop(Kind::Static, Some(SHOP))).unwrap_err();
    assert_eq!(refusal.id, Some("WEB-104"));
    assert!(refusal.words.contains("It could not be taken back"), "{}", refusal.words);
    assert!(refusal.words.contains("nginx did not restart"), "{}", refusal.words);
    // What could be taken back was: the files and the service file.
    assert_eq!(fake.names(SETTINGS), Vec::<String>::new());
    assert_eq!(fake.text(OVERRIDE), None, "the service file is as it was");
}

#[test]
fn https_takes_a_site_to_https_with_its_service_file_first_and_back_with_it_last() {
    let fake = with_shop(None);
    let lines = https(&fake.ctx(), SHOP, &Https::With(SHOP.into())).unwrap();
    assert_eq!(lines, ["shop.example.org is served at https://shop.example.org/"]);
    let spec = Spec { certificate: Some(SHOP.into()), ..shop(Kind::Static, None).spec() };
    assert_eq!(fake.text(&site_view(SHOP)).unwrap(), site_text(&spec));
    assert_eq!(fake.names(SETTINGS), ["_default.conf", "shop.example.org.conf"]);
    assert_eq!(named(&fake), [SHOP]);
    assert_eq!(
        fake.calls(),
        [
            "certificate_names shop.example.org",
            "nginx_check live",
            "service_check nginx",
            "restart nginx",
            "nginx_check staged",
            "restart nginx",
            "fetch 80 shop.example.org file",
            "fetch 443 shop.example.org file",
            "fetch 80 www.shop.example.org file",
            "fetch 443 www.shop.example.org file",
        ]
    );
    assert_eq!(fake.logged(), ["changed shop.example.org to HTTPS with shop.example.org"]);
    fake.clear();
    let lines = https(&fake.ctx(), SHOP, &Https::Off).unwrap();
    assert_eq!(lines, ["shop.example.org is served at http://shop.example.org/"]);
    assert_eq!(fake.text(&site_view(SHOP)).unwrap(), site_text(&shop(Kind::Static, None).spec()));
    assert_eq!(
        fake.names(SETTINGS),
        ["shop.example.org.conf"],
        "the default server goes with the last HTTPS site"
    );
    assert!(named(&fake).is_empty());
    assert!(!fake.text(OVERRIDE).unwrap_or_default().contains("443"));
    assert_eq!(
        fake.calls(),
        [
            "nginx_check live",
            "nginx_check staged",
            "reload nginx",
            "service_check nginx",
            "restart nginx",
            "fetch 80 shop.example.org file",
            "fetch 80 www.shop.example.org file",
        ]
    );
    assert_eq!(fake.logged(), ["took shop.example.org back to HTTP only"]);
}

#[test]
fn a_site_that_is_as_asked_already_is_said_so_and_nothing_is_changed() {
    let fake = with_shop(Some(SHOP));
    let before = fake.snapshot();
    let lines = https(&fake.ctx(), SHOP, &Https::With(SHOP.into())).unwrap();
    assert_eq!(lines, ["shop.example.org serves HTTPS with shop.example.org already"]);
    assert_eq!(fake.snapshot(), before);
    assert!(!fake.changed_services());
    let plain = with_shop(None);
    let lines = https(&plain.ctx(), SHOP, &Https::Off).unwrap();
    assert_eq!(lines, ["shop.example.org is served over HTTP only already"]);
    assert!(!plain.changed_services());
}

#[test]
fn a_site_whose_file_was_changed_by_hand_is_not_rewritten() {
    let fake = with_shop(None);
    let text = fake.text(&site_view(SHOP)).unwrap();
    fake.put(&site_view(SHOP), &format!("{text}# added by a person\n"));
    let before = fake.snapshot();
    let refusal = https(&fake.ctx(), SHOP, &Https::With(SHOP.into())).unwrap_err();
    assert_eq!(refusal.id, Some("WEB-107"));
    assert!(refusal.words.contains("changed by hand"), "{}", refusal.words);
    assert_eq!(fake.snapshot(), before);
    assert!(!fake.changed_services());
}

#[test]
fn https_is_refused_for_a_site_that_is_not_there_and_a_certificate_that_does_not_fit() {
    let fake = with_shop(None);
    let none = https(&fake.ctx(), "nosuch.example.org", &Https::Off).unwrap_err();
    assert_eq!((none.id, none.words.as_str()), (None, "No website nosuch.example.org"));
    let before = fake.snapshot();
    let wrong = https(&fake.ctx(), SHOP, &Https::With("other.example.net".into())).unwrap_err();
    assert_eq!(wrong.id, Some("WEB-106"));
    assert!(wrong.words.contains("does not name shop.example.org"), "{}", wrong.words);
    let gone = https(&fake.ctx(), SHOP, &Https::With("nosuch".into())).unwrap_err();
    assert_eq!(gone.id, Some("WEB-106"));
    assert_eq!(fake.snapshot(), before);
    assert!(!fake.changed_services());
}

#[test]
fn https_is_refused_for_a_site_whose_folder_is_gone() {
    let fake = with_shop(None);
    fs::remove_dir_all(fake.real(SHOP_FOLDER)).unwrap();
    let before = fake.snapshot();
    let refusal = https(&fake.ctx(), SHOP, &Https::With(SHOP.into())).unwrap_err();
    assert!(refusal.words.contains("is not there"), "{}", refusal.words);
    assert_eq!(fake.snapshot(), before);
}

#[test]
fn https_on_is_taken_back_from_the_failure_of_any_call() {
    sweep(|| with_shop(None), |fake| https(&fake.ctx(), SHOP, &Https::With(SHOP.into())));
}

#[test]
fn https_off_is_taken_back_from_the_failure_of_any_call() {
    sweep(|| with_shop(Some(SHOP)), |fake| https(&fake.ctx(), SHOP, &Https::Off));
}

#[test]
fn a_php_site_taken_to_https_is_taken_back_from_the_failure_of_any_call() {
    let make = || {
        let fake = Fake::new().php_on();
        fake.added(&[SHOP, WWW_SHOP], Kind::Php, None);
        fake
    };
    sweep(make, |fake| https(&fake.ctx(), SHOP, &Https::With(SHOP.into())));
}

#[test]
fn remove_takes_the_site_s_file_away_and_keeps_its_folder_unless_asked() {
    let fake = with_shop(None);
    let lines = remove(&fake.ctx(), SHOP, false).unwrap();
    assert_eq!(
        lines,
        [
            "Removed shop.example.org",
            "Its folder /shared/www/sites/shop.example.org is kept: remove it with --files, or by hand"
        ]
    );
    assert_eq!(fake.names(SETTINGS), Vec::<String>::new());
    assert_eq!(fake.names(SHOP_FOLDER), ["index.html"]);
    assert_eq!(fake.calls(), ["nginx_check staged", "reload nginx"]);
    assert_eq!(fake.logged(), ["removed shop.example.org (its folder was kept)"]);
    assert_eq!(fake.serving(), Vec::<String>::new());
}

#[test]
fn remove_with_files_takes_the_folder_and_all_in_it_last() {
    let fake = with_shop(None);
    fake.put(&format!("{SHOP_FOLDER}/img/logo.png"), "x");
    let lines = remove(&fake.ctx(), SHOP, true).unwrap();
    assert_eq!(
        lines[1],
        "Removed its folder /shared/www/sites/shop.example.org and what was in it"
    );
    assert!(!fake.real(SHOP_FOLDER).exists());
    assert!(fake.real("/shared/www/sites").is_dir(), "only the site's own folder goes");
    assert_eq!(fake.logged(), ["removed shop.example.org (its folder was removed)"]);
}

#[test]
fn remove_with_files_keeps_a_folder_another_site_serves() {
    let fake = with_shop(None);
    fake.put(
        &site_view("hand"),
        &format!("server {{ listen 80; server_name hand.example.org; root {SHOP_FOLDER}; }}\n"),
    );
    let lines = remove(&fake.ctx(), SHOP, true).unwrap();
    assert_eq!(
        lines[1],
        "Its folder /shared/www/sites/shop.example.org is kept: another site serves it too"
    );
    assert!(fake.real(SHOP_FOLDER).is_dir());
}

#[test]
fn remove_gives_back_the_certificate_and_the_default_server_but_not_the_certificate_itself() {
    let fake = with_shop(Some(SHOP));
    let lines = remove(&fake.ctx(), SHOP, false).unwrap();
    assert_eq!(
        lines.last().unwrap(),
        "The certificate shop.example.org is kept; `elevate cert remove shop.example.org` removes it."
    );
    assert_eq!(fake.names(SETTINGS), Vec::<String>::new());
    assert!(named(&fake).is_empty());
    assert_eq!(
        fake.calls(),
        ["nginx_check staged", "reload nginx", "service_check nginx", "restart nginx"],
        "the files go first, the service file after"
    );
}

#[test]
fn a_default_server_a_person_changed_stays_when_the_last_https_site_goes() {
    let fake = with_shop(Some(SHOP));
    let own = format!("{}# mine\n", default_text());
    fake.put(&site_view("_default"), &own);
    remove(&fake.ctx(), SHOP, false).unwrap();
    assert_eq!(fake.text(&site_view("_default")), Some(own));
}

#[test]
fn remove_is_refused_for_a_site_that_is_not_there() {
    let fake = with_shop(None);
    let before = fake.snapshot();
    let refusal = remove(&fake.ctx(), "nosuch.example.org", true).unwrap_err();
    assert_eq!((refusal.id, refusal.words.as_str()), (None, "No website nosuch.example.org"));
    assert_eq!(fake.snapshot(), before);
    assert!(!fake.changed_services());
    // Not a name of a file, never taken as one.
    assert!(remove(&fake.ctx(), "../../etc/passwd", true).is_err());
    assert_eq!(fake.snapshot(), before);
}

#[test]
fn remove_is_refused_when_nginx_refuses_the_set_that_is_left_and_nothing_is_changed() {
    let fake = with_shop(None);
    fake.put(
        &site_view("old"),
        &format!("server {{ listen 80; server_name old.example.org; }} # {REFUSED}\n"),
    );
    let before = fake.snapshot();
    let refusal = remove(&fake.ctx(), SHOP, true).unwrap_err();
    assert_eq!(refusal.id, Some("WEB-103"));
    assert!(refusal.words.contains("in /system/settings/websites/old.conf:7"), "{}", refusal.words);
    assert_eq!(fake.snapshot(), before);
    assert!(!fake.changed_services());
}

#[test]
fn remove_is_taken_back_from_the_failure_of_any_call() {
    sweep(|| with_shop(None), |fake| remove(&fake.ctx(), SHOP, false));
    sweep(|| with_shop(Some(SHOP)), |fake| remove(&fake.ctx(), SHOP, false));
}

#[test]
fn a_change_holds_the_lock_for_every_call_it_makes_and_so_one_runs_at_a_time() {
    let fake = Fake::new();
    add(&fake.ctx(), &shop(Kind::Static, Some(SHOP))).unwrap();
    https(&fake.ctx(), SHOP, &Https::Off).unwrap();
    remove(&fake.ctx(), SHOP, false).unwrap();
    let held = fake.held();
    assert!(held.len() > 10 && held.iter().all(|held| *held), "{held:?}");
    // A change that is taken back holds it through the taking back.
    let fake = Fake::new();
    fake.fail_calls("fetch");
    assert!(add(&fake.ctx(), &shop(Kind::Static, None)).is_err());
    assert!(fake.held().iter().all(|held| *held));
    // And lets it go when it is done.
    assert!(Lock::take(fake.paths()).is_ok());
}

#[test]
fn the_files_are_written_whole_and_no_half_written_file_is_left_beside_them() {
    let fake = Fake::new();
    add(&fake.ctx(), &shop(Kind::Static, Some(SHOP))).unwrap();
    for folder in [SETTINGS, "/system/settings/spark/services/settings.d"] {
        for name in fake.names(folder) {
            assert!(
                !name.starts_with('.') && !name.contains("tmp") && !name.contains("new"),
                "{folder}/{name}"
            );
        }
    }
    assert_eq!(fake.names("/system/settings/spark/services/settings.d"), ["nginx.toml"]);
}

#[test]
fn what_the_site_file_says_is_what_was_asked_and_nothing_a_name_could_add() {
    let fake = Fake::new();
    for names in [
        vec!["a-b.example.org"],
        vec!["xn--bcher-kva.example", "1.2.3.example"],
        vec!["x"],
        vec!["shop.example.org", "a.b.c.d.e.f.g.example.org"],
    ] {
        let asked = request(&names, Kind::Static, None);
        add(&fake.ctx(), &asked).unwrap();
        let text = fake.text(&site_view(names[0])).unwrap();
        let read = conf::facts(&conf::parse(&text).unwrap());
        assert_eq!(read.names, asked.names, "{text}");
        assert_eq!(read.folder.as_deref(), Some(asked.folder.as_str()));
        assert_eq!(text, site_text(&asked.spec()));
        let line = text.lines().find(|line| line.trim_start().starts_with("server_name")).unwrap();
        assert_eq!(line.trim(), format!("server_name {};", asked.names.join(" ")));
    }
}
