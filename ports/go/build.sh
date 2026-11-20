#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds the Go toolchain with Go's own make.bash: the compiler, linker and
# tools for the build machine, which compile the standard library of
# whatever GOOS and GOARCH a program names (here, GOOS=radix, which the
# patches add), from the patched source. The toolchain is installed into
# $TOOLCHAIN/go: the folders a Go tree needs to build programs, without the
# repository's history, its tests of the compiler, its API lists and its
# documentation. The standard library is not built here, as Go builds it as
# programs are: golang.rs gives the build cache that holds it.
#
# `ob` runs it (ob(1), PORTS) with PORT_DIR, PORT_SRC, PORT_BUILD and
# TOOLCHAIN set, and GOROOT_BOOTSTRAP set to the tree of a Go release that
# will build this one, as the recipe's [bootstrap] says. Nothing of the
# build machine's own Go settings reaches make.bash.
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
# The toolchain is for the build machine: GOOS and GOARCH are its own, and
# cgo is off, as the programs it builds need no C compiler.
(
	cd src
	env -i \
		PATH="$PATH" \
		HOME="$PORT_BUILD" \
		LC_ALL=C.UTF-8 \
		GOROOT_BOOTSTRAP="${GOROOT_BOOTSTRAP:?}" \
		GOTOOLCHAIN=local \
		GOTELEMETRY=off \
		GOENV=off \
		GOCACHE="$cache" \
		GOPATH="$PORT_BUILD/path" \
		CGO_ENABLED=0 \
		./make.bash
)

go="$TOOLCHAIN/go"
rm -rf "$go"
mkdir -p "$go/pkg"
for item in bin src lib misc VERSION go.env LICENSE PATENTS; do
	cp -R -p "$item" "$go/"
done
cp -R -p pkg/tool pkg/include "$go/pkg/"

# The standard library for Radix, built whole: what the patches change
# must compile. Nothing is checked until they exist.
if ls "$PORT_DIR"/patches/*.patch >/dev/null 2>&1; then
	env -i \
		PATH="$PATH" \
		HOME="$PORT_BUILD" \
		LC_ALL=C.UTF-8 \
		GOROOT="$go" \
		GOTOOLCHAIN=local \
		GOTELEMETRY=off \
		GOENV=off \
		GOFLAGS="-trimpath -buildvcs=false" \
		GOPROXY=off \
		GOSUMDB=off \
		GOWORK=off \
		GOCACHE="$cache" \
		GOMODCACHE="$PORT_BUILD/mod" \
		GOPATH="$PORT_BUILD/path" \
		GOOS=radix \
		GOARCH=amd64 \
		GOAMD64=v1 \
		CGO_ENABLED=0 \
		"$go/bin/go" build -o /dev/null std
fi
echo "go: the Go $version toolchain in $go"
