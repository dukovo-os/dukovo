#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds fzf for Radix with Go for Radix ($GO_RADIX, ob(1), PORTS) into the
# bundle's folder ($PORT_BUNDLE, ADR 0370): bin/fzf and its manual page,
# with the version and commit its Makefile gives it. Lists the modules it
# links for the bundle's SBOM and licenses (ADR 0600).
set -eu

cd "$PORT_SRC"
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$PORT_DIR/recipe.toml")
commit=$(sed -n 's/^commit = "\(.......\).*"$/\1/p' "$PORT_DIR/recipe.toml")
mkdir -p "$PORT_BUNDLE/bin" "$PORT_BUNDLE/man/man1"
"$GO_RADIX" build -p "$JOBS" \
	-ldflags "-s -w -X main.version=$version -X main.revision=$commit" \
	-o "$PORT_BUNDLE/bin/fzf" .
install -m 0644 man/man1/fzf.1 "$PORT_BUNDLE/man/man1/fzf.1"
"$GO_RADIX" version -m "$PORT_BUNDLE/bin/fzf" >"$PORT_INFO/modules.txt"
