git-credential-keyring(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

git-credential-keyring - keep git's tokens in your keyring

# SYNOPSIS

*git credential-keyring* *get*|*store*|*erase*

# DESCRIPTION

*git-credential-keyring* is git's credential helper on your keyring
(*keyring*(1), ADR 0394), and the one the system's gitconfig names,
in _/system/settings/gitconfig_. git runs it itself; you do not.

When a push or a fetch over https asks for a user name and a token,
git asks you once and, once the server takes it, has the helper keep
it in your keyring, sealed under your password. From then on git gives
it again without asking, after a restart too, until the server refuses
it: git then has the helper forget it, and asks again.

Each credential is an entry of your keyring named *git*
_protocol_*://*_user_*@*_host_, with */*_path_ after it when git's
*credential.useHttpPath* is on, holding the token. *keyring list*
shows them, and *keyring remove* forgets one.

When your keyring is locked, as in a session over SSH that started
with a key, the helper says so and git asks for the token, which it
does not keep: *keyring unlock* opens the keyring first.

A *~/.gitconfig* of your own may name another helper, such as git's
memory cache, *cache --timeout=28800*, or none.

# COMMANDS

As *gitcredentials*(7) describes them, with a credential on the
standard input:

*get*
	Writes the user and the token of the entry for the credential's
	address: its user's, or the first one when git names no user.

*store*
	Keeps the credential's token as the entry for its user and address.

*erase*
	Forgets the entries for the credential's address, but only one
	whose token is the one git found wrong.

# EXIT STATUS

0, as git expects of a helper, which says on the standard error what
it could not do.

# SEE ALSO

*keyring*(1), *git*(1), *gitcredentials*(7), *git-credential-cache*(1)
