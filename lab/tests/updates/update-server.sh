#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of the update server tests (ADR 0466): the test update
# server `ob lab` builds beside the test updates, served over HTTP, or its
# bridge release's update file copied onto the machine.
#
#   update-server.sh serve PORT
#   update-server.sh copy-file MACHINE PORT
#
# serve answers on PORT of every address of the lab PC's, as {port:NAME}
# gives one, with the repository at /system/, until the test ends.
# copy-file copies the update file into the lab account's home, as
# /users/lab/bridge.update; MACHINE and PORT are {machine} and
# {machine-port:22}. Without the test update server, as when the lab was
# started with an image of its own, the script exits with 77, which skips
# the test.
set -eu

server="${DUKOVO_LAB_UPDATES:-}/update-server"
if [ -z "${DUKOVO_LAB_UPDATES:-}" ] || [ ! -d "$server/system" ]; then
	echo "no test update server: run the update tests through ./ob lab, which builds it"
	exit 77
fi

case "$1" in
serve)
	echo "serving updates on port $2"
	exec python3 -m http.server --bind 0.0.0.0 --directory "$server" "$2"
	;;
copy-file)
	key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"
	file=$(ls "$server"/dukovo-*.update)
	scp -q -P "$3" -i "$key" \
		-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes \
		"$file" "lab@$2:bridge.update"
	echo "copied: $(basename "$file")"
	;;
*)
	echo "say serve or copy-file" >&2
	exit 2
	;;
esac
