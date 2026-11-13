#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds PHP for Radix (ob(1), PORTS) into the bundle's folder
# ($PORT_BUNDLE, ADRs 0370, 0499 and 0520): bin/php, the command-line
# program, and bin/php-fpm, the FastCGI process manager, each with every
# extension built into it, the php.ini the first starts from in etc/ and
# the settings of the second and its service in conf/ and
# bin/php-fpm-service, their manual pages, and PHP's own test suite in
# tests/.
#
# PHP's source comes from git, which has no configure, parsers or
# scanners: buildconf makes configure (Autoconf is on the build machine),
# bison writes the parsers, and re2c the scanners. re2c is not on the
# build machine, so it is built first, for it, from the extra source. The
# cross build then makes every extension part of the program
# (--disable-shared --enable-static) and has no dlopen to load another
# with. Its prefix is the folder of an installation for everyone, the only
# one a bundle with a service has; the program finds its php.ini from its
# own path, which the kernel gives (patches 0001 and 0002). PHP-FPM is the
# program of a service, which keeps its settings in its account's data
# folder, the sysconfdir it is built with.
set -eu

id=net.php.php
prefix="/applications/$id"

openssl="$PORT_INFO/../openssl/for-bundles"
if [ ! -f "$openssl/libraries/libssl.a" ]; then
	echo "php: no OpenSSL libraries in $openssl: build the openssl port again" >&2
	exit 1
fi

# re2c for the build machine: only the program, from its bootstrap
# sources, so that it needs neither re2c nor Bison itself, and none of
# what else it builds (its other languages' programs, libraries, tests
# and benchmarks).
host="$PORT_BUILD/host"
{
	env -u CMAKE_TOOLCHAIN_FILE -u PKG_CONFIG_LIBDIR -u PKG_CONFIG_SYSROOT_DIR \
		CC=cc CXX=c++ CFLAGS=-O2 CXXFLAGS=-O2 \
		cmake -S "$PORT_SRC/re2c" -B "$host" -DCMAKE_BUILD_TYPE=Release \
		-DRE2C_BUILD_RE2D=OFF -DRE2C_BUILD_RE2GO=OFF -DRE2C_BUILD_RE2HS=OFF \
		-DRE2C_BUILD_RE2JAVA=OFF -DRE2C_BUILD_RE2JS=OFF -DRE2C_BUILD_RE2OCAML=OFF \
		-DRE2C_BUILD_RE2PY=OFF -DRE2C_BUILD_RE2RUST=OFF -DRE2C_BUILD_RE2SWIFT=OFF \
		-DRE2C_BUILD_RE2V=OFF -DRE2C_BUILD_RE2ZIG=OFF -DRE2C_BUILD_LIBS=OFF \
		-DRE2C_BUILD_TESTS=OFF -DRE2C_BUILD_BENCHMARKS=OFF &&
		cmake --build "$host" -j "$JOBS" --target re2c
} >"$PORT_BUILD/host.log" 2>&1 || {
	tail -n 60 "$PORT_BUILD/host.log" >&2
	exit 1
}

# configure, made from configure.ac.
cd "$PORT_SRC"
./buildconf --force >"$PORT_BUILD/buildconf.log" 2>&1 || {
	tail -n 40 "$PORT_BUILD/buildconf.log" >&2
	exit 1
}

# The account php's data folder, where the service keeps PHP-FPM's settings:
# php-fpm.conf and, with patch 0009, the php.ini PHP-FPM reads. This is
# PHP's sysconfdir, which only PHP-FPM reads: PHP_SYSCONFDIR is the
# default place of php-fpm.conf (fpm_conf.c), and with patch 0009 of its
# php.ini, and is the value of the constant PHP_SYSCONFDIR in PHP code.
# The command-line program finds its php.ini by the configured folder
# below and its own path.
sysconf="/shared/services/$id/php/conf"

mkdir -p "$PORT_BUILD/radix"
cd "$PORT_BUILD/radix"
# What configure cannot find out without running a program for Radix is
# answered here, each as Radix has it:
# - PHP_OS, PHP_UNAME and PHP_BUILD_PROVIDER name the system the program
#   runs on, not the build machine's, which uname would give (patch 0004);
# - dlopen is not there to call, as the program is static, and with
#   HAVE_LIBDL PHP would build dl() and the loading of extensions from
#   php.ini; AC_SEARCH_LIBS is the check that sets it (patch 0007 keeps the
#   mechanism of Zend extensions, which opcache is one of, without it);
# - the C library's getaddrinfo(3) answers as POSIX says, and PHP's lookups
#   of names for its streams use it; configure takes it to be so for a host
#   with linux in its name only;
# - the C library's gethostbyname_r(3) links, and finds no name; its
#   gethostbyname(3) does. Said not to be there, the reentrant form is not
#   used: PHP's gethostbyname() and the sockets extension's host names, as
#   socket_create_listen() has, take gethostbyname(3), which a PHP built
#   without threads calls from one thread only;
# - writing to standard output with write(2) works, which PHP's output
#   uses when it can;
# - the kernel has System V shared memory, anonymous shared mappings that
#   a forked process shares, and POSIX shared memory objects, the three
#   ways opcache can allocate the memory its processes share; with none
#   of them it would not start. Each is said to work, so that opcache may
#   choose (it tries them in that order).
# - PHP-FPM waits for requests with poll(2): Radix has no epoll, and
#   configure takes epoll for there when <sys/epoll.h> compiles (the
#   header is not in the C library's, and this keeps it so if it comes);
# - Radix has no ptrace(2) and no /proc/PID/mem, which PHP-FPM's slow log
#   reads a worker's stack with: configure takes ptrace to be there in a
#   cross build, and looks for the proc file on the build machine, which
#   has it. Said no and empty, the slow log is built out, not broken;
# - PHP-FPM reads the length of its listen queue with getsockopt(2)'s
#   TCP_INFO, which the C library's header names and the kernel has no
#   answer to: left in, every pass of its process manager would log the
#   failure;
# - the stack grows down, as on every x86-64 system. Configure runs a
#   program to find out, and in a cross build takes it not to, which
#   builds PHP without its check of how deep calls go: a function that
#   calls itself without end, or a comparison of objects nested too
#   deep, then ends the process with SIGSEGV where PHP throws an Error,
#   and a PHP-FPM worker dies with the request. PHP knows where Linux's
#   and the BSDs' stacks end, not Radix's, so it allows calls to go 2 MiB
#   below where it starts, less 32 KiB, its default for a system it does
#   not know; a program's stack is 8 MiB (radix-posix(7)).
# Left at configure's answer for a cross build, which is right for
# Radix: no sched_getcpu, no pread and pwrite tested (the session files
# handler then uses lseek and write), and no JIT.
OPENSSL_CFLAGS="-I$openssl/include" OPENSSL_LIBS="-L$openssl/libraries -lssl -lcrypto" \
	RE2C="$host/re2c" PREFIX="$prefix" \
	PHP_OS=Dukovo PHP_UNAME="Dukovo x86_64" PHP_BUILD_PROVIDER=Dukovo \
	ac_cv_func_dlopen=no ac_cv_lib_dl_dlopen=no ac_cv_search_dlopen=no \
	php_cv_func_getaddrinfo=yes ac_cv_func_which_gethostbyname_r=no php_cv_have_write_stdout=yes \
	php_cv_shm_ipc=yes php_cv_shm_mmap_anon=yes php_cv_shm_mmap_posix=yes \
	php_cv_have_epoll=no php_cv_func_ptrace=no php_cv_file_proc_mem= php_cv_have_TCP_INFO=no \
	php_cv_have_stack_limit=yes \
	x86_64-radix-configure "$PORT_SRC" --bindir="$prefix/bin" --libdir="$prefix/lib" \
	--datarootdir="$prefix/share" --sysconfdir="$sysconf" --localstatedir="$prefix/var" \
	--with-libdir=libraries --with-config-file-path="$prefix/etc" --with-config-file-scan-dir= \
	--disable-all --disable-shared --enable-static --enable-cli --enable-fpm \
	--with-fpm-user=php --with-fpm-group=php --disable-cgi \
	--disable-phpdbg --without-pear --disable-debug \
	--without-pcre-jit --disable-opcache-jit --disable-mbregex \
	--enable-bcmath --with-bz2="$SYSROOT/system" --enable-calendar --enable-ctype \
	--with-curl --enable-dom --enable-exif --enable-fileinfo --enable-filter --enable-ftp \
	--enable-mbstring --enable-mysqlnd --with-mysqli=mysqlnd --with-openssl \
	--enable-pcntl --enable-pdo --with-pdo-mysql=mysqlnd --enable-phar --enable-posix \
	--enable-session --enable-shmop --enable-simplexml --enable-sockets --enable-sysvsem \
	--enable-sysvshm --enable-tokenizer --enable-xml --enable-xmlreader --enable-xmlwriter \
	--with-zlib --with-libxml \
	>configure.log 2>&1 || {
	tail -n 40 configure.log >&2
	exit 1
}

# What must be in the program, and what must not: configure leaves out an
# extension whose library it does not find, and says so only in its log.
# The extensions it chose are those of the list (internal_functions_cli.c
# is the list it wrote), libxml among them, which the XML extensions need.
for feature in HAVE_LIBXML HAVE_OPENSSL_EXT HAVE_CURL HAVE_ZLIB HAVE_BZ2 HAVE_SOCKETS \
	HAVE_SYSVSEM HAVE_SYSVSHM HAVE_SHMOP HAVE_GETADDRINFO HAVE_FNMATCH PHP_WRITE_STDOUT \
	HAVE_SHM_IPC HAVE_SHM_MMAP_ANON HAVE_SHM_MMAP_POSIX HAVE_POLL ZEND_CHECK_STACK_LIMIT; do
	grep -q "^#define $feature 1" main/php_config.h || {
		echo "php's configure did not set $feature" >&2
		exit 1
	}
done
# No JIT for PCRE, which would map memory writable and executable at once
# (Radix never does), no dl(), and for PHP-FPM no epoll, no ptrace or
# /proc/PID/mem for its slow log, and no TCP_INFO for its listen queue.
for feature in HAVE_PCRE_JIT_SUPPORT HAVE_LIBDL HAVE_EPOLL HAVE_PTRACE PROC_MEM_FILE \
	HAVE_LQ_TCP_INFO; do
	if grep -q "^#define $feature" main/php_config.h; then
		echo "php's configure set $feature" >&2
		exit 1
	fi
done
extensions="bcmath bz2 calendar ctype curl date dom exif fileinfo filter ftp hash json lexbor \
	libxml mbstring mysqli mysqlnd opcache openssl pcntl pcre pdo pdo_mysql phar posix random \
	reflection session shmop simplexml sockets spl standard sysvsem sysvshm tokenizer uri xml \
	xmlreader xmlwriter zlib"
chosen=$(sed -n 's/^[[:space:]]*phpext_\([a-z0-9_]*\)_ptr,.*/\1/p' main/internal_functions_cli.c |
	sort | tr '\n' ' ')
wanted=$(printf '%s\n' $extensions | sort | tr '\n' ' ')
if [ "$chosen" != "$wanted" ]; then
	echo "php's configure chose the extensions: $chosen" >&2
	echo "the bundle's extensions are:          $wanted" >&2
	exit 1
fi

# The two programs only: the default target would also run the first, on
# the build machine, to make the phar tool, and a program for Radix is not
# one the build machine's system can run, though it may try (a static
# program starts on Linux, which then takes its system calls for its own).
make -j"$JOBS" cli fpm >make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}

# What the bundle keeps: the programs, without their symbols (PHP-FPM in
# bin as well, as a bundle's commands are there), and not make install's
# folders of PEAR, headers and php-config, which no bundle needs.
mkdir -p "$PORT_BUNDLE/bin" "$PORT_BUNDLE/etc" "$PORT_BUNDLE/conf" \
	"$PORT_BUNDLE/man/man1" "$PORT_BUNDLE/man/man8"
$STRIP -o "$PORT_BUNDLE/bin/php" sapi/cli/php
$STRIP -o "$PORT_BUNDLE/bin/php-fpm" sapi/fpm/php-fpm
install -m 0644 "$PORT_DIR/php.ini" "$PORT_BUNDLE/etc/php.ini"
# The manual page, as configure wrote it from php.1.in: it names no path.
install -m 0644 sapi/cli/php.1 "$PORT_BUNDLE/man/man1/php.1"
# The service, and the settings it starts from: PHP-FPM's own, and the
# php.ini the command-line program has, which the service copies as well.
install -m 0755 "$PORT_DIR/php-fpm-service" "$PORT_BUNDLE/bin/php-fpm-service"
install -m 0644 "$PORT_DIR/php-fpm.conf" "$PORT_BUNDLE/conf/php-fpm.conf"
install -m 0644 "$PORT_DIR/php.ini" "$PORT_BUNDLE/conf/php.ini"
# PHP-FPM's manual page, as configure wrote it from php-fpm.8.in, but for
# the two places it names that its configuration moves: the log goes to
# the standard error output, and a pid file only if the settings name one.
sed -e 's|Output is logged to [^ ]*/log/php-fpm.log\.|Output is logged to the standard error output, which the system keeps: log php-fpm shows it.|' \
	-e 's|The file [^ ]*/run/php-fpm.pid determines|The file that the pid setting names, if it has one, determines|' \
	sapi/fpm/php-fpm.8 >"$PORT_BUNDLE/man/man8/php-fpm.8"
if grep -q "/var/" "$PORT_BUNDLE/man/man8/php-fpm.8"; then
	echo "php-fpm.8 still names a path under var" >&2
	exit 1
fi

# PHP's test suite, in the tree's own layout, so that the tests find what
# they include and read beside them: run-tests.php, and every tests folder
# of Zend, tests, sapi/cli and each extension the program has, whole, with
# the include files and the data each holds. The extensions are those of
# the program, by the list configure wrote; the tests of the others,
# which are not built, would only skip.
tests="$PORT_BUNDLE/tests"
mkdir -p "$tests"
cp "$PORT_SRC/run-tests.php" "$tests/run-tests.php"
folders="Zend/tests tests sapi/cli/tests"
for name in $chosen; do
	if [ -d "$PORT_SRC/ext/$name/tests" ]; then
		folders="$folders ext/$name/tests"
	fi
done
for folder in $folders; do
	mkdir -p "$tests/$folder"
	cp -R "$PORT_SRC/$folder/." "$tests/$folder/"
done
# The few files that a test of those folders includes from a folder that is
# not in the bundle, as php-cgi's and PHP-FPM's helpers and the loadable
# extension the dl() tests need: the test's own checks then skip it, saying
# why, where a missing file would stop it as broken. (Found by a search of
# the tests for the paths they build from __DIR__ and dirname.)
for file in ext/dl_test/tests/skip.inc sapi/cgi/tests/include.inc sapi/fpm/tests/skipif.inc \
	sapi/fpm/tests/tester.inc ext/xsl/tests/53965/collection.xml \
	ext/xsl/tests/53965/collection.xsl; do
	mkdir -p "$tests/${file%/*}"
	cp "$PORT_SRC/$file" "$tests/$file"
done
