#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Builds Caddy for Radix with Go for Radix ($GO_RADIX, ob(1), PORTS) into
# the bundle's folder ($PORT_BUNDLE, ADRs 0370 and 0601): bin/caddy, with
# the version its releases say, bin/caddy-service, the Caddyfile it starts
# from in conf/, and its manual pages, which Caddy writes itself (`caddy
# manpage`): a build for the build machine writes them, dated as the port
# is (SOURCE_DATE_EPOCH). Lists the modules it links for the bundle's SBOM
# and licenses (ADR 0600).
set -eu

cd "$PORT_SRC"
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$PORT_DIR/recipe.toml")
mkdir -p "$PORT_BUNDLE/bin" "$PORT_BUNDLE/conf" "$PORT_BUNDLE/man"
"$GO_RADIX" build -p "$JOBS" \
	-ldflags "-s -w -X github.com/caddyserver/caddy/v2.CustomVersion=v$version" \
	-o "$PORT_BUNDLE/bin/caddy" ./cmd/caddy
install -m 0755 "$PORT_DIR/caddy-service" "$PORT_BUNDLE/bin/caddy-service"
install -m 0644 "$PORT_DIR/Caddyfile" "$PORT_BUNDLE/conf/Caddyfile"
"$GO_RADIX" --host build -p "$JOBS" -o "$PORT_BUILD/caddy" ./cmd/caddy
"$PORT_BUILD/caddy" manpage --directory "$PORT_BUNDLE/man/man8"
"$GO_RADIX" version -m "$PORT_BUNDLE/bin/caddy" >"$PORT_INFO/modules.txt"
