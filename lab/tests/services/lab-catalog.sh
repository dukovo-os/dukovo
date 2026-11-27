#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of the tests that install the lab's own builds of nginx
# and Python: it copies the lab's catalog onto the machine, into the lab
# account's home as /users/lab/lab-catalog, with lab-catalog-add.sh beside
# it, which a step of the test runs as root to add the catalog as `labtest`.
#
#   lab-catalog.sh MACHINE PORT
#
# MACHINE is the machine's address and PORT the lab PC's port that reaches its
# SSH, as {machine} and {machine-port:22} give them. A copy a test before it
# left on the same boot is replaced.
#
# The lab's catalog is the folder that DUKOVO_LAB_CATALOG names: what the boot
# tests' catalog disk holds (ADR 0383), `catalog/` as a server serves it,
# `root.json`, its first root, and the `.dkv` files it lists, signed with
# throwaway keys, listing at least org.nginx.nginx and org.python.python.
# `ob lab` signs it from the bundles it packs. The lab image turns Dukovo's
# own catalog off, so that no catalog on the internet that vouches for these
# apps' publishers refuses the lab's builds (ADR 0592), and the tests install
# nginx and Python from `labtest` alone: an app installed from a catalog stays
# bound to it after it is removed (ADR 0383), so a test of the same boot that
# installed one from a file would be refused. Without the catalog, as when
# the lab was started with an image of its own, the script exits with 77,
# which skips the test.
set -eu

machine="$1"
port="$2"

folder="${DUKOVO_LAB_CATALOG:-}"
if [ -z "$folder" ] || [ ! -f "$folder/root.json" ] || [ ! -d "$folder/catalog" ]; then
	echo "no test catalog: DUKOVO_LAB_CATALOG names no folder with catalog/ and root.json"
	exit 77
fi
key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"
here=$(cd "$(dirname "$0")" && pwd)
set -- -i "$key" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes

ssh -q -p "$port" "$@" "lab@$machine" "rm -rf lab-catalog"
scp -q -r -P "$port" "$@" "$folder" "lab@$machine:lab-catalog"
scp -q -P "$port" "$@" "$here/lab-catalog-add.sh" "lab@$machine:lab-catalog-add.sh"
echo "copied: the test catalog"
