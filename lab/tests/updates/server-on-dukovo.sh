#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of updates/server-on-dukovo (ADR 0542): it stands in
# for OLD, the server that serves now, and for the laptop, in
# docs/runbooks/update-server-move.md. The machine is NEW.
#
#   server-on-dukovo.sh needs
#   server-on-dukovo.sh copy-setup MACHINE PORT
#   server-on-dukovo.sh keys MACHINE PORT
#   server-on-dukovo.sh kept MACHINE PORT
#   server-on-dukovo.sh seed MACHINE PORT
#   server-on-dukovo.sh send MACHINE PORT REPOSITORY
#   server-on-dukovo.sh copy-only MACHINE PORT REPOSITORY
#   server-on-dukovo.sh check MACHINE WEB-PORT
#   server-on-dukovo.sh counts MACHINE WEB-PORT
#
# MACHINE is the machine's address and PORT the lab PC's port that
# reaches its SSH, WEB-PORT the one that reaches its port 80, as
# {machine}, {machine-port:22} and {machine-port:80} give them.
# REPOSITORY is the name of a folder of the test update server `ob lab`
# builds beside the test updates: repository-before-pull, the release keys'
# repository as the old server served it, or repository-after-pull, with
# the withdrawal of the stable release. The scripts run in the test's own
# folder, which `ob lab` removes when the test ends.
#
#   needs       exits with 77, which skips the test, when what it takes is
#               not there: the test update server as this tree builds it
#               (a build from before this test has none of its folders),
#               the files of the server's setup for Dukovo, and nginx's
#               bundle, which setup would fetch from the catalog.
#   copy-setup  copies those files to dukovo-update-server in the lab
#               account's home, as `ob catalog server setup --dukovo` does.
#   keys        step 4 of the runbook: the online keys of the old server,
#               each secret half as base64 and each public half beside it,
#               in the folder dukovo-update-keys of the lab account's own
#               /tmp, which no one else reaches (ADR 0255).
#   kept        step 5's check: the public halves of the online keys the
#               machine keeps are the old server's.
#   seed        step 6: the old server's metadata folder as a tar file, and
#               the plans of its rollouts, in the lab account's home.
#   send        `ob release send`: deploys REPOSITORY as `ob release
#               deploy` does, and waits for the server to publish it.
#   copy-only   the same, but stops before the word `ready`: the deploy
#               waits in the server's incoming folder.
#   check       `ob release check`: the repository the machine serves, read
#               as a computer online reads it, from the first root of the
#               release keys, every release list and channel.
#   counts      the versions of the old server's timestamp and snapshot,
#               and the new server's, which must have counted on from them.
set -eu

mode="$1"

updates="${DUKOVO_LAB_UPDATES:-}"
old="$updates/update-server"
files="${DUKOVO_LAB_SERVER_FILES:-}"
online="$old/server/var/lib/dukovo-system"
here=$(cd "$(dirname "$0")" && pwd)
ob="$(cd "$here/../../.." && pwd)/ob"

if [ "$mode" = needs ]; then
	for need in repository-before-pull repository-after-pull catalog-key/catalog.key \
		server/var/lib/dukovo-system/online.key server/var/lib/dukovo-system/rollout.json; do
		if [ -z "$updates" ] || [ ! -e "$old/$need" ]; then
			echo "no $need in the test update server: run the update tests through ./ob lab,"
			echo "which builds it"
			exit 77
		fi
	done
	if [ -z "$files" ] || [ ! -f "$files/dukovo/setup" ] || [ ! -f "$files/catalog-repo" ]; then
		echo "no files of the update server's setup for Dukovo: run the update tests through"
		echo "./ob lab, which stages them"
		exit 77
	fi
	if ! ls "${DUKOVO_LAB_BUNDLES:-/nowhere}"/org.nginx.nginx-*.dkv >/dev/null 2>&1; then
		echo "no bundle org.nginx.nginx: run the update tests through ./ob lab, which packs it"
		exit 77
	fi
	echo "the test update server and the setup's files are there"
	exit 0
fi

lab_key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"
machine="$2"
port="$3"
# The words ssh and scp both take, and `ob` too, as OB_UPDATE_SSH_OPTIONS:
# the machine is the lab PC's own port away, and takes the lab key.
words="-o Port=$port -o IdentityFile=$lab_key -o IdentitiesOnly=yes -o StrictHostKeyChecking=no \
-o UserKnownHostsFile=/dev/null -o BatchMode=yes -o LogLevel=ERROR"

# Runs the commands $1 on the machine as the lab account, whose login shell
# is the machine's, as the runbook's `ssh "$NEW"` is. The words are split
# on purpose.
guest() {
	# shellcheck disable=SC2086
	ssh $words "lab@$machine" "$@"
}

case "$mode" in
copy-setup)
	guest 'rm -rf dukovo-update-server'
	# shellcheck disable=SC2086
	scp -q -r -p $words "$files" "lab@$machine:dukovo-update-server"
	echo "copied: the server's files"
	;;

keys)
	guest 'umask 077 && mkdir /tmp/dukovo-update-keys'
	base64 -w0 "$old/catalog-key/catalog.key" |
		guest 'umask 077 && cat > /tmp/dukovo-update-keys/catalog.key'
	base64 -w0 "$online/online.key" |
		guest 'umask 077 && cat > /tmp/dukovo-update-keys/system.key'
	guest 'umask 077 && cat > /tmp/dukovo-update-keys/catalog.json' <"$old/catalog-key/catalog.json"
	guest 'umask 077 && cat > /tmp/dukovo-update-keys/system.json' <"$online/online.json"
	listed=$(guest 'ls /tmp/dukovo-update-keys')
	echo "keys: $(echo "$listed" | wc -l) files in /tmp/dukovo-update-keys: $(echo "$listed" | tr '\n' ' ')"
	;;

kept)
	for kind in system catalog; do
		if [ "$kind" = system ]; then
			sent="$online/online.json"
		else
			sent="$old/catalog-key/catalog.json"
		fi
		guest "cat /var/lib/dukovo-$kind/online.json" | cmp - "$sent"
		echo "kept: the $kind online key's public half is the one sent"
	done
	;;

seed)
	# What the old server served before the release keys withdrew the
	# stable release, as update-server.sh serves it.
	served=$(readlink -f "$old/system-before-pull")
	tar -C "$served" -cf - metadata | guest 'cat > system-metadata.tar'
	guest 'cat > rollout.json' <"$online/rollout.json"
	echo "seed: sent the metadata folder and rollout.json"
	;;

send | copy-only)
	repository="$old/$4"
	if [ ! -d "$repository/metadata" ]; then
		echo "$repository is not an update repository"
		exit 1
	fi
	flag=
	if [ "$mode" = copy-only ]; then flag=--copy-only; fi
	export OB_UPDATE_SSH="lab@$machine"
	export OB_UPDATE_SSH_OPTIONS="$words"
	# shellcheck disable=SC2086
	"$ob" release send $flag "$repository"
	echo "$mode: $4"
	;;

check)
	"$ob" release check --root "$old/repository-before-pull/metadata/1.root.json" \
		"http://$machine:$port/system/"
	;;

counts)
	# The version of the timestamp in the file $1, and of the snapshot it
	# lists: a client keeps both, and takes no lower one.
	versions() {
		python3 -c 'import json, sys; s = json.load(open(sys.argv[1]))["signed"]; print(s["version"], s["meta"]["snapshot.json"]["version"])' "$1"
	}
	served=$(readlink -f "$old/system-before-pull")
	curl -s "http://$machine:$port/system/metadata/timestamp.json" >new-timestamp.json
	# shellcheck disable=SC2046
	set -- $(versions "$served/metadata/timestamp.json")
	old_timestamp=$1 old_snapshot=$2
	# shellcheck disable=SC2046
	set -- $(versions new-timestamp.json)
	new_timestamp=$1 new_snapshot=$2
	echo "timestamp: the old server's $old_timestamp, the new server's $new_timestamp"
	echo "snapshot: the old server's $old_snapshot, the new server's $new_snapshot"
	if [ "$new_timestamp" -gt "$old_timestamp" ] && [ "$new_snapshot" -ge "$old_snapshot" ]; then
		echo "counts on from the old server's"
	else
		echo "does not count on from the old server's"
		exit 1
	fi
	;;

*)
	echo "no such mode: $mode" >&2
	exit 2
	;;
esac
