#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds libevent for Radix (ob(1), PORTS): the static libraries, without
# OpenSSL, their headers and pkg-config files in the sysroot for the ports
# built on it. Nothing of it goes in the image by itself. libevent waits
# with poll, which Radix has. Its kqueue(2) has no EVFILT_SIGNAL, which
# libevent's kqueue backend needs, so configure is told there is no
# kqueue. The C library has epoll, eventfd and timerfd (ADR 0612, 0613),
# and configure is told it has none until a boot test runs libevent's
# epoll backend and its eventfd and timerfd uses (tmux is built on it).
set -eu

cd "$PORT_BUILD"
ac_cv_func_kqueue=no \
	ac_cv_header_sys_epoll_h=no ac_cv_func_epoll_ctl=no ac_cv_func_epoll_create1=no \
	ac_cv_header_sys_eventfd_h=no ac_cv_func_eventfd=no \
	ac_cv_header_sys_timerfd_h=no ac_cv_func_timerfd_create=no \
	x86_64-radix-configure "$PORT_SRC" --disable-shared --disable-openssl --disable-samples \
	--disable-libevent-regress --disable-debug-mode >configure.log 2>&1 || {
	tail -n 40 configure.log >&2
	exit 1
}
make -j"$JOBS" >make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}
make install DESTDIR="$SYSROOT" >/dev/null
# libtool's notes on the libraries name their place without the sysroot,
# and event_rpcgen.py needs a Python Radix does not have yet.
rm -f "$SYSROOT"/system/libraries/libevent*.la "$SYSROOT/system/commands/event_rpcgen.py"

: >"$PORT_INFO/image"
