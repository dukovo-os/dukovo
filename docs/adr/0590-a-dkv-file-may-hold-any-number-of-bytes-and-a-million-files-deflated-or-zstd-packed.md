<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0590. A .dkv file may hold any number of bytes and a million files, deflated or zstd-packed

- Status: Accepted; amends [0222](0222-bundles-are-signed-with-minisign-and-packed-as-dkv-files.md)
- Date: 2026-11-20

## Context

ADR 0222 made the `.dkv` file a plain zip that a strict reader opens, and
left out zip64 and every method but stored and deflate. A bundle was then
at most 4 GiB and 65,535 files, and the writer and the signing of an
archive held each file whole in memory. Bundles of large apps, and npm's
exports (ADR 0389), pass those limits. zstd unpacks faster than deflate
does, and the large apps' files are what that matters for.

This record amends ADR 0222 (what a `.dkv` file holds and what the reader
refuses).

## Decision

- **Zip64, where a field needs it.** The reader takes zip64's end record
  and locator, and the extra field 0x0001 of an entry, and takes each value
  from one place only: a classic field either holds the zip64 marker
  (0xFFFF or 0xFFFFFFFF) or the same value as zip64's. The zip64 end record
  is the 56 bytes just before its locator, with no extensible data. An
  entry's extra field holds the marked fields only, 8 bytes each, in the
  order size, packed size, offset, and nothing else. A marker with no
  zip64 end record is refused. The writer writes zip64 only where it is
  needed: for an entry whose size, packed size or offset is 0xFFFFFFFF or
  more, whose local header then has both sizes in the field (version
  needed 45), and for the end when the count is 0xFFFF or more or the
  directory's size or offset is 0xFFFFFFFF or more. An archive that fits
  a classic zip's fields has none, and packs to the same bytes as before.
- **A million files.** At most 1,048,576 entries, in a directory of at
  most 256 MiB; the reader keeps its checks of overlap, gaps, names and
  the ratio of unpacked to packed bytes, computed so that no value
  overflows.
- **zstd, method 93.** An entry is one frame, with no dictionary, a
  window of at most 32 MiB and nothing after it; its checksum is optional,
  as the CRC-32 checks each file. The writer packs with ruzstd
  (`CompressionLevel::Fastest`, the only level it implements past
  storing) and writes version needed 63; `bundle pack --zstd` chooses it,
  and deflate stays the default, which every unzip reads. A file is
  stored when packing does not make it smaller, as before.
- **A file is not held whole.** A file of more than 256 MiB is read twice
  by `bundle pack` and by the signing of an archive: the first time for its
  CRC-32 and packed size, which come before the bytes, and the second to
  write them, with no data descriptor. Both readings must give the same
  bytes: deflate and zstd are deterministic, and a CRC-32 or packed size
  that differs the second time fails the pack ("changed while it was
  packed"). Signing an archive also hashes the second reading and fails
  on a hash that is not the list's, as for a file held whole.
- **ruzstd 0.9.1**, from crates.io, MIT, with `std` and without its
  xxHash feature, credited in CREDITS.md. Its encoder unwraps the result of
  each read and write, so the writer gives it only a source that cannot
  fail and a sink that keeps the first error and drops the rest, and
  checks the error when it returns.

## Consequences

- The signed catalog bundles made before this record are unchanged and
  stay valid: a small archive's bytes are pinned by a unit test of the
  writer.
- A zstd entry is method 93, which an `unzip` that predates it does not
  read, so `bundle pack --zstd` is for bundles that Dukovo's own installer
  opens; deflate stays the default.
- Packing and signing hold one file's pieces and the directory in memory,
  not the files over 256 MiB, so the size of a bundle is bounded by the
  disk and not by the memory.
- ruzstd's `Fastest` packs less well than libzstd's higher levels. A
  frame that libzstd made at any level is read, within its window.
