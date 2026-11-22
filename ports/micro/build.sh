#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds micro for Radix with Go for Radix ($GO_RADIX, ob(1), PORTS) into
# the bundle's folder ($PORT_BUNDLE, ADR 0370): bin/micro and its manual
# page, as its Makefile's `build` does. The headers of its syntax files,
# which it reads to choose one without reading them all, are made first by
# its own generator, run on the build machine; the date it says it was
# built is the port's (SOURCE_DATE_EPOCH), as its build-date.go takes it.
# Lists the modules it links for the bundle's SBOM and licenses (ADR 0600).
set -eu

cd "$PORT_SRC"
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$PORT_DIR/recipe.toml")
commit=$(sed -n 's/^commit = "\(.......\).*"$/\1/p' "$PORT_DIR/recipe.toml")
date=$(date -u -d "@$SOURCE_DATE_EPOCH" '+%B %d, %Y')
(cd runtime && "$GO_RADIX" --host run syntax/make_headers.go syntax)
util=github.com/zyedidia/micro/v2/internal/util
mkdir -p "$PORT_BUNDLE/bin" "$PORT_BUNDLE/man/man1"
"$GO_RADIX" build -p "$JOBS" \
	-ldflags "-s -w -X $util.Version=$version -X $util.CommitHash=$commit \
		-X '$util.CompileDate=$date'" \
	-o "$PORT_BUNDLE/bin/micro" ./cmd/micro
install -m 0644 assets/packaging/micro.1 "$PORT_BUNDLE/man/man1/micro.1"
"$GO_RADIX" version -m "$PORT_BUNDLE/bin/micro" >"$PORT_INFO/modules.txt"
