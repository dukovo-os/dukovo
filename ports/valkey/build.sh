#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds Valkey for Radix (ob(1), PORTS) into the bundle's folder
# ($PORT_BUNDLE, ADRs 0370 and 0492): bin/valkey-server, valkey-cli,
# valkey-benchmark and valkey-service, the settings it starts from in
# conf/, and Valkey's own annotated settings file in doc/.
#
# Valkey's Makefile takes the system from uname, which names the build
# machine's, so it is given a system it has no settings for: Radix, built
# as a generic POSIX system, with the poll(2) event loop patch 0001 adds,
# the C library's malloc in place of jemalloc, and _GNU_SOURCE, which
# Linux's C library needs no more than mlibc does, for strcasecmp and
# gettimeofday. TLS links OpenSSL's libraries from the openssl port, which
# keeps them for bundles in its folder beside its stamp; its headers are
# in CFLAGS too, as CFLAGS given to make replaces what libvalkey's
# Makefile adds to its own.
set -eu

openssl="$PORT_INFO/../openssl/for-bundles"
if [ ! -f "$openssl/libraries/libssl.a" ]; then
	echo "valkey: no OpenSSL libraries in $openssl: build the openssl port again" >&2
	exit 1
fi
# The layout OPENSSL_PREFIX names: include/ and lib/.
prefix="$PORT_BUILD/openssl"
mkdir -p "$prefix"
ln -sfn "$openssl/include" "$prefix/include"
ln -sfn "$openssl/libraries" "$prefix/lib"

cd "$PORT_SRC"
make -j"$JOBS" uname_S=Radix uname_M=x86_64 CC="$CC" AR="$AR" RANLIB="$RANLIB" \
	MALLOC=libc BUILD_TLS=yes USE_SYSTEMD=no OPENSSL_PREFIX="$prefix" PKG_CONFIG=false \
	OPTIMIZATION=-O2 CFLAGS="-D_GNU_SOURCE -I$prefix/include" V=1 all \
	>"$PORT_BUILD/make.log" 2>&1 || {
	tail -n 60 "$PORT_BUILD/make.log" >&2
	exit 1
}

mkdir -p "$PORT_BUNDLE/bin" "$PORT_BUNDLE/conf" "$PORT_BUNDLE/doc"
for program in valkey-server valkey-cli valkey-benchmark; do
	$STRIP -o "$PORT_BUNDLE/bin/$program" "src/$program"
done
install -m 0755 "$PORT_DIR/valkey-service" "$PORT_BUNDLE/bin/valkey-service"
install -m 0644 "$PORT_DIR/valkey.conf" "$PORT_BUNDLE/conf/valkey.conf"
# Every setting, with Valkey's own words on each, to read.
install -m 0644 valkey.conf "$PORT_BUNDLE/doc/valkey.conf"
