#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds nc for Radix (ob(1), PORTS) with its Makefile, on compat.c in
# place of libbsd, and installs it and its manual page.
set -eu

cd "$PORT_SRC"
make -j"$JOBS" CC=x86_64-radix-cc CFLAGS="-O2 -D_GNU_SOURCE" LDFLAGS= LIBS= \
	SRCS="netcat.c atomicio.c socks.c compat.c" >"$PORT_BUILD/make.log" 2>&1 || {
	tail -n 60 "$PORT_BUILD/make.log" >&2
	exit 1
}
install -D -m 0755 nc "$SYSROOT/system/commands/nc"
install -D -m 0644 nc.1 "$SYSROOT/system/resources/man/man1/nc.1"
$STRIP "$SYSROOT/system/commands/nc"
