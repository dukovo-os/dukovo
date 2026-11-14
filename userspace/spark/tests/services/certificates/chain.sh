#!/system/commands/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# The certificates boot test's certificates for added.example, made as
# the people who use them make theirs: a root authority, an intermediate
# it signed and a server certificate the intermediate signed, in the
# folder given. leaf.pem holds the server's certificate alone, and
# chain.pem the server's followed by the intermediate's, as an
# authority sends them; the root is in root.pem.
#
#   chain.sh FOLDER

set -e
folder="${1:?chain.sh FOLDER}"
mkdir -p "$folder"
cd "$folder"
ec="-newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes"
printf 'basicConstraints=critical,CA:TRUE,pathlen:0\nkeyUsage=critical,keyCertSign,cRLSign\n' > middle.ext
printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:added.example\n' > leaf.ext
openssl req -x509 $ec -days 30 -subj /CN=Test-Root -keyout root.key -out root.pem 2> /dev/null
openssl req $ec -subj /CN=Test-Intermediate -keyout middle.key -out middle.csr 2> /dev/null
openssl x509 -req -in middle.csr -CA root.pem -CAkey root.key -CAcreateserial -days 30 -extfile middle.ext -out middle.pem 2> /dev/null
openssl req $ec -subj /CN=added.example -keyout leaf.key -out leaf.csr 2> /dev/null
openssl x509 -req -in leaf.csr -CA middle.pem -CAkey middle.key -CAcreateserial -days 30 -extfile leaf.ext -out leaf.pem 2> /dev/null
cat leaf.pem middle.pem > chain.pem
echo "chain made"
