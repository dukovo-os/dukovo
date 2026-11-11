<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0428. The support center is published as a copy per release

- Status: Accepted
- Date: 2026-11-11

## Context

`ob docs site` builds the support center from the docs in the tree (ADR
0419), and the docs checks keep it true of the build it comes from (ADR
0420). People run different releases, so the docs for the release they run
must stay where they are when the next one comes out, and those who follow
the main branch need its docs too. The site also has to be refreshed
without anyone remembering to do it, or it falls behind the tree.

dukovo.org is a static site in the private repository dukovo-os/website,
which its host deploys on every push to `main`. The site could be pushed
there from this repository's CI after each merge, which needs a key with
write access to the website held here, or pulled by the website's own
workflow on a schedule, which needs no key and lags by up to its interval.

## Decision

The website pulls. Its "Publish the support center" workflow runs every
six hours, and can be run by hand. It clones this repository and, for
what has changed since it last ran, builds the site with `ob docs site
--check` and commits it, with `build.py --publish`:

- `main` is published as `dukovo.org/support/dev/`, labelled Development,
  when its commit has changed, and refreshes the roadmap on dukovo.org's
  home page from `docs/roadmap.md`.
- A new tag `vX.Y.Z` is published once, as `dukovo.org/support/X.Y.Z/`,
  and stays there. Tags made before `ob docs site` existed are skipped.
- `dukovo.org/support/` itself is a copy of the newest release's, or of the
  development copy until a release has one, so a link to it always lands on
  docs.
- `support/versions.js` lists the copies. The header's version picker reads
  it and opens the same page in the chosen copy, or that copy's home when
  the page is not in it.

`data/support.json` in the website records the commit each copy was built
from, so a run with nothing new ends before it installs anything.

## Consequences

Docs reach dukovo.org within six hours of the change that makes them, by
the same checks a pull request passes, and no credential for the website
is kept in this repository. A release's docs can be published at once by
running the workflow by hand.

The website repository grows by a copy for each release; files that do not
change between copies are stored once. It holds generated pages that are
not edited by hand there; its README says so. Its scheduled runs cost the
website's private Actions minutes: a run with nothing new takes about a
minute, and one that builds takes a few.
