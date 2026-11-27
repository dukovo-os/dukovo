<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0782. The update server's online keys stay files that only their jobs read

- Status: Accepted; changes where [0457](0457-releases-are-signed-on-two-hardware-keys-after-three-builds-agree.md)'s table keeps the snapshot, timestamp and rollout keys, and settles what [0382](0382-the-update-server-publishes-what-the-signer-deploys-over-ssh.md) and [0467](0467-the-update-server-serves-the-systems-releases-and-signs-only-their-snapshot-and-timestamp.md) left open until 1.0.0-alpha.1
- Date: 2026-11-27

## Context

The update server holds two online keys, the catalog's (ADR 0382) and
the system releases' (ADR 0467). Each signs snapshots, timestamps and
rollouts with nobody there: a timer every six hours, the canaries' job
every five minutes, and the publish job after each deploy. ADR 0457's
table keeps them "encrypted at rest". Each is a file only its jobs read:
on an Ubuntu server, readable by its own system user alone; on a Dukovo
server, the secret of its jobs' services (ADR 0542). Whether to encrypt
them was left to be decided before 1.0.0-alpha.1.

A stolen online key can sign snapshots and timestamps, so whoever serves
them can keep old releases current for a machine or hold and open the
waves; it cannot add a release, a channel move or a file, which the
release keys sign (ADR 0467).

## Decision

- **The online keys stay files that only their jobs read**, as they are.
- **They are not encrypted at rest.** The server signs with nobody there,
  so it would decrypt them itself, with a key kept on the same disk. That
  guards only a copy of the disk taken without the running system, and
  such a copy can be kept without the keys instead.
- **A copy of the server's disk is the server.** A snapshot or backup of
  the server holds its keys, and is kept in the same hosting account; a
  copy that leaves it is a stolen key.
- **A stolen online key is replaced by the next root** (ADR 0780), as the
  handbook's chapter on keys and incidents says.

## Consequences

- ADR 0457's table reads, for snapshot, timestamp and rollout: the update
  server, in files only its jobs read.
- Nothing about the server changes.

## Alternatives

- **Encrypted with a passphrase typed at each start** of the server: a
  restart would stop the signing until someone typed it, and the
  timestamps run out in a day, after which every machine refuses the
  server's updates.
- **A hardware key on the server**: the server is a cloud machine with no
  hardware key, and a hosted one is another account to keep, and a cost.
