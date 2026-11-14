admin.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

admin.toml - the sign-in rules and the address of the web console

# SYNOPSIS

_/system/settings/admin.toml_

# DESCRIPTION

*authd*(8) reads this file for the rules of signing in to the web console
(ADR 0514), each time someone signs in, and *admind*(8) for where it
answers and whom, and for the same rules to end sessions, every 30
seconds. The file that comes with the system holds only comments, so the
defaults apply until a setting is uncommented. The file is TOML, in two
tables. *service admind reload* (*service*(8)) has admind read the
certificate again, and a new port takes *service admind restart*.

A value out of its range, or a name that is not a setting, is refused, so
that a typo never loosens a rule unnoticed: authd then refuses sign-ins,
and admind goes on with the settings it has, and says why in the log
(*log admind*).

```
[sign-in]
second-step = "if-set"
session-hours = 12
idle-minutes = 30

[console]
listen = 9090
reach = "private"
certificate = "this-machine"
```

# THE [sign-in] TABLE

*second-step =* _text_
	Who must give a second step after the password, an authenticator
	app's code or a passkey: *"if-set"* (those who have added one,
	the default), *"administrators"* (them, and those who have added
	one) or *"everyone"*. An account that the rule makes give a step
	and that has none may only add one, and signs in again once it
	has.

*session-hours =* _number_
	The hours a session lasts after its sign-in, 1 to 168; 12 by
	default.

*idle-minutes =* _number_
	The minutes a session lasts with no console page open, 5 to 1440;
	30 by default.

# THE [console] TABLE

*listen =* _number_
	The port, 443 or 1024 to 65535; 9090 by default. The service file
	lets admind listen on 9090 only: change *network.listen* in it
	with *service edit admind* too (*service.toml*(5)).

*reach =* _text_
	Whom admind answers: *"private"*, the default, this machine, the
	networks of its cards and the address ranges no one reaches from
	the internet, or *"any"*. *"any"* asks for care: give
	administrators a second step first, with *second-step =
	"administrators"*.

*certificate =* _text_
	The certificate it serves, by certd's name for it (*cert*(8)
	*list*): *"this-machine"*, the default, which a browser warns about
	once, or one from an authority for the console's own name. The
	service file's *certificates* must name it too.

# SEE ALSO

*admind*(8), *authd*(8), *cert*(8), *service*(8), *service.toml*(5)
