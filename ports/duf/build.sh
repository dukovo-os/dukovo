#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds duf for Radix with Go for Radix ($GO_RADIX, ob(1), PORTS) into the
# bundle's folder ($PORT_BUNDLE, ADR 0370): bin/duf and its manual page.
# Lists the modules it links for the bundle's SBOM and licenses (ADR 0600).
set -eu

cd "$PORT_SRC"
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$PORT_DIR/recipe.toml")
mkdir -p "$PORT_BUNDLE/bin" "$PORT_BUNDLE/man/man1"
"$GO_RADIX" build -p "$JOBS" -ldflags "-s -w -X main.Version=$version" \
	-o "$PORT_BUNDLE/bin/duf" .
install -m 0644 duf.1 "$PORT_BUNDLE/man/man1/duf.1"
"$GO_RADIX" version -m "$PORT_BUNDLE/bin/duf" >"$PORT_INFO/modules.txt"
