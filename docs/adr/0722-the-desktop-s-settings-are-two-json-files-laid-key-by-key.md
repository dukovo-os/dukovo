<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0722. The desktop's settings are two JSON files laid key by key

- Status: Accepted
- Date: 2026-11-28

## Context

Until now windowd had nothing a person could set. Three things need it
with window management:

- whether an application may draw a part of its windows' title bars
  (ADR 0720), for all applications and for each by name;
- how fast the pointer moves and how far apart the two presses of a
  double click may be;
- how long a held key waits before it repeats, and how fast it repeats
  then, which windowd tells every program.

The first of them windowd also writes: the person chooses it for one
application from a window's menu, and the choice has to hold when the
desktop is next started.

The system's services keep their settings below `/system/settings`
(ADR 0113), a file for each, which the service reads itself. Most are
TOML that an administrator writes by hand and no program writes. A
desktop's are different in two ways. Programs write them: windowd here,
the Settings app later. And a desktop is one person's: two people who
use one machine do not want one pointer speed. And a desktop that is
running should take a change without being ended, since ending it ends
every program in it.

## Decision

windowd reads two files, `/system/settings/windowd.json` and then
`$HOME/.config/windowd.json`, and lays the second over the first key by
key. Neither has to be there; with neither, everything is as built.
windowd.json(5) is the reference for the keys:

| Key | Values | As built |
| --- | --- | --- |
| `title_bars.apps` | `"decide"` or `"full"` | `"decide"` |
| `title_bars.full` | a list of application ids | none |
| `title_bars.shared` | a list of application ids | none |
| `pointer.speed` | -1 to 1 | 0 |
| `pointer.double_click_ms` | 100 to 2000 | 500 |
| `keyboard.repeat_delay_ms` | 100 to 2000 | 250 |
| `keyboard.repeat_rate` | 1 to 100 | 30 |

- **A key a file does not have keeps the value from before it.** The
  person's file names only what the person changed, and a later change
  to the system's file reaches everyone who did not.
- **The two lists are laid name by name.** An application in a file's
  list has that entry from there on, whichever list an earlier file
  had it in; an application in both lists of one file has the full
  bar, the choice that takes nothing from the person.
- **A mistake costs its key alone.** A value of the wrong kind or out
  of its range, and a key windowd does not know, are said on the
  standard error by the file's name and the key's, and the value from
  before stays. A file that is not JSON at all is said so and laid as
  if it were not there. The desktop always starts.
- **The pointer's speed is a power of two.** Every motion is 2 to the
  power `speed` times as far as the curve of ADR 0715 has it: half as
  far at -1, twice at 1. The curve's shape is not the person's to set;
  one number on a scale that feels even is.
- **SIGUSR1 has windowd read the files again.** Everything they say
  holds from then on: windows that share their bars are told, every
  program is told the keys' repeat. Nothing watches the files; the
  program that writes one sends the signal.
- **windowd writes one key group itself.** The person's choice from a
  window's menu is written to the person's file as an entry in
  `title_bars.full` or `title_bars.shared`, and taken out of the other.
  Every other key in the file stays as it was, those windowd does not
  know among them. The file is written beside itself and renamed over,
  so that it is whole or as it was. A file that is there and is not a
  JSON object is not written over: the choice then holds until the
  settings are next read, and windowd says so on the standard error.

## Alternatives

- **One file, the system's.** As the services have it. It would make
  the pointer's speed the machine's and need an administrator to change
  a person's taste.
- **A settings service the desktop asks.** The right home for settings
  that several programs share, and the Settings app of a later release
  may bring one. For three groups of keys that one program reads, it
  would be a service, a protocol and a start order for nothing a file
  does not do; the file's keys can be served by such a service
  unchanged.
- **TOML, as most of the system's settings are.** A person's TOML file
  has comments and an order of its own, and a program that writes an
  entry into it has to keep both or lose the person's text; the TOML
  code the system has only reads. JSON has neither to lose, and is what
  the files that programs write are in already (install-answers(5),
  manifest.json(5)).
- **Watching the files.** The person's file may not be there, may be on
  a file system that does not tell of changes, and is written in two
  steps by most editors. A signal is exact about when.
- **Writing the menu's choice to a file of its own.** It would keep
  windowd's writes away from the person's text, and leave two places
  that say whether an application shares its bar.
- **Refusing to start on a mistake.** A desktop that does not start
  because of a typing mistake in a settings file leaves the person at a
  console to find it.

## Consequences

- A person sets their desktop without an administrator, and an
  administrator sets the machine's defaults in the place every other
  setting is.
- JSON as `serde_json` writes it has no comments and the keys of an
  object in alphabetical order: a file windowd wrote an entry into
  comes back in that order, with the person's own spacing gone.
- Keys laid by the files are all there is: windowd's own chords, the
  keyboard's layout and the look of the frame are not settings yet
  (docs/deferred.md).
- A program can ask, by naming itself with another application's id,
  for that application's entry. The entry only says who draws a part of
  a title bar; it gives no program anything of another's.
