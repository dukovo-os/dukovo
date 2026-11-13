#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of the dhcp-server test, as root. Each client of the
# test is a network namespace with a macvlan of the lab card that reaches
# the machine, which gives it a MAC address of its own, as another
# computer on the lab network would have; dhcpcd runs in it, with no
# hooks, so that nothing of the lab PC's own settings changes.
#
#   dhcp-server.sh lab MACHINE
#       makes the two namespaces and says "dhcp lab ready", unless
#       something already answers DHCP to a client it does not know, which
#       the machine's server would only share the network with: then it
#       says so and exits with 77. It runs until the lab ends it, which
#       takes the namespaces away.
#   dhcp-server.sh client NUMBER
#       has client 1 or 2 ask for an address, and shows the lease it got.
#
# MACHINE is the machine's address, as {machine} gives it. Client 1's MAC
# address is the one the test's network(5) reserves an address for.
set -eu

here=$(pwd)
pid=$$
client_mac() {
	case "$1" in
	1) echo "02:d1:ab:00:00:01" ;;
	2) echo "02:d1:ab:00:00:02" ;;
	esac
}
dhcpcd=$(command -v dhcpcd || echo /usr/sbin/dhcpcd)

case "$1" in
lab)
	machine="$2"
	[ -x "$dhcpcd" ] || {
		echo "the lab PC has no dhcpcd"
		exit 77
	}
	# The card that reaches the machine.
	nic=$(ip -o route get "$machine" | sed -n 's/.* dev \([^ ]*\).*/\1/p' | head -n 1)
	[ -n "$nic" ] || {
		echo "the lab PC has no route to the machine"
		exit 77
	}
	# No address probing and no link-local fallback: each wait is longer
	# than the test needs.
	printf 'noarp\nnoipv4ll\n' > dhcpcd.conf
	gone() {
		for n in 1 2; do
			ip netns pids "dlab$pid-$n" 2> /dev/null | xargs -r kill 2> /dev/null || true
			ip netns del "dlab$pid-$n" 2> /dev/null || true
		done
	}
	trap 'gone; exit 0' TERM INT
	trap gone EXIT
	for n in 1 2; do
		ns="dlab$pid-$n"
		ip netns add "$ns"
		ip link add link "$nic" name "dlab$pid$n" address "$(client_mac "$n")" type macvlan mode bridge
		ip link set "dlab$pid$n" netns "$ns"
		ip -n "$ns" link set lo up
		ip -n "$ns" link set "dlab$pid$n" up
	done
	echo "$pid" > pid
	# What answers a client the lab network's dnsmasq does not know, which
	# it leaves alone: another DHCP server on the network. In test mode
	# dhcpcd asks and says what it was offered, and takes no address.
	if ip netns exec "dlab$pid-1" "$dhcpcd" -T -4 -c /bin/true -f "$here/dhcpcd.conf" -t 8 \
		"dlab${pid}1" 2>&1 | grep -qi 'offered'; then
		echo "something already answers DHCP on the lab network"
		exit 77
	fi
	echo "dhcp lab ready"
	while :; do
		sleep 1
	done
	;;
client)
	pid=$(cat pid)
	n="$2"
	ns="dlab$pid-$n"
	iface="dlab$pid$n"
	ip netns exec "$ns" "$dhcpcd" -1 -4 -c /bin/true -f "$here/dhcpcd.conf" -t 30 "$iface" \
		> /dev/null 2>&1 || true
	echo "client $n has"
	ip -n "$ns" -4 -br addr show "$iface"
	# The lease as dhcpcd kept it, a variable a line, without the quotes.
	ip netns exec "$ns" "$dhcpcd" -U -4 "$iface" 2> /dev/null | tr -d "'\""
	;;
esac
