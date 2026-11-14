#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds PostgreSQL for Radix (ob(1), PORTS) into the bundle's folder
# ($PORT_BUNDLE, ADRs 0370 and 0521): the server and the client and
# administration programs in bin, libpq in lib, the modules the server
# loads with dlopen in lib/postgresql (PL/pgSQL, the encoding conversions,
# the Snowball dictionaries, pgoutput, libpqwalreceiver and the contrib
# modules), the files the server reads in share, the service's script in
# bin and the files it starts from in conf (ADR 0522), and the test suites
# in tests, with the programs that run them. libpq goes into the sysroot
# too, as a static library with its headers, for the ports built on it
# (PHP).
#
# Radix's ABI is Linux's (radix-posix(7)), so the build takes PostgreSQL's
# linux template: unnamed POSIX semaphores, -fPIC for shared code, the
# shared library rules of src/Makefile.shlib and src/include/port/linux.h,
# which has the server sync its WAL with fdatasync (Radix has it) and
# take EIDRM from a shared memory call for EINVAL (sysv_shmem.c), as a
# Linux kernel needs it to. The macro __linux__, which PostgreSQL's code
# tests in a few places, is not defined: the compiler wrapper undefines it,
# so that no program takes Radix for Linux. That leaves out what is
# guarded by it, none of which PostgreSQL needs: the processes' titles
# (ps_status.c), which Radix has no way to set; the default huge page
# size from /proc/meminfo, which stays 2 MiB; posix_fallocate for the
# memory of a parallel query (ftruncate, which radix-posix(7) says it is
# in Radix, takes its place); the processor's clock rate from
# /proc/cpuinfo; and FICLONE for pg_upgrade --clone and pg_combinebackup
# --clone.
#
# The server is linked dynamically (the compiler wrapper's -dynamic,
# ADR 0071) and exports its symbols (-Wl,--export-dynamic), as the modules
# it loads call back into it, and the modules and libpq are shared
# libraries. So are the client programs: libpq.a does not link on its own,
# as the encoding functions that libpq exports are in libpgcommon_shlib
# only (src/include/mb/pg_wchar.h), and the programs link the libpq.so of
# the bundle as they do on Linux. Bundles may be installed for one person,
# so the run paths are relative to the file's own folder ($ORIGIN) and not
# the prefix's.
#
# PostgreSQL finds its library and share folders from where its program
# is, by the program's path, which the kernel gives (patch 0001).
#
# PostgreSQL's source comes from git, which has no release's generated
# files: the parsers and the catalogs are written by bison, flex and Perl
# on the build machine during make.
set -eu

id=org.postgresql.postgresql
prefix="/applications/$id"

openssl="$PORT_INFO/../openssl/for-bundles"
if [ ! -f "$openssl/libraries/libssl.a" ]; then
	echo "postgresql: no OpenSSL libraries in $openssl: build the openssl port again" >&2
	exit 1
fi

# The files of config/ that configure runs are not executable in a git
# checkout. x86_64-radix-configure puts a script of its own in place of
# config.sub, which hands it every name but Radix's: it has to be able to run
# the one it moved.
chmod +x "$PORT_SRC/config/config.sub"

mkdir -p "$PORT_BUILD/radix"
cd "$PORT_BUILD/radix"
# What configure cannot find out without running a program for Radix, or
# would find out differently than Radix has it:
# - the server's link: configure adds -Wl,--export-dynamic to
#   LDFLAGS_EX_BE, the server's flags alone, only if a program linked with
#   it runs ("for reasons you'd really rather not know about"). A program
#   for Radix does not run on the build machine, so configure takes the
#   option to be missing. LLD has it, and the modules call the server's
#   functions through it;
# - the C library's headers have no <sys/epoll.h> and no <sys/signalfd.h>,
#   and Radix has neither call (radix-posix(7)); configure takes them if
#   they come. The wait on events (waiteventset.c) then uses poll(2) and
#   the self-pipe, as it does on a system without them;
# - nothing here for sync_file_range: the C library does not have it, so
#   configure leaves it out and the settings *_flush_after are 0 as on
#   any system without it. posix_fadvise(2) is there and checks its
#   arguments only (radix-posix(7)), which PostgreSQL's prefetching of
#   pages (effective_io_concurrency) tolerates;
# - the C library's backtrace(3) looks for libgcc_s, which a program of the
#   bundle does not have, and writes that to the standard error output each
#   time PostgreSQL asks for a backtrace (backtrace_functions, or an
#   internal error with backtrace_on_internal_error). Without execinfo.h,
#   and with no backtrace_symbols, PostgreSQL leaves the backtrace out, as
#   it does on a system that has none.
# Left at configure's answer for a cross build, which is right for Radix:
# no __int128 alignment bug, and no -Wl,--as-needed in LDFLAGS (the compiler
# wrapper puts it in the link of a dynamic program and a library).
#
# The link flags configure takes from the environment ($$ is make's for a
# dollar sign, and the single quotes keep the shell from reading it):
# - LDFLAGS_EX is every program's: -dynamic, and its run path, the lib folder
#   beside the program's bin, where libpq is;
# - LDFLAGS_SL is every shared library's: the run path of a module in
#   lib/postgresql, which is lib, where libpq is, and what keeps the
#   static libraries of OpenSSL from exporting their symbols out of libpq.
#   libpq has an OpenSSL of its own, as it is loaded into programs that
#   have none (psql) and into the server (libpqwalreceiver, dblink and
#   postgres_fdw), which has its own and exports it. libpq only touches
#   the objects it makes itself, so the two copies stay apart as long as
#   neither is taken for the other's: exported, libpq's functions would
#   be the ones the dynamic linker finds first for the server's calls, and
#   the server's for libpq's. The modules that work on the server's own
#   OpenSSL objects, pgcrypto and sslinfo, have no copy at all (below).
LDFLAGS_EX="-dynamic -Wl,-rpath,'\$\$ORIGIN/../lib'" \
	LDFLAGS_SL="-Wl,-rpath,'\$\$ORIGIN/..' -Wl,--exclude-libs=libssl.a -Wl,--exclude-libs=libcrypto.a" \
	pgac_cv_prog_cc_LDFLAGS_EX_BE__Wl___export_dynamic=yes \
	ac_cv_header_sys_epoll_h=no ac_cv_header_sys_signalfd_h=no \
	ac_cv_header_execinfo_h=no ac_cv_func_backtrace_symbols=no \
	PREFIX="$prefix" \
	x86_64-radix-configure "$PORT_SRC" --with-template=linux \
	--bindir="$prefix/bin" --libdir="$prefix/lib" --datarootdir="$prefix/share" \
	--sysconfdir="$prefix/etc" --localstatedir="$prefix/var" --disable-rpath \
	--with-includes="$openssl/include" --with-libraries="$openssl/libraries" \
	--with-ssl=openssl --with-zlib --with-lz4 --with-zstd --with-libxml \
	--with-libedit-preferred --without-icu --without-llvm --without-perl \
	--without-python --without-tcl --without-liburing --disable-nls \
	--with-system-tzdata=/system/resources/zoneinfo \
	>configure.log 2>&1 || {
	tail -n 40 configure.log >&2
	exit 1
}

# What must be in the build, and what must not: configure leaves out a
# feature whose library it does not find, and says so only in its log. The
# line editing is libedit's (psql takes it for readline's, which the
# header names) and the waiting for events and the semaphores are the ones
# of the notes above.
for feature in USE_OPENSSL HAVE_LIBZ USE_LZ4 USE_ZSTD USE_LIBXML HAVE_LIBREADLINE \
	HAVE_EDITLINE_READLINE_H USE_UNNAMED_POSIX_SEMAPHORES; do
	grep -q "^#define $feature 1" src/include/pg_config.h || {
		echo "postgresql's configure did not set $feature" >&2
		exit 1
	}
done
# No epoll or signalfd, no ICU (collations are libc's and the builtin
# provider's), no JIT or io_uring, no sync_file_range, and no backtraces.
for feature in HAVE_SYS_EPOLL_H HAVE_SYS_SIGNALFD_H USE_ICU USE_LLVM USE_LIBURING \
	HAVE_SYNC_FILE_RANGE HAVE_EXECINFO_H HAVE_BACKTRACE_SYMBOLS; do
	if grep -q "^#define $feature" src/include/pg_config.h; then
		echo "postgresql's configure set $feature" >&2
		exit 1
	fi
done
# The server's link: -dynamic for every program, and -Wl,--export-dynamic for
# the server alone, which are the flags of src/Makefile.global that the
# link of each program is made of.
grep -q '^LDFLAGS_EX = -dynamic ' src/Makefile.global &&
	grep -q '^LDFLAGS_EX_BE = .*-Wl,--export-dynamic' src/Makefile.global || {
	echo "postgresql's Makefile.global has not the server's link flags" >&2
	grep '^LDFLAGS_EX' src/Makefile.global >&2
	exit 1
}

# libpq must not call exit(3), and make checks that of its undefined
# symbols (libpq_check.pl), those with "exit" in them. The OpenSSL that
# libpq has in itself calls atexit(3) to free what it has allocated, which
# is not a call of exit and not libpq's own: the check is given an nm that
# leaves that one out.
cat >"$PORT_BUILD/nm-libpq" <<EOF
#!/bin/sh
$NM "\$@" | grep -v ' atexit\$' || true
EOF
chmod +x "$PORT_BUILD/nm-libpq"

# Every make has the same two settings: pkglibdir, the folder the server
# loads its modules from, which pg_config tells of as well, is
# lib/postgresql, and libpq's check has the nm above. PostgreSQL appends
# "postgresql" to libdir for its modules only when the prefix has no
# "postgres" in it; this one has, and the modules would be in lib among
# libpq.
pg_make() {
	make -j"$JOBS" pkglibdir="$prefix/lib/postgresql" NM="$PORT_BUILD/nm-libpq" "$@"
}

# The server, the programs and every module of contrib/, without the
# documentation (world-bin), and then the programs that run the test
# suites, with regress.so, the test module of the main suite, which it does
# not have.
pg_make world-bin >make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}
pg_make -C src/test/regress >>make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}
pg_make -C src/test/isolation >>make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}

# pgcrypto and sslinfo link no OpenSSL and no zlib. sslinfo is handed the
# connection's SSL and X509 objects, which only the OpenSSL that made them
# may touch, and a second copy in the process would be one that does not
# know them; pgcrypto has no need of a copy of its own either. Their calls
# are the server's at load (it exports its static libraries with the rest,
# --export-dynamic), as every call of a module into the server is. The
# modules of world-bin have libraries linked in by SHLIB_LINK, which is
# emptied for these two alone.
rm -f contrib/pgcrypto/pgcrypto.so contrib/sslinfo/sslinfo.so
pg_make -C contrib/pgcrypto SHLIB_LINK= >>make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}
pg_make -C contrib/sslinfo SHLIB_LINK= >>make.log 2>&1 || {
	tail -n 60 make.log >&2
	exit 1
}

# Into a folder of its own, and from there what the bundle keeps.
stage="$PORT_BUILD/stage"
pg_make install-world-bin DESTDIR="$stage" >install.log 2>&1 || {
	tail -n 60 install.log >&2
	exit 1
}
installed="$stage$prefix"

# libpq for the ports built on it, PHP's pgsql and pdo_pgsql: libpq.a and
# its headers in the sysroot, with a libpq.pc, as a library's port installs
# them. A static program links libpq.a with libpgcommon_shlib.a and
# libpgport_shlib.a, which PostgreSQL builds for libpq.so, and not with
# libpgcommon.a and libpgport.a: libpq's objects call the encoding
# functions by their public names (pg_char_to_encoding), which only the
# first of each pair defines, while the others name them for the programs
# (pg_char_to_encoding_private, src/include/mb/pg_wchar.h).
# The libpq.pc is not the tree's: that one has the paths of this build in
# its Libs.private and asks for the pkg-config files of OpenSSL, which the
# sysroot does not have. It does not say where OpenSSL's libraries are
# either, as the openssl port keeps them for bundles in a folder of its
# own, and the port that links libpq gives that folder, as PHP's does for
# its own use of OpenSSL.
sysroot_libraries="$SYSROOT$PREFIX/libraries"
sysroot_include="$SYSROOT$PREFIX/include"
mkdir -p "$sysroot_libraries/pkgconfig" "$sysroot_include/libpq"
for archive in libpq.a libpgcommon_shlib.a libpgport_shlib.a; do
	install -m 0644 "$installed/lib/$archive" "$sysroot_libraries/$archive"
done
for header in libpq-fe.h libpq-events.h postgres_ext.h; do
	install -m 0644 "$installed/include/$header" "$sysroot_include/$header"
done
install -m 0644 "$installed/include/libpq/libpq-fs.h" "$sysroot_include/libpq/libpq-fs.h"
version=$(sed -n 's/^Version: //p' "$PORT_BUILD/radix/src/interfaces/libpq/libpq.pc")
cat >"$sysroot_libraries/pkgconfig/libpq.pc" <<EOF
prefix=$PREFIX
exec_prefix=\${prefix}
libdir=\${prefix}/libraries
includedir=\${prefix}/include

Name: libpq
Description: PostgreSQL libpq library
URL: https://www.postgresql.org/
Version: $version
Requires:
Cflags: -I\${includedir}
Libs: -L\${libdir} -lpq
Libs.private: -lpgcommon_shlib -lpgport_shlib -lssl -lcrypto -lm
EOF

# Every program, without its symbols (ecpg, the embedded SQL compiler, needs
# its headers and libraries, which no bundle keeps), and every module.
# Modules and libpq keep their execute bits, as PostgreSQL installs them.
# libpq.so.5 is the file itself, as a bundle has no links, and the soname
# is what the programs and modules ask for. xml2 is not in the bundle: it
# needs libxslt for the half of what it is for (xslt_process), which Dukovo
# has no port of, and is built with libxml alone by the rule of the
# contrib folder.
mkdir -p "$PORT_BUNDLE/bin" "$PORT_BUNDLE/lib/postgresql" "$PORT_BUNDLE/share"
for program in "$installed"/bin/*; do
	name=${program##*/}
	if [ "$name" != ecpg ]; then
		$STRIP -o "$PORT_BUNDLE/bin/$name" "$program"
	fi
done
$STRIP -o "$PORT_BUNDLE/lib/libpq.so.5" "$installed/lib/libpq.so.5"
for module in "$installed"/lib/postgresql/*.so; do
	name=${module##*/}
	if [ "$name" != pgxml.so ]; then
		$STRIP -o "$PORT_BUNDLE/lib/postgresql/$name" "$module"
	fi
done
cp -R "$installed/share/." "$PORT_BUNDLE/share/"
rm -f "$PORT_BUNDLE"/share/extension/xml2*

# The service, and the two files its first start copies into the cluster
# (ADR 0522), in conf as the other bundles' services keep theirs.
mkdir -p "$PORT_BUNDLE/conf"
install -m 0755 "$PORT_DIR/postgresql-service" "$PORT_BUNDLE/bin/postgresql-service"
install -m 0644 "$PORT_DIR/pg_hba.conf" "$PORT_BUNDLE/conf/pg_hba.conf"
install -m 0644 "$PORT_DIR/pg_ident.conf" "$PORT_BUNDLE/conf/pg_ident.conf"

# What the bundle must have: the server's modules that PostgreSQL's own
# programs and its most used extensions need.
for module in plpgsql dict_snowball pgoutput libpqwalreceiver pgcrypto pg_trgm hstore \
	pg_stat_statements postgres_fdw; do
	if [ ! -f "$PORT_BUNDLE/lib/postgresql/$module.so" ]; then
		echo "postgresql: lib/postgresql has no $module.so" >&2
		exit 1
	fi
done

# The test suites, in a folder of their own so that the programs that run
# them, which look for libpq as the others do, are one folder below the
# bundle's: tests/pg_regress, tests/pg_isolation_regress and
# tests/isolationtester, then each suite's folder with the files it reads:
# the main suite's, with its test module regress.so, the isolation suite's,
# PL/pgSQL's, and each contrib module's. Which modules there are is what
# the folder contrib/ has in SUBDIRS (the one for this configuration), as
# the build has made them.
tests="$PORT_BUNDLE/tests"
mkdir -p "$tests/regress" "$tests/isolation" "$tests/plpgsql" "$tests/contrib"
build="$PORT_BUILD/radix"
$STRIP -o "$tests/pg_regress" "$build/src/test/regress/pg_regress"
$STRIP -o "$tests/pg_isolation_regress" "$build/src/test/isolation/pg_isolation_regress"
$STRIP -o "$tests/isolationtester" "$build/src/test/isolation/isolationtester"
$STRIP -o "$tests/regress/regress.so" "$build/src/test/regress/regress.so"
regress="$PORT_SRC/src/test/regress"
for item in sql expected data parallel_schedule resultmap; do
	cp -R "$regress/$item" "$tests/regress/$item"
done
isolation="$PORT_SRC/src/test/isolation"
for item in specs expected isolation_schedule; do
	cp -R "$isolation/$item" "$tests/isolation/$item"
done
for item in sql expected; do
	cp -R "$PORT_SRC/src/pl/plpgsql/src/$item" "$tests/plpgsql/$item"
done
contrib=$(make -s -C "$build/contrib" --eval='print-%: ; @echo $($*)' print-SUBDIRS)
for name in $contrib; do
	if [ "$name" = xml2 ]; then
		continue
	fi
	for item in sql expected specs data input output; do
		if [ -d "$PORT_SRC/contrib/$name/$item" ]; then
			mkdir -p "$tests/contrib/$name"
			cp -R "$PORT_SRC/contrib/$name/$item" "$tests/contrib/$name/$item"
		fi
	done
done

# Where each file finds the libraries it needs, as the build made it: a
# program, and the test programs, in the lib folder beside their own; a
# module, and libpq, which is linked as one, in the folder above theirs
# (for libpq itself and regress.so that is a folder with no library to
# find, and they need none). An absolute run path would be wrong in a
# person's own installation, and nothing but libc.so and libpq.so.5 may be
# asked for. A program's interpreter is the system's dynamic linker, and
# no file has text relocations, which position-independent code has no
# need of and the dynamic linker would have to patch the code for.
readelf=x86_64-radix-readelf
check_elf() {
	file=$1
	runpath=$2
	found=$($readelf -d "$file" | sed -n 's/.*(RUNPATH)[^[]*\[\(.*\)\]/\1/p')
	if [ "$found" != "$runpath" ]; then
		echo "postgresql: ${file#$PORT_BUNDLE/} has the run path '$found', not '$runpath'" >&2
		exit 1
	fi
	if $readelf -d "$file" | grep -q '(RPATH)\|TEXTREL'; then
		echo "postgresql: ${file#$PORT_BUNDLE/} has an RPATH or text relocations" >&2
		exit 1
	fi
	needed=$($readelf -d "$file" | sed -n 's/.*(NEEDED)[^[]*\[\(.*\)\]/\1/p' | grep -v '^libc\.so$\|^libpq\.so\.5$' || true)
	if [ -n "$needed" ]; then
		echo "postgresql: ${file#$PORT_BUNDLE/} needs $needed" >&2
		exit 1
	fi
}
check_program() {
	check_elf "$1" '$ORIGIN/../lib'
	if ! $readelf -l "$1" | grep -q 'Requesting program interpreter: /system/libraries/ld.so\]'; then
		echo "postgresql: ${1#$PORT_BUNDLE/} is not linked dynamically for Radix" >&2
		exit 1
	fi
}
for program in "$PORT_BUNDLE"/bin/* "$tests/pg_regress" "$tests/pg_isolation_regress" \
	"$tests/isolationtester"; do
	# The service is a script, which /bin/sh runs.
	if [ "${program##*/}" != postgresql-service ]; then
		check_program "$program"
	fi
done
for module in "$PORT_BUNDLE"/lib/postgresql/*.so "$PORT_BUNDLE/lib/libpq.so.5" \
	"$tests/regress/regress.so"; do
	check_elf "$module" '$ORIGIN/..'
done
# The server exports what its modules call (palloc, which every one does),
# and pgcrypto and sslinfo have no OpenSSL: no function of OpenSSL's is
# defined in them, and each they call is defined in the server's dynamic
# symbols, where the dynamic linker finds it when they load. What else they
# call is the server's too, or libc.so's.
dynamic_symbols() {
	file=$1
	status=$2
	$readelf --dyn-syms --wide "$file" | awk -v status="$status" '
		$1 ~ /^[0-9]+:$/ && $8 != "" {
			if (status == "undefined" && ($7 != "UND" || $5 == "WEAK")) next
			if (status == "defined" && $7 == "UND") next
			name = $8
			sub(/@.*/, "", name)
			print name
		}' | LC_ALL=C sort -u
}
if ! dynamic_symbols "$PORT_BUNDLE/bin/postgres" defined | grep -qx palloc; then
	echo "postgresql: bin/postgres does not export palloc" >&2
	exit 1
fi
dynamic_symbols "$PORT_BUNDLE/bin/postgres" defined >"$PORT_BUILD/server-symbols"
dynamic_symbols "$SYSROOT/system/libraries/libc.so" defined >"$PORT_BUILD/libc-symbols"
LC_ALL=C sort -u "$PORT_BUILD/server-symbols" "$PORT_BUILD/libc-symbols" >"$PORT_BUILD/known-symbols"
for module in pgcrypto sslinfo; do
	file="$PORT_BUNDLE/lib/postgresql/$module.so"
	if dynamic_symbols "$file" defined | grep -q '^\(EVP_\|SSL_\|X509_\|OPENSSL_\|CRYPTO_\)'; then
		echo "postgresql: $module.so has an OpenSSL of its own" >&2
		exit 1
	fi
	dynamic_symbols "$file" undefined >"$PORT_BUILD/$module-symbols"
	grep -q '^\(EVP_\|SSL_\|X509_\|OPENSSL_\|CRYPTO_\)' "$PORT_BUILD/$module-symbols" || {
		echo "postgresql: $module.so calls no function of OpenSSL's" >&2
		exit 1
	}
	missing=$(LC_ALL=C comm -23 "$PORT_BUILD/$module-symbols" "$PORT_BUILD/known-symbols")
	if [ -n "$missing" ]; then
		echo "postgresql: $module.so calls what neither bin/postgres nor libc.so has:" >&2
		echo "$missing" >&2
		exit 1
	fi
done
