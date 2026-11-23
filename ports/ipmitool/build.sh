#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds ipmitool for Radix (ob(1), PORTS) with its `open` interface only,
# which talks to the BMC through /devices/ipmi0 with Linux's ioctls, and
# installs it and its manual page. Its release archive is a snapshot of
# the repository, with no configure, and ipmitool's Makefile.am downloads
# IANA's registry of enterprise numbers when `make install` runs, so this
# does not run automake, libtool or configure: it writes the config.h that
# configure writes for the `open` interface (with no LAN, LAN plus,
# serial, USB, D-Bus, Intel IMB, FreeIPMI, BMC or readline code, and no
# ipmievd) and compiles the sources of lib/, src/ and the plugin that
# Makefile.am names, with the MD5 code of the LAN plugin's folder, which
# the firmware upgrade command links. The system's header for the interface is not used:
# the plugin's own open.h has Linux's x86-64 structures, 40 bytes for a
# request and 48 for a received message, which the check below makes the
# compiler confirm. The registry is not installed: ipmitool names a
# manufacturer from its table without it, and says nothing about the
# missing file unless run with -v (patch 0001).
set -eu

cd "$PORT_BUILD"
src=$PORT_SRC
version=1.8.19

cat >config.h <<CONFIG
/* What configure finds on Radix for ipmitool's open interface
   (ports/ipmitool/build.sh). */
#define PACKAGE "ipmitool"
#define VERSION "$version"
#define HAVE_CONFIG_H 1
#define HAVE_PRAGMA_PACK 1
#define HAVE_PATHS_H 1
#define HAVE_TERMIOS_H 1
#define ENABLE_ALL_OPTIONS 1
#define IPMI_INTF_OPEN 1
#define IANADIR "/system/resources/misc"
#define IANAUSERDIR ".local/share/misc"
#define PATH_SEPARATOR "/"
CONFIG

# The structures open.c gives the ioctls must be Linux's.
cat >layout.c <<'LAYOUT'
#include <sys/ioctl.h>
#include <ipmitool/ipmi.h>
#include "open.h"
_Static_assert(sizeof(struct ipmi_req) == 40, "ipmi_req is 40 bytes");
_Static_assert(sizeof(struct ipmi_recv) == 48, "ipmi_recv is 48 bytes");
_Static_assert(IPMICTL_SEND_COMMAND == 0x8028690d, "IPMICTL_SEND_COMMAND");
_Static_assert(IPMICTL_RECEIVE_MSG_TRUNC == 0xc030690b, "IPMICTL_RECEIVE_MSG_TRUNC");
LAYOUT

flags="$CFLAGS -std=gnu17 -D_GNU_SOURCE -DHAVE_CONFIG_H -I$PORT_BUILD -I$src/include -I$src/src/plugins/open -Wall -Wno-pointer-sign"
library="helper ipmi_sdr ipmi_sel ipmi_sol ipmi_pef ipmi_lanp ipmi_fru ipmi_chassis ipmi_mc log
dimm_spd ipmi_sensor ipmi_channel ipmi_event ipmi_session ipmi_strings ipmi_user ipmi_raw
ipmi_oem ipmi_isol ipmi_sunoem ipmi_fwum ipmi_picmg ipmi_main ipmi_tsol ipmi_firewall
ipmi_kontronoem ipmi_hpmfwupg ipmi_sdradd ipmi_ekanalyzer ipmi_gendev ipmi_ime ipmi_delloem
ipmi_dcmi hpm2 ipmi_vita ipmi_lanp6 ipmi_cfgp ipmi_quantaoem ipmi_time"
# A makefile of the sources, so that make compiles them $JOBS at a time.
mkdir -p obj
{
	echo "CC = $CC"
	echo "FLAGS = $flags"
	objects=obj/layout.o
	printf 'obj/layout.o: layout.c\n\t@$(CC) $(FLAGS) -c layout.c -o $@\n'
	for name in $library; do
		objects="$objects obj/$name.o"
		printf 'obj/%s.o: %s/lib/%s.c\n\t@$(CC) $(FLAGS) -c %s/lib/%s.c -o $@\n' \
			"$name" "$src" "$name" "$src" "$name"
	done
	for file in src/ipmitool.c src/ipmishell.c src/plugins/open/open.c src/plugins/lan/md5.c; do
		name=$(basename "$file" .c)
		objects="$objects obj/$name.o"
		printf 'obj/%s.o: %s/%s\n\t@$(CC) $(FLAGS) -c %s/%s -o $@\n' \
			"$name" "$src" "$file" "$src" "$file"
	done
	name=ipmi_intf
	objects="$objects obj/$name.o"
	printf 'obj/%s.o: %s/src/plugins/ipmi_intf.c\n\t@$(CC) $(FLAGS) -DDEFAULT_INTF=\\"open\\" -c %s/src/plugins/ipmi_intf.c -o $@\n' \
		"$name" "$src" "$src"
	printf 'ipmitool: %s\n\t$(CC) -o $@ $^ -lm\n' "$objects"
} >Makefile
make -j"$JOBS" ipmitool >make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}

bin="$SYSROOT/system/commands"
man="$SYSROOT/system/resources/man/man1"
mkdir -p "$bin" "$man"
cp ipmitool "$bin/ipmitool"
$STRIP "$bin/ipmitool"
sed -e 's|@IANADIR@|/system/resources/misc|' -e 's|~/@IANAUSERDIR@|~/.local/share/misc|' \
	"$src/doc/ipmitool.1.in" >"$man/ipmitool.1"
