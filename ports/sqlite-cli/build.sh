#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds the SQLite shell for Radix (ob(1), PORTS) into the bundle's folder
# ($PORT_BUNDLE, ADRs 0370 and 0511): bin/sqlite3 and its manual page,
# man/man1/sqlite3.1.
#
# The shell is the shell.c the sqlite port generates and keeps for bundles
# in its folder beside its stamp, linked to the sysroot's libsqlite3.a and
# compiled with the options that library was built with (kept in a file
# beside shell.c, so that the two cannot differ), as SQLite's shell
# decides what to offer from them: without SQLITE_OMIT_LOAD_EXTENSION it
# would have a .load command that no library could serve on Radix.
# SQLITE_HAVE_ZLIB adds .archive and the zip virtual table, on the zlib
# port's library. HAVE_LINENOISE=1 is the line editing of antirez's
# linenoise, which shell.c includes as linenoise.h.
set -eu

sqlite="$PORT_INFO/../sqlite/for-bundles"
if [ ! -f "$sqlite/shell.c" ]; then
	echo "sqlite-cli: no shell.c in $sqlite: build the sqlite port again" >&2
	exit 1
fi
options=$(cat "$sqlite/options")

cd "$PORT_BUILD"
# shellcheck disable=SC2086 # one word per option
$CC $CFLAGS $options -DSQLITE_HAVE_ZLIB=1 -DHAVE_LINENOISE=1 -I"$PORT_SRC/linenoise" \
	-o sqlite3 "$sqlite/shell.c" "$PORT_SRC/linenoise/linenoise.c" -lsqlite3 -lz -lm -lpthread \
	>compile.log 2>&1 || {
	tail -n 60 compile.log >&2
	exit 1
}

mkdir -p "$PORT_BUNDLE/bin" "$PORT_BUNDLE/man/man1"
$STRIP -o "$PORT_BUNDLE/bin/sqlite3" sqlite3
install -m 0644 "$sqlite/sqlite3.1" "$PORT_BUNDLE/man/man1/sqlite3.1"
