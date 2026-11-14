#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds libxml2 for Radix (ob(1), PORTS; ADR 0499) with CMake, as a
# static library with zlib, and installs it, its headers and
# libxml-2.0.pc into the sysroot, for the ports built on it. Without
# iconv, which the C library does not have: libxml2 reads and writes
# UTF-8, UTF-16 and ISO-8859-1 itself, and the other ISO-8859 encodings
# with its own tables, which is what the XML extensions of PHP need. Its
# programs, tests and documentation are left out, and so is loading
# modules, as a static program loads none.
set -eu

cd "$PORT_BUILD"
cmake -G Ninja -S "$PORT_SRC" -B . -Wno-dev \
	-DCMAKE_TOOLCHAIN_FILE="$CMAKE_TOOLCHAIN_FILE" \
	-DCMAKE_BUILD_TYPE=Release \
	-DCMAKE_INSTALL_PREFIX="$PREFIX" \
	-DCMAKE_INSTALL_BINDIR=commands \
	-DCMAKE_INSTALL_LIBDIR=libraries -DCMAKE_LIBRARY_PATH=/system/libraries \
	-DBUILD_SHARED_LIBS=OFF \
	-DLIBXML2_WITH_ICONV=OFF -DLIBXML2_WITH_ICU=OFF -DLIBXML2_WITH_PYTHON=OFF \
	-DLIBXML2_WITH_ZLIB=ON -DLIBXML2_WITH_THREADS=ON \
	-DLIBXML2_WITH_MODULES=OFF -DLIBXML2_WITH_HTTP=OFF \
	-DLIBXML2_WITH_PROGRAMS=OFF -DLIBXML2_WITH_TESTS=OFF -DLIBXML2_WITH_DOCS=OFF \
	>configure.log 2>&1 || {
	tail -n 40 configure.log >&2
	exit 1
}
ninja -j"$JOBS" >build.log 2>&1 || {
	tail -n 60 build.log >&2
	exit 1
}
DESTDIR="$SYSROOT" ninja install >install.log 2>&1
