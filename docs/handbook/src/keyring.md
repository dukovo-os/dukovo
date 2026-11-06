<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Your keyring

Each person has a keyring, which keeps their secrets: tokens for git and
other services, the passphrases of their SSH keys, Kerberos tickets, and
Wi-Fi passwords.
authd keeps it in a file sealed under a key made from the person's
password, so the file holds no secret, and no entry's name, in the
clear. The full references are keyring(1), git-credential-keyring(1),
ssh-askpass-keyring(1) and authd(8).

## Opening and closing

The keyring opens when you sign in with your password, at the console or
over SSH, and is made then if you have none. It stays open while you are
signed in and closes when your last session ends. A session over SSH that
started with a key, and so without your password, opens it with
`keyring unlock`. `keyring lock` closes it until then.

An account with no password has no keyring, since nothing could lock it.

## Keeping a secret

```
$ keyring set example-api
Secret for example-api:
$ keyring get example-api
$ keyring list
example-api
$ keyring remove example-api
```

`keyring set` asks for the secret unseen at a terminal, or reads the
whole of its standard input, without the last newline, so a script can
pipe one in. `keyring get` writes the secret alone, with a newline only
at a terminal.

## git

git keeps its tokens for https in the keyring: the system's gitconfig
names `git-credential-keyring` as git's helper. The first push or fetch
that needs a token asks for it, and once the server takes it, git never
asks again, after a restart too, until the server refuses it. Each token
is an entry named `git https://USER@HOST`. A `~/.gitconfig` of your own
may name another helper, or none.

## SSH keys

OpenSSH asks its questions through `ssh-askpass-keyring`, which your
session names in `SSH_ASKPASS`. When `ssh-add` asks for a key's
passphrase the first time, what you type is kept as the entry
`ssh PATH`, once it opens the key. From then on `ssh-add`, `ssh` and
`ssh-keygen -Y sign`, which git runs to sign a commit with an ssh key,
take the key without asking:

```
$ eval "$(ssh-agent -s)"
$ ssh-add ~/.ssh/id_ed25519
Enter passphrase for /users/alice/.ssh/id_ed25519:
Identity added: /users/alice/.ssh/id_ed25519 (alice)
```

The next session's `ssh-add` asks nothing. A passphrase changed with
`ssh-keygen -p` is asked for again the next time, and the new one is
kept in place of the old. Other questions, such as a server's password
or whether to trust a host's key, are asked at the terminal as before,
and are not kept.

## Kerberos

`kinit` keeps your Kerberos tickets in the keyring, each as the entry
`krb5 CLIENT SERVER`, and `kdestroy` removes them. The chapter
[Kerberos](kerberos.md) says more.

## Apps

An app in its sandbox reaches its own entries alone. The name it gives,
such as `token`, is the entry `app ID token` of your keyring, so
`keyring list` shows an app's entries with its ID, and `keyring remove`
forgets one. The app lists only its own, and cannot read what git, ssh
or another app keeps. An app that needs more asks for `keyring.shared`,
which the console's bottom line asks you about with a warning:

```
 [warning] Tokens (org.example.tokens) asks to: use every secret in your keyring: your tokens and passphrases, and other apps'      O: once    A: always    D: don't allow
```

Given, the app reaches every entry by its own name, as your own
programs do; `bundle revoke ID keyring.shared` takes it back. Programs
outside a sandbox, and an app's commands that run with your access,
reach every entry, as on Linux.

## Your password

`passwd` seals the keyring under your new password, and every entry
stays. When an administrator sets your password without the old one, the
keyring is kept only if it is open at that moment, as while you are
signed in; otherwise it is emptied, and `passwd` says so. No one else
holds the keyring's key, so a forgotten password loses the keyring.
There is no way back in, by design.

## Troubleshooting

- **"your keyring is locked"**: the session started without your
  password, or `keyring lock` closed it. `keyring unlock` opens it.
- **git asks for a token every time**: `git config --show-origin
  credential.helper` shows which helper git uses; one of your own in
  `~/.gitconfig` comes before the system's. `GIT_TRACE=1 git push` shows
  git running the helper, and `keyring list` the tokens it kept.
- **ssh-add asks for a passphrase every time**: `echo
  $SSH_ASKPASS_REQUIRE` should say `force`; a shell profile may unset
  it. `keyring list` should show `ssh` and the key's path. A key that is
  not an OpenSSH key has its passphrase asked for and not kept.
- **To stop keeping a passphrase or token**: `keyring remove` with the
  entry's name, as `keyring list` shows it.
- **An app does not find its secrets**: it reaches its keyring only
  while `bundle` runs it; `keyring list` outside shows its entries as
  `app ID NAME`. `bundle info ID` shows whether you gave it
  `keyring.shared`.
- **The keyring did not open at a sign-in**: authd's log says why.
  `elevate log -s authd` shows lines such as
  `authd: keyring of alice: open` and `authd: keyring of alice: closed`.
