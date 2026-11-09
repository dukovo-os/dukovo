<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0393. A keyring for each person, kept by authd

- Status: Accepted
- Date: 2026-11-06

## Context

The roadmap's 13g asks for "a keyring for each user, kept by authd":
secrets such as git and API tokens, SSH key passphrases and Wi-Fi
passwords, "encrypted at rest with a key derived from the user's
password at sign-in (Argon2id) and held in memory only while the user is
signed in". Changing the password re-wraps the key; a forgotten password
loses the keyring, with no back door. It is done when git pushes over
https after a reboot without asking for the token again, and the
keyring's file holds no secret in the clear.

Until now git keeps a token in the memory of `git credential-cache` for
8 hours (ADR 0292), and asks again after a restart. authd already holds
each password for a moment: it checks it against its Argon2id hash at
each sign-in at a console (ADR 0218) and over SSH (ADR 0348), and it
starts and reaps every session.

## Decision

- **One file per person**, `/system/state/authd/keyrings/NUMBER`, named
  by the account's number, which is never given again (ADR 0220), in
  authd's own folder, root's and mode 0600. It is three lines: the
  format, `dukovo-keyring 1`; the keyring's key, sealed; and the
  entries, sealed.
- **Two keys.** The entries, each a name and a secret of any bytes, are
  sealed together with ChaCha20-Poly1305 under a random 256-bit key of
  the keyring's own. That key is sealed with ChaCha20-Poly1305 under a
  key made from the password with Argon2id, with a salt of its own, at
  the cost the password's hash has: RFC 9106's second recommendation
  (64 MiB, 3 passes, 4 lanes) for a password set on Dukovo. Each sealed
  part is bound to its owner's number and its line, so that a part moved
  elsewhere does not open, and each write seals the entries again with
  a new random nonce. ChaCha20-Poly1305 comes from ring, which systools
  already uses for TLS (ADR 0193), so no dependency is added.
- **Opened at a sign-in with the password.** authd opens the keyring
  when its person signs in with their password, at a console or over
  SSH, before their session's first program starts, and makes it then
  if they have none. A session that started without the password, as one
  over SSH with a key, opens it with `keyring unlock`, which takes the
  password and waits after a wrong one as a sign-in does. An account
  with no password has no keyring: nothing could lock it.
- **Held only while signed in.** authd holds an open keyring in its own
  memory, and closes it, wiping its key and secrets, when the person's
  last session ends, or at once with `keyring lock`.
- **Changing the password seals the same key again.** passwd's change
  opens the keyring, if it is not open, with the password given, and
  seals its key under the new one; the entries stay. A password that
  root sets without the old one, or that the startup-trouble menu
  resets (ADR 0253), cannot open a keyring that is not open, so it is
  emptied, and passwd and the person's next sign-in say so; so is one
  whose new password is empty. There is no other key and no way back
  in.
- **The `keyring` command** sets, gets, lists and removes entries, and
  unlocks and locks the keyring, through new requests to authd (see
  authd(8)): anyone's to make, of their own keyring alone. A secret is
  typed unseen, or read from the standard input.
- **Who reaches an entry.** A program outside an app's sandbox runs with
  its person's own access, as on Linux: it could read their files and
  their processes' memory, so a limit on it would only look like one,
  and every such program reaches every entry of its person's keyring. An
  app in its sandbox reaches only its own entries, and others only as
  its person allows on the trusted prompt, in a later pull request of
  13g.

## Consequences

- A sign-in with a password costs a second Argon2id, a few tenths of a
  second on a laptop, to open the keyring.
- The keyring's file holds no secret and no entry's name in the clear;
  without the password it gives away only how big it is.
- Anyone with root, or the person's processes, can read an open
  keyring's secrets while it is open, as with every keyring on Linux;
  the file at rest, and a disk taken away, give nothing without the
  password.
- A forgotten password loses the keyring. The handbook and keyring(1)
  say so plainly.
- The `keyring` boot test: a sign-in makes and opens the keyring; a
  secret is kept, read and listed, and its file, read by root, holds
  neither it nor its name; it opens again at the next sign-in; passwd
  seals it under the new password; `keyring lock` and `unlock` close and
  open it; and a password set through elevate without the old one
  empties a locked keyring, and says so.
- git's credential helper on the keyring, SSH keys' passphrases from it,
  and apps' own entries follow in 13g's next pull requests.
