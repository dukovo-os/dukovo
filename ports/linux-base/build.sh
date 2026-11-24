#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Takes the libraries Linux programs run on from the layers of the
# Freedesktop SDK platform image, which ob fetched into PORT_SRC as
# layer-1.blob and layer-2.blob (ob(1), PORTS), and installs them into
# PORT_BUNDLE as the bundle's `usr`. The image's layout stays:
# usr/lib/x86_64-linux-gnu holds the libraries, and usr/lib64 the loader
# that programs of other distributions name, /lib64/ld-linux-x86-64.so.2.
# `ob` runs it with PORT_SRC, PORT_BUILD and PORT_BUNDLE set.
#
# A bundle holds regular files only (bundle(1)), and the image's libraries
# are links to files named by their versions, so each is copied under the
# name programs ask for, its soname. The loader is copied to both of its
# places.
#
# The rest of the image is never written: each layer is listed first, and
# only the files this script keeps are unpacked. A later layer's file
# replaces an earlier one's, as unpacking in order does, and neither layer
# holds a whiteout, which would delete one.
set -eu

# The libraries Linux programs name, by soname: glibc's loader, C library
# and the parts of it that were once libraries of their own, the resolver
# and the name service switch's files and DNS modules; GCC's runtime
# libraries; and the libraries that distributions' programs most often link.
libraries="ld-linux-x86-64.so.2 libc.so.6 libm.so.6 libpthread.so.0 libdl.so.2 librt.so.1
	libresolv.so.2 libutil.so.1 libnss_files.so.2 libnss_dns.so.2
	libstdc++.so.6 libgcc_s.so.1 libatomic.so.1
	libz.so.1 libzstd.so.1 liblzma.so.5 libbz2.so.1 libffi.so.8 libexpat.so.1
	libuuid.so.1 libcrypt.so.2"

# The license files of the packages the libraries are from, in the image's
# folders. util-linux, whose libuuid this is, keeps its license texts in
# one place for all its libraries: only libuuid's notice and the text it
# names are kept.
licenses="glibc bzip2 expat libffi libxcrypt xz zlib zstd
	util-linux/libuuid util-linux/Documentation/licenses/COPYING.BSD-3-Clause"

image="$PORT_BUILD/image"
out="$PORT_BUNDLE/usr"
libdir=usr/lib/x86_64-linux-gnu
licensedir=usr/share/licenses/freedesktop-sdk

rm -rf "$image" "$PORT_BUNDLE"
mkdir -p "$image" "$PORT_BUNDLE/bin"

# What of a layer is kept, as the names tar gives: the libraries and the
# files they link to by version (a soname, or one followed by its version),
# the C.utf8 locale, the license files of the packages and the texts their
# absolute links name, which are all in usr/share/licenses/common. Folders
# are not listed: tar makes the ones a file needs.
wanted() {
	tar --warning=no-unknown-keyword -tzf "$1" | while read -r name; do
		case $name in
		*/) ;;
		$libdir/*)
			base=${name##*/}
			for soname in $libraries; do
				case $base in
				"$soname" | "$soname".[0-9]*) echo "$name" ;;
				esac
			done
			;;
		usr/lib/locale/C.utf8/*) echo "$name" ;;
		usr/share/licenses/common/*) echo "$name" ;;
		$licensedir/*)
			for kept in $licenses; do
				case $name in
				"$licensedir/$kept" | "$licensedir/$kept"/*) echo "$name" ;;
				esac
			done
			;;
		esac
	done
}

for layer in "$PORT_SRC"/layer-*.blob; do
	wanted "$layer" >"$PORT_BUILD/kept.txt"
	# The names are exact, and a folder's files are in the list.
	tar --warning=no-unknown-keyword -xzf "$layer" -C "$image" --no-same-owner \
		--no-recursion -T "$PORT_BUILD/kept.txt"
done

# The file a link in the image leads to, as its root sees it: its absolute
# links lead to its own /usr, not the build machine's.
resolve() {
	path=$1
	while [ -L "$path" ]; do
		target=$(readlink "$path")
		case $target in
		/*) path="$image$target" ;;
		*) path=$(dirname "$path")/$target ;;
		esac
	done
	echo "$path"
}

# Copies the file or link at $1 in the image to $2 as a regular file.
copy() {
	from=$(resolve "$1")
	[ -f "$from" ] || {
		echo "linux-base: $1 leads to no file of the image" >&2
		exit 1
	}
	mkdir -p "$(dirname "$2")"
	cp "$from" "$2"
}

for soname in $libraries; do
	copy "$image/$libdir/$soname" "$out/lib/x86_64-linux-gnu/$soname"
done
copy "$image/$libdir/ld-linux-x86-64.so.2" "$out/lib64/ld-linux-x86-64.so.2"
# libc.so.6 names the loader as its interpreter, so that running it prints
# its version, and `ob` takes a file with an execute bit and an interpreter
# for a program that lacks Dukovo's note (ADR 0610). The loader maps the
# library and needs no bit to; the others have no interpreter.
chmod 644 "$out/lib/x86_64-linux-gnu/libc.so.6"

# The locale's files, each in its place.
(cd "$image/usr/lib/locale" && find C.utf8 \( -type f -o -type l \)) | while read -r name; do
	copy "$image/usr/lib/locale/$name" "$out/lib/locale/$name"
done

# The licenses: every file under each folder, links copied as the files
# they lead to.
for kept in $licenses; do
	find "$image/$licensedir/$kept" \( -type f -o -type l \) | while read -r path; do
		copy "$path" "$PORT_BUNDLE/${path#"$image/"}"
	done
done

# A loop at the end of a pipe does not fail the script when what feeds it
# finds nothing, so what each loop was to copy is checked.
for need in "lib/locale/C.utf8/LC_CTYPE" "lib/locale/C.utf8/LC_MESSAGES/SYS_LC_MESSAGES" \
	"share/licenses/freedesktop-sdk/glibc/COPYING.LIB" "share/licenses/freedesktop-sdk/zstd/LICENSE" \
	"share/licenses/freedesktop-sdk/util-linux/libuuid/COPYING" \
	"share/licenses/freedesktop-sdk/util-linux/Documentation/licenses/COPYING.BSD-3-Clause"; do
	[ -f "$out/$need" ] || {
		echo "linux-base: the image has no $need" >&2
		exit 1
	}
done

# The bundle's one program: a manifest names at least one (manifest.json(5)),
# and the bundle has nothing to run.
cat >"$PORT_BUNDLE/bin/linux-base" <<'EOF'
#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The Linux base libraries are files in /usr of the view a Linux program
# runs in; this bundle has nothing to run.
echo "linux-base: this bundle holds the libraries Linux programs run on, and no command" >&2
exit 1
EOF
chmod 755 "$PORT_BUNDLE/bin/linux-base"

# Regular files and folders only, as a bundle holds, and nothing a person
# other than its owner can write.
if [ -n "$(find "$PORT_BUNDLE" ! -type f ! -type d)" ]; then
	echo "linux-base: the bundle holds something that is neither a file nor a folder" >&2
	exit 1
fi
chmod -R go-w "$PORT_BUNDLE"

echo "linux-base: $(find "$PORT_BUNDLE" -type f | wc -l) files, $(du -sk "$PORT_BUNDLE" | cut -f1) KiB"
