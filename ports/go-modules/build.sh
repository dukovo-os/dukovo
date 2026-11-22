#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Installs the Go modules patched for Radix (ADR 0600) into
# $TOOLCHAIN/go-modules, each in its own folder, with the list Go for
# Radix (x86_64-radix-go, ob(1), PORTS) reads: a line `PATH VERSION
# FOLDER all|one GO` for each, GO being the `go` line of its go.mod, the
# oldest Go whose language its code builds with. `ob` runs it with
# PORT_SRC, where each module is unpacked in its folder with its patch
# applied, and modules.list, which names them, beside them.
set -eu

modules="$TOOLCHAIN/go-modules"
rm -rf "$modules"
mkdir -p "$modules"
while read -r path version folder versions; do
	cp -R "$PORT_SRC/$folder" "$modules/$folder"
	mod="$modules/$folder/go.mod"
	# A module from before Go had modules has no go.mod: Go takes it as
	# the proxy serves it, its path alone, and so does a replacement.
	if [ ! -f "$mod" ]; then
		printf 'module %s\n' "$path" >"$mod"
	fi
	if [ "$versions" = all ] && grep -q '^require' "$mod"; then
		echo "go-modules: $path takes the place of every version of it, so it may" \
			"require no other module" >&2
		exit 1
	fi
	# Go 1.16 is what Go takes a go.mod without a `go` line for.
	go=$(sed -n 's/^go[[:space:]]\{1,\}\([0-9.]\{1,\}\).*/\1/p' "$mod")
	echo "$path $version $folder $versions ${go:-1.16}"
done <"$PORT_SRC/modules.list" >"$modules/list"
