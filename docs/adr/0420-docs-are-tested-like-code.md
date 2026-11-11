<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0420. Docs are tested like code

- Status: Accepted
- Date: 2026-11-11

## Context

The support center is built from the docs in the tree (ADR 0419), so it is
only as good as they are. `ob docs --check` already fails when a command,
setting or interface has no man page, but nothing checked that an example
still works, that a guide still leads where it says, that a link still
goes somewhere, or that search still finds a page. Docs that are not
tested go stale in the same way untested code breaks: quietly, and found
by a reader at the worst time.

The project keeps its code honest with tests that run on every pull
request and a definition of done that a change must meet. The docs need
the same, so that a page on dukovo.org/support is one that was true of the
build it describes.

## Decision

**Examples are tests.** A Markdown code block in the handbook, a guide, a
tutorial or a runbook that opens with ```` ```console test ```` is run by
the `docs-examples` boot test. A line that starts with `# ` or `$ ` is a
command, typed at the console of a test image; each command must succeed,
and the screen must then show each other line of the block (a line of
`...` stands for output left out). `ob docs --check` refuses a block the
test cannot run. A pull request that changes a file with such a block runs
the test, as does every pull request that runs boot tests, so a change to
the code that breaks an example fails like any other test.

**Guides, tutorials and runbooks have lab tests.** Each page in
`docs/guides/`, `docs/tutorials/` and `docs/runbooks/` needs a boot test
named for it (`guide-<name>`, `tutorial-<name>`, `runbook-<name>`) that
follows it from start to end; `ob docs --check` fails without one, and a
change to the page runs it.

**Error IDs have pages.** An error a person can act on carries a stable ID
of an area's letters and three digits, such as `UPD-103`, and has a page
in `docs/errors/` named for it. The areas are listed in
`docs/errors/README.md`; `ob docs --check` reads the code for their IDs
and fails when one has no page or a page's ID is no longer given.

**The built site is checked.** `ob docs site --check` fails when a link
between the site's pages, or to a part of a page, leads nowhere, and when
searching for a command, one of its other names or an error ID does not
show its page. Its search follows the site's own script, so the check
finds what a reader would.

**Docs are part of done.** CONTRIBUTING.md's definition of done asks every
change to bring the man pages, the handbook, the support center and the
website's claims up to date in the same pull request, and CI runs both
checks.

## Consequences

An example costs a few seconds of a boot test, and a guide a test of its
own, so docs that say what to type are written with the same care as the
code. The checks hold the pages that exist; what pages ought to exist,
such as a guide for each task people come for, is still a matter of
review.

The `docs-examples` test types at the framebuffer console and reads it
back, so an example's commands and output are ASCII and fit on the screen.
Examples that need a disk, a network or a second machine belong in a
guide, whose own test can set them up.

The check of links reaches the site's own pages. Links to other sites
change without notice and would fail pull requests for reasons outside
them, so they are left to a separate, scheduled check.
