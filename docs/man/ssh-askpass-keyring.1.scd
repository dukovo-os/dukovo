ssh-askpass-keyring(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

ssh-askpass-keyring - give OpenSSH your keys' passphrases from your keyring

# SYNOPSIS

*ssh-askpass-keyring* _question_

# DESCRIPTION

*ssh-askpass-keyring* is OpenSSH's askpass in your session (ADR 0395):
*ssh*(1), *ssh-add*(1) and *ssh-keygen*(1) ask each of their questions
through it, and read its answer. Your session's environment names it,
as *SSH_ASKPASS*, with *SSH_ASKPASS_REQUIRE=force*. OpenSSH runs it
itself; you do not.

When OpenSSH asks for a key's passphrase, the answer comes from your
keyring (*keyring*(1)), from the entry *ssh* _path_, _path_ being the
key file's. The passphrase kept is given only once
*ssh-askpass-keyring* has opened the key with it. When there is none,
or it no longer opens the key, as after *ssh-keygen -p*, it asks you at
the terminal, as OpenSSH would, and keeps what you type once it opens
the key. So *ssh-add* asks for a key's passphrase once, and then never
again, after a restart too; *ssh* and *ssh-keygen -Y sign*, which git
runs to sign a commit with an ssh key, ask nothing either.

Every other question, a password, a PIN or a new passphrase, is asked
at the terminal, unseen, and a yes or no, as for a host's key, seen.
Nothing but a key's passphrase is kept.

When your keyring is locked, as in a session over SSH that started
with a key, the passphrase is asked for and not kept, and
*ssh-askpass-keyring* says so: *keyring unlock* opens the keyring
first.

A key it cannot open itself, as one that is not an OpenSSH key or a
file whose path is not text, has its passphrase asked for and not
kept.

# ENVIRONMENT

*SSH_ASKPASS_PROMPT*
	Set by OpenSSH: *confirm* asks leave to use a key, and the exit
	status is the answer, 0 for yes; *none* only shows the message.

*SSH_ASKPASS_REQUIRE*
	Read by OpenSSH: *never* before a command, or *unset
	SSH_ASKPASS_REQUIRE* in your shell, has OpenSSH ask on its own,
	without the keyring.

# EXIT STATUS

0 with the answer on the standard output, 1 with no answer: no
terminal to ask on, or the question stopped with Ctrl+C.

# FILES

_/devices/tty_
	The terminal it asks on, whatever OpenSSH's input and output are.

# SEE ALSO

*keyring*(1), *ssh*(1), *ssh-add*(1), *ssh-agent*(1), *ssh-keygen*(1),
*git-credential-keyring*(1)
