<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0564. A machine's record carries authd's files of the accounts

- Status: Accepted
- Date: 2026-11-15

## Context

A whole machine is restored from its backups and its record (ADR 0464):
the settings place, with the accounts and groups, and the people's
files come back as subvolumes, and the record carries the files of
`/system/state` that say which apps are installed and what each person
allowed them. ADR 0464 says that a record carries nothing else of the
state, and that a restored machine comes back with its accounts and
passwords. The two do not agree. The password hashes are in authd's
own folder of the state (ADR 0218), not in the settings, so a restored
machine had the old machine's accounts with the new machine's hashes:
on a fresh install, only the account made at setup could sign in, and
with that account's new password.

The same holds for the rest of what authd keeps for each account:

- the sharing passwords' hashes, for signing in to shares from a
  Windows PC (ADR 0228);
- the numbers of removed accounts, which no account gets again (ADR
  0220), while `/users/.removed` holds files with those numbers;
- whether each person's jobs run while they are signed out;
- each person's keyring (ADR 0393), sealed with a key made from their
  password;
- each person's second steps and API tokens for the web console (ADR
  0514), without which an account that needed a second step would sign
  in with its password alone.

## Decision

- **A record of format 2 carries authd's files of the accounts.**
  Beside bundled's registry and the grants, a round's record carries
  authd's `passwords`, `sharing`, `retired` and `jobs`, each always,
  empty if authd has not written it, each person's keyring
  (`keyrings/<number>`) and each person's console file
  (`console/<name>.toml`). A record of format 2 that lacks one of the
  four files, and one of format 1 that has any of authd's, is refused.
  authd's notices for a next sign-in and the addresses it blocks stay
  the machine's own, as its logs do.
- **A restore empties the people's folders first.** As with the grants,
  `backup restore --machine` stages the files with the record's other
  state files, and at the next start Spark empties the grants,
  `authd/keyrings` and `authd/console` of their files before it writes
  the record's, so that each person's are the record's alone. Each is
  root's, with mode 0600. A restore from a record of format 1 leaves
  authd's files as they are.
- **The recovery key opens them as it opens the rest.** A record is
  encrypted to the destination's keys and tagged with a key from the
  recovery key, as the backups are. Whoever holds the recovery key reads
  the password hashes, as they read every person's files; the keyrings
  stay sealed with their owners' passwords.

## Consequences

- A machine restored from its backups signs its people in with the
  passwords, sharing passwords and second steps they had, their API
  tokens still work, and their keyrings open with their passwords.
- A destination, which holds the records encrypted, holds the hashes
  encrypted too. A person who changes a password after a round signs
  in to a restore of that round with the old one.
- A record grows by the size of authd's files: a few hundred bytes for
  each account, and each keyring's size.

## Alternatives

- **Asking each person for a new password after a restore:** nothing
  of authd's in the backups, but every account would need an
  administrator to set its password, second steps would be lost, and
  each keyring, sealed with the old password, would be lost with it.
- **Backing up `/system/state` as a place:** the slot state, the logs
  and each service's own state are the machine's and must not come back
  (ADR 0464), and the record already carries the state that does.
