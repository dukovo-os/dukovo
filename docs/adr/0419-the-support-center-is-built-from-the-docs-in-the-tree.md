<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0419. The support center is built from the docs in the tree

- Status: Accepted
- Date: 2026-11-11

## Context

Dukovo needs one place where users, administrators and developers can
answer any question about the system: how to install it, what every
command does, how the parts fit together, and how to build for it. That
place is the support center at dukovo.org/support.

Docs that are written apart from the code they describe go stale. The
project already keeps its man pages, handbook, design notes and decision
records in this repository, and `ob docs --check` fails the build when a
command or setting has no page. The support center should keep that
property instead of becoming a second copy.

The options were a hosted documentation platform, a general static site
generator (mdBook, Hugo, Docusaurus), or a small generator in `ob`. A
hosted platform adds an account, a second source and third-party requests,
which the site's no-trackers promise rules out. mdBook already builds the
handbook, but it renders one book; it cannot take in man pages, decision
records and generated references with one search and one set of
contents. The other generators bring a Node.js or Go toolchain into the
build for little gain.

## Decision

`ob docs site` builds the support center into `out/docs/site/` from the
files in the tree:

- Every man page, including the section 2 pages ridlc writes from
  `abi/*.ridl`, rendered by `mandoc -T html`, with each `name(n)` it
  mentions linked to that page.
- The handbook, in the order and parts of its `SUMMARY.md`; the design
  notes in `docs/design/`; every decision record, with an index; and the
  roadmap. Markdown is rendered with pulldown-cmark.
- The tutorials, guides, runbooks and error pages in `docs/tutorials/`,
  `docs/guides/`, `docs/runbooks/` and `docs/errors/`, each folder a group
  in the contents, as soon as the folder exists.

A link from one Markdown file to another becomes a link between their
pages; a link to any other file in the tree goes to that file on GitHub.
Every page carries "Report a problem", which opens an issue naming the page
and the version, and "Edit this page" when it has a source in the tree.
The site has no analytics, so these links are how readers tell the
project a page failed them.

Search runs in the browser over an index the build writes, with each
page's title, kind, summary and the other names in its man page's NAME
line, so a search for `7z` finds `7zz(1)`.

The layout, styles, script and home page live in `docs/site/`, in the
dukovo.org design: one header bar with search, the contents on the left
and "On this page" on the right, folding to a contents button and a search
button on a phone.

## Consequences

A pull request that changes a command changes its man page, and with it
the support center; there is nothing to copy across. The site's pages are
only as complete as the docs in the tree, so the checks that keep those
docs complete matter as much as the generator.

`ob` gains one crate family, pulldown-cmark, which is MIT licensed and has
no dependencies outside Rust.

Publishing the built site under dukovo.org/support, one copy per release
and one for the main branch, is separate work on the release workflow.
