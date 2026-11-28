<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0726. Each screen has a row of desktops of its own

- Status: Accepted
- Date: 2026-11-28

## Context

A person with more windows than fit on a screen sorts them into
desktops: mail on one, the work on another, and a key to go between
them. Every desktop system has them, and they differ in one thing that
is hard to change later: whether a desktop spans all the screens or
belongs to one.

With desktops that span, a switch changes every screen at once. The
person who keeps a reference on the second monitor and goes through
their work on the first loses the reference at every switch, or pins
its window to all desktops, one window at a time.

## Decision

Each screen has a row of desktops of its own, one of which shows.

- **A window is on one desktop of the screen it is on**, and shows
  when that desktop does. A window that is moved, dragged or carried to
  another screen joins the desktop that screen shows.
- **The keys act on the screen the pointer is on.** Super+Ctrl+D makes
  a desktop after the one that shows and shows it. Super+Ctrl+Left and
  Right show the one before and the one after. With Shift held too,
  the window worked in goes along. Super+Ctrl+F4 closes the desktop
  that shows: its windows go to the one before it, or to the one after
  for the first. The last desktop of a screen does not close.
- **A row has at most sixteen desktops.** A seventeenth is not made.
- **A switch gives the keyboard to the window last worked in** on the
  desktop that then shows, or to none if it has no window.
- **The list of windows has the windows that show**: those of each
  screen's present desktop. Alt+Tab does not leave the desktop.
- **A screen that goes takes its row with it, and brings it back.**
  Its windows are carried to another screen's present desktop
  (ADR 0725) and remember their desktop with their home.
- **The splits of snapped windows are a desktop's** (ADR 0727): each
  desktop of each screen has its own.

## Alternatives

- **Desktops that span the screens.** One switch changes everything
  the person sees. It is what the systems with one row have, and what
  their users with two monitors work around.
- **Both, by a setting.** Two behaviours to test on every path that
  moves a window, for a choice most people never make. The setting can
  come if it is asked for: a spanning row is a rule that switches all
  screens together.
- **A fixed number of desktops**, four or nine with a key each. Rows
  that grow and shrink need no setting, and the keys for a numbered
  desktop can come with a file of keys (`docs/deferred.md`).
- **Desktops as a grid.** A second pair of keys and a picture to find
  the way in, for a few more places than a row has.

## Consequences

- A switch on one monitor leaves the others as they are.
- Nothing shows which desktop of how many is the present one, or the
  desktops side by side as pictures: that view is the desktop shell's,
  in a later release (`docs/deferred.md`). Until then an empty desktop
  looks like any other.
- The rows are windowd's own state and end with it. A named window
  keeps its desktop's number in the table of places (ADR 0729).
