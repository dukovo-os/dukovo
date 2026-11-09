#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds BRLTTY for Radix (ob(1), PORTS; ADR 0326) and installs brltty and
# its tables in /system/resources/brltty.
#
# BRLTTY is built from its git tree, which has no configure script and no
# make dependencies, as its release archives do: configure is made with
# autoconf, from an aclocal.m4 that includes BRLTTY's own macros (the
# build machine needs no automake), and the dependencies with gendeps.py,
# which does what BRLTTY's Tools/gendeps does (Tcl is not needed).
#
# It is built with every braille driver in it, and the screen driver for
# Dukovo (Dukovo/, copied in) as its only one; without BrlAPI, speech,
# X, its own USB, Bluetooth and HID access (a display is reached through
# the serial port the kernel makes of it), and anything else the system
# lacks. Only brltty itself is built. The transfer tools beside two
# braille drivers, which only BrlAPI would build, are under the GPL and
# are deleted first, so that none of their code is built.
set -eu

cd "$PORT_SRC"
rm -f Drivers/Braille/EuroBraille/eutp_* Drivers/Braille/VisioBraille/vstp_*
rm -rf Drivers/Screen/Dukovo
cp -R "$PORT_DIR/Dukovo" Drivers/Screen/Dukovo
for macros in m4/*.m4; do
	echo "m4_include([$macros])"
done >aclocal.m4
autoconf >"$PORT_BUILD/autoconf.log" 2>&1 || {
	tail -n 40 "$PORT_BUILD/autoconf.log" >&2
	exit 1
}
python3 -I "$PORT_DIR/gendeps.py" "$PORT_SRC"

cd "$PORT_BUILD"
x86_64-radix-configure "$PORT_SRC" \
	--enable-standalone-programs --disable-i18n --disable-icu --disable-iconv \
	--disable-polkit --disable-api --disable-x --disable-expat --disable-liblouis \
	--disable-gpm --disable-speech-support --with-braille-driver=all \
	--with-screen-driver=dk --with-usb-package=no --with-bluetooth-package=no \
	--with-hid-package=no --with-rgx-package=no --with-fm-package=no --with-curses=no \
	--with-text-table=en-nabcc --with-tables-directory=/system/resources/brltty \
	--with-writable-directory=/system/live/brltty \
	--with-updatable-directory=/system/state/brltty \
	--with-configuration-file=braille/brltty.conf PKG_CONFIG=false \
	>configure.log 2>&1 || {
	tail -n 40 configure.log >&2
	exit 1
}
make -j"$JOBS" -C Programs brltty >make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}

mkdir -p "$SYSROOT/system/commands"
cp Programs/brltty "$SYSROOT/system/commands/brltty"
$STRIP "$SYSROOT/system/commands/brltty"

# The tables, as `make install-tables` lays them out: text, contraction,
# attributes, and the key bindings of each display and keyboard.
tables=$SYSROOT/system/resources/brltty
rm -rf "$tables"
(
	cd "$PORT_SRC/Tables"
	find Text Contraction Attributes Input Keyboard -type f \( -name '*.ttb' -o -name '*.tti' \
		-o -name '*.ctb' -o -name '*.cti' -o -name '*.atb' -o -name '*.ati' -o -name '*.ktb' \
		-o -name '*.kti' \) | LC_ALL=C sort
) >"$PORT_BUILD/tables"
while IFS= read -r table; do
	mkdir -p "$tables/$(dirname "$table")"
	cp "$PORT_SRC/Tables/$table" "$tables/$table"
done <"$PORT_BUILD/tables"

{
	echo "system/commands/brltty"
	sed 's|^|system/resources/brltty/|' "$PORT_BUILD/tables"
} >"$PORT_INFO/image"
