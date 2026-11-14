#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of services/certificates: it makes the certificates an
# authority gives, with its own openssl and as the people who use them make
# theirs: a root authority, an intermediate it signed and a server
# certificate for lab-certs.example that the intermediate signed, and copies
# them into the lab account's home on the machine, where cert add reads
# them.
#
#   certificates-chain.sh MACHINE PORT
#
# leaf.pem holds the server's certificate alone, chain.pem the server's
# followed by the intermediate's, leaf.key the key and root.pem the root,
# each as lab-certs-FILE. MACHINE is the machine's address and PORT the lab
# PC's port that reaches its SSH, as {machine} and {machine-port:22} give
# them.
set -eu

machine="$1"
port="$2"
key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"

ec="-newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes"
printf 'basicConstraints=critical,CA:TRUE,pathlen:0\nkeyUsage=critical,keyCertSign,cRLSign\n' > middle.ext
printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:lab-certs.example\n' > leaf.ext
openssl req -x509 $ec -days 30 -subj /CN=Lab-Root -keyout root.key -out root.pem 2> /dev/null
openssl req $ec -subj /CN=Lab-Intermediate -keyout middle.key -out middle.csr 2> /dev/null
openssl x509 -req -in middle.csr -CA root.pem -CAkey root.key -CAcreateserial -days 30 \
	-extfile middle.ext -out middle.pem 2> /dev/null
openssl req $ec -subj /CN=lab-certs.example -keyout leaf.key -out leaf.csr 2> /dev/null
openssl x509 -req -in leaf.csr -CA middle.pem -CAkey middle.key -CAcreateserial -days 30 \
	-extfile leaf.ext -out leaf.pem 2> /dev/null
cat leaf.pem middle.pem > chain.pem

for file in leaf.pem chain.pem leaf.key root.pem; do
	scp -q -P "$port" -i "$key" \
		-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes \
		"$file" "lab@$machine:lab-certs-$file"
done
echo "copied: the chain of lab-certs.example"
