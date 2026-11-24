<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0622. A Linux program runs in a sandbox of its own, with the Linux base bundle at /usr

- Status: Accepted
- Date: 2026-11-24

## Context

A dynamically linked Linux program, copied from any distribution, needs
its loader and libraries at the paths it names (`/lib64/ld-linux-x86-64.so.2`,
glibc, the C++ runtime), Linux's layout of `/proc`, `/sys`, `/dev` and
`/etc`, and whatever it was given to work on. Dukovo has none of these at
those paths, and a program of unknown origin should not reach all of a
person's files: a bundle runs in a view of what its manifest asks for and
the person allows (ADRs 0215, 0224, 0225), and a loose Linux program
deserves no more.

## Decision

- **`linux run`** (linux(1), a command of systools beside `bundle`, so that
  it shares the bundles' sandbox code) runs a Linux program in a view of its
  own, acting for the person without the control right, with linuxd beside
  it (ADR 0621). The view holds:
  - `/usr` from the base bundle, read-only, with `/lib`, `/lib64`, `/bin`
    and `/sbin` as links into it, as merged-`/usr` distributions have them;
  - `/proc` and `/sys` from linuxd's filesystem (ADR 0619);
  - `/dev` with `null`, `zero`, `full`, `random`, `urandom`, `tty`, a
    private `shm`, and the links `fd`, `stdin`, `stdout` and `stderr`;
  - `/etc` made for the run: `passwd` and `group` (the person, root and
    `nobody`), `hosts`, `hostname`, `resolv.conf`, `nsswitch.conf`,
    `os-release` (Dukovo's), `machine-id`, the certificate bundle and the
    time zone;
  - the folder it was started in, read and write, at its real path, as its
    working directory; its own data folder, `~/app-data/linux/NAME`, as
    `HOME`; a private `/tmp`, removed at the end; the program's own folder,
    read-only, if it lies elsewhere;
  - what `--allow` and `--allow-write` name, or with `--home` the whole
    home; and at an interactive terminal, a path among the program's
    arguments that the view does not hold, once the person says yes to
    letting it read it;
  - the network, as a bundle that asks for outbound and local networks
    has it;
  - the endpoint in `SLOT_LINUX`, with the send and grant rights, and the
    copy and transfer rights that let a `fork` pass it on; never the
    receive right, which linuxd alone holds.
- **Supervision.** `linux run` holds linuxd's endpoint and FUSE connection,
  starts linuxd again if it ends (at most five times a minute), passes on
  job-control stops, and exits with the program's status, or as env(1)
  does with 125, 126 and 127 when it cannot run the program. Once it gives
  linuxd up, a thread of its own answers the program's forwarded calls
  with ENOSYS, so that none waits for an answer that cannot come.
- **Nukesh** runs a file through `linux run` when the kernel cannot start
  it for want of its dynamic linker and it is an x86-64 program whose
  loader is not Dukovo's and which carries no Dukovo note: the kernel's own
  rule, through `radix-elf`'s reader and note check. The shell reads the
  file only after such a failure, so other programs cost it nothing more. A static Linux program runs as before
  (ADR 0610), unsandboxed, as any program the person starts; `linux run`
  sandboxes one when asked.
- **The Linux base** is the catalog bundle `org.dukovo.linux-base`, built
  by the port `ports/linux-base` from the Freedesktop SDK's platform image
  for x86-64 (release 25.08), fetched as layer blobs by pinned digest and
  checked, keeping glibc, the C++ runtime and a few core libraries (zlib,
  zstd, xz, bzip2, libffi, expat, libuuid, libcrypt) with their licences:
  a small, well-kept, distribution-neutral runtime, which Flatpak apps
  already target. It is a bundle, not part of the image, as Go and Python
  are (ADRs 0356, 0495): images stay small, and `linux run` says how to
  install it when a program needs it. A bundle holds regular files only,
  so each library is kept under its soname and the loader at both of its
  paths; and as a manifest names at least one program (manifest.json(5)),
  the bundle's one program, `linux-base`, only says what the bundle is.
  ob fetches an image's layers as a port's source of its own kind
  (`registry`, `repository`, `tag` and `layers` in `recipe.toml`, ob(1)).

## Consequences

- A glibc program from a Linux distribution runs from Nukesh by its path,
  sees Linux's layout, and reaches only its folder, its data, its `/tmp`,
  the network and what the person lets it have.
- Programs that need libraries beyond the base (GTK, Qt, OpenSSL's 3.x
  ABI from a distribution) do not run yet; the desktop releases extend the
  base (roadmap 1.15).
- The GCC Runtime Library Exception joins the licences a bundle may carry,
  for libstdc++, libgcc_s and libatomic.
- linuxd runs in the person's resource domain, not one of the sandbox's
  own, until sessions have domains of their own; asking to let a program
  read a path works at a terminal only, not yet in the graphical session;
  rows of docs/deferred.md.
