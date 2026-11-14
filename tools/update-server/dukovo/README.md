<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# The update server on Dukovo

The same duties as the Ubuntu server (`../setup.sh` and the units beside
it, ADRs 0382, 0467 and 0469), done by Dukovo's own services (ADR 0542).
The Ubuntu files stay until the move to Dukovo is done
([`docs/runbooks/update-server-move.md`](../../../docs/runbooks/update-server-move.md)).

## Setting it up

On a Dukovo machine with certd, from the laptop with the source tree:

```
./ob catalog server setup --dukovo     # also builds catalog-repo for Dukovo
./ob release server setup --dukovo     # the same files, by the other command
```

Either copies this folder and the scripts above it with `scp` and runs
`elevate sh dukovo-update-server/dukovo/setup updates.dukovo.org` over
ssh, so `elevate` asks for the password of the login that deploys.
`OB_UPDATE_SSH` names another login or machine. On the machine itself:

```
elevate sh dukovo-update-server/dukovo/setup HOST [--keys DIR]
```

`setup` can be run again: it puts the scripts and service files in again,
leaves each online key and each folder as it is, and starts what is not
running. `--keys DIR`, or the folder `dukovo-update-keys` in the deploying
login's own `/tmp` where it is (`/tmp` is in memory, and each login has
one: `elevate` has the system's, so `setup` looks at the login's at its
real place, `/system/live/authd/tmp/NUMBER`), has it take the online keys
of another server in place of making new ones: `catalog.key` and `system.key`, each the key as
base64, and `catalog.json` and `system.json`, each its public half. The
folder must be the deploying login's, mode 700, and is removed once the
keys are in. It makes the keys, installs nginx from the catalog, writes
nginx's settings, asks certd for HOST's certificate and enables the jobs.
When HOST does not lead to the machine yet, certd cannot get the
certificate: `setup` serves over HTTP and says so, and running it again
once HOST leads here turns HTTPS on.

## What runs

Every job is a service of Spark (`service.toml(5)`) with a `[timer]`
(`timerd(8)`). None has an account of its own: Dukovo has no way to make
an account that no one signs in to, so each runs as root without root's
right to pass owners and modes, in a view of the tree that holds only
what its file names. Each reads only the secret named below, and only the
folders in its file.

| Service | When | Secret | Does |
| --- | --- | --- | --- |
| `dukovo-system-publish` | each minute | `online-key` | Publishes the system releases a deploy put in `dukovo-system-incoming` once `ready` is there; ends at once without it |
| `dukovo-catalog-publish` | each minute | `online-key` | The same for the catalog and its bundles |
| `dukovo-system-refresh` | every 6 hours | `online-key` | Signs the system releases' timestamp and rollouts again |
| `dukovo-catalog-refresh` | every 6 hours | `online-key` | Signs the catalog's timestamp again |
| `dukovo-catalog-flaws` | daily, 04:00 | `online-key` | Signs the list of the bundles' known flaws from what the next job fetched |
| `dukovo-flaws-fetch` | daily, 03:00 | none | Fetches OSV's advisories: the only job with the network, and it cannot read a key |
| `nginx` | always | none | Serves `/catalog/` and `/system/` (the bundle `org.nginx.nginx`) |

The online keys are two: the catalog's, in the secrets of the three
catalog jobs, and the system releases', in those of the two system jobs.
A secret holds the key as base64, since a secret loses a trailing newline
and a key is binary. Their public halves, which the roots name, are in
`/var/lib/dukovo-catalog/online.json` and `/var/lib/dukovo-system/online.json`
(`/var/lib` is `/system/state/lib`).

To see each:

```
service dukovo-system-publish status    # state, last lines of its log, its Timer line
elevate service list --timers           # when each starts next and when it last did
elevate log dukovo-system-publish | tail
elevate service secret list dukovo-system-publish
```

A publish that fails writes why to `result` in its incoming folder, which
the deploy shows, and to the log. A job that is not running as asked is
in `service list --failed`.

## Where things are

| What | Where |
| --- | --- |
| Scripts, `catalog-repo` | `/var/lib/dukovo-update-server` |
| The catalog's repository, store and key's public half | `/var/lib/dukovo-catalog` (`repo` is closed to everyone but the jobs) |
| What is served at `/catalog/` | `/var/lib/dukovo-catalog-site/catalog`, a link the publish job moves |
| What a deploy of the catalog copies | `/var/lib/dukovo-catalog-incoming`, the deploying login's, mode 2770 |
| The system releases' files and plans | `/var/lib/dukovo-system`, `dukovo-system-site`, `dukovo-system-incoming` |
| Advisories | `/var/lib/dukovo-flaws` |
| nginx's settings | `/shared/services/org.nginx.nginx/nginx/conf` and `/system/settings/spark/services/settings.d/nginx.toml` |
| The locks | `/system/live/dukovo-update-server`, which Spark makes and a restart empties |

Change a service file with an override file (`service.toml(5)`,
`settings.d`), not in place: `setup` writes the files again. To deploy
for another login, `chown` the two incoming folders to it.

## The programs the scripts call

`system-release` is in Dukovo's image, one of the commands of `systools`
(`/system/commands/system-release`). `catalog-repo` is not: it is a
program of the machine that builds the catalog, and the image carries
only what a computer runs. `ob catalog server setup --dukovo` builds it
for Radix on the laptop (`userspace/programs/catalog-repo`) and copies it
with the scripts, so the server needs no compiler and builds nothing.

## What the scripts changed for Dukovo

The scripts run as they are under Nukesh's POSIX mode, with these changes,
none of which an Ubuntu server notices:

- `site.sh`, `system-site.sh`: where things are comes from `DUKOVO_*`
  variables, which the service files set, with the Ubuntu places as the
  default; the lock is a folder made with `mkdir` where there is no
  `flock` (Dukovo has none), taken over after 30 minutes; the key is
  read from the job's secret and decoded into a private folder that the
  script takes away, where the Ubuntu server has `online.key`; a `trap`
  takes both away; `$(dirname "$0")` finds the other script.
- `publish.sh`, `system-publish.sh`: end at once when there is no `ready`,
  as a timer starts them, instead of a `.path` unit; `result` is made
  group readable, since the deploy that waits for it is another account.
- `refresh.sh`, `flaws.sh`, `flaws-fetch.sh`: the same paths.
- `system-rollout` and `catalog-rollout`, here: name the Dukovo places
  for `system-rollout.sh` and `catalog-rollout.sh`, which `ob release
  rollout` and `ob catalog rollout` run through `elevate`; `site.sh`
  then reads the catalog's online key from the refresh job's secret.

## Departures from the design

- No accounts `dukovo-catalog`, `dukovo-system`, `dukovo-flaws` and no
  group `dukovo-deploy`: `user add` makes an account that a person signs
  in to, with a password and a home, and Dukovo has no system accounts or
  group tool. The jobs run as root without its bypass; the incoming
  folders belong to the deploying login, with the group root.
- The catalog's bundles are named by id and version (`ID-VERSION.dkv`),
  not by their SHA-256; `ob` sends only the names the server lacks,
  checks what the server answers against that form, and a deploy
  re-sends a bundle whose name it does not know.
