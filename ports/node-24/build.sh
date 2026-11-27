#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Makes the bundle of a Node.js line from Node.js's own build for x86-64
# Linux, which ob fetched, checked against the SHA-256 the recipe pins and
# unpacked into PORT_SRC (ADR 0627). Nothing is built: the bundle is the
# release as Node.js made it, its programs Linux programs that run in a
# Linux view (ADR 0625). `ob` runs it with PORT_SRC, PORT_BUILD and
# PORT_BUNDLE set; this script is the same for every line.
#
# A bundle holds regular files only (bundle(1)), and the release's npm and
# npx are links to the scripts in npm's folder, which find the rest of npm
# from where they are. Each becomes a script that loads npm's own from
# its place, so that npm runs from its folder as the release lays it out.
# Node.js 24's corepack is kept as npm's other tools are, and is no
# command: it would write its links into the bundle, which no one may
# change.
set -eu

rm -rf "$PORT_BUNDLE"
mkdir -p "$PORT_BUNDLE"

# What the release holds: the program, npm and the other modules it comes
# with, the headers that native add-ons build against (node-gyp finds them
# beside the program), the manual page and the license file, which carries
# the licenses of every library Node.js includes. The release's links are
# made again below.
for kept in bin/node lib include share/man LICENSE README.md; do
	[ -e "$PORT_SRC/$kept" ] || {
		echo "node: the release has no $kept" >&2
		exit 1
	}
	mkdir -p "$PORT_BUNDLE/$(dirname "$kept")"
	cp -R "$PORT_SRC/$kept" "$PORT_BUNDLE/$kept"
done

# npm and npx, each a script for Node.js that runs npm's own, which the
# release links to. PATH in a Linux bundle's view names the bundle's bin
# first, so `env` finds this bundle's node (ADR 0625).
for command in npm npx; do
	target=lib/node_modules/npm/bin/$command-cli.js
	[ -f "$PORT_BUNDLE/$target" ] || {
		echo "node: the release has no $target" >&2
		exit 1
	}
	cat >"$PORT_BUNDLE/bin/$command" <<SCRIPT
#!/usr/bin/env node
// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// $command, from its place in npm's folder, which the release links to.
require("../$target");
SCRIPT
	chmod 755 "$PORT_BUNDLE/bin/$command"
done

# The release's own links to npm's scripts are gone with the copy above;
# any other link is something the bundle cannot hold.
links=$(find "$PORT_BUNDLE" -type l)
if [ -n "$links" ]; then
	echo "node: the release holds links a bundle cannot: $links" >&2
	exit 1
fi
if [ -n "$(find "$PORT_BUNDLE" ! -type f ! -type d)" ]; then
	echo "node: the bundle holds something that is neither a file nor a folder" >&2
	exit 1
fi
chmod -R go-w "$PORT_BUNDLE"

echo "node: $("$PORT_BUNDLE/bin/node" --version 2>/dev/null || echo "$PORT_BUNDLE/bin/node"), $(find "$PORT_BUNDLE" -type f | wc -l) files, $(du -sk "$PORT_BUNDLE" | cut -f1) KiB"
