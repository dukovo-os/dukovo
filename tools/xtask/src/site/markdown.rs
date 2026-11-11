// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Markdown pages for the support center: the handbook, the design notes,
//! the decision records and the roadmap.

use pulldown_cmark::{CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd, html};

/// A Markdown file turned into the pieces of a page.
#[derive(Debug, Default, PartialEq)]
pub struct Rendered {
    /// The text of the first top-level heading, which the layout shows.
    pub title: String,
    /// The first paragraph, as plain text, for search results and lists.
    pub summary: String,
    /// The page's HTML without its title.
    pub body: String,
    /// The second-level headings, as (id, text), for "On this page".
    pub headings: Vec<(String, String)>,
}

/// Renders `text`, passing every link and image address through `link`.
pub fn render(text: &str, link: &dyn Fn(&str) -> String) -> Rendered {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_HEADING_ATTRIBUTES;
    let mut out = Rendered::default();
    let mut events: Vec<Event> = Vec::new();
    let mut used_ids: Vec<String> = Vec::new();
    // While inside a heading: its level and the events of its text.
    let mut heading: Option<(HeadingLevel, Vec<Event>)> = None;
    // While inside the first paragraph: its text so far.
    let mut summary: Option<String> = None;
    let mut summary_done = false;
    let mut in_comment = false;

    for event in Parser::new_ext(text, options) {
        // License comments and other HTML comments stay out of the page.
        // A comment block arrives as one event per line.
        if let Event::Html(raw) | Event::InlineHtml(raw) = &event
            && (in_comment || raw.trim_start().starts_with("<!--"))
        {
            in_comment = !raw.contains("-->");
            continue;
        }
        if let Some((level, inner)) = heading.as_mut() {
            if let Event::End(TagEnd::Heading(_)) = event {
                let level = *level;
                let inner = std::mem::take(inner);
                heading = None;
                let words = plain_text(&inner);
                if level == HeadingLevel::H1 && out.title.is_empty() {
                    out.title = words;
                    continue;
                }
                let id = unique_id(&slug(&words), &mut used_ids);
                if level == HeadingLevel::H2 {
                    out.headings.push((id.clone(), words));
                }
                events.push(Event::Start(Tag::Heading {
                    level,
                    id: Some(CowStr::from(id)),
                    classes: Vec::new(),
                    attrs: Vec::new(),
                }));
                events.extend(inner);
                events.push(Event::End(TagEnd::Heading(level)));
            } else {
                inner.push(rewrite(event, link));
            }
            continue;
        }
        match &event {
            Event::Start(Tag::Heading { level, .. }) => {
                heading = Some((*level, Vec::new()));
                continue;
            }
            Event::Start(Tag::Paragraph) if !summary_done => summary = Some(String::new()),
            Event::End(TagEnd::Paragraph) if summary.is_some() => {
                out.summary = first_sentence(&summary.take().unwrap_or_default());
                summary_done = true;
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some(s) = summary.as_mut() {
                    s.push_str(t);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(s) = summary.as_mut() {
                    s.push(' ');
                }
            }
            _ => {}
        }
        events.push(rewrite(event, link));
    }
    html::push_html(&mut out.body, events.into_iter());
    out
}

/// Passes link and image addresses through `link`.
fn rewrite<'a>(event: Event<'a>, link: &dyn Fn(&str) -> String) -> Event<'a> {
    match event {
        Event::Start(Tag::Link { link_type, dest_url, title, id }) => Event::Start(Tag::Link {
            link_type,
            dest_url: CowStr::from(link(&dest_url)),
            title,
            id,
        }),
        Event::Start(Tag::Image { link_type, dest_url, title, id }) => Event::Start(Tag::Image {
            link_type,
            dest_url: CowStr::from(link(&dest_url)),
            title,
            id,
        }),
        other => other,
    }
}

fn plain_text(events: &[Event]) -> String {
    let mut text = String::new();
    for event in events {
        match event {
            Event::Text(t) | Event::Code(t) => text.push_str(t),
            Event::SoftBreak | Event::HardBreak => text.push(' '),
            _ => {}
        }
    }
    text.trim().to_string()
}

/// A heading's anchor: lower case words joined by `-`, as GitHub makes them.
pub fn slug(text: &str) -> String {
    let mut slug = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            slug.extend(c.to_lowercase());
        } else if (c == ' ' || c == '-' || c == '_') && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() { "section".to_string() } else { slug }
}

fn unique_id(base: &str, used: &mut Vec<String>) -> String {
    let mut id = base.to_string();
    let mut n = 1;
    while used.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    used.push(id.clone());
    id
}

/// The first sentence of a paragraph, cut to a length a list can show.
pub fn first_sentence(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let end = text
        .char_indices()
        .find(|&(i, c)| {
            matches!(c, '.' | ':' | ';')
                && text[i + c.len_utf8()..].starts_with(' ')
                && !text[..i].ends_with("e.g")
                && !text[..i].ends_with("i.e")
        })
        .map_or(text.len(), |(i, _)| i);
    let sentence = &text[..end];
    if sentence.chars().count() <= 140 {
        return sentence.to_string();
    }
    let cut: String = sentence.chars().take(137).collect();
    format!("{}…", cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_the_title_summary_and_headings() {
        let text = "<!--\nA note for editors\n-->\n\n# Networking\n\n\
                    This chapter describes networking. It has more.\n\n\
                    ## Wired links\n\nText.\n\n## Wired links\n\n### Deeper\n";
        let page = render(text, &|url| url.to_string());
        assert_eq!(page.title, "Networking");
        assert_eq!(page.summary, "This chapter describes networking");
        assert_eq!(
            page.headings,
            [
                ("wired-links".to_string(), "Wired links".to_string()),
                ("wired-links-1".to_string(), "Wired links".to_string())
            ]
        );
        assert!(page.body.contains("<h2 id=\"wired-links\">Wired links</h2>"));
        assert!(page.body.contains("<h3 id=\"deeper\">Deeper</h3>"));
        assert!(!page.body.contains("editors"));
        assert!(!page.body.contains("<h1"));
    }

    #[test]
    fn rewrites_links() {
        let page = render("See [ssh](ssh.md#keys).", &|url| url.replace(".md", ".html"));
        assert!(page.body.contains("<a href=\"ssh.html#keys\">ssh</a>"));
    }

    #[test]
    fn makes_anchors_and_cuts_long_sentences() {
        assert_eq!(slug("The `log` command: reading logs"), "the-log-command-reading-logs");
        assert_eq!(slug("!!"), "section");
        assert_eq!(first_sentence("Use it, e.g. here. Then"), "Use it, e.g. here");
        let long = "word ".repeat(60);
        assert!(first_sentence(&long).ends_with('…'));
        assert!(first_sentence(&long).chars().count() <= 140);
    }
}
