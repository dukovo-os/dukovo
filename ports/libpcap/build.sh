#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds libpcap for Radix (ob(1), PORTS): the static library, its headers,
# pcap-config and libpcap.pc in the sysroot for tcpdump, and the manual
# pages of its filters, link types, timestamps and files for the image.
# Its configure knows no capture for Radix, so it is configured for none
# (--with-pcap=null) and built with pcap-radix.c, this port's, in place of
# pcap-null.c, and with fad-getad.c, which lists the interfaces through
# getifaddrs; pcap_lookupnet is the module's (patch 0001). The parser is made with bison and flex, as its Makefile
# makes it; nothing of it needs D-Bus, netlink, USB or a remote capture.
# The notices of every file it is compiled from must be those
# image/licenses keeps.
set -eu

cd "$PORT_BUILD"
# Its Makefile compiles each source of $(srcdir).
cp "$PORT_DIR/pcap-radix.c" "$PORT_SRC/pcap-radix.c"
# -MD writes what each object was compiled from, for its notices.
CFLAGS="$CFLAGS -MD" x86_64-radix-configure "$PORT_SRC" --disable-shared --with-pcap=null --disable-remote \
	--disable-usb --disable-netmap --disable-bluetooth --disable-dbus --disable-rdma \
	--without-libnl --without-dag --without-septel --without-snf --without-turbocap \
	--without-dpdk >configure.log 2>&1 || {
	tail -n 40 configure.log >&2
	exit 1
}
platform="PLATFORM_C_SRC=pcap-radix.c fad-getad.c"
make -j"$JOBS" "$platform" libpcap.a >make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}
make "$platform" install DESTDIR="$SYSROOT" >install.log 2>&1 || {
	tail -n 40 install.log >&2
	exit 1
}

# Every notice of the files it was compiled from, which the image keeps
# as the text of LicenseRef-libpcap-notices (ADR 0346).
python3 -I "$PORT_DIR/notices.py" "$PORT_SRC" . "libpcap 1.10.7" LICENSE \
	grammar.y.in scanner.l \
	>"$PORT_INFO/notices.txt"
kept="$PORT_DIR/../../image/licenses/LICENSE-LicenseRef-libpcap-notices.txt"
cmp -s "$PORT_INFO/notices.txt" "$kept" || {
	echo "libpcap: its notices are not those of $kept; they are in $PORT_INFO/notices.txt" >&2
	diff -u "$kept" "$PORT_INFO/notices.txt" | head -n 40 >&2
	exit 1
}

man=system/resources/man
for page in man7/pcap-filter.7 man7/pcap-linktype.7 man7/pcap-tstamp.7 man5/pcap-savefile.5; do
	test -f "$SYSROOT/$man/$page"
	echo "$man/$page"
done >"$PORT_INFO/image"
