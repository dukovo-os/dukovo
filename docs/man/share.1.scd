share(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

share - share folders with Windows PCs and other computers

# SYNOPSIS

*share* [*list*]

*share add* [*--read-only*] [*--comment* _text_] _name_ _folder_

*share remove* _name_

*share on*

*share off*

*share password* [*--clear*] [_name_]

# DESCRIPTION

share shares folders with Windows PCs, Macs and Linux machines on the
network, which *smbd*(8) serves over SMB 3, signed and encrypted (ADR
0228). A PC opens a share as _\\\\machine\\name_, or _smb://machine/name_,
and signs in with the name of a person's account here and their sharing
password; it then sees the share as that person would, with what their
own permissions let them read and change.

*list*, or no command, says whether sharing is on and lists the shares:
each one's name, folder, whether it is read-write or read-only, and
comment.

*add* shares _folder_ as _name_, to read and write, or only to read with
*--read-only*, with *--comment*'s _text_ shown beside it in the PC's
list. A share's name is what the PC asks for, in any case: up to 80
characters, none of *\\ / : \* ? " < > | [ ] ; = + ,*, not starting or
ending with a space, and not *IPC$*. A folder may be shared under more
than one name.

*remove* stops sharing _name_. A PC connected to it is told the share is
gone at its next request.

*on* starts *smbd*(8) and has Spark start it at each boot: it listens on
port 445 from then on. *off* stops it, which ends every share session,
and has Spark leave it off; port 445 is then closed. Sharing is off
until *share on*.

*password* sets the sharing password of _name_, by default your own,
after asking for your login password: the password PCs sign in to the
shares with. It is kept apart from your login password, and only as
the NT hash that NTLMv2 needs, which *authd*(8) alone reads; a PC that
learned it would still not sign in to Dukovo. An empty one is refused.
*--clear* removes it, which ends your share sessions: no PC signs in as
you until it is set again. Root sets and clears anyone's without
theirs.

*add*, *remove*, *on* and *off* change the system's settings, and need
the administrator: run them with *elevate*(8). Anyone lists the shares,
and sets their own sharing password.

The new password is read from the terminal without showing it, twice,
or from the standard input, a line each.

# EXIT STATUS

0 when share did what was asked; 1 when it could not, or for a mistake
in the command line, with why on the standard error.

# EXAMPLES

Share the Public folder of Ann's home with the network, and turn sharing
on:

```
$ elevate share add --comment "Ann's things" Public /users/ann/Public
Shared /users/ann/Public as Public, read-write.
Sharing is off; `share on` turns it on.
$ elevate share on
Sharing is on: smbd(8) serves the shares on port 445, and starts at each boot.
Each person signs in with their sharing password: `share password`.
$ share password
Setting the sharing password of ann, which PCs sign in to the shares with.
Your login password:
New password:
The same again:
The sharing password of ann is set.
```

On a Windows PC, open _\\\\dukovo\\Public_ in Explorer, or map it:

```
C:\> net use P: \\dukovo\Public /user:ann
```

From Linux:

```
$ sudo mount -t cifs -o user=ann,seal //dukovo/Public /mnt/public
```

# FILES

_/system/settings/shares.toml_
	The shares (*shares.toml*(5)).

# SEE ALSO

*smbd*(8), *shares.toml*(5), *smb*(1), *authd*(8), *elevate*(8)
