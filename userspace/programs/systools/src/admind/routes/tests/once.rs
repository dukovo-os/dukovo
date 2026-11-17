// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The one-time link to a report's text (ADR 0568): made for a signed-in
//! person from a draft reportd keeps for them, and fetched once with no
//! session, by a phone.

use super::*;

/// The draft the test worker's `report text` knows, its hash and its text.
pub(super) const DRAFT: &str = "0123456789abcdef0123456789abcdef";
pub(super) const SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub(super) const TEXT: &str = "Dukovo problem report\nAbout: the hardware\n";

fn phone(draft: &str, sha256: &str) -> Req {
    post(&format!("/api/1/reports/drafts/{draft}/phone"), &json!({ "sha256": sha256 }))
}

/// The address part of `url` after the host: `/once/TOKEN/report.txt`.
fn path_of(url: &str) -> String {
    let rest = url.strip_prefix("https://").unwrap();
    rest[rest.find('/').unwrap()..].to_string()
}

fn head(path: &str) -> Req {
    Req { method: "HEAD", ..get(path) }
}

#[test]
fn a_link_gives_the_draft_s_text_once_to_a_request_with_no_session() {
    run(async {
        let world = world("once");
        let chase = sign_in(&world, "chase").await;
        let made = send(&world, phone(DRAFT, SHA256).by(&chase)).await;
        assert_eq!(made.status, 200, "{}", made.body);
        let made = made.json();
        let url = made["url"].as_str().unwrap();
        let path = path_of(url);
        let token = path.strip_prefix("/once/").unwrap().strip_suffix("/report.txt").unwrap();
        assert!(crate::admind::once::is_token(token), "{url}");
        assert!(url.contains(&format!(":{}/once/", world.console.shared.port)), "{url}");
        assert!(made["expires_ms"].as_i64().unwrap() > unix_now() * 1000);
        assert_eq!(
            made["fingerprint_sha256"],
            world.console.shared.certificate.borrow().fingerprint
        );
        // The worker ran `report text` as the person.
        let asked = json!({ "argv": ["report", "text", DRAFT, "--sha256", SHA256] });
        assert!(world.calls.lock().unwrap().iter().any(|(m, p)| m == "run" && *p == asked));

        // The phone has no session, and gets the text as a file to keep.
        let first = send_from(&world, "192.168.1.30", get(&path)).await;
        assert_eq!((first.status, first.body.as_str()), (200, TEXT));
        assert_eq!(first.header("content-type"), Some("text/plain; charset=utf-8"));
        assert_eq!(
            first.header("content-disposition"),
            Some("attachment; filename=\"report.txt\"")
        );
        assert_eq!(first.header("cache-control"), Some("no-store"));
        assert_eq!(first.header("content-security-policy"), Some("default-src 'none'"));
        assert_eq!(first.header("referrer-policy"), Some("no-referrer"));
        assert_eq!(first.header("x-content-type-options"), Some("nosniff"));
        assert!(first.cookies().is_empty());

        // Then it is used: in plain words, with ADM-140.
        let again = send_from(&world, "192.168.1.30", get(&path)).await;
        assert_eq!(again.status, 410);
        assert!(again.body.starts_with("This one-time link was used"), "{}", again.body);
        assert!(again.body.contains("ADM-140"));
        assert_eq!(again.header("content-type"), Some("text/plain; charset=utf-8"));

        // A token that was never made, or is not one, is nothing.
        let none = send(&world, get(&format!("/once/{}/report.txt", "b".repeat(32)))).await;
        assert_eq!(none.status, 404);
        assert!(none.body.contains("ADM-140"));
        let odd = send(&world, get("/once/NOT-A-TOKEN/report.txt")).await;
        assert_eq!(odd.status, 404);
        // The route is a GET or a HEAD.
        let posted = send(&world, post(&path, &json!({}))).await;
        assert_eq!(posted.status, 405);
    });
}

#[test]
fn a_head_counts_as_the_download() {
    run(async {
        let world = world("once-head");
        let chase = sign_in(&world, "chase").await;
        let made = send(&world, phone(DRAFT, SHA256).by(&chase)).await.json();
        let path = path_of(made["url"].as_str().unwrap());
        let looked = send(&world, head(&path)).await;
        assert_eq!((looked.status, looked.body.as_str()), (200, ""));
        assert_eq!(looked.header("content-length"), Some(TEXT.len().to_string().as_str()));
        assert_eq!(send(&world, get(&path)).await.status, 410);
    });
}

#[test]
fn a_draft_reportd_does_not_keep_is_its_own_refusal_and_a_session_has_four_links() {
    run(async {
        let world = world("once-refused");
        let chase = sign_in(&world, "chase").await;
        let ended = send(&world, phone(&"c".repeat(32), SHA256).by(&chase)).await;
        assert_eq!((ended.status, ended.error().as_str()), (422, "RPT-101"), "{}", ended.body);
        let bad = send(&world, phone("x", SHA256).by(&chase)).await;
        assert_eq!((bad.status, bad.error().as_str()), (400, "ADM-125"));
        let bad = send(&world, phone(DRAFT, "short").by(&chase)).await;
        assert_eq!(bad.json()["error"]["field"], "sha256");
        // No session, no link.
        assert_eq!(send(&world, phone(DRAFT, SHA256)).await.status, 401);
        for _ in 0..crate::admind::once::EACH {
            assert_eq!(send(&world, phone(DRAFT, SHA256).by(&chase)).await.status, 200);
        }
        let more = send(&world, phone(DRAFT, SHA256).by(&chase)).await;
        assert_eq!((more.status, more.error().as_str()), (429, "ADM-105"));
        assert!(more.header("retry-after").is_some());
    });
}

#[test]
fn the_tries_for_links_count_against_the_address_as_a_sign_in_does() {
    run(async {
        let world = world("once-tries");
        let path = format!("/once/{}/report.txt", "d".repeat(32));
        let mut statuses = Vec::new();
        for _ in 0..12 {
            statuses.push(send_from(&world, "198.51.100.4", get(&path)).await.status);
        }
        assert_eq!(statuses.iter().filter(|status| **status == 404).count(), 10, "{statuses:?}");
        assert_eq!(statuses.last(), Some(&429));
    });
}
