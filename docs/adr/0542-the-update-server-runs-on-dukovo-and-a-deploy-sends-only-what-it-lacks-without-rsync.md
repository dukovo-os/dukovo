<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0542. The update server runs on Dukovo, and a deploy sends only what it lacks, without rsync

- Status: Accepted
- Date: 2026-11-14

## Context

updates.dukovo.org serves the catalog (ADR 0382) and the system releases
(ADRs 0467 and 0469) from an Ubuntu virtual machine: Caddy for HTTPS,
systemd's path units and timers for the jobs that publish and sign
again, a system account for each online key, and rsync for the deploys.

Dukovo 1.0 is a server system with a web server, certificates it gets
and renews itself (certd), timers in service files (ADR 0478) and
secrets that only the services named may read. The project's own update
server is the first production server it can run, and the one whose
failure the project hears of first.

rsync is not on Dukovo, and a deploy needs no more than what the
server's names already say: the release files and the catalog's bundles
are named by what they hold, so a file the server has by name is the
same file.

## Decision

1. **The jobs are Spark's services.** Each of the six jobs of the Ubuntu
   server is a service file with a `[timer]`: the two publish jobs every
   minute, ending at once when no `ready` waits in their incoming folder;
   the two refresh jobs every six hours; the flaws jobs daily. The
   scripts are the Ubuntu server's, run by Nukesh's POSIX mode, with
   their places from `DUKOVO_*` variables that the service files set, and
   a lock made with `mkdir` where there is no `flock`.
2. **Each online key is a secret** of the jobs that sign with it
   (`service secret set`): the catalog's of the three catalog jobs, the
   system releases' of the two system jobs. The fetch of OSV's advisories
   is the only job with the network, and it reads no key. Each job runs
   in a view of the tree that holds only the folders its file names.
3. **nginx serves**, from the catalog's bundle, with the machine's
   certificate from certd; `setup` writes its settings and turns HTTPS on
   once certd has the certificate.
4. **The jobs run as root without root's right to pass owners and
   modes**, in their views, since an account outside a bundle is one a
   person signs in to. The incoming folders belong to the deploying login,
   with the group root, mode 2770.
5. **A deploy sends only what the server lacks.** `ob` asks the server
   over SSH for the names it holds, sends the files it lacks as one `tar`
   stream over the same SSH, and the server unpacks the stream into a new
   folder that takes the old one's place by renames only once the stream
   is whole. A file a name does not match the expected form for, a
   SHA-256 for release files and `ID-VERSION.dkv` for bundles, is sent
   again. The same deploy works to the Ubuntu server.
6. **`setup` sets a Dukovo machine up**, from `ob catalog server setup
   --dukovo` or `ob release server setup --dukovo`, and can run again.
   It makes the online keys, or takes another server's (`--keys`). The
   server builds nothing: `ob` builds `catalog-repo` for Dukovo and copies
   it with the scripts.
7. **The move keeps the version numbers.** A client refuses a snapshot
   or timestamp older than the newest it has read. The new server
   therefore starts from the old one's metadata, copied before its first
   deploy, so that its first snapshot and timestamp are newer than any a
   client holds; `docs/runbooks/update-server-move.md` says how, and how
   to go back.

## Alternatives

- **rsync on Dukovo.** A port to keep for one use, which the names the
  files already have make unneeded.
- **Accounts made for the jobs.** `user add` makes accounts that people
  sign in to, with a password and a home; bundled makes accounts only for
  the services of the bundles it installs. The jobs' views give them what
  the accounts gave them on Ubuntu, a narrow reach, by another means.
- **Keys in files closed to all but one account**, as on Ubuntu. A secret
  is given only to the services its file names, and `service secret
  list` shows where each is.

## Consequences

- The Ubuntu server's files stay until the move is done, and go in a
  later change.
- The boot test `runbook-update-server-move` follows the runbook on a
  test machine: setup, the keys taken from another server, a deploy of a
  test release and catalog, `ob release check` reading them back through
  TUF, a refresh signing a new timestamp, and a deploy without `ready`
  publishing nothing. The lab test `updates/server-on-dukovo` sets a
  machine up as the server over the lab network, carries the version
  numbers over from another server, and has a client accept it.
- ADRs 0382 and 0467 describe the Ubuntu server, which this replaces.
