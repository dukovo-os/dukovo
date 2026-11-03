#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds tcpdump for Radix (ob(1), PORTS) on the libpcap port, which
# configure finds through libpcap.pc (and not in a folder beside its
# source), and installs it and its manual page. Every printer is built
# but SMB's, which tcpdump's configure leaves out by default; without
# libcrypto it does not decrypt ESP, and without libsmi it names no SNMP
# MIB objects. It changes to no other user and no other root folder: a
# program on Dukovo has only the rights it was given. The notices of
# every file it is compiled from must be those image/licenses keeps;
# libpcap's notices.py finds them.
set -eu

cd "$PORT_BUILD"
# -MD writes what each object was compiled from, for its notices.
CFLAGS="$CFLAGS -MD" x86_64-radix-configure "$PORT_SRC" --disable-local-libpcap \
	--without-crypto --without-cap-ng --without-smi --without-sandbox-capsicum >configure.log 2>&1 || {
	tail -n 40 configure.log >&2
	exit 1
}
# configure takes the build machine's <netinet/ip6.h> for a stack of
# Linux's libinet6, and its folder for one of the include path's.
sed -i 's| -I/usr/inet6/include||' Makefile
make -j"$JOBS" tcpdump >make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}

# Every notice of the files it was compiled from, which the image keeps
# as the text of LicenseRef-tcpdump-notices (ADR 0346).
python3 -I "$PORT_DIR/../libpcap/notices.py" "$PORT_SRC" . "tcpdump 4.99.6" LICENSE \
	>"$PORT_INFO/notices.txt"
kept="$PORT_DIR/../../image/licenses/LICENSE-LicenseRef-tcpdump-notices.txt"
cmp -s "$PORT_INFO/notices.txt" "$kept" || {
	echo "tcpdump: its notices are not those of $kept; they are in $PORT_INFO/notices.txt" >&2
	diff -u "$kept" "$PORT_INFO/notices.txt" | head -n 40 >&2
	exit 1
}

bin="$SYSROOT/system/commands"
man="$SYSROOT/system/resources/man/man1"
mkdir -p "$bin" "$man"
cp tcpdump "$bin/tcpdump"
$STRIP "$bin/tcpdump"
cp tcpdump.1 "$man/tcpdump.1"
