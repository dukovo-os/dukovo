#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds Go's own commands for Radix with Go's own make.bash and its
# -distpack, into the bundle's folder ($PORT_BUNDLE, ADR 0356): bin/go and
# bin/gofmt, the programs the go command runs in pkg/tool/radix_amd64, the
# standard library's source with its tests, and what a Go tree holds
# besides. make.bash is run as a cross build: the build machine's tools,
# which the go port's toolchain ($TOOLCHAIN/go) builds, compile the
# commands and the standard library for GOOS=radix, which the patches add.
# -distpack writes the archive Go ships a release of a system in, so that
# the bundle holds what Go chooses to ship and nothing else.
#
# `ob` runs it (ob(1), PORTS) with PORT_DIR, PORT_SRC, PORT_BUILD,
# PORT_BUNDLE and TOOLCHAIN set. Nothing of the build machine's own Go
# settings reaches make.bash.
set -eu

cd "$PORT_SRC"
# A Go tree made from git has no VERSION file, which make.bash needs when
# it is not in a repository: the release is the recipe's.
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$PORT_DIR/recipe.toml")
echo "go$version" >VERSION

# The build cache and the module folders of the build are in the build's
# folder, which ob removes when the next build begins.
cache="$PORT_BUILD/cache"
mkdir -p "$cache"
# cgo is off, and make.bash bakes that in as the default of the commands:
# Dukovo has no C compiler for them to call.
(
	cd src
	env -i \
		PATH="$PATH" \
		HOME="$PORT_BUILD" \
		LC_ALL=C.UTF-8 \
		GOROOT_BOOTSTRAP="$TOOLCHAIN/go" \
		GOTOOLCHAIN=local \
		GOTELEMETRY=off \
		GOENV=off \
		GOCACHE="$cache" \
		GOPATH="$PORT_BUILD/path" \
		GOOS=radix \
		GOARCH=amd64 \
		GOAMD64=v1 \
		CGO_ENABLED=0 \
		./make.bash -distpack
)

archive="pkg/distpack/go$version.radix-amd64.tar.gz"
if [ ! -f "$archive" ]; then
	echo "go-radix: make.bash wrote no $archive" >&2
	exit 1
fi
# The archive's files are in a folder of its own, go/.
tar -xzf "$archive" -C "$PORT_BUNDLE" --strip-components=1 --no-same-owner

# The race detector's and BoringCrypto's prebuilt objects, for other
# systems and under other licenses, are in the source; nothing on Radix
# links them.
find "$PORT_BUNDLE" -name '*.syso' -type f -exec rm -f {} +

# A go.env that lets the go command fetch a newer Go for a module that asks
# for one would download a release that has no Radix in it.
env_file="$PORT_BUNDLE/go.env"
sed -i 's/^GOTOOLCHAIN=auto$/# No release of Go from go.dev builds for Radix: a module that asks for a\n# newer Go fails instead of fetching one.\nGOTOOLCHAIN=local/' "$env_file"
grep -qx 'GOTOOLCHAIN=local' "$env_file" || {
	echo "go-radix: go.env has no GOTOOLCHAIN=auto to make local" >&2
	exit 1
}

mkdir -p "$PORT_BUNDLE/man/man1"
for page in go gofmt; do
	scdoc <"$PORT_DIR/$page.1.scd" >"$PORT_BUNDLE/man/man1/$page.1"
done
echo "go-radix: the Go $version commands in $PORT_BUNDLE"
