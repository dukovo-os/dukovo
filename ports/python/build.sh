#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds CPython for Radix (ob(1), PORTS) into the bundle's folder
# ($PORT_BUNDLE, ADRs 0370 and 0495): bin/python3 with every module of
# the standard library that has its C code built in, the library itself
# in lib/python3.15, compiled, with its test suite, and pip.
#
# A cross build of CPython runs a Python of the same version on the build
# machine, to freeze modules and write the build's own files, so one is
# built first, for Linux, in host/. Radix links programs statically and
# loads no library at run time, so the cross build makes every extension
# module part of the interpreter (MODULE_BUILDTYPE=static) and has no
# dlopen to import one with. Its prefix is the folder of an installation
# for everyone; Python finds its library from its own path, which the
# kernel gives (patch 0003), so a person's own installation, in
# ~/applications, finds its library as well.
set -eu

id=org.python.python
version=3.15
prefix="/applications/$id"

openssl="$PORT_INFO/../openssl/for-bundles"
if [ ! -f "$openssl/libraries/libssl.a" ]; then
	echo "python: no OpenSSL libraries in $openssl: build the openssl port again" >&2
	exit 1
fi
# The layout --with-openssl names: include/ and lib/.
ssl="$PORT_BUILD/openssl"
mkdir -p "$ssl"
ln -sfn "$openssl/include" "$ssl/include"
ln -sfn "$openssl/libraries" "$ssl/lib"

# The build machine's Python, with what the build machine has.
mkdir -p "$PORT_BUILD/host"
cd "$PORT_BUILD/host"
env CC=cc CXX=c++ AR=ar RANLIB=ranlib NM=nm STRIP=strip CFLAGS=-O2 \
	PKG_CONFIG_LIBDIR= PKG_CONFIG_SYSROOT_DIR= CONFIG_SITE= \
	"$PORT_SRC/configure" --prefix="$PORT_BUILD/host-install" --without-ensurepip \
	--disable-test-modules >configure.log 2>&1 || {
	tail -n 40 configure.log >&2
	exit 1
}
make -j"$JOBS" >make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}

mkdir -p "$PORT_BUILD/radix"
cd "$PORT_BUILD/radix"
# configure cannot run what it builds for Radix: /dev/ptmx is there, and
# /dev/ptc, AIX's, is not. getaddrinfo answers as the standard says. The
# C library's backtrace(3) looks for libgcc_s, which a static program
# cannot load, and says so on standard error each time Python calls it
# as it starts: without execinfo.h, Python does not.
MODULE_BUILDTYPE=static PREFIX="$prefix" \
	ac_cv_file__dev_ptmx=yes ac_cv_file__dev_ptc=no ac_cv_buggy_getaddrinfo=no \
	ac_cv_header_execinfo_h=no \
	ac_cv_func_dlopen=no ac_cv_lib_dl_dlopen=no \
	x86_64-radix-configure "$PORT_SRC" --bindir="$prefix/bin" --libdir="$prefix/lib" \
	--datarootdir="$prefix/share" --sysconfdir="$prefix/etc" --localstatedir="$prefix/var" \
	--with-build-python="$PORT_BUILD/host/python" --disable-shared --without-ensurepip \
	--with-openssl="$ssl" --without-system-libmpdec --without-mimalloc --enable-ipv6 \
	>configure.log 2>&1 || {
	tail -n 40 configure.log >&2
	exit 1
}
make -j"$JOBS" >make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}
make install DESTDIR="$PORT_BUILD/dest" >install.log 2>&1 || {
	tail -n 60 install.log >&2
	exit 1
}

# What the bundle keeps of the install: the interpreter, without its
# symbols, and the library, without what serves only a program built
# against Python's C library (libpython, its headers, python3-config),
# which nothing on Radix could load, and Tk's modules and IDLE, as
# Dukovo has no Tk.
from="$PORT_BUILD/dest$prefix"
lib="$PORT_BUNDLE/lib/python$version"
mkdir -p "$PORT_BUNDLE/bin" "$PORT_BUNDLE/lib"
$STRIP -o "$PORT_BUNDLE/bin/python3" "$from/bin/python$version"
cp -R "$from/lib/python$version" "$lib"
rm -rf "$lib/config-$version"* "$lib/idlelib" "$lib/tkinter" "$lib/turtledemo" "$lib/turtle.py" \
	"$lib/test/test_tkinter" "$lib/test/test_ttk" "$lib/test/test_idle"
# Python finds its platform's library by lib-dynload, where other systems
# keep extension modules: a file keeps it in the bundle.
printf '%s\n' "The extension modules are part of python3 on Dukovo, which loads no" \
	"library at run time. Python finds its library by this folder." >"$lib/lib-dynload/README"
install -m 0755 "$PORT_DIR/pip3" "$PORT_BUNDLE/bin/pip3"
# pip, from the wheel CPython carries for ensurepip: python3 -m pip
# installs for the person, in ~/.local, as the bundle's folder is sealed.
# CMake unpacks the wheel, a ZIP archive: the build machine's Python has
# no zlib where the build machine has no zlib headers, as in ob's
# container.
wheel=$(echo "$lib"/ensurepip/_bundled/pip-*.whl)
(cd "$lib/site-packages" && cmake -E tar xf "$wheel")
python="$PORT_BUILD/host/python"

# Compiled once here, as nobody may write beside the bundle's modules: a
# hash that is not checked, as the files of a sealed bundle do not change
# and keep no times of their own; the test suite is left to compile as it
# runs, which keeps the bundle smaller.
find "$lib" -name __pycache__ -type d -prune -exec rm -rf {} +
"$python" -I -m compileall -q -j "$JOBS" --invalidation-mode unchecked-hash \
	-s "$PORT_BUNDLE" -p "$prefix" -x "/python$version/test/" "$lib" >/dev/null
