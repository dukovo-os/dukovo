<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Bundles and permissions

This chapter describes apps on Dukovo as of Phase 13e of the roadmap:
how they are installed, from a catalog or from a file, what they may
do, and how they update and go back. An app, a command-line tool or a
service comes as a bundle: one `.dkv` file, signed by its publisher,
which the `bundle` command installs, sealed and apart from the system.
The full references are bundle(1), bundled(8), manifest.json(5) and
service.toml(5).

## What works now

- **Installing from a catalog**, by the bundle's ID, with `bundle
  search` to find it; the catalog is a signed repository that vouches
  for each file it lists
  ([ADR 0383](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0383-bundled-installs-from-the-catalogs-an-administrator-trusts.md)).
- **Installing from a file**, signed by a key this computer may not
  know, after a warning
  ([ADR 0223](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0223-bundled-installs-seals-and-removes-bundles.md)),
  and a file with no signature only when you say so
  ([ADR 0391](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0391-an-unsigned-bundle-installs-only-with-an-override-that-signs-it-here.md)).
- **Permissions you give**, asked on the console's bottom line, which
  no program can draw or answer, and taken back at any time
  ([ADR 0224](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0224-grants-and-the-trusted-permission-prompt.md)).
- **A sandbox for each app**, which sees its own folders and what you
  gave it, and nothing else
  ([ADR 0225](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0225-bundles-run-in-a-sandbox-of-their-own.md)).
- **Updates, going back and holding**, with the app's data kept from
  before each update
  ([ADR 0384](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0384-bundles-update-from-their-catalog-go-back-and-hold.md)).
- **Several major versions side by side**, such as Node.js 20 and 22,
  each with its own add-ons
  ([ADR 0387](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0387-a-bundles-line-keeps-its-major-versions-side-by-side.md),
  [ADR 0388](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0388-a-lines-add-ons-are-each-persons-and-run-on-it.md)).
- **Known flaws**, which the catalog lists for each bundle and the
  daily check reads for what is installed
  ([ADR 0389](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0389-a-catalog-lists-its-bundles-known-flaws-which-the-daily-check-reads.md)).
- **Services**, which a bundle may run for everyone, under accounts of
  their own, and whose updates prove themselves or go back
  ([ADR 0385](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0385-a-bundles-services-run-as-accounts-of-their-own.md),
  [ADR 0386](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0386-a-service-bundles-update-proves-itself-or-goes-back-with-its-data.md)).
- **A feed of its own** for a bundle no catalog lists, so that it
  updates too
  ([ADR 0390](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0390-a-bundle-from-no-catalog-updates-from-a-feed-its-publisher-signs.md)).

Apps live outside `/system`: yours in `~/applications`, everyone's in
`/applications`. Updating an app never touches the system, and updating
the system never touches an app.

## Installing from a catalog

Dukovo comes with its own catalog, `dukovo`. Search it by any words of
a bundle's ID or name, then install by the ID:

```
$ bundle search ripgrep
ID                            VERSION  CATALOG  FLAWS  NAME
io.github.burntsushi.ripgrep  15.2.0   dukovo   none   ripgrep
$ bundle install io.github.burntsushi.ripgrep
```

bundle shows the install sheet before anything is installed: what the
bundle is and who publishes it, which catalog lists it, its size, the
permissions it asks for in plain words, its commands, and its known
flaws. It then asks `Install it? [y/N]`; `--yes` answers for scripts.
Installed, the bundle is sealed, root's and read-only, so nothing
changes it, and its commands are on your `PATH` at once: `rg` runs
ripgrep. Nothing is added to your shell's profile, and no environment
variable is set for everyone.

`bundle install` installs for you alone. An administrator installs for
everyone through elevate, `elevate bundle install --for-everyone ID`.
`bundle list` lists what you can run, yours and everyone's, and `bundle
info ID` shows a bundle's sheet again, with what you said of its
permissions.

An administrator can add other catalogs, each with the first root its
publisher gives, which every file the catalog serves is checked
against:

```
$ elevate bundle catalog add example https://apps.example.org/ example-root.json
$ bundle catalog
```

`bundle install ID --from example` takes a bundle from that catalog
when several list it. Once a bundle is installed from a catalog, its ID
installs only from that catalog's files, whoever offers another.

## Installing from a file

A `.dkv` file from a website or a friend installs too:

```
$ bundle install notes-1.0.0.dkv
```

When no catalog this computer trusts lists the file, the sheet says so,
and warns when the key that signed it is new here: the publisher's
name on the sheet is then its own word, which nothing has checked.
Install only what you trust. From then on, only that key may install
that ID, so another publisher cannot replace the app with one of their
own.

A file with no signature at all is refused: nothing says who made it,
or that it is as they made it. If you trust where it came from, install
it anyway with `--unsigned`, which you give for each such file:

```
$ bundle install --unsigned notes.dkv
notes.dkv is not signed: no publisher vouches for it, so nothing says who made it, or that it is as they made it.
signed here by your developer key RW…, which only this computer trusts, to install it; a later version of it needs --unsigned too
```

bundle signs a copy with your developer key and installs that, as your
own build. A file whose signature is there but fails is never
installed: someone changed it after it was signed.

## Permissions

An app runs in a sandbox of its own: it sees the system's programs and
libraries, read-only, its own folder of data, `~/app-data/ID`, and only
what you give it. It cannot see your files, other apps' data, or the
network unless it asks and you agree.

A permission is asked about the first time the app needs it, on the
console's bottom line, which only the console draws:

```
 [permission] Notes (org.example.notes) asks to: use your documents folder, to read and change it      O: once    A: always    D: don't allow
```

`O` allows it this time, `A` from now on, and `D` not. A permission that
reaches what other apps keep, such as all of your files, is marked
`[warning]`. Some permissions, such as connecting to the internet, are
given at install, where the sheet names them, unless you take them back.

```
$ bundle revoke org.example.notes files.folder
$ bundle grant org.example.notes files.folder documents=read
```

`bundle revoke` takes a permission away at once, stopping the app if it
holds it now. `bundle grant` gives one back, or gives less than the app
asks, such as your documents to read only; it asks you on the console
first, so no program can give itself anything.

Command-line tools such as ripgrep run with your own access, as on
Linux, because searching your files is their job. The sheet says so
plainly, and the first run of such a command asks, with a warning,
whether it may.

## Updates

```
$ bundle update --check
$ bundle update
```

`bundle update` updates your bundles, or the ones named, each from the
catalog it came from. A new version that asks for no more than the one
before installs without a question; one that asks for more shows its
sheet, with what is new marked, and waits for your yes. A new version
never gains a permission silently.

Bundles update only when you run `bundle update`, unless you turn on
updating by themselves, which runs daily:

```
$ bundle update --automatic on
```

An update that asks for more still waits for you then, and `bundle
history` says so.

The version an update replaced is kept, sealed, to go back to:

```
$ bundle rollback org.example.notes
```

Going back holds the bundle at that version, so no update undoes it;
`bundle unhold` lets it update again, and `bundle hold` holds one
yourself. The first time a new version runs, bundle keeps a copy of the
app's data as the old version left it, for 30 days; going back puts that
copy back. `bundle history` shows each install, update, going back,
hold and removal, and who asked.

## Known flaws

The catalog's server checks the parts each bundle is built from, which
the bundle lists in its SBOM, against the published advisories of known
vulnerabilities, every day. `bundle search` and `bundle list` show what
it found in the FLAWS column, such as `2, high`, and the sheet and
`bundle info` name each advisory, the part it is in and the version that
fixes it. Each day bundled reads it again for what is installed and
writes what changed in the history. Nothing is removed or blocked:
whether to update, go back or remove is yours.

## Several versions side by side

A bundle with a line, such as Node.js 22, installs beside its other
lines as a bundle of its own, `org.nodejs.node@22`, and updates only
within that line. When several lines add the same command, the one that
runs is the line a project asks for, in a file such as `.nvmrc` from
the working folder up; then your default, set with `bundle default node
22`; then the newest. `node-22` always runs line 22.

What a line's package manager installs, such as `npm install -g`, goes
into your own add-ons folder for that line, `~/applications/add-ons`,
with no elevate, and its commands join your `PATH` until you remove them
or the line.

## Services

A bundle may run services, such as a web server or a database. It
installs only for everyone, through elevate, and each service runs
under an account of its own, made for it, whose data folder in
`/shared/services` is that account's alone. Spark starts them, at boot
or when enabled, and starts them again when they crash. Your own
permissions never reach a service.

An update stops the services, keeps a copy of their data, and starts
the new version. If a service does not answer its health check within
60 seconds, the update goes back by itself, with the data as it was, and
holds the bundle. An update never asks about settings an administrator
changed: their file stays, and the new default goes beside it as
`NAME.toml.default`, which `service check` compares. Changes kept in
`settings.d/NAME.toml` are never touched.

## Removing

```
$ bundle remove org.example.notes
```

Removing asks whether to delete the app's settings and data too, which
then go to the Trash; `--keep-data` keeps them for a later version from
the same publisher, and `--purge` deletes them. Its commands go with
it.

## For publishers

`bundle new ID` starts a bundle and makes your developer key, which
signs your own builds: they install on your computer with no warning,
and on others after the warning for a key they do not know. `bundle
check`, `bundle sign` and `bundle pack` check, sign and pack it into its
`.dkv` file; bundle(1) has the steps, and manifest.json(5) the
manifest.

A bundle no catalog lists can still update by itself: name a feed in
its manifest's `update`, with the key it is rooted in, and sign the feed
with `bundle feed FOLDER FILE.dkv` each time you publish, and at least
once a month, then serve `FOLDER/site` at that address. The feed is a
small repository signed as a catalog is, so computers that installed the
bundle refuse an old feed served again, one past its time, and any file
other than the one you signed.
