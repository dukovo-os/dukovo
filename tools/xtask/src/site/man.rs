// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Man pages for the support center: mandoc's HTML of each page, tidied
//! to sit in the site's layout, with references to other pages as links.

use std::collections::BTreeSet;

/// The parts of a man page the site needs, from mandoc's `-T html -O
/// fragment` output.
#[derive(Debug, PartialEq)]
pub struct ManHtml {
    /// The page's sections, as (id, title), for "On this page".
    pub headings: Vec<(String, String)>,
    /// The body without mandoc's title table.
    pub body: String,
}

/// Tidies mandoc's fragment: drops the title table (the layout shows the
/// title), makes its section headings second-level ones, drops the line
/// breaks mandoc adds inside preformatted blocks, and links `name(n)`
/// wherever `pages` holds `name.n`.
pub fn tidy(fragment: &str, pages: &BTreeSet<String>) -> ManHtml {
    let mut body = match fragment.find("</table>") {
        Some(end) if fragment.trim_start().starts_with("<table class=\"head\">") => {
            fragment[end + "</table>".len()..].to_string()
        }
        _ => fragment.to_string(),
    };
    if let Some(start) = body.rfind("<table class=\"foot\">") {
        body.truncate(start);
    }
    body = body.replace("<h1 class=\"Sh\"", "<h2 class=\"Sh\"").replace("</h1>", "</h2>");
    body = body.replace("\n<br/>\n", "\n").replace("<br/>\n", "\n");

    body = drop_name_section(&body);
    let mut headings = Vec::new();
    for part in body.split("<h2 class=\"Sh\" id=\"").skip(1) {
        let Some((id, rest)) = part.split_once('"') else { continue };
        let title = strip_tags(rest.split("</h2>").next().unwrap_or_default());
        let title = title.trim_start_matches('>').trim().to_string();
        headings.push((id.to_string(), sentence_case(&title)));
    }
    for (id, title) in &headings {
        let upper = format!("href=\"#{id}\">{}</a>", title.to_uppercase());
        body = body.replacen(&upper, &format!("href=\"#{id}\">{title}</a>"), 1);
    }
    ManHtml { headings, body: link_references(&body, pages) }
}

/// The layout shows the NAME line as the page's title and lead.
fn drop_name_section(body: &str) -> String {
    let Some(start) = body.find("<section class=\"Sh\">\n<h2 class=\"Sh\" id=\"NAME\"") else {
        return body.to_string();
    };
    match body[start..].find("</section>") {
        Some(end) => format!("{}{}", &body[..start], &body[start + end + "</section>".len()..]),
        None => body.to_string(),
    }
}

/// `<b>name</b>(8)` becomes a link to `name.8.html` when that page exists.
fn link_references(body: &str, pages: &BTreeSet<String>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("<b>") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        let Some(end) = after.find("</b>") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        let tail = &after[end + 4..];
        let section: String = tail
            .strip_prefix('(')
            .map_or_default(|t| t.chars().take_while(char::is_ascii_alphanumeric).collect());
        let closes = tail.get(section.len() + 1..).is_some_and(|t| t.starts_with(')'));
        let page = format!("{name}.{section}");
        if !section.is_empty() && closes && pages.contains(&page) {
            out.push_str(&format!("<a href=\"{page}.html\"><b>{name}</b>({section})</a>"));
            rest = &tail[section.len() + 2..];
        } else {
            out.push_str("<b>");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

pub fn strip_tags(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    text.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"")
}

/// `SEE ALSO` reads as `See also` in the page's contents.
fn sentence_case(title: &str) -> String {
    let lower = title.to_lowercase();
    let mut chars = lower.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The names and summary in a page's NAME section: `7zz, 7z - the 7-Zip
/// archiver` gives (["7zz", "7z"], "the 7-Zip archiver").
pub fn name_line(scdoc: &str) -> (Vec<String>, String) {
    let Some(start) = scdoc.find("# NAME") else { return (Vec::new(), String::new()) };
    let after = scdoc[start + "# NAME".len()..].trim_start();
    let line = after.split("\n\n").next().unwrap_or_default();
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let (names, summary) = line.split_once(" - ").unwrap_or((line.as_str(), ""));
    let names = names.split(',').map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    (names.collect(), summary.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidies_mandoc_output() {
        let fragment = "<table class=\"head\">\n<tr><td>service(8)</td></tr>\n</table>\n\
            <div class=\"manual-text\">\n<section class=\"Sh\">\n\
            <h1 class=\"Sh\" id=\"NAME\"><a class=\"permalink\" href=\"#NAME\">NAME</a></h1>\n\
            <p class=\"Pp\">service - x</p>\n</section>\n<section class=\"Sh\">\n\
            <h1 class=\"Sh\" id=\"SEE_ALSO\"><a class=\"permalink\" href=\"#SEE_ALSO\">SEE ALSO</a></h1>\n\
            <p class=\"Pp\"><b>spark</b>(8), <b>log</b>(1), <b>nowhere</b>(1)</p>\n\
            <pre>a\n<br/>\nb</pre>\n</section>\n</div>\n\
            <table class=\"foot\"><tr><td>foot</td></tr></table>\n";
        let pages = BTreeSet::from(["spark.8".to_string(), "log.1".to_string()]);
        let man = tidy(fragment, &pages);
        assert_eq!(man.headings, [("SEE_ALSO".to_string(), "See also".to_string())]);
        assert!(!man.body.contains("head"));
        assert!(!man.body.contains("foot"));
        assert!(man.body.contains("<h2 class=\"Sh\" id=\"SEE_ALSO\">"));
        assert!(man.body.contains("href=\"#SEE_ALSO\">See also</a>"));
        assert!(!man.body.contains("NAME"));
        assert!(man.body.contains("<a href=\"spark.8.html\"><b>spark</b>(8)</a>"));
        assert!(man.body.contains("<a href=\"log.1.html\"><b>log</b>(1)</a>"));
        assert!(man.body.contains("<b>nowhere</b>(1)"));
        assert!(man.body.contains("<pre>a\nb</pre>"));
    }

    #[test]
    fn reads_the_name_line() {
        let page = "7zz(1) \"Dukovo\"\n\n# NAME\n\n7zz, 7z - the 7-Zip\narchiver\n\n# SYNOPSIS\n";
        assert_eq!(
            name_line(page),
            (vec!["7zz".to_string(), "7z".to_string()], "the 7-Zip archiver".to_string())
        );
    }
}
