#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds SQLite for Radix (ob(1), PORTS, ADR 0511): libsqlite3.a, sqlite3.h,
# sqlite3ext.h and sqlite3.pc in the sysroot, for programs and ports to link
# statically. Three more things stay in $PORT_INFO for other builds:
# for-bundles/ holds the shell's source, its manual page and the compile
# options, which ports/sqlite-cli builds the shell from; for-lab/ holds
# sqlite-tests.tar.gz, SQLite's test program with its test scripts, which
# the lab's sqlite-suite test unpacks and runs on Radix.
#
# SQLite's tree is a repository's, not a release's, so its amalgamation
# (sqlite3.c), its shell (shell.c) and the headers are generated first, by
# the make rules of main.mk. configure runs for Radix as a cross build:
# the build machine's compiler (CC_FOR_BUILD) makes the code generators and
# Radix's the library, and configure probes Radix's C library by linking,
# never by running, which sets sqlite_cfg.h's HAVE_ macros from what mlibc
# has. SQLite's autosetup sends the host name through its own
# autosetup-config.sub, which does not know Radix, so that script is made
# unrunnable: autosetup then takes the name as it is.
#
# One set of compile options, $options, builds the library, the shell and
# testfixture alike, so that the suite proves what the library is. They are
# SQLite's defaults but for these:
#   SQLITE_THREADSAFE=1: serialized, as admind's worker threads need.
#   SQLITE_OMIT_LOAD_EXTENSION: a Radix program is static and loads
#     nothing at run time.
#   FTS4 (which gives FTS3), FTS5, R-Tree, the dbstat table and column
#     metadata: the extensions Dukovo's programs and Python's sqlite3
#     module expect.
#   SQLITE_ENABLE_MATH_FUNCTIONS: SQLite's configure turns the math
#     functions on but the amalgamation does not, so a build that does not
#     go through configure's flags needs it.
#   SQLITE_MAX_MMAP_SIZE=0: Radix gives a read-only shared mapping of a
#     file as the process's own copy, which does not see later writes, so
#     memory-mapped I/O would read stale pages, the connection's own
#     included.
#   HAVE_MREMAP=0: mlibc's mremap has no system call under it on Radix.
#   SQLITE_MAX_PATHLEN=4096: SQLite sizes the buffer for the working
#     folder, which it resolves a relative name from, by FILENAME_MAX, and
#     mlibc's is 256, so a database named relative to a folder of 255
#     bytes or more would not open. PATH_MAX, which Radix's paths allow, is
#     4096.
# The unix VFS keeps the index of a WAL database in a shared writable
# mapping of the -shm file. Radix refuses that for any file outside
# /dev/shm, so a WAL database shared between connections fails with
# SQLITE_IOERR_SHMMAP until the kernel maps page-cache pages shared. A
# connection in exclusive locking mode keeps the index in memory and makes
# no -shm file, so WAL works for it; the rollback journal works across
# processes with fcntl locks. See ADR 0511 and docs/deferred.md.
#
# testfixture is made by main.mk's own rule, with Radix's Tcl from
# $TOOLCHAIN/radix-tcl. The rule asks TCLSH_CMD, which would be Radix's
# tclsh and cannot run here, whether it is Tcl 8.5 or later and where its
# libraries are; `true` and TCLLIBDIR answer both, as nothing of Tcl runs
# during this build.
set -eu

options="-DSQLITE_THREADSAFE=1 -DSQLITE_OMIT_LOAD_EXTENSION -DSQLITE_ENABLE_FTS4"
options="$options -DSQLITE_ENABLE_FTS5 -DSQLITE_ENABLE_RTREE -DSQLITE_ENABLE_DBSTAT_VTAB"
options="$options -DSQLITE_ENABLE_COLUMN_METADATA -DSQLITE_ENABLE_MATH_FUNCTIONS"
options="$options -DSQLITE_MAX_MMAP_SIZE=0 -DHAVE_MREMAP=0 -DSQLITE_MAX_PATHLEN=4096"

tcl="$TOOLCHAIN/radix-tcl"
if [ ! -f "$tcl/lib/tclConfig.sh" ]; then
	echo "sqlite: no Tcl in $tcl: build the tcl port again" >&2
	exit 1
fi

chmod -x "$PORT_SRC/autosetup/autosetup-config.sub"
cd "$PORT_BUILD"
CC="$CC" CC_FOR_BUILD=cc AR="$AR" RANLIB="$RANLIB" STRIP="$STRIP" CFLAGS="$CFLAGS" \
	"$PORT_SRC/configure" --host=x86_64-pc-radix --prefix="$PREFIX" --libdir="$PREFIX/libraries" \
	--disable-shared --enable-static --disable-load-extension --with-tcl="$tcl/lib" \
	>configure.log 2>&1 || {
	tail -n 40 configure.log >&2
	exit 1
}

make -j"$JOBS" sqlite3.c sqlite3.h sqlite3ext.h shell.c >generate.log 2>&1 || {
	tail -n 60 generate.log >&2
	exit 1
}
make libsqlite3.a OPT_FEATURE_FLAGS="$options" >library.log 2>&1 || {
	tail -n 60 library.log >&2
	exit 1
}

mkdir -p "$SYSROOT$PREFIX/libraries/pkgconfig" "$SYSROOT$PREFIX/include"
install -m 0644 libsqlite3.a "$SYSROOT$PREFIX/libraries/libsqlite3.a"
install -m 0644 sqlite3.h sqlite3ext.h "$SYSROOT$PREFIX/include"
cat >"$SYSROOT$PREFIX/libraries/pkgconfig/sqlite3.pc" <<EOF
prefix=$PREFIX
exec_prefix=\${prefix}
libdir=$PREFIX/libraries
includedir=\${prefix}/include

Name: SQLite
Description: SQL database engine
Version: 3.54.1
Libs: -L\${libdir} -lsqlite3
Libs.private: -lm -lpthread
Cflags: -I\${includedir}
EOF
for file in libraries/libsqlite3.a include/sqlite3.h include/sqlite3ext.h \
	libraries/pkgconfig/sqlite3.pc; do
	if [ ! -f "$SYSROOT$PREFIX/$file" ]; then
		echo "sqlite: the build installed no $file in the sysroot" >&2
		exit 1
	fi
done

# What the shell is built from: the generated shell.c, the manual page and
# the options the library was built with, as one line of flags.
mkdir -p "$PORT_INFO/for-bundles"
cp shell.c "$PORT_SRC/sqlite3.1" "$PORT_INFO/for-bundles"
echo "$options" >"$PORT_INFO/for-bundles/options"

# testfixture, which main.mk adds the test-only options to (SQLITE_TEST and
# the extensions the tests use) and links without a runtime library path.
make testfixture OPT_FEATURE_FLAGS="$options" TCLSH_CMD=true TCLLIBDIR=/unused \
	LDFLAGS.rpath= >testfixture.log 2>&1 || {
	tail -n 60 testfixture.log >&2
	exit 1
}

# The test scripts run from one folder: test/, and beside it the extensions'
# own tests, which tester.tcl and the suites reach as ../ext/NAME; fts3/
# unicode holds the tables and the script that fts5's unicode test reads.
# Tcl's script library goes in as tcl9.1/ and its modules as tcl9/, where
# Tcl looks for them from TCL_LIBRARY. testrunner.tcl veryquick runs the
# files of test/ and of these extensions.
tests="$PORT_BUILD/sqlite-tests"
mkdir -p "$tests/ext" "$PORT_INFO/for-lab"
$STRIP -o "$tests/testfixture" testfixture
cp -R "$PORT_SRC/test" "$tests/test"
for dir in expert fts3/unicode fts5/test intck rbu recover rtree session vec1/test; do
	mkdir -p "$tests/ext/$dir"
	cp -R "$PORT_SRC/ext/$dir/." "$tests/ext/$dir"
done
find "$tests/ext" \( -name '*.c' -o -name '*.h' \) -delete
cp -R "$tcl/lib/tcl9.1" "$tests/tcl9.1"
cp -R "$tcl/lib/tcl9" "$tests/tcl9"
tar -C "$PORT_BUILD" --sort=name --owner=0 --group=0 --numeric-owner \
	--mtime="@$SOURCE_DATE_EPOCH" -cf - sqlite-tests |
	gzip -n -9 >"$PORT_INFO/for-lab/sqlite-tests.tar.gz"
