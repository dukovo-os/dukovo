// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a count reads of the sites' logs, what it keeps, and what `list`
//! says of it, on a machine in a folder.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;

use serde_json::Value;

use super::super::conf::Kind;
use super::super::fake::{Fake, NGINX_CONF};
use super::super::list;
use super::super::plan::{LOG_FORMAT, LOG_FORMAT_LINE, SETTINGS};
use super::*;

const SHOP: &str = "shop.example.org";

/// The line of a request made `second` seconds into `day`, which nginx
/// writes as the format says.
fn line(day: i64, second: i64, status: u16, bytes: u64) -> String {
    format!("{}.{:03} {status} {bytes}\n", day * DAY + second, second % 1000)
}

/// Where the sites' logs are on `fake`.
fn logs(fake: &Fake) -> PathBuf {
    fake.real(LOGS)
}

/// Writes the log file `file`.
fn put(fake: &Fake, file: &str, bytes: &[u8]) {
    fs::create_dir_all(logs(fake)).unwrap();
    fs::write(logs(fake).join(file), bytes).unwrap();
}

/// `text` as logturn compresses a turned file.
fn packed(text: &str) -> Vec<u8> {
    zstd::stream::encode_all(text.as_bytes(), 3).unwrap()
}

/// A count of yesterday, or of today.
fn counted(fake: &Fake, today: bool) -> Report {
    count(&fake.ctx(), today, Limits::STANDARD).expect("it counts")
}

/// What the report says of `name`'s day.
fn day_of(report: &Report, name: &str) -> Day {
    let site = report.sites.iter().find(|site| site.name == name).expect("the site is there");
    site.day.clone().expect("it has a day")
}

/// A machine with the site `SHOP`.
fn with_shop() -> Fake {
    let fake = Fake::new();
    fake.added(&[SHOP], Kind::Static, None);
    fake
}

/// What `list` says, as JSON.
fn listed(fake: &Fake) -> Value {
    let listing = list::list(&fake.ctx()).expect("it lists");
    serde_json::from_str(&listing.json().unwrap()).unwrap()
}

#[test]
fn the_format_nginx_writes_is_the_one_a_line_is_read_by() {
    assert_eq!(LOG_FORMAT, "$msec $status $body_bytes_sent");
    assert_eq!(LOG_FORMAT_LINE, format!("log_format site '{LOG_FORMAT}';"));
    // What nginx writes for `$msec $status $body_bytes_sent`.
    let read = parse(b"1763175600.123 200 512").expect("a line of the format");
    assert_eq!(read, Logged { day: 1763175600 / DAY, status: 200, bytes: 512 });
    // The bundle's own settings define it, once, in the `http` block.
    let lines = NGINX_CONF.lines().filter(|held| held.trim() == LOG_FORMAT_LINE);
    assert_eq!(lines.count(), 1, "the bundle's nginx.conf");
}

#[test]
fn a_day_is_counted_from_the_live_file_and_the_two_turned_ones_by_the_time_of_each_line() {
    let fake = with_shop();
    let (yesterday, today) = (fake.today() - 1, fake.today());
    let oldest = line(yesterday - 1, 80_000, 200, 10)
        + &line(yesterday, 5, 200, 100)
        + &line(yesterday, 100, 404, 20);
    put(&fake, &format!("{SHOP}.log.2.zst"), &packed(&oldest));
    let newer = line(yesterday, 40_000, 301, 0)
        + &line(yesterday, 86_399, 200, 1000)
        + &line(today, 3, 200, 7);
    put(&fake, &format!("{SHOP}.log.1"), newer.as_bytes());
    // Written to the copy until nginx opened its file again.
    let live = line(yesterday, 50_000, 200, 1) + &line(today, 100, 200, 5);
    put(&fake, &format!("{SHOP}.log"), live.as_bytes());

    let report = counted(&fake, false);
    assert_eq!(
        day_of(&report, SHOP),
        Day { day: day_text(yesterday), visits: 4, errors: 1, bytes: 1121, complete: true }
    );
    assert_eq!(report.day, day_text(yesterday));

    // Today so far is what the same files hold of today.
    let report = counted(&fake, true);
    assert_eq!(
        day_of(&report, SHOP),
        Day { day: day_text(today), visits: 2, errors: 0, bytes: 12, complete: true }
    );
}

#[test]
fn an_older_copy_that_is_not_compressed_yet_is_read_and_one_that_is_is_read_once() {
    let fake = with_shop();
    let yesterday = fake.today() - 1;
    let text = line(yesterday, 5, 200, 100);
    put(&fake, &format!("{SHOP}.log.2"), text.as_bytes());
    assert_eq!(day_of(&counted(&fake, false), SHOP).visits, 1);
    // Both are there while logturn makes the one from the other.
    put(&fake, &format!("{SHOP}.log.2.zst"), &packed(&text));
    assert_eq!(day_of(&counted(&fake, false), SHOP).visits, 1);
}

#[test]
fn a_status_from_200_to_399_is_a_visit_and_from_400_to_599_an_error_and_others_are_neither() {
    let fake = with_shop();
    let yesterday = fake.today() - 1;
    let mut text = String::new();
    for status in [200, 204, 301, 399] {
        text.push_str(&line(yesterday, 1, status, 10));
    }
    for status in [400, 404, 499, 502, 599] {
        text.push_str(&line(yesterday, 2, status, 100));
    }
    for status in [0, 100, 199, 600, 999] {
        text.push_str(&line(yesterday, 3, status, 1000));
    }
    put(&fake, &format!("{SHOP}.log"), text.as_bytes());
    let day = day_of(&counted(&fake, false), SHOP);
    assert_eq!((day.visits, day.errors, day.bytes), (4, 5, 4 * 10 + 5 * 100));
}

#[test]
fn a_line_that_is_not_a_request_of_the_format_is_passed_over() {
    let fake = with_shop();
    let yesterday = fake.today() - 1;
    let t = yesterday * DAY + 100;
    let mut text = line(yesterday, 1, 200, 1);
    for bad in [
        "garbage".to_string(),
        String::new(),
        format!("{t}.123 200"),
        format!("{t}.123 200 5 6"),
        format!("{t} 200 5"),
        format!("{t}.12 200 5"),
        format!("{t}.1234 200 5"),
        format!("x{t}.123 200 5"),
        format!("-{t}.123 200 5"),
        format!("{t}.123 20 5"),
        format!("{t}.123 2000 5"),
        format!("{t}.123 +20 5"),
        format!("{t}.123 200 -5"),
        format!("{t}.123 200 5x"),
        format!("{t}.123  200 5"),
        format!("{t}.123 200 99999999999999999999999"),
        "x".repeat(300),
        format!("{} {t}.123 200 5", "y".repeat(LINE_MAX)),
    ] {
        text.push_str(&bad);
        text.push('\n');
    }
    // A line that is not UTF-8.
    let mut bytes = text.into_bytes();
    bytes.extend_from_slice(b"\xff\xfe 200 5\n");
    bytes.extend_from_slice(line(yesterday, 2, 404, 2).as_bytes());
    // The last line is cut, as one nginx is writing is.
    bytes.extend_from_slice(format!("{t}.123 200 9").as_bytes());
    put(&fake, &format!("{SHOP}.log"), &bytes);

    let report = counted(&fake, false);
    let day = day_of(&report, SHOP);
    assert_eq!((day.visits, day.errors, day.bytes, day.complete), (1, 1, 3, true));
    assert!(report.sites[0].problems.is_empty());
}

#[test]
fn a_line_of_the_longest_length_the_format_has_is_still_read() {
    let fake = with_shop();
    let yesterday = fake.today() - 1;
    let widest = format!("{}.999 599 {}\n", yesterday * DAY + 5, u64::MAX);
    assert!(widest.len() < LINE_MAX);
    put(&fake, &format!("{SHOP}.log"), widest.as_bytes());
    let day = day_of(&counted(&fake, false), SHOP);
    assert_eq!((day.errors, day.bytes), (1, u64::MAX));
}

#[test]
fn a_file_over_the_byte_limit_is_counted_as_far_as_it_goes_and_marked() {
    let fake = with_shop();
    let yesterday = fake.today() - 1;
    let text: String = (0..10).map(|n| line(yesterday, n, 200, 1)).collect();
    let each = line(yesterday, 0, 200, 1).len() as u64;
    put(&fake, &format!("{SHOP}.log"), text.as_bytes());

    let limits = Limits { bytes: 2 * each + 1, time: Duration::from_secs(3600) };
    let report = count(&fake.ctx(), false, limits).unwrap();
    let day = day_of(&report, SHOP);
    assert_eq!((day.visits, day.complete), (3, false));

    // A limit that the file ends at is no cut.
    let limits = Limits { bytes: 10 * each, time: Duration::from_secs(3600) };
    let day = day_of(&count(&fake.ctx(), false, limits).unwrap(), SHOP);
    assert_eq!((day.visits, day.complete), (10, true));

    // The same for a compressed copy, which is cut where it is read.
    fs::remove_file(logs(&fake).join(format!("{SHOP}.log"))).unwrap();
    put(&fake, &format!("{SHOP}.log.2.zst"), &packed(&text));
    let limits = Limits { bytes: 2 * each + 1, time: Duration::from_secs(3600) };
    let day = day_of(&count(&fake.ctx(), false, limits).unwrap(), SHOP);
    assert_eq!((day.visits, day.complete), (3, false));
}

#[test]
fn a_file_read_for_longer_than_the_time_limit_is_counted_so_far_and_marked() {
    let fake = with_shop();
    let yesterday = fake.today() - 1;
    put(&fake, &format!("{SHOP}.log"), line(yesterday, 1, 200, 1).as_bytes());
    let limits = Limits { bytes: u64::MAX, time: Duration::ZERO };
    let day = day_of(&count(&fake.ctx(), false, limits).unwrap(), SHOP);
    assert_eq!((day.visits, day.complete), (0, false));
    // A file with nothing in it is read to its end in no time.
    put(&fake, &format!("{SHOP}.log"), b"");
    let day = day_of(&count(&fake.ctx(), false, limits).unwrap(), SHOP);
    assert_eq!((day.visits, day.complete), (0, true));
}

#[test]
fn a_log_that_is_a_link_is_not_followed_and_is_told() {
    let fake = with_shop();
    let yesterday = fake.today() - 1;
    let secret = fake.real("/secret.log");
    fs::write(&secret, line(yesterday, 1, 200, 1).repeat(5)).unwrap();
    fs::create_dir_all(logs(&fake)).unwrap();
    symlink(&secret, logs(&fake).join(format!("{SHOP}.log"))).unwrap();
    put(&fake, &format!("{SHOP}.log.1"), line(yesterday, 2, 200, 1).as_bytes());

    let report = counted(&fake, false);
    let day = day_of(&report, SHOP);
    assert_eq!((day.visits, day.complete), (1, false));
    let problems = &report.sites[0].problems;
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].starts_with(&format!("{SHOP}.log: ")), "{problems:?}");
    assert!(report.plain().contains("at least 1 visits"), "{}", report.plain());
}

#[test]
fn a_log_that_is_not_a_file_and_a_folder_of_logs_that_is_a_link_are_not_read() {
    let fake = with_shop();
    let yesterday = fake.today() - 1;
    fs::create_dir_all(logs(&fake).join(format!("{SHOP}.log"))).unwrap();
    let report = counted(&fake, false);
    assert!(!day_of(&report, SHOP).complete);
    assert!(report.sites[0].problems[0].contains("not a file"), "{:?}", report.sites[0].problems);

    // The folder is nginx's, so nginx's account may make it a link.
    let elsewhere = fake.real("/elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join(format!("{SHOP}.log")), line(yesterday, 1, 200, 1)).unwrap();
    fs::remove_dir_all(logs(&fake)).unwrap();
    symlink(&elsewhere, logs(&fake)).unwrap();
    let report = counted(&fake, false);
    assert!(report.sites[0].day.is_none());
    assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
    assert!(report.problems[0].starts_with(LOGS), "{:?}", report.problems);
}

#[test]
fn a_compressed_copy_that_asks_for_more_than_a_window_of_2_mib_is_refused() {
    let fake = with_shop();
    let yesterday = fake.today() - 1;
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 3).unwrap();
    encoder.window_log(27).unwrap();
    encoder.write_all(line(yesterday, 1, 200, 1).as_bytes()).unwrap();
    put(&fake, &format!("{SHOP}.log.2.zst"), &encoder.finish().unwrap());
    let report = counted(&fake, false);
    assert!(!day_of(&report, SHOP).complete);
    assert!(report.sites[0].problems[0].starts_with(&format!("{SHOP}.log.2.zst: ")));
}

#[test]
fn a_compressed_copy_that_is_not_one_is_told() {
    let fake = with_shop();
    put(&fake, &format!("{SHOP}.log.2.zst"), b"this is not zstd, though it is long enough to say");
    let report = counted(&fake, false);
    let day = day_of(&report, SHOP);
    assert_eq!((day.visits, day.complete), (0, false));
    assert_eq!(report.sites[0].problems.len(), 1);
}

#[test]
fn a_site_without_a_log_has_no_day_and_one_with_an_empty_log_has_a_day_of_none() {
    let fake = Fake::new();
    fake.added(&[SHOP], Kind::Static, None);
    let report = counted(&fake, false);
    assert!(report.sites[0].day.is_none());
    assert!(report.plain().contains("no log of its requests"), "{}", report.plain());
    assert!(kept(fake.paths()).unwrap().sites[SHOP].days.is_empty());

    put(&fake, &format!("{SHOP}.log"), b"");
    let day = day_of(&counted(&fake, false), SHOP);
    assert_eq!((day.visits, day.errors, day.bytes, day.complete), (0, 0, 0, true));
}

#[test]
fn what_is_counted_is_kept_for_anyone_to_read() {
    let fake = with_shop();
    let yesterday = fake.today() - 1;
    put(&fake, &format!("{SHOP}.log"), line(yesterday, 1, 200, 10).as_bytes());
    counted(&fake, false);

    let mode = |view: &str| fs::metadata(fake.real(view)).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(COUNTS), 0o644);
    assert_eq!(mode(STATE), 0o755);
    let text = fake.text(COUNTS).unwrap();
    assert!(!text.contains('\n'));
    let counts = Counts::parse(&text).unwrap();
    assert_eq!(counts.format, 1);
    let site = &counts.sites[SHOP];
    assert_eq!(site.days.len(), 1);
    assert_eq!(site.days[0].visits, 1);
    assert_eq!(site.gone_since_ms, None);
    let size = site.size.expect("the folder was walked");
    assert!(size.complete && size.files >= 1 && size.bytes > 0, "{size:?}");
    assert_eq!(size.measured_ms, u64::try_from(fake.today() * DAY + 12 * 3600).unwrap() * 1000);
    assert_eq!(fake.calls(), [format!("measure {SHOP}")]);

    // Counted again, the day is the one day, and nothing is added to it.
    put(&fake, &format!("{SHOP}.log.1"), line(yesterday, 2, 200, 5).as_bytes());
    counted(&fake, false);
    let counts = kept(fake.paths()).unwrap();
    assert_eq!(counts.sites[SHOP].days.len(), 1);
    assert_eq!(counts.sites[SHOP].days[0].visits, 2);
}

#[test]
fn list_says_the_visits_and_the_size_of_each_site() {
    let fake = with_shop();
    let today = fake.today();
    let mut text = String::new();
    for (back, visits) in [(1, 4), (2, 3), (5, 2), (8, 7), (20, 1), (31, 100)] {
        for n in 0..visits {
            text.push_str(&line(today - back, n, 200, 1));
        }
        text.push_str(&line(today - back, 999, 500, 1));
    }
    // Each day is counted on its day: a count is of the files of then.
    for back in [1, 2, 5, 8, 20, 31] {
        fake.set_now((today - back + 1) * DAY + 3600);
        put(&fake, &format!("{SHOP}.log"), text.as_bytes());
        counted(&fake, false);
    }
    fake.set_now(today * DAY + 12 * 3600);

    let listing = listed(&fake);
    let site = &listing["websites"][0];
    let visits = &site["visits"];
    assert_eq!(visits["yesterday"], 4);
    assert_eq!(visits["last_7_days"], 4 + 3 + 2);
    assert_eq!(visits["last_30_days"], 4 + 3 + 2 + 7 + 1);
    assert_eq!(visits["complete"], true);
    let days = visits["days"].as_array().unwrap();
    assert_eq!(days.len(), 6);
    assert_eq!(days[0], serde_json::json!([day_text(today - 1), 4, 1, 4 + 1]));
    assert_eq!(days[5][0], day_text(today - 31));
    let mut members: Vec<&str> = visits.as_object().unwrap().keys().map(String::as_str).collect();
    members.sort_unstable();
    assert_eq!(members, ["complete", "days", "last_30_days", "last_7_days", "yesterday"]);

    let size = &site["size"];
    let mut members: Vec<&str> = size.as_object().unwrap().keys().map(String::as_str).collect();
    members.sort_unstable();
    assert_eq!(members, ["bytes", "complete", "files", "measured_ms"]);
    assert!(size["files"].as_u64().unwrap() >= 1);

    let plain = list::list(&fake.ctx()).unwrap().plain();
    let head = plain.lines().next().unwrap();
    assert!(head.starts_with("NAME") && head.contains("SIZE") && head.ends_with("FOLDER"), "{head}");
}

#[test]
fn list_shows_at_most_30_days_newest_first() {
    let fake = with_shop();
    let today = fake.today();
    let mut counts = Counts::default();
    for back in 1..=40 {
        counts.record(
            SHOP,
            Day { day: day_text(today - back), visits: 1, errors: 0, bytes: 0, complete: true },
        );
    }
    keep(fake.paths(), &counts).map_err(|refusal| refusal.words).unwrap();
    let listing = listed(&fake);
    let days = listing["websites"][0]["visits"]["days"].as_array().unwrap().clone();
    assert_eq!(days.len(), DAYS_SHOWN);
    assert_eq!(days[0][0], day_text(today - 1));
    assert_eq!(days[29][0], day_text(today - 30));
    assert_eq!(listing["websites"][0]["visits"]["last_30_days"], 30);
}

#[test]
fn a_site_nothing_was_counted_of_has_visits_and_size_of_null() {
    let fake = with_shop();
    let listing = listed(&fake);
    assert_eq!(listing["websites"][0]["visits"], Value::Null);
    assert_eq!(listing["websites"][0]["size"], Value::Null);
    let plain = list::list(&fake.ctx()).unwrap().plain();
    let row = plain.lines().nth(1).unwrap();
    assert!(row.split_whitespace().any(|word| word == "-"), "{row}");
}

#[test]
fn a_site_that_is_listed_with_a_size_that_is_a_least_says_so() {
    let fake = with_shop();
    fake.cut_walks();
    counted(&fake, false);
    let listing = listed(&fake);
    assert_eq!(listing["websites"][0]["size"]["complete"], false);
    let plain = list::list(&fake.ctx()).unwrap().plain();
    assert!(plain.lines().nth(1).unwrap().split_whitespace().any(|word| word.ends_with('+')));
}

#[test]
fn a_site_keeps_the_newest_400_days() {
    let mut counts = Counts::default();
    for back in 0..450 {
        let day = Day { day: day_text(20_000 - back), visits: 1, errors: 0, bytes: 0, complete: true };
        counts.record(SHOP, day);
    }
    let days = &counts.sites[SHOP].days;
    assert_eq!(days.len(), DAYS_KEPT);
    assert_eq!(days[0].day, day_text(20_000));
    assert_eq!(days[DAYS_KEPT - 1].day, day_text(20_000 - 399));
    // A day newer than all of them is put first, and the oldest goes.
    counts.record(
        SHOP,
        Day { day: day_text(20_001), visits: 9, errors: 0, bytes: 0, complete: true },
    );
    let days = &counts.sites[SHOP].days;
    assert_eq!((days.len(), days[0].visits), (DAYS_KEPT, 9));
    assert_eq!(days[DAYS_KEPT - 1].day, day_text(20_000 - 398));
    // A day older than all of them is not kept.
    counts.record(SHOP, Day { day: day_text(1000), visits: 1, errors: 0, bytes: 0, complete: true });
    assert_eq!(counts.sites[SHOP].days.len(), DAYS_KEPT);
    assert!(counts.sites[SHOP].days.iter().all(|day| day.day != day_text(1000)));
}

#[test]
fn what_is_kept_of_a_gone_site_goes_after_30_days_and_comes_back_with_the_site() {
    let mut counts = Counts::default();
    let day = Day { day: day_text(20_000), visits: 1, errors: 0, bytes: 0, complete: true };
    counts.record("kept", day.clone());
    counts.record("gone", day);
    let start = 1_000_000_000_000u64;
    counts.forget_gone(&["kept", "gone"], start);
    assert_eq!(counts.sites["gone"].gone_since_ms, None);

    counts.forget_gone(&["kept"], start + DAY_MS);
    assert_eq!(counts.sites["gone"].gone_since_ms, Some(start + DAY_MS));
    assert_eq!(counts.sites["kept"].gone_since_ms, None);
    // Missed again, it is still missed since the first time.
    counts.forget_gone(&["kept"], start + 30 * DAY_MS);
    assert_eq!(counts.sites["gone"].gone_since_ms, Some(start + DAY_MS));
    assert!(counts.sites.contains_key("gone"), "29 days gone is kept");
    counts.forget_gone(&["kept"], start + 31 * DAY_MS + 1);
    assert!(!counts.sites.contains_key("gone"));
    assert!(counts.sites.contains_key("kept"));

    let mut counts = Counts::default();
    counts.record("back", Day { day: day_text(20_000), visits: 1, errors: 0, bytes: 0, complete: true });
    counts.forget_gone(&[], start);
    counts.forget_gone(&["back"], start + 10 * DAY_MS);
    assert_eq!(counts.sites["back"].gone_since_ms, None);
    assert_eq!(counts.sites["back"].days.len(), 1);
}

#[test]
fn a_count_drops_a_gone_site_after_30_days_in_the_file() {
    let fake = with_shop();
    let mut counts = Counts::default();
    counts.record("old.example.org", Day { day: day_text(1), visits: 1, errors: 0, bytes: 0, complete: true });
    keep(fake.paths(), &counts).map_err(|refusal| refusal.words).unwrap();
    counted(&fake, false);
    let seen = kept(fake.paths()).unwrap();
    assert!(seen.sites["old.example.org"].gone_since_ms.is_some());
    fake.set_now(fake.today() * DAY + 12 * 3600 + 29 * DAY);
    counted(&fake, false);
    assert!(kept(fake.paths()).unwrap().sites.contains_key("old.example.org"));
    fake.set_now(fake.today() * DAY + 12 * 3600 + 3 * DAY);
    counted(&fake, false);
    assert!(!kept(fake.paths()).unwrap().sites.contains_key("old.example.org"));
}

#[test]
fn counts_that_cannot_be_read_are_made_anew_and_told() {
    let fake = with_shop();
    fake.put(COUNTS, "{ this is not what was written");
    let report = counted(&fake, false);
    assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
    assert!(report.problems[0].starts_with(COUNTS) && report.problems[0].contains("made anew"));
    assert!(kept(fake.paths()).is_ok());

    fake.put(COUNTS, "{\"format\":2,\"sites\":{}}");
    assert!(kept(fake.paths()).unwrap_err().contains("format"));
    // A listing does not fail for it: it lists without them.
    assert_eq!(listed(&fake)["websites"][0]["visits"], Value::Null);
}

#[test]
fn a_walk_that_cannot_be_made_is_told_and_what_was_counted_is_kept() {
    let fake = with_shop();
    let yesterday = fake.today() - 1;
    put(&fake, &format!("{SHOP}.log"), line(yesterday, 1, 200, 10).as_bytes());
    fake.fail_calls("measure");
    let report = counted(&fake, false);
    assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
    assert!(report.problems[0].contains("not measured"));
    let counts = kept(fake.paths()).unwrap();
    assert_eq!(counts.sites[SHOP].days[0].visits, 1);
    assert_eq!(counts.sites[SHOP].size, None);
}

#[test]
fn a_walk_cut_short_marks_the_size_as_at_least() {
    let fake = with_shop();
    fake.cut_walks();
    let report = counted(&fake, false);
    assert!(!report.sites[0].size.unwrap().complete);
    assert!(report.plain().contains("folder at least "), "{}", report.plain());
}

#[test]
fn the_report_says_each_site_in_words_and_in_json() {
    let fake = with_shop();
    let yesterday = fake.today() - 1;
    put(&fake, &format!("{SHOP}.log"), (line(yesterday, 1, 200, 2048) + &line(yesterday, 2, 404, 5)).as_bytes());
    let report = counted(&fake, false);
    let plain = report.plain();
    assert!(
        plain.starts_with(&format!("{SHOP}: {}: 1 visits, 1 errors, 2.1K sent; folder ", day_text(yesterday))),
        "{plain}"
    );
    assert!(plain.ends_with(" files\n"), "{plain}");
    let json: Value = serde_json::from_str(&report.json().unwrap()).unwrap();
    assert_eq!(json["day"], day_text(yesterday));
    assert_eq!(json["sites"][0]["name"], SHOP);
    assert_eq!(json["sites"][0]["day"]["visits"], 1);
    assert_eq!(json["sites"][0]["day"]["errors"], 1);
    assert_eq!(json["sites"][0]["day"]["bytes"], 2053);
    assert_eq!(json["sites"][0]["day"]["complete"], true);
    assert!(json["sites"][0]["size"]["bytes"].as_u64().unwrap() > 0);
    assert_eq!(json["problems"], serde_json::json!([]));
}

#[test]
fn the_files_of_a_site_s_log_are_told_from_those_of_another_s() {
    for file in [
        "a.log",
        "a.log.1",
        "a.log.2",
        "a.log.2.zst",
        "a.log.2.zst.new",
        "a.log.10",
    ] {
        assert!(is_log_of(file, "a"), "{file}");
    }
    for file in [
        "b.log",
        "a",
        "a.lo",
        "a.logs",
        "a.log.x",
        "a.log.",
        "a.log.1x",
        "a.log.zst",
        "a.log.1.log",
        "a.example.log",
        "a.example.log.1",
        "ba.log",
    ] {
        assert!(!is_log_of(file, "a"), "{file}");
    }
    assert!(is_log_of("a.example.log.1", "a.example"));
    assert!(!is_log_of("a.example.log", "a"));
}

#[test]
fn a_removed_site_has_the_files_of_its_log_removed_and_no_other() {
    let fake = with_shop();
    fake.added(&["other.example.org"], Kind::Static, None);
    let files = [
        format!("{SHOP}.log"),
        format!("{SHOP}.log.1"),
        format!("{SHOP}.log.2.zst"),
        format!("{SHOP}.log.2.zst.new"),
        "other.example.org.log".to_string(),
        "other.example.org.log.1".to_string(),
        format!("x{SHOP}.log"),
    ];
    for file in &files {
        put(&fake, file, b"x");
    }
    super::super::remove::remove(&fake.ctx(), SHOP, false).unwrap();
    let left = fake.names(LOGS);
    assert_eq!(left, ["other.example.org.log", "other.example.org.log.1", "xshop.example.org.log"]);
}

#[test]
fn a_site_removed_without_a_folder_of_logs_is_removed() {
    let fake = with_shop();
    fs::remove_dir_all(fake.real("/shared/services/org.nginx.nginx/nginx/logs")).unwrap();
    super::super::remove::remove(&fake.ctx(), SHOP, false).unwrap();
}

#[test]
fn a_removed_site_s_log_that_is_a_link_is_unlinked_and_not_followed() {
    let fake = with_shop();
    let secret = fake.real("/secret");
    fs::write(&secret, "keep me").unwrap();
    fs::create_dir_all(logs(&fake)).unwrap();
    symlink(&secret, logs(&fake).join(format!("{SHOP}.log"))).unwrap();
    super::super::remove::remove(&fake.ctx(), SHOP, false).unwrap();
    assert!(fake.names(LOGS).is_empty());
    assert_eq!(fs::read_to_string(secret).unwrap(), "keep me");
}

#[test]
fn a_site_that_was_added_before_the_sites_were_logged_has_no_visits_until_it_is_written_again() {
    let fake = with_shop();
    let file = format!("{SETTINGS}/{SHOP}.conf");
    let logged = fake.text(&file).unwrap();
    assert_eq!(logged.matches("access_log").count(), 2);
    let old: String = logged
        .lines()
        .filter(|line| !line.contains("access_log") && !line.contains("Requests go to"))
        .map(|line| format!("{line}\n"))
        .collect();
    assert!(!old.contains("access_log"));
    fake.put(&file, &old);
    // It is not a site that was changed by hand.
    let listing = listed(&fake);
    assert_eq!(listing["websites"][0]["by_hand"], false);
    assert_eq!(listing["websites"][0]["visits"], Value::Null);
    let report = counted(&fake, false);
    assert!(report.sites[0].day.is_none());

    // Written again, it logs.
    super::super::change::https(
        &fake.ctx(),
        SHOP,
        &super::super::change::Https::With(SHOP.to_string()),
    )
    .unwrap();
    let text = fake.text(&file).unwrap();
    assert_eq!(text.matches("access_log stderr;").count(), 2, "{text}");
    assert_eq!(text.matches(&format!("access_log {LOGS}/{SHOP}.log site;")).count(), 2, "{text}");
}

#[test]
fn a_count_runs_on_a_machine_with_no_sites_and_with_no_folder_of_logs() {
    let fake = Fake::new();
    let report = counted(&fake, false);
    assert!(report.sites.is_empty() && report.problems.is_empty());
    assert_eq!(report.plain(), "");
    assert_eq!(fake.calls(), Vec::<String>::new());
    assert!(kept(fake.paths()).unwrap().sites.is_empty());
}

#[test]
fn the_days_are_numbered_and_named_the_same_way_both_ways() {
    for day in [0, 1, 59, 365, 19_000, 20_000, 40_000] {
        assert_eq!(day_number(&day_text(day)), Some(day), "{day}");
    }
    assert_eq!(day_text(0), "1970-01-01");
    assert_eq!(day_text(20_000), "2024-10-04");
    for bad in ["", "2026", "2026-11", "2026-1-15", "26-11-15", "2026-13-01", "2026-00-01", "2026-11-32", "abcd-ef-gh", "2026-11-15-01"] {
        assert_eq!(day_number(bad), None, "{bad}");
    }
}

#[test]
fn a_site_with_a_day_the_text_of_which_is_not_a_date_is_summed_without_it() {
    let today = 20_000;
    let site = SiteCounts {
        days: vec![
            Day { day: day_text(today - 1), visits: 5, errors: 0, bytes: 0, complete: true },
            Day { day: "not a day".into(), visits: 100, errors: 0, bytes: 0, complete: true },
        ],
        size: None,
        gone_since_ms: None,
    };
    let visits = site.visits(today).unwrap();
    assert_eq!((visits.yesterday, visits.last_7_days, visits.last_30_days), (5, 5, 5));
    assert_eq!(SiteCounts::default().visits(today), None);
}

#[test]
fn today_is_summed_when_it_was_counted_and_marks_the_days_it_covers_as_incomplete() {
    let today = 20_000;
    let site = SiteCounts {
        days: vec![
            Day { day: day_text(today), visits: 3, errors: 0, bytes: 0, complete: false },
            Day { day: day_text(today - 1), visits: 5, errors: 0, bytes: 0, complete: true },
            Day { day: day_text(today - 7), visits: 2, errors: 0, bytes: 0, complete: true },
            Day { day: day_text(today - 8), visits: 50, errors: 0, bytes: 0, complete: true },
        ],
        size: None,
        gone_since_ms: None,
    };
    let visits = site.visits(today).unwrap();
    assert_eq!((visits.yesterday, visits.last_7_days, visits.last_30_days), (5, 10, 60));
    assert!(!visits.complete);
}
