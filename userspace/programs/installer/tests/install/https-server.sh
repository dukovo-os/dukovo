#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Serves this folder's answers files as setup files on the web, for the
# boot tests of setup files downloaded over HTTPS (ADR 0319), with
# openssl(1)'s s_server, from a copy in /tmp/web:
#
#   https://127.0.0.1:8443/   a certificate from a test authority made
#                             here, which the system is told to trust
#   https://127.0.0.1:8444/   a certificate signed by itself, which
#                             nothing trusts, so its files are refused
#
# The keys are made afresh each time, for the test alone, and protect
# nothing. It prints the SHA-256 of answers.json, as sha256sum(1) does,
# and "serving setup files" once both servers listen.
set -eu

web=/tmp/web
mkdir -p "$web"
cp /system/tests/install/*.json "$web/"
cd "$web"

# A key of its own for each certificate, on P-256, as quick to make as
# any, and good for two days.
key() {
	openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$1" 2>/dev/null
}
for name in ca server other; do
	key "$name.key"
done
openssl req -x509 -new -key ca.key -out ca.pem -days 2 -subj "/CN=Setup file test authority" \
	-addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign" 2>/dev/null
printf '%s\n' 'basicConstraints=critical,CA:FALSE' 'keyUsage=critical,digitalSignature' \
	'extendedKeyUsage=serverAuth' 'subjectAltName=IP:127.0.0.1' >server.ext
openssl req -new -key server.key -subj "/CN=127.0.0.1" -out server.csr 2>/dev/null
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -days 2 \
	-extfile server.ext -out server.pem 2>/dev/null
openssl req -x509 -new -key other.key -out other.pem -days 2 -subj "/CN=127.0.0.1" \
	-addext "subjectAltName=IP:127.0.0.1" 2>/dev/null

# The test authority is trusted as dukovo-certificates(7) says a
# company's is: the settings' list becomes a copy of the system's, with
# its certificate added, in place of the link to the system's.
roots=/system/settings/ssl/certs/ca-certificates.crt
cat /system/resources/certificates/ca-certificates.crt ca.pem >"$roots.new"
mv "$roots.new" "$roots"

openssl s_server -quiet -accept 8443 -cert server.pem -key server.key -WWW >server.log 2>&1 &
openssl s_server -quiet -accept 8444 -cert other.pem -key other.key -WWW >other.log 2>&1 &

# Each listens once its port takes a connection.
for port in 8443 8444; do
	tries=0
	until nc -z 127.0.0.1 "$port" 2>/dev/null; do
		tries=$((tries + 1))
		if [ "$tries" -gt 100 ]; then
			echo "https-server: nothing listens on port $port" >&2
			exit 1
		fi
		sleep 0.1
	done
done
sha256sum answers.json
echo "serving setup files"
