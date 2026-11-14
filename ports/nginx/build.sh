#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds nginx for Radix (ob(1), PORTS) into the bundle's folder
# ($PORT_BUNDLE, ADRs 0370 and 0491): bin/nginx, bin/nginx-service, the
# settings it starts from in conf/, its welcome page in html/ and its
# manual page.
#
# nginx's configure is its own, not Autoconf's: patch 0001 lets it
# configure a cross build (NGX_CROSS=yes), and --crossbuild names a
# system it has no file of, so it builds as for a POSIX system, with
# poll; select is left out, so that nginx does not choose it. PCRE2 is
# built here from its sources, as a static library, and OpenSSL's
# libraries come from the openssl port, which keeps them for bundles in
# its folder beside its stamp.
set -eu

openssl="$PORT_INFO/../openssl/for-bundles"
if [ ! -f "$openssl/libraries/libssl.a" ]; then
	echo "nginx: no OpenSSL libraries in $openssl: build the openssl port again" >&2
	exit 1
fi

# PCRE2's 8-bit library, with Unicode, as NON-AUTOTOOLS-BUILD says,
# without JIT.
pcre="$PORT_BUILD/pcre2"
mkdir -p "$pcre/include" "$pcre/objects"
cp "$PORT_SRC/pcre2/src/config.h.generic" "$pcre/include/config.h"
cp "$PORT_SRC/pcre2/src/pcre2.h.generic" "$pcre/include/pcre2.h"
cp "$PORT_SRC/pcre2/src/pcre2_chartables.c.dist" "$PORT_SRC/pcre2/src/pcre2_chartables.c"
for name in auto_possess chkdint chartables compile compile_cgroup compile_class config \
	context convert dfa_match error extuni find_bracket jit_compile maketables match \
	match_data match_next newline ord2utf pattern_info script_run serialize string_utils \
	study substitute substring tables ucd valid_utf xclass; do
	$CC -O2 -DHAVE_CONFIG_H -DPCRE2_STATIC -DPCRE2_CODE_UNIT_WIDTH=8 -DSUPPORT_UNICODE \
		-I"$pcre/include" -I"$PORT_SRC/pcre2/src" \
		-c "$PORT_SRC/pcre2/src/pcre2_$name.c" -o "$pcre/objects/$name.o"
done
$AR rcs "$pcre/libpcre2-8.a" "$pcre"/objects/*.o

# The paths nginx is built with: the bundle's folder, where it is
# installed for everyone; the data folder of its account, where its
# settings are; its account's live folder for its process number; and
# the standard error output for its log, which Spark keeps in the
# system's. _GNU_SOURCE, which nginx's own file for Linux sets, brings
# the C library's struct in6_pktinfo, which its UDP code uses.
id=org.nginx.nginx
home="/shared/services/$id/nginx"
live=/system/live/services/nginx
cd "$PORT_SRC"
NGX_CROSS=yes CC="$CC" ./auto/configure --crossbuild=Radix::x86_64 \
	--builddir="$PORT_BUILD/objs" \
	--prefix="/applications/$id" --sbin-path="/applications/$id/bin/nginx" \
	--conf-path="$home/conf/nginx.conf" --error-log-path=stderr \
	--http-log-path="$home/logs/access.log" \
	--pid-path="$live/nginx.pid" --lock-path="$live/nginx.lock" \
	--http-client-body-temp-path="$home/temp/client_body" \
	--http-proxy-temp-path="$home/temp/proxy" \
	--http-fastcgi-temp-path="$home/temp/fastcgi" \
	--http-uwsgi-temp-path="$home/temp/uwsgi" \
	--http-scgi-temp-path="$home/temp/scgi" \
	--user=nginx --group=nginx \
	--with-http_ssl_module --with-http_v2_module --with-http_realip_module \
	--with-http_sub_module --with-http_gunzip_module --with-http_gzip_static_module \
	--with-http_auth_request_module --with-http_secure_link_module \
	--with-http_slice_module --with-http_stub_status_module \
	--with-stream --with-stream_ssl_module --with-stream_realip_module \
	--with-stream_ssl_preread_module \
	--with-pcre --without-select_module \
	--with-cc-opt="-O2 -D_GNU_SOURCE -I$pcre/include -I$openssl/include" \
	--with-ld-opt="-L$pcre -L$openssl/libraries" \
	>"$PORT_BUILD/configure.log" 2>&1 || {
	tail -n 40 "$PORT_BUILD/configure.log" >&2
	tail -n 40 "$PORT_BUILD/objs/autoconf.err" >&2
	exit 1
}
make -j"$JOBS" >"$PORT_BUILD/make.log" 2>&1 || {
	tail -n 60 "$PORT_BUILD/make.log" >&2
	exit 1
}

mkdir -p "$PORT_BUNDLE/bin" "$PORT_BUNDLE/conf" "$PORT_BUNDLE/html" "$PORT_BUNDLE/man/man8"
$STRIP -o "$PORT_BUNDLE/bin/nginx" "$PORT_BUILD/objs/nginx"
install -m 0755 "$PORT_DIR/nginx-service" "$PORT_BUNDLE/bin/nginx-service"
# The settings it starts from: Dukovo's nginx.conf, and nginx's own
# files beside it, which nginx.conf includes.
install -m 0644 "$PORT_DIR/nginx.conf" "$PORT_BUNDLE/conf/nginx.conf"
# The location every site includes for an ACME authority's checks (ADR 0513).
install -m 0644 "$PORT_DIR/acme-challenge.conf" "$PORT_BUNDLE/conf/acme-challenge.conf"
for file in mime.types fastcgi.conf fastcgi_params scgi_params uwsgi_params; do
	install -m 0644 "conf/$file" "$PORT_BUNDLE/conf/$file"
done
install -m 0644 docs/html/index.html docs/html/50x.html "$PORT_BUNDLE/html/"
# Its manual page, with the paths it was built with, as nginx's own
# install makes it.
sed -e "s|%%PREFIX%%|/applications/$id|" -e "s|%%PID_PATH%%|$live/nginx.pid|" \
	-e "s|%%CONF_PATH%%|$home/conf/nginx.conf|" -e "s|%%ERROR_LOG_PATH%%|stderr|" \
	docs/man/nginx.8 >"$PORT_BUNDLE/man/man8/nginx.8"
