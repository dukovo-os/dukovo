// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Statements that an advisory which matches a part of Dukovo does not
//! affect it (ADR 0537), kept in [`FILE`] as one OpenVEX 0.2.0 document.
//! `ob security check` checks it, `ob security scan` leaves out each
//! finding a statement clears, and `ob docs site` publishes it, so that a
//! scanner reading Dukovo's parts reads the same statements.
//!
//! A statement names the advisory, by its ID and any aliases, and each
//! product it is about, by the package URL the scan gives the part:
//! `pkg:generic/<port>@<version>` for a port and its extra sources, and
//! `pkg:cargo/<crate>@<version>` for a crate. The version is part of the
//! name, so a statement lapses when the part moves to another version,
//! and the advisory is weighed again then. A statement whose status is
//! `not_affected` or `fixed` clears the advisory in its products; one that
//! is `affected` or `under_investigation` clears nothing. Each says why,
//! in `status_notes` or `impact_statement`, which the scan's issue quotes.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value};

use crate::{Context, Result, bail};

/// Where the statements are in the source tree.
pub const FILE: &str = "security/vex.json";

/// The `@context` of an OpenVEX 0.2.0 document.
const CONTEXT: &str = "https://openvex.dev/ns/v0.2.0";

/// What a statement says of the advisory in its products.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    NotAffected,
    Fixed,
    Affected,
    UnderInvestigation,
}

impl Status {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "not_affected" => Some(Self::NotAffected),
            "fixed" => Some(Self::Fixed),
            "affected" => Some(Self::Affected),
            "under_investigation" => Some(Self::UnderInvestigation),
            _ => None,
        }
    }

    /// The status as OpenVEX names it.
    pub fn name(self) -> &'static str {
        match self {
            Self::NotAffected => "not_affected",
            Self::Fixed => "fixed",
            Self::Affected => "affected",
            Self::UnderInvestigation => "under_investigation",
        }
    }

    /// Whether a statement of this status clears the advisory.
    pub fn clears(self) -> bool {
        matches!(self, Self::NotAffected | Self::Fixed)
    }
}

/// The justifications OpenVEX 0.2.0 defines for `not_affected`.
const JUSTIFICATIONS: [&str; 5] = [
    "component_not_present",
    "vulnerable_code_not_present",
    "vulnerable_code_not_in_execute_path",
    "vulnerable_code_cannot_be_controlled_by_adversary",
    "inline_mitigations_already_exist",
];

/// The fields of a statement that OpenVEX 0.2.0 defines and the scan
/// accepts. A statement with another field, often a misspelt one, is
/// refused rather than read without it. `subcomponents` is not among
/// them: a statement about a part of a product would be read as one
/// about all of it.
const STATEMENT_FIELDS: [&str; 12] = [
    "@id",
    "vulnerability",
    "timestamp",
    "last_updated",
    "products",
    "status",
    "status_notes",
    "supplier",
    "justification",
    "impact_statement",
    "action_statement",
    "action_statement_timestamp",
];

/// One statement of the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// The advisory's ID, then its aliases.
    pub ids: Vec<String>,
    /// The package URLs of the products it is about.
    pub products: Vec<String>,
    pub status: Status,
    /// One of [`JUSTIFICATIONS`], for some statements of `not_affected`.
    pub justification: Option<String>,
    /// Why, as the statement says: its impact statement, its notes, or
    /// both.
    pub reason: String,
}

/// The statements of [`FILE`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Document {
    pub statements: Vec<Statement>,
}

impl Document {
    /// The statement that clears an advisory known by `ids` in `product`,
    /// if there is one.
    pub fn clearing(&self, ids: &[&str], product: &str) -> Option<&Statement> {
        self.statements.iter().find(|statement| {
            statement.status.clears()
                && statement.products.iter().any(|p| p == product)
                && statement.ids.iter().any(|id| ids.contains(&id.as_str()))
        })
    }
}

/// Reads and checks [`FILE`] beneath `root`. A tree without it has no
/// statements.
pub fn read(root: &Path) -> Result<Document> {
    let path = root.join(FILE);
    if !path.exists() {
        return Ok(Document::default());
    }
    let text = std::fs::read_to_string(&path).context(|| format!("reading {FILE}"))?;
    match parse(&text) {
        Ok(document) => Ok(document),
        Err(problem) => bail!("{FILE}: {problem}"),
    }
}

/// The package URL the scan gives a port, and a statement names it by.
pub fn port_product(name: &str, version: &str) -> String {
    format!("pkg:generic/{}@{}", encode(name), encode(version))
}

/// The package URL the scan gives a crate of crates.io.
pub fn crate_product(name: &str, version: &str) -> String {
    format!("pkg:cargo/{}@{}", encode(name), encode(version))
}

/// Text as one part of a package URL: what is not a letter, a digit or
/// one of `-._~` is percent-encoded, as `+` in `1.0.0+build`.
fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Reads a document and checks it as OpenVEX 0.2.0 and the rules above
/// require, naming the first problem it finds.
pub fn parse(text: &str) -> std::result::Result<Document, String> {
    let document: Value = serde_json::from_str(text).map_err(|err| format!("not JSON: {err}"))?;
    let Some(top) = document.as_object() else {
        return Err("not a JSON object".to_string());
    };
    if top.get("@context").and_then(Value::as_str) != Some(CONTEXT) {
        return Err(format!("@context is not {CONTEXT}"));
    }
    for field in ["@id", "author", "timestamp"] {
        if text_of(top, field).is_none() {
            return Err(format!("the document has no {field}"));
        }
    }
    if top.get("version").and_then(Value::as_u64).is_none_or(|version| version < 1) {
        return Err("the document's version is not a whole number of 1 or more".to_string());
    }
    let Some(list) = top.get("statements").and_then(Value::as_array) else {
        return Err("the document has no list of statements".to_string());
    };
    let mut statements = Vec::new();
    let mut seen = BTreeSet::new();
    for (n, entry) in list.iter().enumerate() {
        let statement =
            statement(entry).map_err(|problem| format!("statement {}: {problem}", n + 1))?;
        for id in &statement.ids {
            for product in &statement.products {
                if !seen.insert((id.clone(), product.clone())) {
                    return Err(format!(
                        "statement {}: {id} in {product} is the subject of an earlier statement \
                         too",
                        n + 1
                    ));
                }
            }
        }
        statements.push(statement);
    }
    Ok(Document { statements })
}

/// The non-empty text of a field, if it has one.
fn text_of<'a>(object: &'a Map<String, Value>, field: &str) -> Option<&'a str> {
    object.get(field).and_then(Value::as_str).filter(|text| !text.trim().is_empty())
}

/// One statement, checked.
fn statement(entry: &Value) -> std::result::Result<Statement, String> {
    let Some(object) = entry.as_object() else {
        return Err("not a JSON object".to_string());
    };
    if let Some(field) = object.keys().find(|key| !STATEMENT_FIELDS.contains(&key.as_str())) {
        return Err(format!("`{field}` is not a field of a statement the scan reads"));
    }
    let ids = advisory_ids(object.get("vulnerability"))?;
    let products = products(object.get("products"))?;
    let Some(status) = object.get("status").and_then(Value::as_str).and_then(Status::from_name)
    else {
        return Err(
            "status is not not_affected, fixed, affected or under_investigation".to_string()
        );
    };
    let justification = match object.get("justification") {
        None => None,
        Some(value) => match value.as_str() {
            Some(name) if JUSTIFICATIONS.contains(&name) => Some(name.to_string()),
            _ => return Err(format!("justification is not one of {}", JUSTIFICATIONS.join(", "))),
        },
    };
    if justification.is_some() && status != Status::NotAffected {
        return Err("only a statement of not_affected has a justification".to_string());
    }
    let impact = text_of(object, "impact_statement");
    let notes = text_of(object, "status_notes");
    match status {
        Status::NotAffected if justification.is_none() && impact.is_none() => {
            return Err("not_affected needs a justification or an impact_statement".to_string());
        }
        Status::Affected if text_of(object, "action_statement").is_none() => {
            return Err("affected needs an action_statement".to_string());
        }
        _ => {}
    }
    let reason = match (impact, notes) {
        (Some(impact), Some(notes)) => format!("{} {}", impact.trim(), notes.trim()),
        (Some(text), None) | (None, Some(text)) => text.trim().to_string(),
        (None, None) if status.clears() => {
            return Err("a statement that clears an advisory says why, in status_notes or \
                        impact_statement"
                .to_string());
        }
        (None, None) => text_of(object, "action_statement").unwrap_or_default().trim().to_string(),
    };
    Ok(Statement { ids, products, status, justification, reason })
}

/// The ID and aliases of a statement's `vulnerability`, each a name such
/// as CVE-2026-1234, GHSA-xxxx-xxxx-xxxx or RUSTSEC-2026-0001.
fn advisory_ids(vulnerability: Option<&Value>) -> std::result::Result<Vec<String>, String> {
    let Some(object) = vulnerability.and_then(Value::as_object) else {
        return Err("vulnerability is not an object with a name".to_string());
    };
    let plain = |id: &str| {
        !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
    };
    let Some(name) = object.get("name").and_then(Value::as_str).filter(|name| plain(name)) else {
        return Err("vulnerability has no name, such as CVE-2026-1234".to_string());
    };
    let mut ids = vec![name.to_string()];
    if let Some(aliases) = object.get("aliases") {
        let Some(aliases) = aliases.as_array() else {
            return Err("vulnerability's aliases are not a list".to_string());
        };
        for alias in aliases {
            match alias.as_str() {
                Some(alias) if plain(alias) => ids.push(alias.to_string()),
                _ => return Err(format!("vulnerability's alias {alias} is not an ID")),
            }
        }
    }
    Ok(ids)
}

/// The package URLs of a statement's `products`. Each names a version, so
/// that a statement does not outlive the version it was made for.
fn products(products: Option<&Value>) -> std::result::Result<Vec<String>, String> {
    let Some(list) = products.and_then(Value::as_array).filter(|list| !list.is_empty()) else {
        return Err("products is not a list of one or more products".to_string());
    };
    let mut out = Vec::new();
    for product in list {
        let Some(object) = product.as_object() else {
            return Err("a product is not an object with an @id".to_string());
        };
        if object.contains_key("subcomponents") {
            return Err("subcomponents are not read: name the product as the scan names it".into());
        }
        let Some(id) = object.get("@id").and_then(Value::as_str) else {
            return Err("a product has no @id".to_string());
        };
        if !product_named(id) {
            return Err(format!(
                "product {id} is not a package URL with a version, such as \
                 pkg:generic/openssl@3.5.9 or pkg:cargo/rsa@0.9.8"
            ));
        }
        out.push(id.to_string());
    }
    Ok(out)
}

/// Whether `id` is a package URL of a type, a name and a version, written
/// as the scan writes them: `pkg:<type>/<name>@<version>`, of letters,
/// digits, `-._~` and percent-encoded bytes.
fn product_named(id: &str) -> bool {
    let Some((kind, rest)) = id.strip_prefix("pkg:").and_then(|rest| rest.split_once('/')) else {
        return false;
    };
    let Some((name, version)) = rest.split_once('@') else {
        return false;
    };
    let part = |text: &str| {
        !text.is_empty() && text.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~%".contains(&b))
    };
    !kind.is_empty() && kind.bytes().all(|b| b.is_ascii_lowercase()) && part(name) && part(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document of `statements`, each the JSON of one.
    fn document(statements: &[&str]) -> String {
        format!(
            r#"{{
                "@context": "https://openvex.dev/ns/v0.2.0",
                "@id": "https://dukovo.org/support/security/vex.json",
                "author": "Dukovo security team",
                "timestamp": "2026-11-15T14:00:00Z",
                "version": 1,
                "statements": [{}]
            }}"#,
            statements.join(",")
        )
    }

    const FIXED: &str = r#"{
        "vulnerability": { "name": "CVE-2023-49990", "aliases": ["GHSA-aaaa-bbbb-cccc"] },
        "products": [{ "@id": "pkg:generic/espeak-ng@1.52.0" }],
        "status": "fixed",
        "status_notes": "Fixed by 58f1e0b6, which 1.52.0 holds."
    }"#;

    #[test]
    fn the_tree_s_statements_are_read_and_checked() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let document = read(&root).unwrap();
        assert!(!document.statements.is_empty());
    }

    #[test]
    fn a_statement_of_fixed_or_not_affected_clears_its_ids_in_its_products() {
        let not_affected = r#"{
            "vulnerability": { "name": "CVE-2026-62960" },
            "products": [{ "@id": "pkg:generic/git@2.55.0" }, { "@id": "pkg:generic/git@2.56.1" }],
            "status": "not_affected",
            "justification": "vulnerable_code_not_present",
            "impact_statement": "The code is Git for Windows' alone."
        }"#;
        let document = parse(&document(&[FIXED, not_affected])).unwrap();
        let espeak = "pkg:generic/espeak-ng@1.52.0";
        assert_eq!(
            document.clearing(&["CVE-2023-49990"], espeak).map(|s| s.reason.as_str()),
            Some("Fixed by 58f1e0b6, which 1.52.0 holds.")
        );
        // By an alias, on either side.
        assert!(document.clearing(&["GHSA-aaaa-bbbb-cccc"], espeak).is_some());
        assert!(document.clearing(&["OSV-1", "CVE-2023-49990"], espeak).is_some());
        // Not another ID, nor another version or product.
        assert!(document.clearing(&["CVE-2023-49991"], espeak).is_none());
        assert!(document.clearing(&["CVE-2023-49990"], "pkg:generic/espeak-ng@1.53.0").is_none());
        assert!(document.clearing(&["CVE-2023-49990"], "pkg:cargo/espeak-ng@1.52.0").is_none());
        assert!(document.clearing(&["CVE-2026-62960"], "pkg:generic/git@2.56.1").is_some());
        let statement = document.clearing(&["CVE-2026-62960"], "pkg:generic/git@2.55.0").unwrap();
        assert_eq!(statement.justification.as_deref(), Some("vulnerable_code_not_present"));
    }

    #[test]
    fn affected_and_under_investigation_clear_nothing() {
        let affected = r#"{
            "vulnerability": { "name": "CVE-2026-1" },
            "products": [{ "@id": "pkg:generic/openssl@3.5.4" }],
            "status": "affected",
            "action_statement": "Move to 3.5.9."
        }"#;
        let investigating = r#"{
            "vulnerability": { "name": "CVE-2026-2" },
            "products": [{ "@id": "pkg:generic/openssl@3.5.4" }],
            "status": "under_investigation"
        }"#;
        let document = parse(&document(&[affected, investigating])).unwrap();
        for id in ["CVE-2026-1", "CVE-2026-2"] {
            assert!(document.clearing(&[id], "pkg:generic/openssl@3.5.4").is_none());
        }
    }

    #[test]
    fn products_are_named_as_the_scan_names_them() {
        assert_eq!(port_product("espeak-ng", "1.52.0"), "pkg:generic/espeak-ng@1.52.0");
        assert_eq!(crate_product("rsa", "0.9.8+wasi"), "pkg:cargo/rsa@0.9.8%2Bwasi");
        for id in ["pkg:generic/espeak-ng@1.52.0", "pkg:cargo/rsa@0.9.8%2Bwasi"] {
            assert!(product_named(id), "{id}");
        }
        for id in [
            "pkg:generic/espeak-ng",
            "generic/x@1",
            "pkg:/x@1",
            "pkg:Generic/x@1",
            "pkg:generic/x@",
            "pkg:generic/x y@1",
        ] {
            assert!(!product_named(id), "{id}");
        }
    }

    #[test]
    fn a_document_that_is_not_openvex_or_misses_a_rule_is_refused() {
        let bad = |field: &str, value: &str| {
            let mut statement: Value = serde_json::from_str(FIXED).unwrap();
            if value.is_empty() {
                statement.as_object_mut().unwrap().remove(field);
            } else {
                statement[field] = serde_json::from_str(value).unwrap();
            }
            parse(&document(&[&statement.to_string()])).unwrap_err()
        };
        for (field, value, expected) in [
            ("status", "\"fine\"", "status is not"),
            ("status", "", "status is not"),
            ("status_notes", "", "says why"),
            ("status_notes", "\" \"", "says why"),
            ("products", "[]", "one or more products"),
            ("products", "[{\"@id\": \"pkg:generic/espeak-ng\"}]", "with a version"),
            (
                "products",
                "[{\"@id\": \"pkg:generic/espeak-ng@1.52.0\", \"subcomponents\": []}]",
                "subcomponents",
            ),
            ("vulnerability", "\"CVE-2023-49990\"", "an object with a name"),
            ("vulnerability", "{\"name\": \"CVE 1\"}", "has no name"),
            ("vulnerability", "{\"name\": \"CVE-1\", \"aliases\": [\"a b\"]}", "is not an ID"),
            (
                "justification",
                "\"vulnerable_code_not_present\"",
                "only a statement of not_affected",
            ),
            ("justfication", "\"vulnerable_code_not_present\"", "`justfication` is not a field"),
        ] {
            let problem = bad(field, value);
            assert!(problem.contains(expected), "{field} = {value}: `{problem}`, not `{expected}`");
        }

        let not_affected = r#"{
            "vulnerability": { "name": "CVE-1" },
            "products": [{ "@id": "pkg:generic/x@1" }],
            "status": "not_affected",
            "status_notes": "Not used."
        }"#;
        let problem = parse(&document(&[not_affected])).unwrap_err();
        assert!(problem.contains("justification or an impact_statement"), "{problem}");
        let with_reason = not_affected.replace(
            "\"status_notes\"",
            "\"justification\": \"component_not_present\", \"status_notes\"",
        );
        assert!(parse(&document(&[&with_reason])).is_ok());
        let unknown = not_affected
            .replace("\"status_notes\"", "\"justification\": \"not_used\", \"status_notes\"");
        assert!(parse(&document(&[&unknown])).unwrap_err().contains("justification is not one of"));

        let affected = r#"{
            "vulnerability": { "name": "CVE-1" },
            "products": [{ "@id": "pkg:generic/x@1" }],
            "status": "affected"
        }"#;
        assert!(parse(&document(&[affected])).unwrap_err().contains("action_statement"));

        let twice = parse(&document(&[FIXED, FIXED])).unwrap_err();
        assert!(
            twice.contains("statement 2: CVE-2023-49990 in pkg:generic/espeak-ng@1.52.0"),
            "{twice}"
        );

        let old = document(&[FIXED]).replace("v0.2.0", "v0.0.1");
        assert!(parse(&old).unwrap_err().contains("@context"));
        let unversioned = document(&[FIXED]).replace("\"version\": 1", "\"version\": 0");
        assert!(parse(&unversioned).unwrap_err().contains("version"));
        let authorless = document(&[FIXED]).replace("\"author\": \"Dukovo security team\",", "");
        assert!(parse(&authorless).unwrap_err().contains("author"));
    }
}
