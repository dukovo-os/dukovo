#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of web-console/add-app: it copies the test catalog onto
# the machine, into the lab account's home, for a step of the test to add as a
# catalog (`bundle catalog add`).
#
#   add-app-catalog.sh MACHINE PORT
#
# MACHINE is the machine's address and PORT the lab PC's port that reaches its
# SSH, as {machine} and {machine-port:22} give them. The folder is copied to
# /users/lab/lab-catalog, which must not be there yet.
#
# The test catalog is the folder that DUKOVO_LAB_CATALOG names: what the boot
# tests' catalog disk holds (ADR 0383), `catalog/` as a server serves it,
# `root.json`, its first root, and the `.dkv` files it lists, signed with
# throwaway keys, listing at least org.nginx.nginx and org.python.python. The
# lab does not stage it yet; without it, as when the lab was started with an
# image of its own, the script exits with 77, which skips the test.
set -eu

machine="$1"
port="$2"

folder="${DUKOVO_LAB_CATALOG:-}"
if [ -z "$folder" ] || [ ! -f "$folder/root.json" ] || [ ! -d "$folder/catalog" ]; then
	echo "no test catalog: DUKOVO_LAB_CATALOG names no folder with catalog/ and root.json"
	exit 77
fi
key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"

scp -q -r -P "$port" -i "$key" \
	-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes \
	"$folder" "lab@$machine:lab-catalog"
echo "copied: the test catalog"
