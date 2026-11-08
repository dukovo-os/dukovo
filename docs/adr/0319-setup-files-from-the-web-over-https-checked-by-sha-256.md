<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0319. Setup files from the web, over HTTPS, checked by SHA-256

- Status: Accepted
- Date: 2026-11-08

## Context

An answers file (ADR 0309) installs a machine with no questions, and a
setup file, `install.json` on the stick's EFI system partition, is one
the screens offer (ADR 0310). For a fleet, a file for each machine on
a server they all reach is easier than a stick for each. Phase 14's
plan asks for setup files from the web, and the approved mock of the
installer's screens has *Download it from a web address* on the setup
file page, with an address and an optional SHA-256, and two refusals:
"Only https:// addresses are used, so the file can't be changed on the
way." and "The SHA-256 does not match the downloaded file. It was not
used."

Nothing signs an answers file: unlike the release on the stick, which
the root the system trusts checks (ADR 0308), it is the person's own,
written for one install. Yet it chooses the disk to erase, the
accounts and their password hashes, and what the machine runs.

## Decision

- **Only `https://` is fetched.** `tuf::http::get_https`, beside the
  update client's `get` (ADR 0380), refuses an `http://` address and a
  redirect to one before it connects, so that TLS is what keeps the
  file from being changed on the way. The server's certificate must be
  from a root the system trusts: Mozilla's list, or the person's own
  added to the settings' copy (dukovo-certificates(7)). The live
  system's roots are the release's.
- **The SHA-256 is checked before the file is read.** Given with the
  address, by `--sha256` or the page's field, as `sha256sum` prints
  it, in either case, it must match the bytes downloaded, or the file
  is refused, whole, before it is parsed. It is optional, as the mock
  has it: TLS alone protects the way, and the SHA-256 adds that the
  server served the file the person meant. A file larger than 1 MiB is
  no answers file and is refused.
- **The command line takes an address.** `dukovo-install --answers`
  takes a file or an `https://` address (anything with `://` is an
  address), and `--sha256` checks either. The install log notes where
  the answers came from, with the file's SHA-256 and whether it was
  checked, so that an install can be traced to the file it used.
- **The screens download it on Load.** The setup file page lists the
  stick's files, then *Download it from a web address* with *HTTPS
  only* beside it, chosen when the stick holds none; choosing it shows
  *Address* and *SHA-256 (optional)*. Load checks both, says
  *Downloading the setup file…* while it downloads, and takes the file
  as one from the stick: its answers checked, and its features this
  release's. The review then says where it came from and whether its
  SHA-256 matched; Back forgets it.
- **The installer uses the update client's HTTP.** It already reads
  certificates the way every Rust program here does, rustls on ring
  (ADR 0192), follows redirects, and reads chunked and close-ended
  bodies; the installer gains a path dependency, not a new crate.

## Consequences

- A fleet's machines install from one server, each from its own file,
  typed once on the screens or given on the command line, and the
  file's SHA-256 can be published beside its address.
- The live system needs the network for a file on the web; the stick
  and the EFI partition still need none.
- The boot test `install-web` runs openssl's `s_server` on the live
  system, with a test authority made there and trusted as a company's
  would be, and a second server nothing trusts: the command line
  refuses `http://`, the untrusted server and a SHA-256 that differs;
  the screens refuse a SHA-256 that differs, review the file with the
  right one, and write nothing to the disk. `install-bios` now installs
  from such a file, end to end.
- The lab's `install/web-setup-file` test downloads from dukovo.org on
  the stick's screens, over a real network with real DNS and the
  public roots: a SHA-256 that differs, a page that is no answers file
  and an `http://` address are refused, and nothing is written.

## Alternatives considered

- **`http://` with the SHA-256 required.** A hash makes a plain
  download safe, but the mock makes it optional, and an address alone
  is what a person types; TLS keeps that case safe too.
- **Signing setup files.** A key a fleet signs its files with would
  need a place in the installer to trust it, which the live system has
  none of before the install; TLS and a hash need nothing new.
- **curl as the downloader.** curl is on the live system, but the
  installer would have to check its exit and output, and its
  redirects would need `--proto =https` to stay on HTTPS; the update
  client's HTTP is in the program already.
