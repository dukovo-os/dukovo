#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's end of services/acme: Pebble, Let's Encrypt's own test
# authority, and pebble-challtestsrv, a DNS server whose answers the test
# sets (the kit, lab/pebble/kit.sh, builds both from a pinned release).
#
#   acme-pebble.sh ACME-PORT MANAGEMENT-PORT DNS-PORT CHALLENGE-PORT HTTP-PORT PEER MACHINE
#
# Pebble answers ACME on ACME-PORT with a certificate for PEER, the lab PC's
# address as the machine sees it, that an authority of the test's own
# signed: that authority's certificate is ca.pem in the test's folder, for
# the test to copy to the machine, which then trusts Pebble as it would
# Let's Encrypt. The DNS server answers on DNS-PORT, and every name it has
# no record for leads to MACHINE, the machine's address as the lab PC
# reaches it, so that Pebble's check of a name connects to HTTP-PORT, the
# lab PC's port that reaches the machine's port 80. The test makes the
# machine ask this DNS server and no other. CHALLENGE-PORT is the DNS
# server's management interface, on the lab PC's own loopback, where the
# test sets a TXT record.
#
# Pebble is set to be easy to test with: nothing sleeps, no nonce is
# refused for the sake of it, an authorization is not reused for another
# order, an order and an authorization are ready within a second, and
# a certificate's life is 90 days with the profile "default", or 150
# seconds with the profile "short"; an order that names none gets one at
# random, so the test names one.
# It says "pebble ready" once both answer, and runs until the lab ends it.
# Everything is in the test's folder; the lab PC's own settings are left
# alone.
#
# Exits with 77 where the lab PC cannot do its part: the kit cannot be
# built, or there is no openssl.
set -eu

acme_port=$1
management_port=$2
dns_port=$3
challenge_port=$4
http_port=$5
peer=$6
machine=$7

here=$(cd "$(dirname "$0")" && pwd)
if ! command -v openssl > /dev/null 2>&1 || ! command -v curl > /dev/null 2>&1; then
	echo "the lab PC has no openssl or curl"
	exit 77
fi
status=0
made=$(sh "$here/../../pebble/kit.sh") || status=$?
if [ "$status" -ne 0 ]; then
	echo "$made"
	exit "$status"
fi
kit=${made#kit: }

# An authority of the test's own, and the certificate it signs for Pebble's
# address; both are for this test alone.
key="openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:prime256v1"
cat > server.ext << END
subjectAltName=IP:$peer,IP:127.0.0.1,DNS:localhost
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=serverAuth
END
{
	$key -out ca.key
	openssl req -new -x509 -key ca.key -subj /CN=lab-pebble-ca -days 30 -out ca.pem
	$key -out server.key
	openssl req -new -key server.key -subj /CN=lab-pebble -out server.csr
	openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -set_serial 2 -days 30 \
		-extfile server.ext -out server.pem
} > openssl.out 2>&1 || {
	cat openssl.out
	exit 1
}

work=$(pwd)
cat > pebble.json << END
{
  "pebble": {
    "listenAddress": "0.0.0.0:$acme_port",
    "managementListenAddress": "127.0.0.1:$management_port",
    "certificate": "$work/server.pem",
    "privateKey": "$work/server.key",
    "httpPort": $http_port,
    "tlsPort": $http_port,
    "ocspResponderURL": "",
    "externalAccountBindingRequired": false,
    "retryAfter": { "authz": 1, "order": 1 },
    "keyAlgorithm": "ecdsa",
    "profiles": {
      "default": { "description": "90 days", "validityPeriod": 7776000 },
      "short": { "description": "150 seconds", "validityPeriod": 150 }
    }
  }
}
END

pids=""
stop() {
	# shellcheck disable=SC2086
	[ -z "$pids" ] || kill $pids 2> /dev/null || true
	exit 0
}
trap stop TERM INT

"$kit/pebble-challtestsrv" -http01 "" -https01 "" -doh "" -tlsalpn01 "" \
	-dnsserver "0.0.0.0:$dns_port" -management "127.0.0.1:$challenge_port" \
	-defaultIPv4 "$machine" -defaultIPv6 "" > challtestsrv.log 2>&1 &
pids="$!"
PEBBLE_VA_NOSLEEP=1 PEBBLE_WFE_NONCEREJECT=0 PEBBLE_AUTHZREUSE=0 \
	"$kit/pebble" -config pebble.json -dnsserver "127.0.0.1:$dns_port" > pebble.log 2>&1 &
pids="$pids $!"

# Both answer within seconds; a bound, and what was seen when it is spent.
seen=""
i=0
while [ "$i" -lt 60 ]; do
	seen=$(curl -sS --noproxy '*' --cacert ca.pem -o /dev/null -w '%{http_code}' \
		"https://127.0.0.1:$acme_port/dir" 2>&1 || true)
	if [ "$seen" = 200 ] && nc -z 127.0.0.1 "$challenge_port" 2> /dev/null; then
		echo "pebble ready: ACME on $acme_port, DNS on $dns_port, names lead to $machine"
		wait
		exit 0
	fi
	i=$((i + 1))
	sleep 0.5
done
echo "pebble did not answer in 30 seconds: its directory gave '$seen'"
tail -5 pebble.log challtestsrv.log
exit 1
