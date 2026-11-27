#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds libxkbcommon's static library for Radix (ob(1), PORTS) and
# installs it, its headers and its pkg-config file into the sysroot, for
# windowd. Where it looks for the keyboard data is said here, not taken
# from the xkeyboard-config port's pkg-config file: the folders are
# Dukovo's own, and the library names all four of them once it names one.
set -eu

meson setup "$PORT_BUILD" "$PORT_SRC" \
	--cross-file "$CROSS_FILE" \
	--prefix="$PREFIX" --libdir=libraries --includedir=include \
	--buildtype=release \
	-Ddefault_library=static \
	-Denable-x11=false -Denable-wayland=false -Denable-xkbregistry=false \
	-Denable-tools=false -Denable-docs=false -Denable-bash-completion=false \
	-Dxkb-config-root=/system/resources/xkb \
	-Dxkb-config-unversioned-extensions-path=/system/resources/xkb.d \
	-Dxkb-config-versioned-extensions-path=/system/resources/xkb-2.d \
	-Dxkb-config-extra-path=/system/settings/xkb \
	-Dx-locale-root=/system/resources/xkb/locale \
	>configure.log 2>&1 || {
	tail -n 40 configure.log >&2
	exit 1
}
meson compile -C "$PORT_BUILD" -j "$JOBS" >build.log 2>&1 || {
	tail -n 60 build.log >&2
	exit 1
}
DESTDIR="$SYSROOT" meson install -C "$PORT_BUILD" --no-rebuild --quiet
