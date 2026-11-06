<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0396. An app in a sandbox reaches its own keyring entries alone

- Status: Accepted
- Date: 2026-11-06

## Context

The roadmap's 13g asks that "a program reaches only its own entries
unless the user grants more". authd keeps each person's keyring (ADR
0393) and answers whoever acts for that person, and a program's name
proves nothing: any program may call itself `git`. What does tell
programs apart is the sandbox each bundle's program runs in (ADR 0225),
which the runner builds after checking the bundle's seal and
signature, so it knows the app's ID for certain. The security design
reserved a permission for the rest, "Use keychain items another app
saved", asked with a warning; until now an app in a sandbox could not
reach authd at all, as its view holds no `/system/live/authd`.

On 2026-11-06 Chase chose that apps in a sandbox reach only their own
entries, and more only if allowed on the trusted console prompt, while
programs outside a sandbox reach every entry, as on Linux.

## Decision

- **The runner serves the app's keyring.** For each run of a sandboxed
  program, `bundle` makes a folder of the person's alone, mode 0700,
  `/tmp/.app-ID.keyring-PID`, binds a socket in it, and attaches the
  folder at `/system/live/authd` in the program's view, read-only. The
  app's programs, `keyring` among them, ask there as they would ask
  authd. A thread of the runner answers until the runner ends, and the
  folder is removed when the program ends.
- **The keyring alone passes.** The runner forwards to authd, as the
  person, only `keyring-set`, `keyring-get`, `keyring-list` and
  `keyring-remove`. Anything else, an unlock, a lock, a sign-in or an
  account, is refused, saying that the app reaches its keyring alone.
- **Its names are its own entries.** The name `NAME` an app says is the
  entry `app ID NAME` of the person's keyring, and the app lists only
  those, by its names for them. Outside the sandbox, `keyring list`
  shows them with their prefix, and `keyring remove` forgets one.
- **`keyring.shared`** takes the place of the reserved
  `keychain.shared`, as Dukovo's word is keyring: "Use every secret in
  your keyring, those of your programs and of other apps", asked with a
  warning on the trusted prompt (ADR 0224). Given, the names pass as
  they are, and the app reaches every entry as the person's own programs
  do, its own among them by their full names. Taken away while the
  program runs, the program is stopped, as for any permission.
- **No permission for its own entries**, as none is asked for its own
  data folder: an app's secrets are its own.

## Consequences

- An app keeps tokens and passwords sealed under the person's password
  with nothing asked, and cannot read what git, ssh or another app
  keeps.
- A program that an app starts in the background and that outlives the
  runner loses its keyring with it.
- Programs outside a sandbox, and commands that run with the person's
  access, reach every entry, as on Linux.
- The `bundle-keyring` boot test: an app keeps a secret and reads it
  back, lists only its own entry, does not find alice's, and is refused
  a lock; outside, its entry is `app ID NAME`. Given `keyring.shared` on
  the trusted prompt's warning, it reads hers.
