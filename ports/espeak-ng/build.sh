#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds eSpeak NG for Radix (ob(1), PORTS) and installs espeak-ng and the
# voice data of all its languages in /system/resources/espeak-ng-data.
# The data is compiled by espeak-ng itself, and one for Radix cannot run
# here, so espeak-ng is first built for the build machine, which compiles
# it: the compiled data is the same on any little-endian machine. Only
# the espeak-ng program is built for Radix, with the library built into
# it, and without what readerd(8) does not need: MBROLA voices, libsonic,
# a sound output of its own (it writes WAV with --stdout, which readerd
# plays), the asynchronous mode and speechPlayer (a C++ engine; the
# Klatt voices stay). Russian's extended word list is left out: it makes
# its dictionary 8 MiB of the data's 19, and the live system holds the
# data in memory. Russian is still spoken, by its rules and its main
# list, as with eSpeak NG's --with-extdict-ru=no.
set -eu

options="-DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DBUILD_TESTING=OFF
-DUSE_MBROLA=OFF -DUSE_LIBSONIC=OFF -DUSE_LIBPCAUDIO=OFF -DUSE_ASYNC=OFF
-DUSE_SPEECHPLAYER=OFF -DUSE_KLATT=ON -DESPEAK_BUILD_MANPAGES=OFF -DEXTRA_ru=OFF -Wno-dev"

# The build machine's espeak-ng, and the data it compiles. CMake would
# take Radix's toolchain from the environment.
host=$PORT_BUILD/host
# shellcheck disable=SC2086 # the options are words
{
	env -u CMAKE_TOOLCHAIN_FILE -u PKG_CONFIG_LIBDIR -u PKG_CONFIG_SYSROOT_DIR CC=cc CFLAGS=-O2 \
		cmake -S "$PORT_SRC" -B "$host" $options &&
		cmake --build "$host" -j "$JOBS" --target data
} >"$PORT_BUILD/host.log" 2>&1 || {
	tail -n 60 "$PORT_BUILD/host.log" >&2
	exit 1
}

# Radix's espeak-ng, which looks for the data where Dukovo keeps it
# (patches/0001).
radix=$PORT_BUILD/radix
# shellcheck disable=SC2086
{
	cmake -S "$PORT_SRC" -B "$radix" $options -DCMAKE_TOOLCHAIN_FILE="$CMAKE_TOOLCHAIN_FILE" \
		-DCMAKE_INSTALL_PREFIX="$PREFIX" -DESPEAK_DATA_DIR="$PREFIX/resources/espeak-ng-data" &&
		cmake --build "$radix" -j "$JOBS" --target espeak-ng-bin
} >"$PORT_BUILD/radix.log" 2>&1 || {
	tail -n 60 "$PORT_BUILD/radix.log" >&2
	exit 1
}

mkdir -p "$SYSROOT/system/commands" "$SYSROOT/system/resources"
cp "$radix/src/espeak-ng" "$SYSROOT/system/commands/espeak-ng"
$STRIP "$SYSROOT/system/commands/espeak-ng"
rm -rf "$SYSROOT/system/resources/espeak-ng-data"
cp -R "$host/espeak-ng-data" "$SYSROOT/system/resources/espeak-ng-data"

{
	echo "system/commands/espeak-ng"
	(cd "$SYSROOT" && find system/resources/espeak-ng-data -type f | LC_ALL=C sort)
} >"$PORT_INFO/image"
