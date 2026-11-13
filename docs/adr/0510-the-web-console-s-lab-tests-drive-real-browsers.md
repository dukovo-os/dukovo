<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0510. The web console's lab tests drive real browsers

- Status: Accepted
- Date: 2026-11-13

## Context

The web console is used in browsers, and the three engines behind every
major one lay a page out, draw its controls, apply its colour scheme and
move its focus in ways of their own: Chromium (Chrome, Edge, Brave),
Firefox and WebKit (Safari). A page that works in one can lose a button
or its contrast in another. The console must also meet WCAG 2.2 levels A
and AA on every page, in the light and the dark theme, and a contrast rule
that fails in one theme can pass in the other.

The lab's steps reach a machine through SSH, the serial console, the
screen it draws, its power and its cables, and run the lab PC's own
programs against it (ADR 0401). None of them loads a page the way a
person does: a `curl` sees the bytes, not what a browser makes of them,
and a screenshot of the machine's own screen shows no browser at all.

The cloud build machines carry Chromium alone, at a path of their own
(`PLAYWRIGHT_BROWSERS_PATH`), and the lab PC is a computer whose
libraries and network are its owner's.

## Decision

- **A `browser` step in `dukovo-lab`.** It runs a Node.js script that
  sits beside the test on the lab PC, once in each engine the step
  names (all three by default), against
  `https://{machine}:{machine-port:PORT}`, with the same placeholders
  and the same forwarded port under QEMU as a `host` step. Its control
  is `peer`: a machine with no lab PC at its other end skips it.
- **Playwright and axe-core, pinned in `lab/browsers`.** `package.json`
  names each at one exact version and `package-lock.json` locks the
  rest. `ob lab` makes the kit in `out/lab-browsers` with `npm ci`,
  installs the three engines into its `browsers` folder with
  `playwright install` (unless `PLAYWRIGHT_BROWSERS_PATH` says the
  machine has its own), and passes the folder to `dukovo-lab` as
  `--browsers`. It makes the kit again when the lock changes. A lab PC
  without `npm`, or one that cannot reach the registry, still runs every
  other test: the browser steps skip, and `ob lab` says why.
- **One runner, and a small API for the scripts.** `dukovo-lab` carries
  the runner script and writes it into the test's work folder before the
  first engine. It loads the kit, launches the engine and hands the
  test's script an object `t`: `t.open` makes a context and a page with
  options for the colour scheme, forced colours, contrast, motion,
  viewport and the rest; `t.see` and `t.gone` wait up to 15 seconds for
  a locator; `t.axe` fails on any violation of WCAG 2.0, 2.1 and 2.2 A
  and AA that axe-core finds, and `t.violations` lists them; and
  `t.screenshot` keeps a full-page PNG. A script is written as a person
  would use the page, through roles and visible text, so a control a
  person could not find is one the test cannot find either.
- **A runner's exit status says what happened to its engine.** 0 is a
  pass, 77 a skip with its reason, and anything else a failure, whose
  message is on `fail:` lines. The step fails when an engine fails,
  skips when every engine skips, and otherwise passes with a note of
  which engines ran and which were skipped and why.
- **An engine that cannot run is skipped, not failed**: one that is not
  in the kit, and one that will not start on the lab PC, nearly always
  for want of a system library that `playwright install-deps` brings.
  The step's note and the test's log name each skipped engine, so a run
  that checked one engine does not look like a run that checked three.
- **A failure keeps what shows it.** For each page still open the runner
  keeps a screenshot, and for each context a Playwright trace (the page's
  steps, snapshots and network, opened with `playwright show-trace`),
  in the run's bundle with the other screenshots and logs.
- **The browser accepts the page's certificate.** A page test checks
  pages, not certificates: the lab's machines make certificates of their
  own, and a test that stopped at a warning would test nothing else. The
  console's TLS settings and certificates get lab tests of their own.
- **The step is proved against nginx.** `web-console/browser` has the
  machine's nginx serve two pages over HTTPS: one the script uses as a
  person would, in the dark and the light theme and in each engine, and
  one with a picture that has no text, which the accessibility check
  must find. The console's pages replace them as they are written.
- **`ob lab coverage` reads a test's browser script** for the words of
  the page it covers, as it reads the test's file and the scripts it
  runs as `{here}/NAME`: a page's entry is covered by a script that
  opens it.

## Consequences

- A lab PC that runs the web console's tests needs Node.js 22 or later,
  the network for the first `./ob lab` (the packages and the three
  engines), and the engines' system libraries, which on Linux Mint are
  one `sudo npx --prefix out/lab-browsers playwright install-deps`.
  The handbook says so.
- The cloud build machines run the tests in Chromium and skip Firefox
  and WebKit, and say so in each result. The hardware campaign runs all
  three. A skipped engine is a gap the result names, not a pass.
- Playwright builds its engines to the version that is pinned, so a new
  version of Playwright or axe-core is a change to `lab/browsers` that
  carries its lock, and the engines download again.
- axe-core is under the Mozilla Public License 2.0, and Playwright under
  Apache-2.0. The kit lives in `out` on a developer's or the lab's
  computer; neither is in the image, a release or the repository, which
  holds the names and versions alone.
- A browser step starts each engine it names, and a browser that hangs
  is stopped by the test's time limit, which fails the step.
- CI runs no lab tests, so the web console's tests run on QEMU before each
  pull request that changes the console; `docs/deferred.md` records
  running them in CI.
- Sign-in, passkeys and the stand-in services the console's pages need
  come with the pages that first use them.
