#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds pixman's static library for Radix (ob(1), PORTS) and installs
# it, its header and its pkg-config file into the sysroot, for windowd.
# Its tests and demonstrations, which need libpng and GTK, are left out,
# and so is OpenMP, which only the tests use.
set -eu

meson setup "$PORT_BUILD" "$PORT_SRC" \
	--cross-file "$CROSS_FILE" \
	--prefix="$PREFIX" --libdir=libraries --includedir=include \
	--buildtype=release \
	-Ddefault_library=static \
	-Dtests=disabled -Ddemos=disabled \
	-Dlibpng=disabled -Dgtk=disabled -Dopenmp=disabled \
	>configure.log 2>&1 || {
	tail -n 40 configure.log >&2
	exit 1
}
meson compile -C "$PORT_BUILD" -j "$JOBS" >build.log 2>&1 || {
	tail -n 60 build.log >&2
	exit 1
}
DESTDIR="$SYSROOT" meson install -C "$PORT_BUILD" --no-rebuild --quiet
