<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0760. An app says which files and links it opens, each person chooses the app that opens them, and the app is handed only that file

- Status: Accepted; built with the desktop, in releases 1.6 to 1.8
- Date: 2026-11-25

## Context

Apps come from catalogs as sealed bundles and each runs in a sandbox of
its own (ADRs 0223 and 0225). An app sees its own folder, its data and
what it was granted, and none of a person's files unless a permission
gives them. A desktop still needs what every desktop has: a PNG opens in
the person's image viewer, a .docx in their word processor, a `mailto:`
link in their mail app, and the person can change which app that is.

Some of this is settled already.

- Manifest version 1 reserves `entries` (ADR 0221), the place where an
  app registers with the desktop. It is data only, and nothing in it runs
  in the shell. The security design gives it the file types and link
  schemes an app opens, and whether it edits or only views them. The
  refused version-2 fixture of the manifest crate, Photos, has that shape.
- Becoming the default is the person's choice, made in Open with or in
  Settings. An app may ask once through a trusted prompt, and can never
  make itself the default (the security design, section 12).
- An app that opens a file is given that one file and nothing else, and
  a file picked in the file dialog needs no permission (the security
  design). Until the desktop's file dialog, apps reach files through
  folder grants (ADR 0224).
- The approved desktop design has Open with in Atlas's right-click and
  File menus. It marks the default, changes it in place and has "Choose
  another app…". Settings has Apps > Default apps.
- The roadmap puts Open with and the file-picker portal in release 1.7,
  and the default apps page in 1.8.

Five things are not settled anywhere.

1. How a file's type is known: by its name, by its content or by both,
   from which list of types, and under which license.
2. Where each person's choices are kept, who may write them, and which
   app opens a file that nobody has chosen an app for.
3. How a confined app receives the file it is handed, saves it back
   safely, and reopens it from its recent files after a restart. A view
   never detaches what it attached (ADR 0215), and a file attached on its
   own has no folder to save beside.
4. What happens to the choices when an app is updated, removed or
   installed again, what an administrator sets for everyone, and how the
   Linux apps of releases 1.15 and 1.16 take part.
5. The release that brings `entries`. ADR 0221 and manifest.json(5) name
   1.5, from the numbering in use before the desktop moved to 1.6.

Nothing before 1.0 needs these answers. The web console serves its own
pages and reports, never a person's files, and the console has no
graphical apps to choose between.

## Decision

### Types

- **A type is a media type,** named as IANA registers it and as the web,
  mail and Linux's desktop files name it: `image/png`, `text/markdown`,
  `application/vnd.openxmlformats-officedocument.wordprocessingml.document`
  for a .docx. A link scheme is its name: `https`, `mailto`.
- **Dukovo's list of types is its own,** in `/system/resources/types`.
  Each type has its name, the other names it goes by
  (`application/x-zip-compressed` is `application/zip`), its file name
  extensions, a description to translate, an icon, and the type it is a
  kind of: `text/markdown` is a kind of `text/plain`, and every type is a
  kind of its group, such as `image/*`. The list is written from IANA's
  registry for Dukovo and is Apache-2.0 OR MIT, so the crate that reads
  it stays permissive. freedesktop.org's shared-mime-info database is not
  copied: it is GPL-2.0-or-later.
- **The name decides first.** A file's type is its longest extension the
  list knows, in any case, so `.tar.gz` comes before `.gz`. A name with
  no known extension is typed by its first bytes, which file(1)'s magic
  database reads (a port already, BSD-2-Clause) in a throwaway sandbox
  that holds only that file and no network, as release 1.7 runs every
  parser of untrusted files. When the name and the content disagree, the
  name chooses the app. The app is confined and holds only that file, so
  a wrong name at worst opens a file in an app that cannot read it.
- **An app may add types** in `entries.types`, with the same fields as
  the list, for formats the list does not know, such as a game's saved
  files. It cannot declare a type the list has, and an extension that the
  list or an app installed earlier maps keeps that mapping, so no app
  takes `.pdf` from another type. `bundle check` and the catalog's
  review say so, and an app's types leave with it.
- **Types that run code have no app.** A bundle (`.dkv`) opens only in
  the system's install sheet, with Install or Open once (the security
  design). A program or a shared library is never opened by an app, and
  starts only through Run, which asks once for a file from the internet
  (release 1.7). A script is text: it opens in a text editor, and runs
  only through Run or a shell. The list marks these types, and no app
  may declare that it opens them.

### What an app declares

`entries.opens` is a list. Each item has one of these:

- `types`: media types, or a group such as `image/*`, with a `role`:
  `edit` hands the app the file to read and change, `view` to read only;
- `schemes`: link schemes, such as `mailto`. `http` and `https` are one
  choice, the web browser. No app may declare `file`, `data`,
  `javascript`, `about` or the system's own schemes.

An item names its `launcher` when the app has more than one, as an office
suite has a word processor and a spreadsheet. The system's own programs
declare what they open in the same form, beside the list of types, so
the approved Open with design can offer nano for text "in Terminal". A
bundle with no `entries`, such as a service or a command-line tool,
opens nothing and appears nowhere on the desktop.

`entries` ships in the manifest version of release 1.6, with
`launchers`, `opens` and `types`, as a preview. It freezes in release
1.8, once Open with, Settings and the Store have used it.

### Which app opens a file

bundled keeps each person's choices in `/system/state/defaults/UID`,
root's, mode 0600, written whole through a rename, as it keeps their
grants (ADR 0224). Each line is a type or scheme, the app's ID, its
publisher's key, and since when. An administrator's choices for everyone
are kept in `/system/settings/defaults` in the same form.

A file of type T opens in the first app these steps find:

1. the person's choice for T;
2. an administrator's choice for T, for everyone;
3. the system's choice for T, among the defaults Dukovo ships, such as
   Image preview for images and the PDF viewer for PDF (release 1.9), if
   that app is installed;
4. of the installed apps that declare T itself, the first one installed:
   the first app installed for a type opens it, and no app installed
   later takes a type over;
5. the same steps for the type T is a kind of, then its group (`image/*`),
   then `application/octet-stream`;
6. no app: Open shows the Open with chooser, which says that no app here
   opens the type and offers the Store, searched on the machine by type.

The system's own programs count as installed before any bundle. On the
desktop, step 4 passes over a program that runs in a terminal, such as
nano, so a .txt opens in nano only when the person chose it; at a console
with no desktop, only such programs count. A choice of an app that is
gone, or that no longer declares T, is passed over. The answer depends
only on what is installed and what was chosen,
so `open --explain` can say which step chose the app. A link takes the
same steps by its scheme.

### Choosing

- **Open with** lists the apps that declare T, the default first and
  marked, then those that declare a type T is a kind of. "Always use it"
  records the person's choice. "Choose another app…" lists every app
  with a launcher; one that does not declare T is handed the file to read
  only.
- **Settings > Apps > Default apps** (release 1.8) lists the choices by
  app and by type, with the web browser, mail and the common types first,
  and changes any of them.
- **An app may ask once** to become the default for what it declares.
  The UI kit sends the request to bundled, which shows the trusted
  prompt (release 1.6's desktop look, or the console's bottom line) with
  the types named. Yes records the person's choice. No is kept with the
  app's grants, and that app is not asked about again, in any version.
- **No program records a choice.** bundled records one only from the
  system's own Open with and Settings, which it knows by the sealed
  program in `/system` that the asking process runs, read from
  `/processes` as it finds an asker's terminal (ADR 0224), from a trusted
  prompt, or from `open --default`
  after the prompt's Y, as `bundle grant` asks (ADR 0224). A command that
  runs with the person's access cannot make an app the default, and
  neither can a script. This is why the choices live with bundled and
  not in a file in the person's home.
- **An administrator** sets a choice for everyone with
  `open --default --for-everyone` through elevate, as
  `bundle default --for-everyone` sets a line. A person's own choice
  comes before it.

### Updates and removal

- A version that stops declaring T loses its place for T, and the next
  step decides.
- Removing an app removes its entries at once. Its choices are kept with
  its data and grants when the person keeps them, so the same publisher's
  key installed again picks them up (ADR 0224). `--purge` forgets them.

### Handing a file to a confined app

docd, the documents service, runs as each person while they have a
session. It keeps the files that person has handed to each app, by Open,
Open with, the file picker, drag and drop or another app, and serves
them as a filesystem through `/devices/fuse` (ADR 0226). `bundle run` attaches the app's part of
it at `/documents` in the app's view when the app starts. Each file
handed is a folder there, `/documents/N/NAME`, that holds the one file
under its own name.

- **Rights.** `role: edit` gives read and write; `role: view`, or a file
  handed on by an app that may only read it, gives read only. A handing
  app gives no more than it holds: it passes docd the file's capability,
  never a path.
- **Saving.** An app writes a new file in the folder and renames it over
  the file's name, as careful programs save everywhere. For a file the
  person handed through the system's own Atlas, file picker or drop, docd
  also holds its real folder, and turns the rename into an atomic
  replacement there: a crash during a save loses nothing, and the file
  stays where it was. A file handed on by another app holds no folder,
  and a save writes the file itself. Other files an app makes in the
  folder stay in docd's store and go with the file.
- **A running app** gets a new file at once: docd adds its folder, and
  the UI kit's open event, or the command line of a program started for
  it, names it. The view does not change, so nothing has to be detached.
- **Taking a file back** is immediate: docd stops serving its folder and
  the app's next read fails, as a revoked permission ends at once
  (ADR 0224).
- **Recent files reopen.** A file handed to an app stays its own until
  the person removes it in the app's page in Settings (release 1.8), the
  file is deleted, or the app is removed with `--purge`. bundled records
  each one with the person's grants, so nothing that runs as the person
  can add one. A file is kept by its place and its identity, so one moved
  or replaced by another is not followed; the person hands it over
  again. The taskbar's recent files for an app are these records.
- **The real place is shown.** docd tells the UI kit where each file
  really is, so title bars and recent lists show the person's own
  folders. Knowing a file's place reaches nothing.

A link reaches the app that opens it as text, an argument and an open
event, with nothing else. The app treats it as input anyone could have
written.

### The command

`open` (open(1), release 1.7) opens a file or a link from a terminal as
Atlas does: `open FILE`, `open --with ID FILE`, `open --explain FILE`
(the type, how it was found, the apps that declare it and the step that
chose one) and `open --default TYPE [ID | --clear] [--for-everyone]`. At
a console with no desktop it offers only the apps that run in a
terminal.

### Linux apps

Release 1.15's portal backend answers xdg-desktop-portal's OpenURI and
file chooser requests through the same steps and docd, and shows docd at
`/run/user/UID/doc`, where Flatpak's apps find their documents. A Linux
app cannot set a default: `xdg-mime` and its like change nothing outside
the app's own view. Release 1.16's catalog bots turn each desktop file's
`MimeType` list and `x-scheme-handler` entries into `entries.opens` with
`role: edit`, which the review may narrow to `view`.

### When each part ships

| Release | What it brings |
| --- | --- |
| 1.6 | `entries` in the manifest, with `launchers`, `opens` and `types`, as a preview; the trusted prompt's desktop look, for an app's one request |
| 1.7 | The list of types and its sandboxed reader of first bytes; bundled's choices and the steps above; Open with; docd for Open, the file picker and drag and drop; `open` |
| 1.8 | Settings > Apps > Default apps and each app's files; the install sheet lists what an app opens; the catalog index carries each bundle's types, so the Store finds an app for a type on the machine; `entries` freezes |
| 1.9 | The everyday apps, each the system's default for its types as it ships |
| 1.15, 1.16 | Linux apps, as above |

## Consequences

- A person who installs one app for a type is asked nothing: the type
  opens in it. A second app for the type appears in Open with, and
  nothing changes unless the person chooses it or says yes to its one
  request.
- An app that declares every type still reaches only the files it is
  handed. It shows in more Open with lists, and the catalog's review
  sees types an app claims but does not handle.
- Defaults survive what people do to apps: an update, a removal that
  keeps the data, and installing again leave them as they were.
- Files keep their place and their safe saves while apps stay confined:
  an editor sees one folder per file it was handed, not the person's
  home.
- docd is one more service in every session and holds the folders of
  the files the person handed through the system's own tools. It runs as
  the person and holds nothing they do not.
- The catalog index grows by each bundle's list of types in release 1.8,
  and the bots of release 1.16 have one more field to map.
- Tests: release 1.7's boot test installs two bundles that open one type
  and checks that the first opens it, that the person's choice comes
  before it, that a command run with the person's access cannot record a
  choice, that a save through docd replaces the file atomically, that a
  file taken back fails at the next read, and that an app removed with
  its data kept and installed again has its choice back. The list of
  types is checked against IANA's registry and fuzzed by name and by
  content.

## Alternatives considered

- **Extensions alone, as the only types.** One file format has many
  extensions (`.jpg`, `.jpeg`, `.jfif`), a file with none has no type,
  and the web, mail and Linux apps all name formats by media type.
- **Content first.** Every listing would read every file, through a
  parser of untrusted bytes, to choose an app that is confined anyway.
- **The last app installed becomes the default,** as installers on some
  systems set themselves. An app would take types over by being
  installed, which the security design forbids.
- **Asking at the next open once a second app arrives.** A question on an
  ordinary day for a choice most people never make; Open with's "Always
  use it" is there when they want it.
- **Attaching each file to the app's view.** A view never detaches,
  holds at most 512 entries, and gives a file attached alone no folder to
  save beside, so a careful save fails.
- **A descriptor alone.** Ported programs open files by name, and a
  descriptor cannot be reopened from a recent list after a restart.
- **Choices in a file in the person's home.** Any command that runs with
  the person's access could write it, and so make an app the default.
