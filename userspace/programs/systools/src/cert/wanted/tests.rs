// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::os::unix::fs::MetadataExt;
use std::time::Duration;

use super::*;
use crate::cert::fixtures::{DAY, NOW, Scratch};
use crate::cert::store::{Acme, Challenge, Files};

const HOUR: i64 = 3_600;
const DIRECTORY: &str = "https://pebble.example:14000/dir";
const ACCOUNT: &str = "pebble.example-14000.json";

fn store(scratch: &Scratch) -> Store {
    Store::at(&scratch.0.join("certificates"), Duration::from_secs(5))
}

fn order(name: &str) -> Order {
    Order {
        name: name.to_string(),
        names: vec![format!("{name}.example.org"), format!("www.{name}.example.org")],
        directory: DIRECTORY.to_string(),
        key: KeyKind::EcdsaP256,
        profile: None,
        account: ACCOUNT.to_string(),
        email: None,
        agree_terms: false,
    }
}

fn record(name: &str) -> Record {
    Record::first(order(name), NOW, "port 80 is closed (CRT-103)", None)
}

/// Keeps `record`, as certd or `cert get` does, under the lock.
fn keep(store: &Store, record: &Record) {
    let lock = store.lock().unwrap();
    write(store, &lock, record).unwrap();
}

fn account(store: &Store) {
    let lock = store.lock().unwrap();
    store.add_account(&lock, ACCOUNT, "{}").unwrap();
}

fn install(store: &Store, name: &str, kind: Kind) {
    let lock = store.lock().unwrap();
    let acme = Acme {
        directory: DIRECTORY.to_string(),
        names: vec![format!("{name}.example.org")],
        challenge: Challenge::Http01,
        key: KeyKind::EcdsaP256,
        profile: None,
        renew_at: None,
        revoked: false,
    };
    let files = Files {
        chain: "chain",
        key: "key",
        kind,
        made: NOW,
        acme: (kind == Kind::Acme).then_some(&acme),
    };
    store.install(&lock, name, &files).unwrap();
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().mode() & 0o7777
}

#[test]
fn the_first_day_is_tried_every_hour_and_then_every_6_hours() {
    let first = NOW;
    assert_eq!(next_try(first, first, None), Some(first + HOUR));
    assert_eq!(next_try(first, first + HOUR, None), Some(first + 2 * HOUR));
    // The last try of the first day is the one before its end.
    assert_eq!(next_try(first, first + 24 * HOUR - 1, None), Some(first + 25 * HOUR - 1));
    assert_eq!(next_try(first, first + 24 * HOUR, None), Some(first + 30 * HOUR));
    assert_eq!(next_try(first, first + 3 * DAY, None), Some(first + 3 * DAY + 6 * HOUR));
}

#[test]
fn the_tries_end_14_days_after_the_first() {
    let first = NOW;
    assert_eq!(next_try(first, first + 14 * DAY - 6 * HOUR, None), Some(first + 14 * DAY));
    assert_eq!(next_try(first, first + 14 * DAY - 6 * HOUR + 1, None), None);
    assert_eq!(next_try(first, first + 14 * DAY, None), None);
}

#[test]
fn an_authority_that_asks_for_a_wait_is_waited_for_and_never_asked_sooner() {
    let first = NOW;
    assert_eq!(next_try(first, first, Some(first + 5 * HOUR)), Some(first + 5 * HOUR));
    assert_eq!(next_try(first, first, Some(first + 600)), Some(first + HOUR));
    assert_eq!(next_try(first, first, Some(first - DAY)), Some(first + HOUR));
    // A wait past the 14 days is the end of the tries.
    assert_eq!(next_try(first, first, Some(first + 15 * DAY)), None);
}

#[test]
fn a_try_that_failed_for_the_first_time_is_the_certs_and_comes_again_in_an_hour() {
    let record = record("shop");
    assert_eq!(record.tries, 1);
    assert_eq!(record.by, By::Cert);
    assert_eq!((record.first_ms, record.last_ms), (NOW * 1_000, NOW * 1_000));
    assert_eq!(record.next_ms, (NOW + HOUR) * 1_000);
    assert!(!record.given_up);
    assert!(!record.is_due(NOW));
    assert!(!record.is_due(NOW + HOUR - 1));
    assert!(record.is_due(NOW + HOUR));
}

#[test]
fn a_try_of_certds_that_failed_counts_and_keeps_the_first_try() {
    let first = record("shop");
    let again = first.failed_again(NOW + HOUR, "the authority is down", None);
    assert_eq!(again.tries, 2);
    assert_eq!(again.by, By::Certd);
    assert_eq!(again.first_ms, first.first_ms);
    assert_eq!(again.last_ms, (NOW + HOUR) * 1_000);
    assert_eq!(again.next_ms, (NOW + 2 * HOUR) * 1_000);
    assert_eq!(again.why, "the authority is down");
    assert_eq!(again.order, first.order);
    // Told by the authority to wait, it is not asked before.
    let told = again.failed_again(NOW + 2 * HOUR, "rate limited", Some(NOW + 9 * HOUR));
    assert_eq!(told.next_ms, (NOW + 9 * HOUR) * 1_000);
}

#[test]
fn a_try_that_fails_after_14_days_gives_the_record_up() {
    let first = record("shop");
    let last = first.failed_again(NOW + 14 * DAY - 6 * HOUR, "still closed", None);
    assert!(!last.given_up);
    assert_eq!(last.next_ms, (NOW + 14 * DAY) * 1_000);
    let ended = last.failed_again(NOW + 14 * DAY, "still closed", None);
    assert!(ended.given_up);
    assert_eq!(ended.why, "still closed (tried for 14 days)");
    assert_eq!(ended.tries, 3);
    assert!(!ended.is_due(NOW + 100 * DAY));
    // Put to the end by hand, with the reason it gives.
    let gave = first.gave_up(NOW + 5, "the account was taken away");
    assert!(gave.given_up);
    assert_eq!((gave.why.as_str(), gave.by), ("the account was taken away", By::Certd));
    assert_eq!(gave.last_ms, (NOW + 5) * 1_000);
}

#[test]
fn a_record_has_failed_for_a_day_from_24_hours_after_the_first_try_until_it_is_given_up() {
    let first = record("shop");
    assert!(!first.has_failed_for_a_day(NOW + DAY - 1));
    assert!(first.has_failed_for_a_day(NOW + DAY));
    assert!(!first.gave_up(NOW + DAY, "x").has_failed_for_a_day(NOW + DAY));
    assert_eq!(first.gives_up_at(), NOW + 14 * DAY);
}

#[test]
fn a_record_is_kept_in_a_folder_of_root_and_reads_back_as_it_was() {
    let scratch = Scratch::new("wanted-round");
    let store = store(&scratch);
    let mut shop = order("shop");
    shop.profile = Some(String::from("shortlived"));
    shop.key = KeyKind::EcdsaP384;
    shop.email = Some(String::from("me@example.org"));
    shop.agree_terms = true;
    let wanted = Record::first(shop, NOW, "says \"no\"\nin two lines \\ ✓", Some(NOW + 2 * HOUR));
    keep(&store, &wanted);
    assert_eq!(mode(&store.wanted()), 0o700);
    assert_eq!(mode(&store.wanted().join("shop.toml")), 0o600);
    let lock = store.lock().unwrap();
    assert_eq!(get(&store, &lock, "shop").unwrap(), Some(wanted.clone()));
    assert_eq!(get(&store, &lock, "other").unwrap(), None);
    let text = fs::read_to_string(store.wanted().join("shop.toml")).unwrap();
    assert!(text.contains("key = \"ecdsa-p384\"\n"), "{text}");
    assert!(text.contains("email = \"me@example.org\"\nagree_terms = true\n"), "{text}");
    assert!(!text.contains("given_up"), "{text}");
    let gave = wanted.gave_up(NOW + 1, "gone");
    write(&store, &lock, &gave).unwrap();
    assert_eq!(get(&store, &lock, "shop").unwrap(), Some(gave));
}

#[test]
fn a_folder_made_with_other_modes_is_made_right() {
    let scratch = Scratch::new("wanted-modes");
    let store = store(&scratch);
    fs::create_dir_all(store.wanted()).unwrap();
    fs::set_permissions(store.wanted(), fs::Permissions::from_mode(0o755)).unwrap();
    keep(&store, &record("shop"));
    assert_eq!(mode(&store.wanted()), 0o700);
}

#[test]
fn writing_a_record_again_replaces_it_and_leaves_no_other_file_behind() {
    let scratch = Scratch::new("wanted-replace");
    let store = store(&scratch);
    keep(&store, &record("shop"));
    let again = record("shop").failed_again(NOW + HOUR, "second", None);
    keep(&store, &again);
    let lock = store.lock().unwrap();
    assert_eq!(get(&store, &lock, "shop").unwrap().unwrap().tries, 2);
    let mut files: Vec<String> = fs::read_dir(store.wanted())
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().into_string().unwrap())
        .collect();
    files.sort();
    assert_eq!(files, ["shop.toml"]);
}

#[test]
fn a_file_a_crash_left_does_not_stop_the_next_write() {
    let scratch = Scratch::new("wanted-crash");
    let store = store(&scratch);
    keep(&store, &record("shop"));
    fs::write(store.wanted().join("shop.toml.new"), "half").unwrap();
    keep(&store, &record("shop").failed_again(NOW + HOUR, "second", None));
    let lock = store.lock().unwrap();
    assert_eq!(get(&store, &lock, "shop").unwrap().unwrap().tries, 2);
    // Not a record: it does not end in `.toml`.
    assert_eq!(list(&store, &lock).unwrap().len(), 1);
}

#[test]
fn removing_a_record_says_whether_there_was_one() {
    let scratch = Scratch::new("wanted-remove");
    let store = store(&scratch);
    keep(&store, &record("shop"));
    keep(&store, &record("blog"));
    let lock = store.lock().unwrap();
    assert!(remove(&store, &lock, "shop").unwrap());
    assert!(!remove(&store, &lock, "shop").unwrap());
    assert!(!remove(&store, &lock, "never").unwrap());
    let names: Vec<String> =
        list(&store, &lock).unwrap().into_iter().map(|found| found.unwrap().order.name).collect();
    assert_eq!(names, ["blog"]);
}

#[test]
fn a_name_that_is_not_a_certificates_never_becomes_a_file_name() {
    let scratch = Scratch::new("wanted-names");
    let store = store(&scratch);
    let lock = store.lock().unwrap();
    for bad in ["../shop", "Shop", "a/b", "", "a..b", ".hidden"] {
        let mut wanted = record("shop");
        wanted.order.name = bad.to_string();
        assert!(write(&store, &lock, &wanted).is_err(), "{bad:?}");
        assert!(get(&store, &lock, bad).is_err(), "{bad:?}");
        assert!(remove(&store, &lock, bad).is_err(), "{bad:?}");
    }
    assert!(!store.wanted().exists());
}

#[test]
fn a_record_that_does_not_read_is_left_alone_and_told_of() {
    let scratch = Scratch::new("wanted-unreadable");
    let store = store(&scratch);
    keep(&store, &record("blog"));
    let junk = store.wanted().join("shop.toml");
    fs::write(&junk, "this is not toml = = =").unwrap();
    let lock = store.lock().unwrap();
    let found = list(&store, &lock).unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].as_ref().unwrap().order.name, "blog");
    let unreadable = found[1].as_ref().unwrap_err();
    assert_eq!(unreadable.name, "shop");
    assert!(unreadable.why.contains("shop.toml"), "{}", unreadable.why);
    assert!(get(&store, &lock, "shop").is_err());
    // Writing the others does not touch it, and the summary leaves it out.
    write(&store, &lock, &record("blog").failed_again(NOW + HOUR, "again", None)).unwrap();
    assert_eq!(fs::read_to_string(&junk).unwrap(), "this is not toml = = =");
    let names: Vec<String> =
        read_public(&store).unwrap().into_iter().map(|public| public.name).collect();
    assert_eq!(names, ["blog"]);
    // Only taking it away removes it.
    assert!(remove(&store, &lock, "shop").unwrap());
    assert!(!junk.exists());
}

#[test]
fn a_file_with_a_bad_name_is_told_of_and_not_acted_on() {
    let scratch = Scratch::new("wanted-badfile");
    let store = store(&scratch);
    keep(&store, &record("blog"));
    fs::write(store.wanted().join("Bad Name.toml"), "x = 1").unwrap();
    let lock = store.lock().unwrap();
    let found = list(&store, &lock).unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].as_ref().unwrap_err().name, "Bad Name");
}

#[test]
fn a_record_is_refused_for_what_is_wrong_with_it() {
    let good = text_of(&record("shop"));
    assert!(parse("shop", &good).is_ok());
    let refused = |text: &str| parse("shop", text).unwrap_err();
    assert!(parse("blog", &good).unwrap_err().contains("not its file's name"));
    assert!(refused(&good.replace("name = \"shop\"", "name = \"..\"")).contains("file's name"));
    assert!(refused(&good.replace("tries = 1", "tries = 0")).contains("`tries` is 0"));
    assert!(
        refused(&good.replace("by = \"cert\"", "by = \"someone\"")).contains("not cert or certd")
    );
    assert!(refused(&good.replace("ecdsa-p256", "rsa")).contains("is not a key type"));
    assert!(
        refused(&good.replace("pebble.example-14000.json", "other.json")).contains("`account`")
    );
    assert!(
        refused(&good.replace("shop.example.org\"", "10.0.0.1\"")).contains("IP address"),
        "names are checked as `cert get` checks them"
    );
    assert!(refused(&format!("{good}extra = 1\n")).contains("extra"));
    assert!(refused(&format!("{good}email = \"nobody\"\n")).contains("is not an email address"));
    assert!(refused(&good.replace("first_ms = ", "first = ")).contains("first_ms"));
    assert!(
        refused(
            &good.replace("directory = \"https://pebble.example:14000/dir\"", "directory = \"no\"")
        )
        .contains("`directory`")
    );
}

#[test]
fn the_summary_tells_all_but_the_account_and_is_readable_by_anyone() {
    let scratch = Scratch::new("wanted-public");
    let store = store(&scratch);
    assert_eq!(read_public(&store).unwrap(), Vec::new());
    keep(&store, &record("shop"));
    keep(&store, &record("blog").gave_up(NOW + 3, "the account was taken away"));
    assert_eq!(mode(&store.wanted_public()), 0o644);
    let text = fs::read_to_string(store.wanted_public()).unwrap();
    assert!(!text.contains(ACCOUNT) && !text.contains("\"account\""), "{text}");
    assert!(text.starts_with("{\"format\":1,\"wanted\":["), "{text}");
    let public = read_public(&store).unwrap();
    assert_eq!(public.len(), 2);
    let shop = public.iter().find(|one| one.name == "shop").unwrap();
    assert_eq!(
        *shop,
        Public {
            name: String::from("shop"),
            names: vec![String::from("shop.example.org"), String::from("www.shop.example.org")],
            directory: DIRECTORY.to_string(),
            first_ms: NOW * 1_000,
            last_ms: NOW * 1_000,
            next_ms: (NOW + HOUR) * 1_000,
            tries: 1,
            why: String::from("port 80 is closed (CRT-103)"),
            given_up: false,
        }
    );
    assert!(public.iter().find(|one| one.name == "blog").unwrap().given_up);
}

#[test]
fn the_summary_follows_each_change_and_is_empty_when_nothing_is_wanted() {
    let scratch = Scratch::new("wanted-follow");
    let store = store(&scratch);
    keep(&store, &record("shop"));
    let lock = store.lock().unwrap();
    assert!(remove(&store, &lock, "shop").unwrap());
    assert_eq!(read_public(&store).unwrap(), Vec::new());
    assert_eq!(
        fs::read_to_string(store.wanted_public()).unwrap(),
        "{\"format\":1,\"wanted\":[]}\n"
    );
    // Nothing wanted and nothing said makes no file.
    let fresh = Scratch::new("wanted-fresh");
    let none = self::store(&fresh);
    let lock = none.lock().unwrap();
    publish(&none, &lock).unwrap();
    assert!(!none.wanted_public().exists());
}

#[test]
fn a_summary_of_another_format_or_that_is_not_one_is_refused() {
    let scratch = Scratch::new("wanted-format");
    let store = store(&scratch);
    fs::write(store.wanted_public(), "{\"format\":2,\"wanted\":[]}").unwrap();
    assert!(read_public(&store).unwrap_err().contains("its format is 2, not 1"));
    fs::write(store.wanted_public(), "nonsense").unwrap();
    assert!(read_public(&store).is_err());
}

#[test]
fn the_json_has_the_fields_the_console_reads() {
    let scratch = Scratch::new("wanted-json");
    let store = store(&scratch);
    keep(&store, &record("shop"));
    let document = Document { format: FORMAT, wanted: read_public(&store).unwrap() };
    let json: serde_json::Value = serde_json::to_value(&document).unwrap();
    assert_eq!(json["format"], 1);
    let mut fields: Vec<&str> =
        json["wanted"][0].as_object().unwrap().keys().map(String::as_str).collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        [
            "directory",
            "first_ms",
            "given_up",
            "last_ms",
            "name",
            "names",
            "next_ms",
            "tries",
            "why"
        ]
    );
}

#[test]
fn a_record_is_current_until_it_is_taken_away_or_made_again() {
    let scratch = Scratch::new("wanted-current");
    let store = store(&scratch);
    let read = record("shop");
    keep(&store, &read);
    let lock = store.lock().unwrap();
    assert!(is_current(&store, &lock, &read));
    // A try of certd's changes it, and it is the same one still.
    write(&store, &lock, &read.failed_again(NOW + HOUR, "again", None)).unwrap();
    assert!(is_current(&store, &lock, &read));
    // `cert get` made it again, a new first try.
    let again = Record::first(order("shop"), NOW + 5 * HOUR, "again", None);
    write(&store, &lock, &again).unwrap();
    assert!(!is_current(&store, &lock, &read));
    remove(&store, &lock, "shop").unwrap();
    assert!(!is_current(&store, &lock, &again));
}

#[test]
fn certd_cannot_go_on_without_the_account_or_over_a_certificate_added_by_hand() {
    let scratch = Scratch::new("wanted-cannot");
    let store = store(&scratch);
    let shop = record("shop");
    let why = cannot_go_on(&store, &shop).unwrap();
    assert!(why.contains("the account at pebble.example was taken away"), "{why}");
    assert!(why.contains("`elevate cert get shop`"), "{why}");
    // The person agreed to the terms: certd makes the account, and has not
    // given up for want of one.
    let mut agreed = record("shop");
    agreed.order.agree_terms = true;
    assert_eq!(cannot_go_on(&store, &agreed), None);
    account(&store);
    assert_eq!(cannot_go_on(&store, &shop), None);
    // A certificate from an authority of its name is replaced, as `cert get`
    // does; one added by hand is not.
    install(&store, "shop", Kind::Acme);
    assert_eq!(cannot_go_on(&store, &shop), None);
    install(&store, "blog", Kind::Added);
    let why = cannot_go_on(&store, &record("blog")).unwrap();
    assert!(why.contains("blog was added by hand since"), "{why}");
}
