#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds libedit for Radix (ob(1), PORTS; ADR 0521): the static library, on
# ncurses' terminal library, its headers (histedit.h and the readline
# compatibility header editline/readline.h) and libedit.pc in the sysroot,
# for the ports built on it. Nothing of it goes in the image by itself.
#
# libedit stops at a C library that does not say its wchar_t holds ISO 10646
# characters, which mlibc does in stdc-predef.h, a header GCC reads before
# every file and Clang does not: it is read here. Its wcsdup.c tests
# HAVE_WCSDUP before it includes config.h, so it always defines wcsdup,
# which the C library has too, and a static link of both fails with a
# duplicate symbol: HAVE_WCSDUP is given on the command line, where the
# test sees it.
set -eu

cd "$PORT_BUILD"
x86_64-radix-configure "$PORT_SRC" CPPFLAGS="-include stdc-predef.h -DHAVE_WCSDUP" \
	--disable-shared --enable-static --disable-examples >configure.log 2>&1 || {
	tail -n 40 configure.log >&2
	exit 1
}
make -j"$JOBS" >make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}
make install DESTDIR="$SYSROOT" >/dev/null
# libtool's notes on the library, which name its place without the sysroot.
rm -f "$SYSROOT/system/libraries/libedit.la"
