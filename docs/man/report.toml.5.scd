report.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

report.toml - where problem reports are sent

# SYNOPSIS

_/system/settings/report.toml_

# DESCRIPTION

*reportd*(8) sends the problem reports that people approve to a reports
server, and encrypts the ones for the security team to the keys of that
team (ADR 0531). This file names the server, the authorities that signed
its certificate when the system does not know them, and the file of those
keys. The file is TOML. The system works without it: reports go to the
project's server.

reportd reads the file when it starts; *service reportd restart*
(*service*(8)) makes it read the file again. A key it does not know, and a
value it cannot use, are logged with what and where (*log reportd*) and
leave the default.

```
server = "https://reports.example.org:8443"
ca = ["/system/settings/ssl/reports-ca.pem"]
security-recipients = "/system/settings/report-security.txt"
```

The file holds no secret: a report is not signed, and has no account. It
is for a machine whose reports are to go to a server of its own, such as a
company's, or to a staging server that tests the reports of a build.

# SETTINGS

*server =* _text_
	The reports server, as *https://* and a host name or address, and
	a port if it is not 443, with no path: *https://reports.dukovo.org*,
	which is the default. Reports are sent over HTTPS only. reportd
	checks that the server's certificate is valid for the host and was
	signed by an authority the system trusts, or by one that *ca* names;
	a server whose certificate is not is refused, and the log says why.

*ca =* _list_
	The whole paths of files of certificates, in PEM, of authorities
	that may sign the server's certificate besides the ones the system
	trusts, as a list of text. It is for a server of an organization's
	own, or for a staging server whose authority is made for a test.
	The system's own roots stay trusted; *dukovo-certificates*(7) says
	how to trust an authority for every program instead.

*security-recipients =* _text_
	The whole path of a file of the keys that a report for the security
	team (*report --security*) is encrypted to, with age
	(https://age-encryption.org), before it is queued or sent: public
	keys of X25519, which begin with *age1*, one on each line. Only the
	holders of the matching secret keys can read the report. The default
	is _/system/resources/report/security-recipients.txt_, which the
	system does not bring yet; until the file exists or this setting
	names another, *report --security* refuses and says to write to
	security@dukovo.org instead.

# FILES

_/system/settings/report.toml_
	This file.

_/system/resources/report/security-recipients.txt_
	The security team's keys, a key on each line, when the system
	brings them.

# SEE ALSO

*reportd*(8), *report*(1), *service*(8), *dukovo-certificates*(7),
*dukovo-privacy*(7)
