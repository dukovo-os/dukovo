sshd_config(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

sshd_config - how people may sign in over SSH, and who

# SYNOPSIS

_/system/settings/ssh/sshd_config_

# DESCRIPTION

*sshd*(8) reads this file as it starts, and again when *service sshd
reload* asks it to, for the connections that come after (ADR 0348).
The file is OpenSSH's, with the keywords below; the default one says
nothing but comments, so keys alone sign in, and everyone with a
person's account may:

```
# Passwords sign in too, but only alice's and bob's.
PasswordAuthentication yes
AllowUsers alice bob
```

Each line is a keyword, in any case, and its values, after spaces or
one *=*. A value in double quotes may hold spaces. A line, or a value,
that starts with *#* starts a comment. A keyword given twice keeps its
first value, as OpenSSH's sshd keeps it; the names of *AllowUsers* and
the others add up.

A keyword sshd does not carry out, such as *Port*, *Match* or
*X11Forwarding*, or a value it does not take, is a mistake: sshd says
which line, and does not start until the file is right; a reload with a
mistake in the file keeps the settings sshd had, and the log says why.
OpenSSH's sshd would carry such a line out, so it is not passed over:
a line meant to keep people out never does nothing. *sshd -t* checks
the file before a reload, and *sshd -T* shows what it says.

# KEYWORDS

*PasswordAuthentication* *yes*|*no*
	Whether a person's password signs them in. The default: *no*,
	keys only. A password is checked by *authd*(8), as at the console,
	and a wrong one waits as one there does, counted for the
	client's address; *lastb*(1) shows it, and its person is told at
	their next sign-in. An empty password never signs anyone in over
	SSH, as OpenSSH's *PermitEmptyPasswords no* keeps it.

*PubkeyAuthentication* *yes*|*no*
	Whether a key in the person's _~/.ssh/authorized_keys_ signs them
	in (*sshd*(8)). The default: *yes*.

*DenyUsers* _name_...
	No one with one of these names signs in.

*AllowUsers* _name_...
	Only people with one of these names sign in, when the keyword is
	given at all.

*DenyGroups* _group_...
	No one in one of these groups signs in: their own group, or one
	that lists them in _/system/settings/group_.

*AllowGroups* _group_...
	Only people in one of these groups sign in, when the keyword is
	given at all.

The names and groups are patterns: *\** matches any characters and
*?* any one, so *AllowGroups admins* lets the administrators in, and
*DenyUsers test\** keeps out every name that starts with _test_. They are
checked in the order above, as OpenSSH checks them, before a key or a
password is. OpenSSH's _name_*@*_address_ patterns are not taken yet.

Root never signs in over SSH, whatever this file says: an
administrator signs in as themselves and uses *elevate*(8).

# FILES

_/system/settings/ssh/sshd_config_
	The file.

_/system/resources/default-settings/ssh/sshd_config_
	The system's own, which Spark copies where it is missing
	(*spark*(8)).

# SEE ALSO

*sshd*(8), *authd*(8), *service*(8), *ssh_config*(5)
