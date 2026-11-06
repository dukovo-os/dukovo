<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0394. git keeps its tokens in the keyring

- Status: Accepted
- Date: 2026-11-06

## Context

git asks for a token the first time it pushes or fetches over https,
and hands it to a credential helper to keep (ADR 0292). The system's
gitconfig names git's memory cache, so the token is asked for again
after 8 hours and after every restart. ADR 0292 named the keyring as
the helper to come, and the roadmap's 13g makes it git's default "in
place of the memory cache". 13g is done "when git pushes over https
after a reboot without asking for the token again, and the keyring's
file holds no secret in the clear". Each person now has a keyring,
which authd keeps sealed under their password (ADR 0393).

## Decision

- **`git-credential-keyring`**, a command of systools, is a git
  credential helper (gitcredentials(7)): `get`, `store` and `erase`,
  with the credential on the standard input.
- **An entry per user and address**: `git PROTOCOL://USER@HOST`, with
  `/PATH` after it when git sends a path (`credential.useHttpPath`),
  holding the token. `get` answers with the entry of the user git names,
  or with the first entry for the address when it names none, as a
  remote's URL usually does not.
- **`erase` removes only a wrong token.** git erases the credential the
  server refused. The helper removes an entry only if it still holds
  that token, so one stored since stays.
- **The system's gitconfig names it**, `helper = keyring`, in place of
  `cache --timeout=28800`. A person's `~/.gitconfig` may name another
  helper, or none.
- **A locked keyring changes nothing for git**: the helper says on the
  standard error that the keyring is locked, and git asks for the token
  as it would with no helper.

## Consequences

- A token typed once is given again without asking, across restarts,
  until the server refuses it.
- In a session over SSH that started with a key, git asks for tokens
  until `keyring unlock` opens the keyring.
- The `git-https` boot test, which Spark runs as no person, names the
  memory cache for its own git and checks that the system names the
  keyring. git-https-test gains `--serve`, which runs its server alone
  in a person's session.
- The `git-keyring` boot test is 13g's: on a Btrfs root, alice gives git
  the token once, and clones, commits and pushes with git asking
  nothing. No file of hers holds the token, and the keyring's file
  holds it neither in text nor in hexadecimal. After a reboot she
  pushes again, and git still asks nothing.
