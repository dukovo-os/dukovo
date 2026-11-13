#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of the nts-server test, as root: chrony in a network
# namespace, on a macvlan of the lab card that reaches the machine, which
# gives it a source address of its own on the lab network. chronyd takes
# no part in the lab PC's own clock (-x) and keeps everything in the
# test's folder.
#
#   nts-server.sh lab MACHINE PORT
#       fetches the certificate the machine made, over SSH as the lab
#       account, makes chronyd trust it, and has chronyd use the machine as
#       an NTS server; says "chrony ready" once chronyd has made key
#       establishment with the machine and holds cookies, and runs until
#       the lab ends it, which takes chronyd and the namespace away.
#   nts-server.sh authdata
#       shows what chrony says of the machine's NTS: `chronyc -N authdata`,
#       and whether it holds cookies.
#
# MACHINE is the machine's address and PORT the lab PC's port that
# reaches its SSH, as {machine} and {machine-port:22} give them.
set -eu

here=$(pwd)
pid=$$
# The lab PC's address in the namespace.
source_address=10.77.0.230/24

# chronyc, run in the namespace, which talks to chronyd by its socket.
ask_chrony() {
	ip netns exec "dlab$(cat pid)" "$chronyc" -h "$here/chronyd.sock" "$@"
}
chronyc=$(command -v chronyc || echo /usr/bin/chronyc)

case "$1" in
lab)
	machine="$2"
	port="$3"
	key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"
	chronyd=$(command -v chronyd || echo /usr/sbin/chronyd)
	[ -x "$chronyd" ] || {
		echo "the lab PC has no chronyd"
		exit 77
	}
	nic=$(ip -o route get "$machine" | sed -n 's/.* dev \([^ ]*\).*/\1/p' | head -n 1)
	[ -n "$nic" ] || {
		echo "the lab PC has no route to the machine"
		exit 77
	}
	# The certificate the machine's timed serves, which it made for the
	# test: chrony trusts it alone.
	ssh -p "$port" -i "$key" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
		-o BatchMode=yes -o LogLevel=ERROR "lab@$machine" "cat /users/lab/nts-lab.pem" > machine.pem
	ns="dlab$pid"
	ip netns add "$ns"
	gone() {
		[ -f chronyd.pid ] && kill "$(cat chronyd.pid)" 2> /dev/null || true
		ip netns del "$ns" 2> /dev/null || true
	}
	trap 'gone; exit 0' TERM INT
	trap gone EXIT
	echo "$pid" > pid
	ip link add link "$nic" name "dlab$pid" type macvlan mode bridge
	ip link set "dlab$pid" netns "$ns"
	ip -n "$ns" link set lo up
	ip -n "$ns" address add "$source_address" dev "dlab$pid"
	ip -n "$ns" link set "dlab$pid" up
	mkdir -p ntsdump
	cat > chrony.conf <<CONF
server $machine nts iburst minpoll 3 maxpoll 4
ntstrustedcerts $here/machine.pem
ntsdumpdir $here/ntsdump
driftfile $here/drift
pidfile $here/chronyd.pid
bindcmdaddress $here/chronyd.sock
cmdport 0
port 0
user root
CONF
	ip netns exec "$ns" "$chronyd" -x -f "$here/chrony.conf" > chronyd.log 2>&1 &
	i=0
	until [ -S chronyd.sock ] && cookies=$(ask_chrony -N authdata 2> /dev/null | awk '$2 == "NTS" { print $9 }') &&
		[ "${cookies:-0}" -gt 0 ]; do
		i=$((i + 1))
		if [ "$i" -gt 60 ]; then
			echo "chrony got no cookies from $machine in a minute"
			cat chronyd.log
			[ -S chronyd.sock ] && ask_chrony -N authdata || true
			exit 1
		fi
		sleep 1
	done
	echo "chrony ready"
	while :; do
		sleep 1
	done
	;;
authdata)
	ask_chrony -N authdata
	cookies=$(ask_chrony -N authdata | awk '$2 == "NTS" { print $9 }')
	if [ "${cookies:-0}" -gt 0 ]; then
		echo "NTS with $cookies cookies"
	else
		echo "NTS with no cookies"
		exit 1
	fi
	;;
esac
