logforward.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

logforward.toml - the syslog servers logforward sends the log to

# SYNOPSIS

_/system/settings/logforward.toml_

# DESCRIPTION

*logforward*(8) sends the system's log to the syslog servers this file
names (ADR 0482). There is no file until the administrator makes one, and
logforward sends nothing without it, or with one that names no server.
The file is TOML, a *[[destination]]* table for each server.
*service logforward reload* (*service*(8)) has logforward read it again;
it reads it when it starts too.

```
[[destination]]
name = "siem"
address = "logs.example.com"
transport = "tls"
level = "info"
names = ["sshd", "kernel"]
audit = false
ca = "/system/settings/ssl/siem-ca.pem"
server_name = "logs.example.com"
certificate = "/system/settings/ssl/siem-client.pem"
key = "/system/settings/ssl/siem-client.key"
```

# SETTINGS

*name =* _text_
	What *logforward status* and the log call the destination, and
	what its place in the log and its status are named after. Letters,
	digits, hyphens and underscores, at most 64 of them, and no other
	destination has it. It is needed.

*address =* _text_
	The server: a name or an address, with a port, _host_*:*_port_, or
	without. An IPv6 address goes in brackets when it has a port. The
	port is 6514 over TLS and 514 over TCP and UDP when it is not
	given. A name is looked up through *resolved*(8) when
	logforward connects. It is needed.

*transport =* _text_
	How messages reach the server: *tls* (the default), *tcp* or
	*udp*. TLS is RFC 5425, a connection that is checked and encrypted;
	TCP is RFC 6587's octet counting, which neither checks nor
	encrypts, for a server on a network the log may cross; UDP is RFC
	5426, which does not even tell whether a message arrived.

*level =* _text_
	The least urgent records sent: *error*, *warning*, *info* (the
	default) or *debug*. The records more urgent than it are sent too,
	so *info* sends everything but debugging.

*names =* _list_
	Only the records of these services and programs, by the name the
	log shows (*log*(1)), such as *["sshd", "kernel"]*; the kernel's own
	records are *kernel*. Every record is sent without it, and a list
	with nothing in it is a mistake.

*audit =* _true_ | _false_
	*true* sends only the security events (*logd*(8)), which it keeps in
	the audit log too. *false* is the default. It works together with
	*level* and *names*: a record has to pass each.

*ca =* _path_
	Over TLS, the PEM file of the certificates to trust for this
	server instead of the root certificates the system trusts
	(*dukovo-certificates*(7)): its certificate authority's, or its own
	certificate, to pin it. Only these are trusted for the destination
	when it is given.

*server_name =* _text_
	Over TLS, the name the server's certificate must be valid for. It
	is the host of *address* without it. Set it when *address* is an IP
	address or a name that is not on the certificate.

*certificate =* _path_, *key =* _path_
	Over TLS, the PEM files of a client certificate and its private
	key, which logforward shows to a server that asks for one. The two
	go together. Keep the key readable by root alone.

The files *ca*, *certificate* and *key* name must be in
_/system/settings/ssl_ or a folder in it, where the system keeps its
certificates and keys, so that a settings file cannot have logforward
read any other file as one. These three
and *server_name* are for *transport = "tls"* only. The server's
certificate is always checked: no setting turns that off.

logforward says what is wrong in the file in the system's log, a line
for each mistake that names the file, the destination and the setting,
and leaves out the destination it is in while the others go on. A
setting it does not know, a value of the wrong kind, a name or address
that cannot be used, a *level* that is not one of the four, and a file
outside the folder are mistakes, and so is a line that is not TOML, in
which case there are no destinations at all. *logforward test*
(*logforward*(8)) says them again.

# EXAMPLES

Everything to the company's collector over TLS, its certificate checked
against the company's authority, and a copy of the security events only
to a second one that pins its own certificate, whose name is not the
address:

```
[[destination]]
name = "siem"
address = "logs.example.com"
ca = "/system/settings/ssl/example-ca.pem"

[[destination]]
name = "audit-copy"
address = "10.0.0.20:6514"
server_name = "audit.example.com"
ca = "/system/settings/ssl/audit-server.pem"
audit = true
```

Warnings and errors to a machine that takes them over UDP, and only the
SSH server's and the kernel's records to another over TCP:

```
[[destination]]
name = "backup-box"
address = "10.0.0.9"
transport = "udp"
level = "warning"

[[destination]]
name = "ssh-watch"
address = "10.0.0.10:601"
transport = "tcp"
names = ["sshd", "kernel"]
```

# FILES

_/system/settings/logforward.toml_
	This file.

_/system/settings/ssl_
	Where the certificates and keys it names are.

# SEE ALSO

*logforward*(8), *logd*(8), *log*(1), *dukovo-certificates*(7),
*dukovo-privacy*(7), *service*(8)
