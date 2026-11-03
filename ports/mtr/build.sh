#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds mtr and mtr-packet for Radix (ob(1), PORTS) and installs them and
# their manual pages. mtr's configure.ac finds what the C library has by
# building programs; this writes the config.h it would write for Radix,
# with ncurses, IPv6, the AS lookups of -y and -z (res_query) and Linux's
# queue of ICMP errors (linux/errqueue.h), without GTK, JSON output or
# capabilities, and compiles the sources Makefile.am names for them. No
# raw socket is there to open, so mtr-packet needs no privilege: it is
# installed as any other command.
set -eu

cd "$PORT_BUILD"
src=$PORT_SRC
version=0.95

cat >config.h <<CONFIG
/* What mtr's configure finds on Radix (ports/mtr/build.sh). */
#define PACKAGE_NAME "mtr"
#define PACKAGE_VERSION "$version"
#define ENABLE_IPV6 1
#define HAVE_ARPA_NAMESER_COMPAT_H 1
#define HAVE_CURSES 1
#define HAVE_CURSES_H 1
#define HAVE_ERROR_H 1
#define HAVE_FCNTL 1
#define HAVE_FCNTL_H 1
#define HAVE_GETOPT 1
#define HAVE_IPINFO 1
#define HAVE_LINUX_ERRQUEUE_H 1
#define HAVE_NCURSES_H 1
#define HAVE_NETINET_IN_H 1
#define HAVE_SOCKLEN_T
#define HAVE_STDIO_EXT_H 1
#define HAVE_SYS_SOCKET_H 1
#define HAVE_SYS_TYPES_H 1
#define HAVE_TIME_T 1
#define HAVE___FPENDING 1
#define SIZEOF_UNSIGNED_INT 4
#define SIZEOF_UNSIGNED_LONG 8
CONFIG

flags="$CFLAGS -std=gnu17 -D_GNU_SOURCE -I$PORT_BUILD -I$src -Wall -Wno-pointer-sign"
mtr="ui/mtr.c ui/net.c ui/cmdpipe.c ui/dns.c ui/raw.c ui/split.c ui/display.c ui/report.c
ui/select.c ui/utils.c ui/asn.c ui/curses.c packet/cmdparse.c packet/sockaddr.c"
packet="packet/packet.c packet/cmdparse.c packet/command.c packet/probe.c packet/timeval.c
packet/sockaddr.c packet/construct_unix.c packet/deconstruct_unix.c packet/probe_unix.c
packet/wait_unix.c"
# A makefile of the sources, so that make compiles them $JOBS at a time.
mkdir -p obj
{
	echo "CC = $CC"
	echo "FLAGS = $flags"
	for program in mtr packet; do
		eval sources=\$$program
		objects=
		for source in $sources; do
			object=obj/$program-$(echo "$source" | tr / -)
			object=${object%.c}.o
			objects="$objects $object"
			printf '%s: %s\n\t@$(CC) $(FLAGS) -c %s -o $@\n' "$object" "$src/$source" "$src/$source"
		done
		case $program in
		mtr) printf 'mtr:%s\n\t$(CC) -o $@ $^ -lncursesw -lm\n' "$objects" ;;
		packet) printf 'mtr-packet:%s\n\t$(CC) -o $@ $^\n' "$objects" ;;
		esac
	done
} >Makefile
make -j"$JOBS" mtr mtr-packet >make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}

bin="$SYSROOT/system/commands"
man="$SYSROOT/system/resources/man/man8"
mkdir -p "$bin" "$man"
for program in mtr mtr-packet; do
	cp "$program" "$bin/$program"
	$STRIP "$bin/$program"
	sh "$src/build-aux/mangen.sh" "$version" "$src/man/$program.8.in" "$man/$program.8"
done
