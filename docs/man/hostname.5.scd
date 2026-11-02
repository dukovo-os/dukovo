hostname(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

hostname - the machine's name

# SYNOPSIS

_/system/settings/hostname_

# DESCRIPTION

The first line of this file is the machine's name, as Linux's
_/etc/hostname_ is (ADR 0338): *gethostname*(2), *uname*(2) and
*uname -n* give it, and *discoverd*(8) answers for it on the local
networks as _NAME.local_. Spaces, tabs and a carriage return at the
end of the line are left out. Without the file, or with an empty first
line or one longer than 64 bytes, the name is *dukovo*.

*hostname* _name_, which calls *sethostname*(2), writes the file: an
administrator may, as the file is the system's settings, and anyone
else is refused (*EPERM*, as on Linux). A name longer than 64 bytes, or
with a new line in it, is refused (*EINVAL*).

A new name is the machine's at once for every program that asks; on the
local networks after *service discoverd restart* (*service*(8)).

```
$ hostname
dukovo
$ sudo hostname studio
$ uname -n
studio
$ cat /system/settings/hostname
studio
```

# FILES

_/system/settings/hostname_
	The machine's name.

# SEE ALSO

*discoverd*(8), *network*(5), *resolved*(8), *sudo*(8)
