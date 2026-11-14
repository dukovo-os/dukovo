#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's end of the reports tests: the staging reports server
# (report-server.py beside this file) that reportd(8) sends to, and the
# phone that fetches a report's one-time download (ADR 0531). Everything
# is in the test's folder, and the lab PC's own services are left alone.
#
#   report-lab.sh serve PORT PEER
#   report-lab.sh download MACHINE SSH-PORT PORT
#
# serve makes a certificate authority of the test's own (ca.pem, for the
# test to copy to the machine) and a certificate it signs for PEER, the
# lab PC's address as the machine sees it, as {peer} gives it, which is
# how reportd names the server. It then runs the server on PORT, keeping
# the reports it takes in the folder reports, and says "report server
# ready" once it listens. It runs until the lab ends it. The authority and
# the keys are the test's, of no use elsewhere, and the lab PC's clock has
# to be right, as the certificate holds dates.
#
# download reads what `report --save --port N` printed, in save.out in the
# lab account's home on the machine, and fetches the address's path from
# the machine's port PORT as a phone does, twice. MACHINE and SSH-PORT are
# the machine's address and the lab PC's port that reaches its SSH, and
# PORT the lab PC's port that reaches the download's, as {machine},
# {machine-port:22} and {machine-port:N} give them. It says what each
# fetch got:
#
#   first fetch: 200
#   text: Dukovo problem report
#   crash: fault_here in the text
#   second fetch: refused (410)
#
# The certificate is not checked: it is made for the one download, so
# that the only check a phone has is the fingerprint report prints beside
# the address.
#
# serve exits with 77 where the lab PC cannot do its part: no Python or
# openssl.
set -eu

action=$1

case "$action" in
serve)
	port=$2
	peer=$3
	for program in python3 openssl; do
		if ! command -v "$program" > /dev/null 2>&1; then
			echo "the lab PC has no $program"
			exit 77
		fi
	done
	cat > server.ext << END
subjectAltName=IP:$peer,DNS:lab-pc.test
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=serverAuth
END
	key="openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:prime256v1"
	{
		$key -out ca.key
		openssl req -new -x509 -key ca.key -subj /CN=lab-reports-ca -days 30 -out ca.pem
		$key -out server.key
		openssl req -new -key server.key -subj /CN=lab-pc.test -out server.csr
		openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -set_serial 2 -days 30 \
			-extfile server.ext -out server.pem
	} > openssl.out 2>&1 || {
		cat openssl.out
		exit 1
	}
	mkdir -p reports
	exec python3 -I "$(dirname "$0")/report-server.py" serve --port "$port" \
		--cert server.pem --key server.key --dir reports
	;;
download)
	machine=$2
	ssh_port=$3
	port=$4
	key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"
	printed=$(ssh -p "$ssh_port" -i "$key" -o StrictHostKeyChecking=no \
		-o UserKnownHostsFile=/dev/null -o BatchMode=yes "lab@$machine" cat save.out)
	path=$(printf '%s\n' "$printed" \
		| sed -n 's|.*https://[^/]*\(/[^ ]*/report\.txt\).*|\1|p' | head -n 1)
	if [ -z "$path" ]; then
		echo "report printed no download address:"
		echo "$printed"
		exit 1
	fi
	# A fetch that got nothing leaves no file and is told as status 000.
	fetch() {
		rm -f "$1"
		curl -k -sS --max-time 30 -o "$1" -w '%{http_code}' "https://$machine:$port$path" \
			2> /dev/null || true
	}
	first=$(fetch first.txt)
	echo "first fetch: $first"
	echo "text: $(head -n 1 first.txt 2> /dev/null)"
	if grep -q fault_here first.txt 2> /dev/null; then
		echo "crash: fault_here in the text"
	else
		echo "crash: fault_here is not in the text"
	fi
	second=$(fetch second.txt)
	if [ "$second" = 200 ]; then
		echo "second fetch: not refused (200)"
	else
		echo "second fetch: refused ($second)"
	fi
	;;
*)
	echo "usage: report-lab.sh serve PORT PEER | download MACHINE SSH-PORT PORT"
	exit 2
	;;
esac
