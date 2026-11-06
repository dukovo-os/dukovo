keyring(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

keyring - keep secrets in your keyring

# SYNOPSIS

*keyring set* _name_

*keyring get* _name_

*keyring list*

*keyring remove* _name_

*keyring unlock*

*keyring lock*

# DESCRIPTION

Your keyring keeps secrets such as tokens for git and other services,
the passphrases of your SSH keys and Wi-Fi passwords, each as an entry
with a name. *authd*(8) keeps it (ADR 0393), in a file sealed under a
key made from your password, which holds no secret, and no entry's
name, in the clear.

authd opens your keyring when you sign in with your password, at a
console or over SSH, and makes it then if you have none. It stays open
while you are signed in, and closes when your last session ends. A
session that started without your password, as one over SSH with a key,
opens it with *keyring unlock*.

An account with no password has no keyring: nothing could lock it.

Two helpers keep their secrets in it themselves: git's tokens for
https, as *git* _protocol_*://*_user_*@*_host_ entries
(*git-credential-keyring*(1)), and the passphrases of your SSH keys, as
*ssh* _path_ entries (*ssh-askpass-keyring*(1)).

# COMMANDS

*set* _name_
	Keeps a secret as the entry _name_, in place of one of that name.
	At a terminal it asks for the secret, unseen; otherwise the secret
	is the whole of the standard input, without its last newline.

*get* _name_
	Writes the secret of the entry _name_. A newline follows it only
	when the output is a terminal.

*list*
	Writes the entries' names, one to a line.

*remove* _name_
	Removes the entry _name_.

*unlock*
	Opens your keyring with your password, asked unseen at a terminal,
	or as a line of the standard input. A wrong password is answered
	only after a wait, as at a sign-in.

*lock*
	Closes your keyring until you unlock it, or sign in with your
	password again.

An entry's name is up to 256 bytes of text, with no control characters;
a secret is up to 16 KiB, of any bytes.

# YOUR PASSWORD

Changing your password with *passwd*(1) seals the keyring under the new
one, and its entries stay. When an administrator sets your password
without the old one, the keyring opens only if it is open then, as
while you are signed in; otherwise it is emptied, and *passwd* says so.
No one else holds its key, so a forgotten password loses the keyring:
there is no way back in, by design.

# EXAMPLES

```
$ printf '%s' "$TOKEN" | keyring set example-api
$ keyring get example-api
$ keyring list
example-api
```

# EXIT STATUS

0 when it was done; 1 when it was not, saying why, as when the keyring
is locked or has no such entry.

# SEE ALSO

*passwd*(1), *authd*(8), *git-credential-keyring*(1),
*ssh-askpass-keyring*(1), *systools*(1)
