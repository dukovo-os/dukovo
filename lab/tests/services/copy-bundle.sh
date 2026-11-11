#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of the service bundles' tests: it copies a bundle of
# the catalog onto the machine, into the lab account's home, for `bundle
# install` to read.
#
#   copy-bundle.sh ID MACHINE PORT
#
# ID is the bundle's ID, such as org.nginx.nginx, whose .dkv file `ob lab`
# packs with the catalog's other bundles; MACHINE the machine's address and
# PORT the lab PC's port that reaches its SSH, as {machine} and
# {machine-port:22} give them. The file goes to /users/lab/ID.dkv. Without
# the bundles, as when the lab was started with an image of its own, the
# script exits with 77, which skips the test.
set -eu

id="$1"
machine="$2"
port="$3"

bundles="${DUKOVO_LAB_BUNDLES:-}"
file=""
if [ -n "$bundles" ]; then
	for candidate in "$bundles/$id"-*.dkv; do
		[ -f "$candidate" ] && file="$candidate"
	done
fi
if [ -z "$file" ]; then
	echo "no bundle $id: run the service bundles' tests through ./ob lab, which packs them"
	exit 77
fi
key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"

scp -q -P "$port" -i "$key" \
	-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes \
	"$file" "lab@$machine:$id.dkv"
echo "copied: $id"
