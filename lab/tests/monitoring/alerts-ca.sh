#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Makes the certificates the lab PC's mail catcher and webhook receiver
# use (ADR 0485), in the working directory, which is the test's own
# folder: a certificate authority of the test's own (ca.pem), and a
# certificate it signs (server.pem, with server.key) that is valid for the
# lab PC's address as the machine sees it, which is how alertd names the
# server, and for lab-pc.test. The authority and the keys are the test's,
# of no use elsewhere. The lab PC's clock has to be right, as the
# certificate holds dates.
#
#   alerts-ca.sh PEER
#
# PEER is the lab PC's address, as {peer} gives it.
set -eu

peer=$1

cat > server.ext << END
subjectAltName=IP:$peer,DNS:lab-pc.test
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=serverAuth
END
key="openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:prime256v1"
{
	$key -out ca.key
	openssl req -new -x509 -key ca.key -subj /CN=lab-alerts-ca -days 30 -out ca.pem
	$key -out server.key
	openssl req -new -key server.key -subj /CN=lab-pc.test -out server.csr
	openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -set_serial 2 -days 30 \
		-extfile server.ext -out server.pem
} > openssl.out 2>&1 || {
	cat openssl.out
	exit 1
}
