#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds bat for Radix with Cargo for Radix ($CARGO_RADIX, ob(1), PORTS)
# into the bundle's folder ($PORT_BUNDLE, ADR 0370): bin/bat, and the
# manual page its build writes. Without the git feature (libgit2 carries
# LGPL code) and, by the patch, without bugreport; with Dukovo's own
# ansi_colours in the place of crates.io's, through Cargo's [patch], which
# changes only that crate's entry in Cargo.lock. Lists the crates it links
# for the bundle's SBOM and licenses.
set -eu

cd "$PORT_SRC"
patch="patch.crates-io.ansi_colours.path=\"$PORT_DIR/ansi_colours\""
features="--no-default-features --features application"
"$CARGO_RADIX" update --config "$patch" -p ansi_colours --offline 2>/dev/null ||
	"$CARGO_RADIX" update --config "$patch" -p ansi_colours
"$CARGO_RADIX" build --config "$patch" --release --locked $features --bin bat \
	--target-dir "$PORT_BUILD/radix" --jobs "$JOBS"
mkdir -p "$PORT_BUNDLE/bin" "$PORT_BUNDLE/man/man1"
$STRIP -o "$PORT_BUNDLE/bin/bat" "$PORT_BUILD/radix/x86_64-unknown-radix/release/bat"
# The build script writes the manual page into its output folder.
man=$(find "$PORT_BUILD/radix" -path '*/out/assets/manual/bat.1' | head -n 1)
cp "$man" "$PORT_BUNDLE/man/man1/bat.1"

# The licenses and notices of the syntaxes and themes bat carries, which
# `bat --acknowledgements` prints from this asset: bincode's string,
# zlib-compressed.
mkdir -p "$PORT_BUNDLE/licenses/notices/bat-assets"
python3 -I - "$PORT_SRC/assets/acknowledgements.bin" \
	"$PORT_BUNDLE/licenses/notices/bat-assets/ACKNOWLEDGEMENTS" <<'END'
import struct, sys, zlib
data = zlib.decompress(open(sys.argv[1], "rb").read())
(size,) = struct.unpack("<Q", data[:8])
open(sys.argv[2], "w").write(data[8 : 8 + size].decode())
END

# What the program links, for the bundle's licenses (ADR 0021): not the
# procedural macros, which run while it builds and are not in it.
"$CARGO_RADIX" tree --config "$patch" --locked $features -e normal,no-proc-macro \
	--prefix none --format '{p}|{l}' >"$PORT_INFO/crates.txt"
cp Cargo.lock "$PORT_INFO/Cargo.lock"
# The crate's lock entry no longer names crates.io's ansi_colours.
! grep -A2 '^name = "ansi_colours"' Cargo.lock | grep -q '^source'
