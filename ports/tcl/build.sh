#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds Tcl for Radix (ob(1), PORTS) and installs it into the toolchain,
# $TOOLCHAIN/radix-tcl: libtcl9.1.a, tclConfig.sh, the headers and the
# script library, tcl9.1/. It exists to build SQLite's test program,
# testfixture, which the sqlite port packs for the lab (ADR 0511); its
# configuration is the one that port reads, and nothing of it goes into
# the sysroot, so no other port's configure finds Tcl.
#
# Tcl's configure takes its settings from the build machine's uname, which
# says Linux: Radix's ABI is Linux's x86-64 one, so those are right. The
# library is static and loads nothing at run time, as every program on
# Radix is and does (--disable-shared, --disable-load). Tcl 9 keeps its
# script library in a ZIP archive that its zipfs mounts, and making the
# archive needs a compiler and a zip program of the build machine's own;
# --disable-zipfs leaves the library a folder, which the lab's tarball
# carries and TCL_LIBRARY names. The prefix is given again after the
# options x86_64-radix-configure passes, as the last of each wins: Tcl's
# files belong in one folder of the toolchain, not in /system.
set -eu

tcl="$TOOLCHAIN/radix-tcl"
rm -rf "$tcl"

cd "$PORT_BUILD"
x86_64-radix-configure "$PORT_SRC/unix" --prefix="$tcl" --exec-prefix="$tcl" \
	--bindir="$tcl/bin" --sbindir="$tcl/bin" --libdir="$tcl/lib" --libexecdir="$tcl/lib" \
	--datarootdir="$tcl/share" --mandir="$tcl/share/man" --disable-shared --disable-load \
	--disable-zipfs >configure.log 2>&1 || {
	tail -n 40 configure.log >&2
	exit 1
}
make -j"$JOBS" binaries >make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}
# The manual pages and the packages of Tcl's own tree (Tcl 9.1 keeps none
# in its repository) are left out, as nothing reads them.
make install-binaries install-libraries install-tzdata install-msgs install-headers \
	>install.log 2>&1 || {
	tail -n 60 install.log >&2
	exit 1
}

for file in lib/libtcl9.1.a lib/tclConfig.sh include/tcl.h lib/tcl9.1/init.tcl; do
	if [ ! -f "$tcl/$file" ]; then
		echo "tcl: the build installed no $file in $tcl" >&2
		exit 1
	fi
done
