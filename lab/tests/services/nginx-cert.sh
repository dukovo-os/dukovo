#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of services/nginx's certificate: nginx serves the
# machine's own certificate, which certd keeps.
#
#   nginx-cert.sh trust|served MACHINE SSH-PORT HTTPS-PORT
#
# `trust` fetches the certificate as `cert show this-machine --pem` gives
# it, and asks for nginx's page over HTTPS with that certificate as the
# only authority and the machine's own name, with no -k: the certificate
# must name the machine. `served` compares the fingerprint of the
# certificate nginx serves now with the one `cert show this-machine`
# gives, which a renewal changes. MACHINE is the machine's address and the
# ports the lab PC's that reach its SSH and its port 443, as {machine},
# {machine-port:22} and {machine-port:443} give them.
set -eu

mode="$1"
machine="$2"
ssh_port="$3"
https_port="$4"
key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"

on_machine() {
	ssh -q -p "$ssh_port" -i "$key" -o StrictHostKeyChecking=no \
		-o UserKnownHostsFile=/dev/null -o BatchMode=yes "lab@$machine" "$@"
}

name=$(on_machine hostname)
case "$mode" in
trust)
	on_machine cert show this-machine --pem > this-machine.pem
	# The machine's name is known to --resolve alone, so a proxy in the lab
	# PC's environment, which would be asked for it, is not used.
	curl -sS --noproxy '*' --cacert this-machine.pem --resolve "$name:$https_port:$machine" \
		"https://$name:$https_port/"
	;;
served)
	want=$(on_machine cert show this-machine | sed -n 's/^SHA-256: *//p')
	got=$(echo | openssl s_client -connect "$machine:$https_port" -servername "$name" \
		2> /dev/null | openssl x509 -noout -fingerprint -sha256 | sed 's/^.*=//')
	if [ -n "$want" ] && [ "$want" = "$got" ]; then
		echo "serving the fingerprint cert show gives: $got"
	else
		echo "nginx serves $got, and cert show says $want"
		exit 1
	fi
	;;
*)
	echo "nginx-cert.sh: trust or served" >&2
	exit 2
	;;
esac
