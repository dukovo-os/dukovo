#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Puts the XKB keyboard data into the sysroot's /system/resources/xkb
# (ob(1), PORTS). Its build compiles nothing: it puts the rules files
# together from their parts, on the build host. The data is installed
# into a folder of the build's first, since its own install names the
# folder after its version and adds links and pkg-config files that
# nothing on Dukovo reads, and the folder's contents are then copied.
# The translations of the layouts' names are left out: no program shows
# them yet.
set -eu

meson setup "$PORT_BUILD" "$PORT_SRC" \
	--prefix="$PREFIX" --datadir=resources \
	-Dnls=false -Dxorg-rules-symlinks=false \
	>configure.log 2>&1 || {
	tail -n 40 configure.log >&2
	exit 1
}
meson compile -C "$PORT_BUILD" >build.log 2>&1 || {
	tail -n 60 build.log >&2
	exit 1
}
staged="$PWD/staged"
rm -rf "$staged"
DESTDIR="$staged" meson install -C "$PORT_BUILD" --no-rebuild --quiet
rm -rf "$SYSROOT/system/resources/xkb"
mkdir -p "$SYSROOT/system/resources"
cp -R "$staged$PREFIX/resources/xkeyboard-config-2" "$SYSROOT/system/resources/xkb"
