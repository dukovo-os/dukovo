#!/system/commands/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# The certificates boot test's attacks: what a certd that is not what it
# was, or anything else that holds its folder, could do to the pair a
# service reads, one at a time, as root. Each puts something wrong where
# this-machine was and has Spark start the test service again, which it
# must refuse; `restore` puts the real link back.
#
#   attack.sh folder|outside|link|big|fifo|restore

set -e
cd /system/state/certd/certificates
secrets=/system/state/spark/secrets/leakdir

# The real link, kept beside the store, and a generation of the name
# certd never made, holding the real chain.
fake() {
	good=$(readlink this-machine)
	mv this-machine ../kept
	mkdir .this-machine.9
	cp "$good/fullchain.pem" .this-machine.9/fullchain.pem
}

case "${1:-}" in
folder)
	mv this-machine ../kept
	mkdir this-machine
	;;
outside)
	# A folder of Spark's own, which holds a key of its own: the marker
	# says if it reaches a service.
	mv this-machine ../kept
	mkdir -p "$secrets"
	printf 'LEAK-%s\n' MARKER > "$secrets/fullchain.pem"
	printf 'LEAK-%s\n' MARKER > "$secrets/key.pem"
	ln -s ../../spark/secrets/leakdir this-machine
	;;
link)
	fake
	mkdir -p "$secrets"
	printf 'LEAK-%s\n' MARKER > "$secrets/key.pem"
	ln -s ../../../spark/secrets/leakdir/key.pem .this-machine.9/key.pem
	ln -s .this-machine.9 this-machine
	;;
big)
	fake
	dd if=/dev/zero of=.this-machine.9/key.pem bs=1024 count=65 2> /dev/null
	ln -s .this-machine.9 this-machine
	;;
fifo)
	fake
	mkfifo .this-machine.9/key.pem
	ln -s .this-machine.9 this-machine
	;;
restore)
	rm -rf this-machine .this-machine.9 "$secrets"
	mv ../kept this-machine
	;;
*)
	echo "attack.sh: folder, outside, link, big, fifo or restore" >&2
	exit 2
	;;
esac
# A restart that Spark cannot make says so, and ends with status 1.
rc=0
service certtest restart || rc=$?
echo "attack ${1}: restart ended with $rc"
