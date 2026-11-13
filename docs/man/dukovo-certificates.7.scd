dukovo-certificates(7) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

dukovo-certificates - the certificate authorities Dukovo trusts, and TLS

# DESCRIPTION

A program that connects with TLS, as *curl* does to an *https://*
address, checks the server's certificate against the root certificates
of the authorities it trusts. Dukovo trusts the authorities Mozilla
trusts for TLS servers, the list Firefox and most Linux systems use, as
the Common CA Database (CCADB) publishes it (ADR 0192). They are one
file:

_/system/resources/certificates/ca-certificates.crt_

Each certificate is in PEM, after a line with its name:

```
# ISRG Root X1
-----BEGIN CERTIFICATE-----
MIIFazCCA1OgAwIBAgIRAIIQz7DSQONZRGPgu2OCiwAwDQYJKoZIhvcNAQELBQAw
...
-----END CERTIFICATE-----
```

The file comes with the system and changes only with an update. Programs
from Linux look for it by the names Debian and Alpine give it, which the
default settings have as symbolic links to it:

_/etc/ssl/certs/ca-certificates.crt_
	Debian's name, which curl, Go's programs and OpenSSL's on Debian
	read.

_/etc/ssl/cert.pem_
	Alpine's and the BSDs' name.

_/etc_ is _/system/settings_, so these are
_/system/settings/ssl/certs/ca-certificates.crt_ and
_/system/settings/ssl/cert.pem_.

# TRUSTING OTHER AUTHORITIES

To trust an authority of your own, such as a company's, as well as
Mozilla's, make the settings' file a copy of the system's with your
authority's certificate added, in place of the link:

```
cd /system/settings/ssl/certs
cat /system/resources/certificates/ca-certificates.crt my-ca.pem >ca-certificates.crt.new
mv ca-certificates.crt.new ca-certificates.crt
ln -sf certs/ca-certificates.crt ../cert.pem
```

To trust fewer, leave out the ones you do not want. A file of your own
does not change with updates: to follow Mozilla's list again, make it a
link once more:

```
ln -sf /system/resources/certificates/ca-certificates.crt /system/settings/ssl/certs/ca-certificates.crt
```

# THE MACHINE'S OWN CERTIFICATES

The certificates above are the ones this machine trusts when it connects
to others. The ones it presents when others connect to it, such as a
browser to the web console or to *nginx*, are kept by *certd*(8) and
listed by *cert*(8), each under a name, in _/system/state/certd_ (ADR
0512). A service's file names the ones it serves
(*service.toml*(5)), and Spark gives it a copy of the chain and the key,
read-only, and has it read them again when one changes.

*this-machine* is the machine's own: a certificate certd makes for the
machine's name, its name with _.local_ and _localhost_, valid for 800
days and made again 30 days before it runs out, or when the machine is
renamed. Nothing but the machine vouches for it, so a browser warns of
it, as it does of any certificate of one's own. To trust it, compare
what the browser shows with what the machine says, so that nobody
between can have put a certificate of their own in its place:

```
cert show this-machine
```

shows its SHA-256 fingerprint, as _AB:CD:..._, as browsers do. Trust it
in the browser once they agree. A remade certificate has a new
fingerprint, which the machine's sign-in notice tells of, and the
browser asks again.

*cert show this-machine --pem* prints the certificate itself, which a
program takes as the only authority it trusts, as in
*curl --cacert* _file_ _https://name/_. It checks that the server is
this machine, whose names the certificate holds, and no other.

A certificate from an authority, for a name the internet knows, goes in
with *cert add*, which refuses what the machine's authorities above
would not vouch for, and says which intermediate certificate a chain
lacks. The authorities it checks against are the settings' file of this
page's first section, so an authority of your own, added as above, is
trusted there too.

# TLS

Dukovo's programs in Rust use rustls on ring for TLS, each linked into
the program, and programs in C use rustls through rustls-ffi. Both
speak TLS 1.2 and 1.3, with AES-GCM and ChaCha20-Poly1305, X25519, P-256
and P-384, and ECDSA, Ed25519 and RSA certificates. There is no OpenSSL.

# FILES

_/system/resources/certificates/ca-certificates.crt_
	The root certificates Dukovo trusts.

_/system/resources/licenses_
	Their license, the Community Data License Agreement - Permissive
	2.0, with the license manifest.

# SEE ALSO

*cert*(8), *certd*(8), *network*(5), *netd*(8), *spark*(8)
