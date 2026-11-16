// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `cert get --check` tells (ADR 0513): the findings of
//! [`preflight`](super::preflight) as they are, those that passed with
//! those that did not, in words for a person and in JSON for a page.
//!
//! The JSON is `{"format":1, "checks":[{"id", "result", "name", "words"}],
//! "firewall_rule", "port80_holder", "terms_url", "dns_records":[{"name",
//! "type"}]}`: `id` is the error ID of a warning or a failure that has one,
//! `firewall_rule` the number `firewall list` gives the rule that decides
//! on port 80, `port80_holder` who holds it, and `dns_records` the names a
//! TXT record is made at when the names are proved by DNS.

use serde::Serialize;

use super::dns::Record;
use super::preflight::{self, Facts, Finding, Holder, Outcome};
use crate::firewall;

/// One check, as the JSON gives it.
#[derive(Debug, Serialize)]
struct Row {
    id: Option<&'static str>,
    result: &'static str,
    name: Option<String>,
    words: String,
}

/// A TXT record to add, before its value is known.
#[derive(Debug, Serialize)]
struct Wanted {
    name: String,
    #[serde(rename = "type")]
    kind: &'static str,
}

/// What `cert get --check` tells.
#[derive(Debug, Serialize)]
pub struct Report {
    format: u32,
    checks: Vec<Row>,
    firewall_rule: Option<usize>,
    port80_holder: Option<String>,
    terms_url: Option<String>,
    dns_records: Vec<Wanted>,
}

impl Report {
    /// The report of `facts`, with the address of the authority's terms of
    /// service when it gave one, and the `records` that proving the names by
    /// DNS would ask for (none for HTTP).
    pub fn of(facts: &Facts<'_>, terms_url: Option<String>, records: &[Record]) -> Report {
        let checks = preflight::findings(facts).iter().map(row).collect();
        let firewall_rule = facts
            .http
            .then(|| firewall::lets_tcp_in(facts.firewall, 80, facts.with_inbound).rule)
            .flatten();
        let port80_holder = match &facts.holder {
            Some(Holder::Nginx) => Some(String::from("nginx")),
            Some(Holder::Certd) => Some(String::from("certd")),
            Some(Holder::Other(program)) => Some(program.clone()),
            Some(Holder::Free | Holder::Unknown) | None => None,
        };
        let dns_records =
            records.iter().map(|record| Wanted { name: record.label(), kind: "TXT" }).collect();
        Report { format: 1, checks, firewall_rule, port80_holder, terms_url, dns_records }
    }

    /// Whether any check failed.
    pub fn failed(&self) -> bool {
        self.checks.iter().any(|row| row.result == Outcome::Failed.word())
    }

    /// The report as JSON, on one line.
    ///
    /// # Errors
    ///
    /// If it cannot be written, which it can not be.
    pub fn json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|why| why.to_string())
    }

    /// The report for a person: a line a check, with how it came out and
    /// the ID of its page when it has one.
    pub fn text(&self) -> String {
        let mut text = String::new();
        for row in &self.checks {
            let id = row.id.map_or_else(String::new, |id| format!(" ({id})"));
            text.push_str(&format!("{:<8}  {}{id}\n", row.result, row.words));
        }
        if let Some(url) = &self.terms_url {
            text.push_str(&format!("The authority's terms of service: {url}\n"));
        }
        for record in &self.dns_records {
            text.push_str(&format!("Add a TXT record at {}\n", record.name));
        }
        text
    }
}

fn row(finding: &Finding) -> Row {
    Row {
        id: finding.id,
        result: finding.outcome.word(),
        name: finding.name.clone(),
        words: finding.words.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn facts<'a>(names: &'a [(String, Vec<IpAddr>)], firewall: &'a str) -> Facts<'a> {
        Facts {
            now: NOW,
            names,
            http: true,
            public_authority: true,
            own_ipv6: Vec::new(),
            firewall,
            with_inbound: false,
            holder: Some(Holder::Free),
        }
    }

    fn names(name: &str, addresses: &[&str]) -> Vec<(String, Vec<IpAddr>)> {
        vec![(name.into(), addresses.iter().map(|text| text.parse().unwrap()).collect())]
    }

    #[test]
    fn a_name_that_passes_gives_the_documented_json_and_exit_0() {
        let names = names("shop.example.org", &["203.0.113.10"]);
        let report = Report::of(
            &facts(&names, "allow tcp 22\nallow tcp 80\n"),
            Some(String::from("https://letsencrypt.org/documents/LE-SA.pdf")),
            &[],
        );
        assert!(!report.failed());
        assert_eq!(
            report.json().unwrap(),
            "{\"format\":1,\"checks\":[\
             {\"id\":null,\"result\":\"passed\",\"name\":\"shop.example.org\",\
             \"words\":\"shop.example.org leads to 203.0.113.10\"},\
             {\"id\":null,\"result\":\"passed\",\"name\":null,\
             \"words\":\"port 80 is open (firewall rule 2)\"},\
             {\"id\":null,\"result\":\"passed\",\"name\":null,\
             \"words\":\"port 80 is free: the order answers the authority itself\"}],\
             \"firewall_rule\":2,\"port80_holder\":null,\
             \"terms_url\":\"https://letsencrypt.org/documents/LE-SA.pdf\",\"dns_records\":[]}"
        );
    }

    #[test]
    fn a_failure_has_its_id_and_the_exit_is_1() {
        let names = names("a.example", &[]);
        let mut closed = facts(&names, "deny tcp 80\n");
        closed.holder = Some(Holder::Other(String::from("lighttpd")));
        let report = Report::of(&closed, None, &[]);
        assert!(report.failed());
        let value: serde_json::Value = serde_json::from_str(&report.json().unwrap()).unwrap();
        let ids: Vec<&str> = value["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["CRT-101", "CRT-103", "CRT-106"]);
        assert_eq!(value["firewall_rule"], 1);
        assert_eq!(value["port80_holder"], "lighttpd");
        assert_eq!(value["terms_url"], serde_json::Value::Null);
        let text = report.text();
        assert!(text.starts_with("failed    a.example does not resolve to an address"), "{text}");
        assert!(text.lines().all(|line| line.ends_with(')')), "{text}");
    }

    #[test]
    fn a_warning_does_not_fail_the_check() {
        let names = names("a.example", &["203.0.113.7", "2001:db8::7"]);
        let mut with_ipv6 = facts(&names, "allow tcp 80\n");
        with_ipv6.holder = Some(Holder::Nginx);
        let report = Report::of(&with_ipv6, None, &[]);
        assert!(!report.failed());
        assert!(report.text().contains("warning   a.example has an IPv6 address"));
        let value: serde_json::Value = serde_json::from_str(&report.json().unwrap()).unwrap();
        assert_eq!(value["port80_holder"], "nginx");
        assert_eq!(value["checks"][1]["id"], "CRT-104");
        assert_eq!(value["checks"][1]["result"], "warning");
    }

    #[test]
    fn by_dns_the_checks_are_skipped_and_the_records_are_named() {
        let names = names("shop.example.org", &[]);
        let mut by_hand = facts(&names, "");
        by_hand.http = false;
        by_hand.holder = None;
        let records = [
            Record { name: String::from("shop.example.org"), value: String::new() },
            Record { name: String::from("*.shop.example.org"), value: String::new() },
        ];
        let report = Report::of(&by_hand, None, &records);
        assert!(!report.failed());
        let value: serde_json::Value = serde_json::from_str(&report.json().unwrap()).unwrap();
        assert_eq!(value["firewall_rule"], serde_json::Value::Null);
        assert!(value["checks"].as_array().unwrap().iter().all(|row| row["result"] == "skipped"));
        assert_eq!(
            value["dns_records"],
            serde_json::json!([
                {"name": "_acme-challenge.shop.example.org", "type": "TXT"},
                {"name": "_acme-challenge.shop.example.org", "type": "TXT"}
            ])
        );
        assert!(report.text().contains("Add a TXT record at _acme-challenge.shop.example.org\n"));
    }

    #[test]
    fn a_clock_that_is_wrong_fails_first_with_its_id() {
        let names = names("a.example", &["203.0.113.7"]);
        let mut wrong = facts(&names, "allow tcp 80\n");
        wrong.now = 0;
        let report = Report::of(&wrong, None, &[]);
        let value: serde_json::Value = serde_json::from_str(&report.json().unwrap()).unwrap();
        assert_eq!(value["checks"][0]["id"], "CRT-105");
        assert!(report.failed());
    }
}
