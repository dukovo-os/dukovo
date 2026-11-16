<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0535. A release is built twice by CI, in different places, and must come out the same

- Status: Accepted
- Date: 2026-11-15

## Context

ADR 0457 has CI build every release twice and the maintainer's machine a
third time, and lets the hardware keys sign only when the three release
lists have the same hash. ADR 0010 noted that images were not
reproducible, and left them for later. A build of one commit differs
from another of the same commit for five reasons, each of which shows
when it is built in two places:

- **Identifiers.** A GPT's disk and partition GUIDs, a Btrfs's
  filesystem, device and subvolume UUIDs and a FAT's volume serial came
  from random numbers.
- **Times.** What an image or a filesystem recorded as its time was the
  clock's.
- **Names of places.** A compiler writes down where it read each source:
  in a panic's location, in `__FILE__`, in an assertion's message and in
  debug information. The checkout, Cargo's home and the toolchain's copy
  of Rust's sources are in different places on different machines, so
  the kernel, every program and every port carried some of them. GRUB's
  modules did, and one manual page, ncurses's `terminfo(5)`, names the
  directory its build read its parts from.
- **Order, owners and modes.** A walk of a directory gives its entries in
  the order the filesystem keeps them, and a folder that a program makes
  without saying its mode gets the user's umask.
- **The machine's own keys.** Until the key ceremony signs the system's
  first root, each build machine signs with keys of its own
  (`.cache/release-keys`). The boot archive holds the root the system
  trusts, so a tree built on one machine differs from the same tree built
  on another, and so does every signed release list.

## Decision

- **No build draws on chance.**
  - Every identifier of an image is the HMAC-SHA-256, under a key that
    names the format, of the version, `SOURCE_DATE_EPOCH`, the kind of
    image (`stick`, `iso`, `installed` and the others of `ob image`) and
    the identifier's purpose, such as `gpt-disk`, `gpt-part-2:root` or
    `btrfs-subvol:@system.a`. They are made in `tools/xtask/src/identity.rs`.
  - They keep the layout of a version 4, variant 1 UUID, so that nothing
    that reads them can tell them from a random one. A GUID is written
    in a GPT's order, whose first three fields are little-endian.
  - A partition added to a disk that is made already takes its GUID from
    the disk's, so that it depends on the disk and on its place.
  - Two images of one kind, version and time carry the same identifiers
    and two of different kinds do not. They tell disks apart, and are
    no secret.
  - Btrfs draws its identifiers through a function `radix_vfs` lets its
    caller set. `ob` sets it for the thread that makes a filesystem, so
    that each draw is a new value named by its purpose and a count, and
    the same ones in each build.
- **An installed system's identifiers are its own.** The installer makes
  its disks and filesystems with `rdisk`, `mkfs-fat` and `mkfs-btrfs`,
  which draw on the system's random numbers (`disk::Rng::system`).
  Nothing in this decision touches them: only `ob` seeds an identifier,
  and only for the images it writes.
- **Times come from the commit.** `SOURCE_DATE_EPOCH` is the commit's
  time unless it is set. Ports are the exception: every port is built at
  2026-01-01 00:00:00 UTC (`PORT_TIME` in `tools/xtask/src/ports.rs`),
  which is part of each port's stamp. A port is cached by its stamp, and
  one built at an earlier commit would otherwise hold that commit's time
  in each `__DATE__` and `__TIME__` and each manual page it dates, and
  differ from the port the same recipe builds now. Files newer than it are dated by it in the FAT of
  the EFI system partition, in Btrfs and on an ISO, as `tar
  --clamp-mtime` does, and a folder `ob` makes takes the image's time.
  The boot archive records the same owner, a mode of 0755 or 0644, and
  the commit's time for every entry, and no other archive goes into a
  release.
- **The paths a compiler writes down are fixed.**
  - `tools/xtask/src/repro.rs` names four directories: the checkout is
    `/dukovo`, a build directory outside it `/dukovo-target`, Cargo's
    home `/cargo` and the toolchain's copy of Rust's sources `/rustc`. A
    directory reached through a link is named by both its paths.
  - Rust gets one `--remap-path-prefix` for each. They are in the
    flags of the kernel's build, of the user programs for the user target,
    of the wrapper of `cargo` that builds the programs on Rust's standard
    library and the ports in Rust, and of the `rust-std` port, which sets
    its own flags and takes them from `$REMAP_RUSTFLAGS`.
  - C and C++ get `-ffile-prefix-map` for the checkout and the build
    directory: in the toolchain's compiler wrappers, which every port
    uses, in the Meson cross file of the C library, in uACPI's
    `build.rs` and in GRUB's `TARGET_CFLAGS`, where the unpacked source and
    its build directories are `/build/grub`. The recipe's version
    changes, so that the cached GRUB is built again.
  - Each crate's identity is what it is, not where the checkout is.
    Cargo gives rustc a hash for each crate, `-C metadata`, that every
    symbol of the crate carries. For a path dependency outside the
    workspace that uses it (`kernel/abi`, `kernel/lib/elf` and
    `tools/ridlc`, which the kernel and every user program need, and the
    crates the ports patch for Radix) Cargo hashes the dependency's
    absolute path. So `ob` runs rustc for Cargo (`RUSTC_WRAPPER`, in
    `tools/xtask/src/rustc_wrapper.rs`) and gives it a hash of what the
    compilation is instead:
    - rustc's arguments with the four directories written as their names;
    - the package's name, version and folder;
    - each dependency's own hash, which the wrapper keeps beside the
      dependency's output, by the hash in its file names, or by its name
      for a library that is a static or C library too, whose file names
      hold no hash (boringtun).

    Where the outputs go and where libraries are looked for are left out,
    as Cargo names those folders by its own hashes. The file names Cargo
    chose stay, so Cargo finds what it built. A build script's output
    folder, `OUT_DIR`, is written as `/dukovo-out/<package>`. A wrapper
    already set, such as sccache, runs after it.
  - Cargo for Radix gives the crates the ports patch, `libc` among them,
    through a link to the toolchain's Rust folder they are in,
    `.radix-rust`, that it makes in the workspace's folder. Cargo names a
    path dependency inside the workspace by its path from there and one
    outside by its whole path, and the name is in the file names of all
    it builds and of all that depends on it, which the wrapper leaves as
    they are. rustc takes a program's crates under ThinLTO in the order of
    the names of their object files, which begin with those file names:
    the two versions of `getrandom` and of `nix` that uutils use, and the
    patched `libc` and that of Rust's standard library, came in another
    order in a copy of the checkout elsewhere, and so did their code and
    data, or their symbols alone, which the linker's build ID covers.
  - The programs on Rust's standard library are built with
    `SOURCE_DATE_EPOCH` at the ports' time: rust-embed, which `age`
    builds its translations in with, records when each file was made
    unless it is set, and that is when Cargo unpacked the crate.
  - A port is built with `LANG` and `LC_ALL` as `C.UTF-8`: uutils'
    `uucore` builds in the translations of `LANG`'s language beside
    English.
  - `i18n-embed-fl`, whose `fl!` macro `age` writes its messages with,
    is patched in `rust-crates` to write the arguments it is given in
    the order of their names: it took the order of a `HashMap`, which
    differs each time the macro runs.
  - `terminfo(5)` is edited after ncurses installs it, to give its
    source files as `../include/...`.
  - Each of these changes a stamp: the wrappers' text is part of what
    every port's stamp hashes, so the first CI run after this change
    builds every port, about 30 minutes, once.
  - The archiver (`llvm-ar` keeps no times or owners by default), the
    time a C file's `__DATE__` gives (Clang takes `SOURCE_DATE_EPOCH`,
    which every port is built with, at `PORT_TIME`) and the directory
    walks of `ob` that
    decide what an archive or a filesystem holds (each sorts the names it
    reads) needed no change.
- **`./ob release build [--out DIR]` makes what the builds are compared
  by.** It builds the kernel, the programs and the ports, stages the slot
  as `stage_system` stages it for an image and writes into `DIR`:
  - `files.txt`, one line for each folder, file and link of the tree,
    sorted by path as bytes: kind, mode, SHA-256, size, time, path and a
    link's target, separated by tabs;
  - `system.json`, the release list unsigned, which `system-release list`
    makes from the tree and `system-release make` signs;
  - `system.json.sha256`, and as its one line on the standard output, the
    SHA-256 of that list: the value the builds compare.

  It signs nothing, and the tree trusts a fixed root,
  `releases/metadata/1.root.json` when the ceremony has signed it and
  `releases/stand-in-root.json` until then: a root made once, with its
  secret keys thrown away, that no shipped system trusts. The tree stays in
  `out/release-build-tree` for reading a file that differs.
- **Images are not what is compared.** An image holds a release signed
  with the build machine's keys and the root those keys make, so images
  differ between machines until the first root is signed on the hardware
  keys. The tree, its list and each image's identifiers are compared, and
  the images are the tree in a container whose identifiers and times the
  tests above fix.
- **CI builds twice, in places that differ.**
  `.github/workflows/reproducible.yml` runs weekly, on a push to
  `release/**`, by hand and on a pull request that changes the workflow,
  what makes identifiers, images or the release build, what the ports and
  the programs are built with, or a recipe of this decision.
  - The first job builds with the ports the ports job of `ci.yml` cached,
    under the keys it uses, in the runner's own checkout and home.
  - The second builds on another runner, in `/srv/dukovo-second`, as
    another user with a home of its own, under `umask 0002`, in the time
    zone of Auckland, with `LC_ALL=C.UTF-8`, one processor fewer, and
    every port from its source.
  - The third job compares the two lists of files and their release
    lists' hashes, names every path that differs and why, runs diffoscope
    on the first twenty that differ in content with a time limit each,
    uploads its reports and fails.
  - Each job holds `contents: read` only.
- **`ob release sign`'s third build is `./ob release build`.** The
  maintainer's machine runs it and compares the last line with the
  workflow's.

## Consequences

- A change that puts a path, a time or a random number into a build is
  found by the next weekly run, or by the pull request that makes it when
  it touches one of the files the workflow watches, and the report names
  the file.
- Two disks written from one image carry the same identifiers. A person
  who needs them told apart changes a partition's GUID with `rdisk`'s
  `u`. An installed system is not affected, as the installer gives it
  fresh identifiers.
- The second build takes the ports from source, about an hour of a
  runner, well inside GitHub's six-hour limit. In CI the two builds' trees
  are the same in all 4,613 files, folders and links.
- `./ob release build` needs every port, the kernel and GRUB, so it is as
  long as `./ob build`.
- What a local build showed, from two checkouts built under different
  umasks, time zones, locales, job counts and Cargo homes, and what the
  flags above did:
  - memtest86+ came out the same with no flag, and the identifiers, and
    the disks that carry them, are the same in the tests that make a disk
    twice.
  - GRUB's modules and images for a PC's BIOS, built in two folders with
    the flag, were the same in all 281 files; four of them hold
    `/build/grub` in the place of the folder. Its UEFI platform was not
    built twice.
  - The kernel, 101 MB with its debug information, came out the same byte
    for byte.
  - The two builds installed 5,082 files of the ports in common (the second
    stopped at PHP for want of disk). 5,073 were the same. `jq` and
    `curl-config` held the compiler as configure was given it, by where the
    checkout was: a port's build is now given the toolchain's programs by
    name, found on its `PATH`, and the name is part of each port's stamp,
    as is the format of the crates' identity, so that ports built before
    are built again. The other seven are files a build reads and no image
    holds: ncurses' `curses.h` (a path in a comment) and its archives (the
    members' times and modes), libarchive's pkg-config file (the folder of
    the sysroot's libraries) and `librustls.a` (its members are named by
    Cargo's file names, which take a patched crate's path).
  - The user programs were not built twice there, as the first build ran
    past its time limit; CI's runs compare them.

- **Not yet:**
  - The images themselves are not compared, until the system's first root
    is signed on the hardware keys and every machine's image holds the same
    release.
  - `ob release sign`, which compares the third build, is ADR 0457's.
  - The sysroot's own files, which a build reads and no image holds, are
    not compared.
