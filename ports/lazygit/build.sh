#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds lazygit for Radix with Go for Radix ($GO_RADIX, ob(1), PORTS) into
# the bundle's folder ($PORT_BUNDLE, ADR 0370): bin/lazygit, with the
# version and commit its releases are given, and its manual page, which
# lazygit has none of upstream (lazygit.1.scd). Lists the modules it links
# for the bundle's SBOM and licenses (ADR 0600).
set -eu

cd "$PORT_SRC"
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$PORT_DIR/recipe.toml")
commit=$(sed -n 's/^commit = "\(.*\)"$/\1/p' "$PORT_DIR/recipe.toml")
mkdir -p "$PORT_BUNDLE/bin" "$PORT_BUNDLE/man/man1"
"$GO_RADIX" build -p "$JOBS" \
	-ldflags "-s -w -X main.version=$version -X main.commit=$commit \
		-X main.buildSource=Dukovo" \
	-o "$PORT_BUNDLE/bin/lazygit" .
scdoc <"$PORT_DIR/lazygit.1.scd" >"$PORT_BUNDLE/man/man1/lazygit.1"
"$GO_RADIX" version -m "$PORT_BUNDLE/bin/lazygit" >"$PORT_INFO/modules.txt"
