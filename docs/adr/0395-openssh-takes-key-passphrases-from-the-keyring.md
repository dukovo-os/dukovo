<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0395. OpenSSH takes key passphrases from the keyring

- Status: Accepted
- Date: 2026-11-06

## Context

The roadmap's 13g asks for "ssh-agent able to take key passphrases from
the keyring". An SSH key is best sealed with a passphrase, which
OpenSSH's ssh-add asks for each time it gives the key to an agent, and
ssh and ssh-keygen each time they use the key without one. Each person
now has a keyring, which authd keeps sealed under their password (ADR
0393).

OpenSSH (ADR 0289) asks every question, passphrase or not, through
`read_passphrase`, which runs the program `SSH_ASKPASS` names in place
of reading the terminal when `SSH_ASKPASS_REQUIRE` is `force`, or is
`prefer` on a graphical display. The program gets the question as its
operand, and nothing else: a key's file is named only in the words of
the question, and ssh asks again in the same words after a wrong
passphrase. Linux desktops do this with a graphical askpass, whose
keyring opens the key at sign-in.

## Decision

- **`ssh-askpass-keyring`**, a command of systools, is OpenSSH's askpass
  in every session: `signin::environment` sets
  `SSH_ASKPASS=/system/commands/ssh-askpass-keyring` and
  `SSH_ASKPASS_REQUIRE=force`, the second because Dukovo has no
  graphical display yet.
- **A key's passphrase is the entry `ssh PATH`**, PATH being the key
  file's, made absolute against the current folder. The command finds
  the file in OpenSSH 10.6's questions: ssh's `Enter passphrase for key
  'FILE':`, ssh-keygen's `Enter passphrase for "FILE":`, and ssh-add's
  `Enter passphrase for FILE:` and `Bad passphrase, try again for
  FILE:`.
- **It gives a passphrase only once it has opened the key with it**,
  with RustCrypto's ssh-key, which sshd already has through russh. A
  kept passphrase that no longer opens the key, as after `ssh-keygen
  -p`, is asked for again; so ssh, which asks again in the same words, is
  never given the same wrong passphrase twice.
- **What is typed is kept once it opens the key**, without asking: a
  person who wants a key's passphrase asked each time unsets
  `SSH_ASKPASS_REQUIRE`, or removes the entry and uses a key without the
  keyring. A typed passphrase that does not open the key is handed to
  OpenSSH as it is, which says so and asks again.
- **Every other question is asked at the terminal**, `/devices/tty`, as
  OpenSSH asks it without an askpass: unseen, but for a yes or no
  (`(yes/no` in the question, or `Please type 'yes'`), seen. OpenSSH's
  hint `SSH_ASKPASS_PROMPT=confirm` is answered with the exit status,
  and `none` only shows the message. No terminal, or Ctrl+C, is no
  answer, as OpenSSH has without one.
- **A locked keyring changes nothing but the keeping**: the passphrase
  is asked for and not kept, and the command says so.

## Consequences

- ssh-add asks for a key's passphrase once. After that, after a restart
  too, ssh-add, ssh and `ssh-keygen -Y sign` (git's commit signing with
  an ssh key) take the key with nothing asked.
- Each use of a kept passphrase opens the key twice, once in the
  askpass and once in OpenSSH: bcrypt_pbkdf's 16 rounds, a fraction of a
  second.
- OpenSSH's answers to keyboard-interactive questions that a server
  asks to be shown are typed unseen, as under a graphical askpass.
- A key whose path is longer than ssh's 100 characters in its question,
  or that is not an OpenSSH key, has its passphrase asked for and not
  kept.
- The `ssh-keyring` boot test: alice makes a key with a passphrase,
  ssh-add asks once and the passphrase is kept, a second agent and
  `ssh-keygen -Y sign` ask nothing, and a passphrase changed with
  `ssh-keygen -p` is asked for again and kept in place of the old.
