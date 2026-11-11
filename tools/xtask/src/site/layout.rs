// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The support center's page layout: one header bar with search, the
//! contents on the left, the page, and "On this page" on the right.

const REPO: &str = "https://github.com/dukovo-os/dukovo";

/// A group of links in the contents: a title and (text, site path) pairs.
pub struct Group {
    pub title: String,
    pub links: Vec<(String, String)>,
}

/// What every page shares.
pub struct Shell<'a> {
    /// The release these docs describe, such as `0.14.1` or `Development`.
    pub version: &'a str,
    pub groups: &'a [Group],
}

/// One page to write.
pub struct Page<'a> {
    /// Where the page goes, from the site's root, such as `man/ls.1.html`.
    pub path: &'a str,
    pub title: &'a str,
    /// Shown under the title, when the page has one.
    pub lead: &'a str,
    /// The page's description for search engines and link previews.
    pub description: &'a str,
    /// Labels under the lead, such as "Section 8 · Administration".
    pub badges: &'a [String],
    pub body: &'a str,
    /// The page's sections, as (id, text).
    pub headings: &'a [(String, String)],
    /// The file the page is made from, from the repository's root, when
    /// one can be edited there.
    pub source: Option<&'a str>,
    /// Extra lines for the foot of "On this page", such as how the page is
    /// made.
    pub note: &'a str,
    /// Wraps the body: `man` for man pages.
    pub class: &'a str,
}

pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Escapes text for a query string.
fn query(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// How to reach the site's root from `path`: `../` per directory.
pub fn root_of(path: &str) -> String {
    "../".repeat(path.matches('/').count())
}

const FAVICON: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 64 64'%3E%3Crect width='64' height='64' rx='14' fill='%230D1A33'/%3E%3Ctext x='32' y='46' font-family='Arial,sans-serif' font-size='40' font-weight='800' text-anchor='middle' fill='%23F0A04B'%3ED%3C/text%3E%3C/svg%3E";

const SEARCH_ICON: &str = "<svg width=\"18\" height=\"18\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" aria-hidden=\"true\"><circle cx=\"11\" cy=\"11\" r=\"7\"/><path d=\"m20 20-3.5-3.5\"/></svg>";

fn head(root: &str, title: &str, description: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1,viewport-fit=cover\">\n\
         <title>{}</title>\n<meta name=\"description\" content=\"{}\">\n\
         <link rel=\"icon\" href=\"{FAVICON}\">\n<meta name=\"theme-color\" content=\"#070C18\">\n\
         <link rel=\"stylesheet\" href=\"{root}site.css\">\n</head>\n",
        escape(title),
        escape(description)
    )
}

/// The single header bar: contents button (small screens), the name,
/// search, the version, and the site's own links.
fn header(root: &str, version: &str, docs: bool) -> String {
    let contents = if docs {
        "<button class=\"menu-btn menu-docs\" type=\"button\" aria-expanded=\"false\">Contents</button>"
    } else {
        ""
    };
    format!(
        "<div class=\"gut dochead-wrap\"><header class=\"wrap docs-top dochead\">{contents}\
         <a class=\"word\" href=\"https://dukovo.org/\">Dukovo<sup>™</sup></a>\
         <a class=\"dlabel\" href=\"{root}index.html\">Support</a>\
         <div class=\"search\" data-search>{SEARCH_ICON}<input type=\"search\" placeholder=\"Search the docs\" \
         aria-label=\"Search the docs\" autocomplete=\"off\"><kbd>/</kbd><div class=\"results\" role=\"listbox\"></div></div>\
         <span class=\"badge\" style=\"margin-left:auto\">{}</span>\
         <button class=\"menu-btn search-btn\" type=\"button\" aria-label=\"Search the docs\" aria-expanded=\"false\">{SEARCH_ICON}</button>\
         <button class=\"menu-btn\" type=\"button\" aria-expanded=\"false\" aria-controls=\"site-nav\"><i aria-hidden=\"true\"></i>Menu</button>\
         <nav id=\"site-nav\" aria-label=\"Site\"><a href=\"https://dukovo.org/\">Server</a>\
         <a href=\"https://dukovo.org/preview/\">2.0 Preview</a><a href=\"{REPO}\">Source</a>\
         <a href=\"https://github.com/sponsors/dukovo-os\">Sponsor</a></nav></header></div>\n",
        escape(version)
    )
}

const FOOTER: &str = "<div class=\"gut\"><div class=\"wrap\"><footer>\
    <span>© 2026 The Dukovo contributors. Dukovo™ is a trademark (U.S. serial no. 50194034). \
    Documentation is open source under Apache-2.0 or MIT.</span>\
    <span>No cookies. No trackers. No analytics.</span></footer></div></div>\n";

fn scripts(root: &str) -> String {
    format!(
        "<script src=\"{root}search-index.js\"></script>\n<script src=\"{root}site.js\"></script>\n"
    )
}

fn sidebar(root: &str, groups: &[Group], current: &str) -> String {
    let mut html = String::from(
        "<aside class=\"side\" aria-label=\"Docs contents\">\
         <input type=\"search\" placeholder=\"Filter contents\" aria-label=\"Filter contents\">",
    );
    for group in groups {
        html.push_str(&format!("<h4>{}</h4>", escape(&group.title)));
        for (text, path) in &group.links {
            let here = if path == current { " aria-current=\"page\"" } else { "" };
            html.push_str(&format!("<a href=\"{root}{path}\"{here}>{}</a>", escape(text)));
        }
    }
    html.push_str("</aside>");
    html
}

/// A docs page: header, contents, the page and "On this page".
pub fn page(shell: &Shell, page: &Page) -> String {
    let root = root_of(page.path);
    let mut html = head(&root, &format!("{} · Dukovo Support", page.title), page.description);
    html.push_str(&format!("<body data-root=\"{root}\">\n"));
    html.push_str(&header(&root, shell.version, true));
    html.push_str("<div class=\"gut\"><div class=\"docs\">");
    html.push_str(&sidebar(&root, shell.groups, page.path));
    html.push_str("<article class=\"doc\">");
    html.push_str(&format!(
        "<div class=\"crumbs\"><a href=\"{root}index.html\">Support</a> › {}</div>",
        escape(page.title)
    ));
    html.push_str(&format!("<h1>{}</h1>", escape(page.title)));
    if !page.lead.is_empty() {
        html.push_str(&format!("<p class=\"lead\">{}</p>", escape(page.lead)));
    }
    if !page.badges.is_empty() {
        html.push_str("<div class=\"docmeta\">");
        for badge in page.badges {
            html.push_str(&format!("<span class=\"badge\">{}</span>", escape(badge)));
        }
        html.push_str("</div>");
    }
    if page.class.is_empty() {
        html.push_str(page.body);
    } else {
        html.push_str(&format!("<div class=\"{}\">{}</div>", page.class, page.body));
    }
    html.push_str(&feedback(page, shell.version));
    html.push_str("</article>");
    html.push_str(&toc(page));
    html.push_str("</div></div>\n");
    html.push_str(FOOTER);
    html.push_str(&scripts(&root));
    html.push_str("</body></html>\n");
    html
}

/// The support center's home: the header, `body` and the footer.
pub fn home(shell: &Shell, title: &str, description: &str, body: &str) -> String {
    let mut html = head("", title, description);
    html.push_str("<body class=\"home\" data-root=\"\">\n");
    html.push_str(&header("", shell.version, false));
    html.push_str(body);
    html.push_str(FOOTER);
    html.push_str(&scripts(""));
    html.push_str("</body></html>\n");
    html
}

/// "Report a problem" opens an issue that names the page and version;
/// "Edit this page" opens its source on GitHub. The site has no analytics,
/// so these are how readers say a page let them down.
fn feedback(page: &Page, version: &str) -> String {
    let title = query(&format!("Docs: {}", page.title));
    let body =
        query(&format!("Page: {}\nVersion: {version}\n\nWhat was wrong or missing:\n", page.path));
    let mut links =
        format!("<a href=\"{REPO}/issues/new?title={title}&amp;body={body}\">Report a problem</a>");
    if let Some(source) = page.source {
        links.push_str(&format!("<a href=\"{REPO}/edit/main/{source}\">Edit this page</a>"));
    }
    links.push_str(&format!("<a href=\"{REPO}/discussions\">Ask a question</a>"));
    format!(
        "<div class=\"feedback\"><span>Something wrong or missing on this page?</span>\
         <span class=\"fb\">{links}</span></div>"
    )
}

fn toc(page: &Page) -> String {
    let mut html = String::from("<nav class=\"toc\" aria-label=\"On this page\">");
    if !page.headings.is_empty() {
        html.push_str("<b>On this page</b>");
        for (id, text) in page.headings {
            html.push_str(&format!("<a href=\"#{}\">{}</a>", escape(id), escape(text)));
        }
    }
    let mut meta = Vec::new();
    if let Some(source) = page.source {
        meta.push(format!("Source: {}", escape(source)));
    }
    if !page.note.is_empty() {
        meta.push(escape(page.note));
    }
    if !meta.is_empty() {
        html.push_str(&format!("<div class=\"meta\">{}</div>", meta.join("<br>")));
    }
    html.push_str("</nav>");
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_root_and_escapes() {
        assert_eq!(root_of("index.html"), "");
        assert_eq!(root_of("man/ls.1.html"), "../");
        assert_eq!(escape("<a & \"b\">"), "&lt;a &amp; &quot;b&quot;&gt;");
        assert_eq!(query("Docs: ls(1)"), "Docs%3A%20ls%281%29");
    }

    #[test]
    fn writes_a_page_with_its_links() {
        let groups = [Group {
            title: "Commands".into(),
            links: vec![("ls".into(), "man/ls.1.html".into())],
        }];
        let shell = Shell { version: "0.14.1", groups: &groups };
        let headings = [("NAME".to_string(), "Name".to_string())];
        let html = page(
            &shell,
            &Page {
                path: "man/ls.1.html",
                title: "ls(1)",
                lead: "list directory contents",
                description: "list directory contents",
                badges: &[],
                body: "<p>body</p>",
                headings: &headings,
                source: Some("docs/man/ls.1.scd"),
                note: "",
                class: "man",
            },
        );
        assert!(html.contains("<link rel=\"stylesheet\" href=\"../site.css\">"));
        assert!(html.contains("<a href=\"../man/ls.1.html\" aria-current=\"page\">ls</a>"));
        assert!(html.contains("<a href=\"#NAME\">Name</a>"));
        assert!(html.contains("edit/main/docs/man/ls.1.scd"));
        assert!(html.contains("<div class=\"man\"><p>body</p></div>"));
        assert!(html.contains("Version%3A%200.14.1"));
    }
}
