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

## NFS with Kerberos

Kerberos is how NFS proves who calls, rather than taking the client's
word: `sec=krb5` checks each person's ticket, `krb5i` also signs each
call and reply, and `krb5p` also seals them, so the network sees
nothing of the files. Use `krb5p` unless a server cannot.

To mount a folder a server shares with Kerberos, take a ticket and
mount by the server's name, which its service `nfs/NAME` is named by:

```
$ kinit
$ mkdir ~/home
$ elevate mount -t nfs -o sec=krb5p nas.example.org:/srv/home ~/home
```

Each person's calls go under their own ticket from then on: someone who
has none, or whose ticket ran out, is refused until they `kinit`. If the
name does not resolve, add `addr=` with the server's address; the name
still names the service.

To share a folder with Kerberos, the machine needs the keytab of its
own `nfs/NAME` (above), where `NAME` is how clients name it. Then:

```
$ elevate share add --nfs 192.168.1.0/24 --nfs-sec krb5p Home /users/alice
$ elevate share nfs on
```

A Linux client mounts it with `mount -t nfs4 -o sec=krb5p NAME:/Home
/mnt`, with rpc.gssd running and a ticket of the person's. A principal
of one part in the machine's realm, `alice@EXAMPLE.ORG`, is served as
the account of that name; any other, the client machine's own among
them, as no one. nfsd(8) and nfs(1) say more.

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
- **An NFS mount says "Permission denied"**: the person has no ticket,
  or it ran out: `klist` shows, `kinit` takes another. "the server
  refused ...'s ticket" means the server has no key for the ticket's
  version: its keytab is out of date (`klist -k` there, `kvno
  nfs/NAME` here).
- **"the server takes another security flavor for this folder"**: the
  share takes Kerberos and the mount did not ask for it, or the other way
  round: mount with the `sec=` the share lists (`share list` shows it on
  Dukovo, `exportfs -v` on Linux).
- **A Linux client's mount of a Dukovo share hangs or is refused**:
  nfsd's log names each context it refused (`log nfsd`), most often a
  keytab without the key for the name the client used; on the client,
  rpc.gssd's log (`journalctl -u rpc-gssd`) says which ticket it looked
  for.
- **What the KDC saw**: its log records each request and why it refused
  one; MIT's is the file `[logging]` names in its kdc.conf.
