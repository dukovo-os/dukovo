<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Kerberos

Many offices and universities sign people in with Kerberos: a realm's
KDC, MIT's, Heimdal's, Samba's or Active Directory's, gives each person
a ticket when they prove who they are, and services take that ticket
instead of a password. Dukovo is a Kerberos client of such a realm. The
full references are kinit(1), klist(1), kvno(1), kdestroy(1) and
krb5.conf(5).

## Joining a realm

An administrator names the realm and its KDCs in
`/system/settings/krb5.conf`, in MIT's format, so a Linux host's file
works as it is:

```
[libdefaults]
    default_realm = EXAMPLE.ORG

[realms]
    EXAMPLE.ORG = {
        kdc = kdc1.example.org
        kdc = kdc2.example.org
    }

[domain_realm]
    .example.org = EXAMPLE.ORG
```

Dukovo does not look KDCs up in DNS: each realm names its own.

## Tickets

```
$ kinit alice
Password for alice@EXAMPLE.ORG:
$ klist
Ticket cache: keyring
Default principal: alice@EXAMPLE.ORG

Valid starting           Expires                  Service principal
2026-11-06 08:34:54 UTC  2026-11-06 18:34:54 UTC  krbtgt/EXAMPLE.ORG@EXAMPLE.ORG
	Flags: IA
$ kvno nfs/server.example.org
nfs/server.example.org@EXAMPLE.ORG: kvno = 2
$ kdestroy
```

Your tickets are kept in your keyring, each as the entry
`krb5 CLIENT SERVER`, not in a file under /tmp as on Linux. They close
with the keyring when your last session ends, so a ticket is never left
behind for someone else to find. `kinit` for another principal replaces
the tickets of the one before.

## A machine's services

A service on this machine, as an NFS server, proves itself with a
keytab: its keys in a file, which the KDC's administrator makes with
`kadmin -q "ktadd -k nfs.keytab nfs/HOST"`. Copy it to
`/system/settings/krb5.keytab`, readable by root alone, as it holds keys
as a password would:

```
$ elevate install -m 600 nfs.keytab /system/settings/krb5.keytab
$ elevate klist -k -e
```

`kinit -k -t FILE PRINCIPAL` signs in with a keytab's key instead of a
password, which shows that the keytab and the KDC agree.

## Troubleshooting

- **"no KDC answered"**: the error names each KDC and why. A refused
  connection means no KDC listens there; a timeout, that a firewall is
  in the way. Check the `kdc` lines of krb5.conf, and `ping` the host.
- **"the password or key is wrong"**: for a keytab, compare the key's
  version in `klist -k` with the one `kvno` reports from the KDC; a
  newer one at the KDC means the keytab is out of date.
- **"this computer's clock is more than five minutes from the KDC's"**:
  Kerberos refuses a clock that far off. `timedatectl` shows the clock,
  and timed sets it from the network.
- **"names no realm"**: set `default_realm` in krb5.conf, or write the
  realm after the name, as `alice@EXAMPLE.ORG`.
- **"takes none of the AES encryption types"**: the principal has only
  DES or RC4 keys, which Dukovo does not speak; the KDC's administrator
  gives it AES keys by changing its password or with `kadmin cpw
  -randkey`.
- **"your keyring is locked"**: kinit keeps the tickets in your keyring;
  `keyring unlock` opens it.
- **What the KDC saw**: its log records each request and why it refused
  one; MIT's is the file `[logging]` names in its kdc.conf.
