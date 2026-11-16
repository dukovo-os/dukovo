#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds ripgrep for Radix with Cargo for Radix ($CARGO_RADIX, ob(1),
# PORTS) into the bundle's folder ($PORT_BUNDLE, ADR 0370): bin/rg, and
# its manual page, which rg writes itself, from a build for the host.
# Lists the crates it links for the bundle's SBOM and licenses.
set -eu

cd "$PORT_SRC"
"$CARGO_RADIX" build --release --bin rg --target-dir "$PORT_BUILD/radix" --jobs "$JOBS"
mkdir -p "$PORT_BUNDLE/bin" "$PORT_BUNDLE/man/man1"
$STRIP -o "$PORT_BUNDLE/bin/rg" "$PORT_BUILD/radix/x86_64-unknown-radix/release/rg"

unset CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_TARGET
cargo build --release --bin rg --target-dir "$PORT_BUILD/host" --jobs "$JOBS"
"$PORT_BUILD/host/release/rg" --generate man >"$PORT_BUNDLE/man/man1/rg.1"

# What the program links, for the bundle's licenses (ADR 0021): not the
# procedural macros, which run while it builds and are not in it.
"$CARGO_RADIX" tree -e normal,no-proc-macro --prefix none --format '{p}|{l}' \
	>"$PORT_INFO/crates.txt"
cp Cargo.lock "$PORT_INFO/Cargo.lock"
