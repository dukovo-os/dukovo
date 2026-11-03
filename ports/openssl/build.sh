#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds openssl for Radix (ob(1), PORTS) as Linux's x86-64 build, with
# its assembly, static, without modules or engines, and without what
# Radix lacks: the asynchronous jobs' contexts, kernel TLS and the
# kernel's crypto interface. Its folder is /etc/ssl's,
# /system/settings/ssl, where the default settings put the root
# certificates as cert.pem and certs/ (dukovo-certificates(7)) and its
# openssl.cnf. It installs into a folder of its own, and only the
# command, its section 1 manual pages and openssl.cnf go into the
# sysroot.
set -eu

cd "$PORT_BUILD"
stage="$PORT_BUILD/stage"
CC=x86_64-radix-cc AR=x86_64-radix-ar RANLIB=x86_64-radix-ranlib \
	perl "$PORT_SRC/Configure" linux-x86_64 --prefix=/system --libdir=libraries \
	--openssldir=/system/settings/ssl no-shared no-dso no-module no-engine \
	no-async no-ktls no-afalgeng no-tests >configure.log 2>&1 || {
	tail -n 40 configure.log >&2
	exit 1
}
make -j"$JOBS" build_programs >make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}
# Its pages keep their own names, openssl-x509.1, without the suffix
# OpenSSL gives them by default.
make install_programs install_man_docs MANSUFFIX= DESTDIR="$stage" >install.log 2>&1 || {
	tail -n 40 install.log >&2
	exit 1
}

install -D -m 0755 "$stage/system/bin/openssl" "$SYSROOT/system/commands/openssl"
$STRIP "$SYSROOT/system/commands/openssl"
echo system/commands/openssl >"$PORT_INFO/image"
mkdir -p "$SYSROOT/system/resources/man/man1"
# Only openssl's own pages: the others are its commands' old names, as
# passwd.1 and version.1, which would take other commands' places, and
# its Perl scripts', which are not installed.
for page in "$stage"/system/share/man/man1/openssl.1 "$stage"/system/share/man/man1/openssl-*.1; do
	name=$(basename "$page")
	install -m 0644 "$page" "$SYSROOT/system/resources/man/man1/$name"
	echo "system/resources/man/man1/$name" >>"$PORT_INFO/image"
done
install -D -m 0644 "$PORT_SRC/apps/openssl.cnf" \
	"$SYSROOT/system/resources/default-settings/ssl/openssl.cnf"
echo system/resources/default-settings/ssl/openssl.cnf >>"$PORT_INFO/image"
