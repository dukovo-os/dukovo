#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The kit the ACME lab test runs the lab PC's end with: Pebble, Let's
# Encrypt's own test authority, and its companion pebble-challtestsrv, a
# DNS server whose answers a test sets.
#
#   kit.sh
#
# Prints "kit: FOLDER" for the folder that holds the two programs, `pebble`
# and `pebble-challtestsrv`, and builds them first when the folder does not
# have them yet. The source is one release of Pebble, named and pinned
# below by its version and the SHA-256 of the archive the Go module
# proxy serves for it; an archive with any other hash is not built. The
# source's own go.sum pins the modules it builds with. The programs are
# built with the Go the lab PC has, as Pebble's release says it needs, and
# nothing is installed: the folder is
# ${XDG_CACHE_HOME:-$HOME/.cache}/dukovo-lab/pebble-VERSION, or the one
# DUKOVO_LAB_PEBBLE names.
#
# Exits with 77 where the lab PC cannot do its part: no Go, no curl,
# unzip or sha256sum, or no way to fetch the source. That skips the test.
set -eu

VERSION=v2.10.1
SHA256=e192e248ad0322420af7c769a8e48286c1ebc561b5c9e89f04acc264594f26ca
URL="https://proxy.golang.org/github.com/letsencrypt/pebble/v2/@v/$VERSION.zip"

kit="${DUKOVO_LAB_PEBBLE:-${XDG_CACHE_HOME:-$HOME/.cache}/dukovo-lab/pebble-$VERSION}"

if [ -x "$kit/pebble" ] && [ -x "$kit/pebble-challtestsrv" ] \
	&& [ "$(cat "$kit/sha256" 2> /dev/null)" = "$SHA256" ]; then
	echo "kit: $kit"
	exit 0
fi

for program in go curl unzip sha256sum; do
	if ! command -v "$program" > /dev/null 2>&1; then
		echo "the lab PC has no $program, which building Pebble $VERSION takes"
		exit 77
	fi
done

mkdir -p "$kit"
build=$(mktemp -d "$kit/build.XXXXXX")
# Go's module files are read-only: make them writable before the folder goes.
gone() {
	chmod -R u+w "$build" 2> /dev/null || true
	rm -rf "$build"
}
trap gone EXIT

if ! curl -fsS --proto '=https' --retry 2 --max-time 120 -o "$build/pebble.zip" "$URL" \
	2> "$build/curl.err"; then
	echo "cannot fetch Pebble $VERSION from $URL: $(tr '\n' ' ' < "$build/curl.err")"
	exit 77
fi
got=$(sha256sum "$build/pebble.zip" | sed 's/ .*//')
if [ "$got" != "$SHA256" ]; then
	echo "Pebble $VERSION has SHA-256 $got, not the $SHA256 this kit pins: not built"
	exit 1
fi
unzip -q "$build/pebble.zip" -d "$build/src"
source=$(ls -d "$build"/src/github.com/letsencrypt/pebble/v2@"$VERSION")

# The toolchain the lab PC has: a newer one the go.mod asks for is not
# fetched. Modules come from the proxy and are checked against go.sum.
cd "$source"
export GOTOOLCHAIN=local GOFLAGS=-modcacherw CGO_ENABLED=0
export GOPATH="$build/gopath" GOMODCACHE="$build/gopath/mod" GOCACHE="$build/gocache"
export GOPROXY=https://proxy.golang.org
if ! go build -trimpath -o "$build/out/" ./cmd/pebble ./cmd/pebble-challtestsrv \
	> "$build/go.out" 2>&1; then
	echo "go cannot build Pebble $VERSION: $(tail -3 "$build/go.out" | tr '\n' ' ')"
	exit 77
fi
cp "$build/out/pebble" "$build/out/pebble-challtestsrv" "$kit/"
echo "$SHA256" > "$kit/sha256"
echo "kit: $kit"
