#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds iperf3 for Radix (ob(1), PORTS), without OpenSSL, SCTP or
# libiperf's shared library, and installs it and its manual page.
set -eu

cd "$PORT_BUILD"
x86_64-radix-configure "$PORT_SRC" --disable-shared --without-openssl --without-sctp \
	--disable-profiling >configure.log 2>&1 || {
	tail -n 40 configure.log >&2
	exit 1
}
make -j"$JOBS" >make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}
make install DESTDIR="$SYSROOT" >/dev/null
rm -f "$SYSROOT/system/libraries/libiperf.la" "$SYSROOT/system/libraries/libiperf.a" \
	"$SYSROOT/system/include/iperf_api.h" "$SYSROOT/system/resources/man/man3/libiperf.3"
$STRIP "$SYSROOT/system/commands/iperf3"
