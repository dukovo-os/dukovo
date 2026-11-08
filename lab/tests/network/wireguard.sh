#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's end of the WireGuard test, as root: a WireGuard interface
# of the test's own (the kernel's, or wireguard-go where the kernel has
# none), listening on UDP port $1, at 10.213.77.1, whose one peer is
# the machine at 10.213.77.2: IPv4 alone, since a lab PC may have no
# IPv6. The keys are the test's, made for it alone and of no use
# elsewhere. The machine finds the lab PC by its Endpoint, and the lab
# PC the machine by where its packets come from. It says "wireguard
# ready" once the interface is up, and runs until the lab ends it, which
# takes the interface away.
set -eu

port=$1
name="dlab$$"
echo "uJlI8TLdoca3Z7q3Lv7nuRQ0MKISm9p6MkV6td/ZHEE=" > lab-pc.key
echo "i36PFj7+L+F5oEcXMP17y1SyhwSqDqqULqDwqAeOo54=" > preshared.key
chmod 600 lab-pc.key preshared.key

gone() {
	ip link del "$name" 2> /dev/null || true
}
if ip link add "$name" type wireguard 2> /dev/null; then
	trap 'gone; exit 0' TERM INT
	echo "$name: the kernel's WireGuard"
elif command -v wireguard-go > /dev/null; then
	# In this script's process group, which the lab ends at the test's
	# end; the interface goes with it.
	wireguard-go -f "$name" &
	trap 'kill $! 2> /dev/null; gone; exit 0' TERM INT
	i=0
	until ip link show "$name" > /dev/null 2>&1; do
		i=$((i + 1))
		if [ "$i" -gt 100 ]; then
			echo "wireguard-go did not make $name"
			exit 1
		fi
		sleep 0.1
	done
	echo "$name: wireguard-go"
else
	echo "the lab PC has neither the kernel's WireGuard nor wireguard-go"
	exit 77
fi
wg set "$name" listen-port "$port" private-key ./lab-pc.key \
	peer sE3zt+aLAMgH49aIWem8AnakrtrNgjYsrHgn4Lylih0= preshared-key ./preshared.key \
	allowed-ips 10.213.77.2/32
ip address add 10.213.77.1/24 dev "$name"
ip link set "$name" up
echo "$name" > interface
echo "wireguard ready"
while :; do
	sleep 1
done
