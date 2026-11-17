#!/system/commands/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# The certificates of the boot test admin-certificates, made as the people
# who use them make theirs, in the folder given: a root authority the
# machine does not trust, an intermediate it signed, and two server
# certificates the intermediate signed, each with its key.
#
#   lab.pem and lab.key      the certificate for lab.example.org
#   renamed.pem and renamed.key
#                            the certificate for lab-renamed.example.org and
#                            localhost, which is what curl asks the console
#                            for
#   middle.pem               the intermediate's, which follows a server's
#                            certificate in a chain
#   other.key                a key that is neither server's
#
#   chain.sh FOLDER

set -e
folder="${1:?chain.sh FOLDER}"
mkdir -p "$folder"
cd "$folder"
ec="-newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes"
printf 'basicConstraints=critical,CA:TRUE,pathlen:0\nkeyUsage=critical,keyCertSign,cRLSign\n' > middle.ext
printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:lab.example.org\n' > lab.ext
printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:lab-renamed.example.org,DNS:localhost\n' > renamed.ext
openssl req -x509 $ec -days 30 -subj /CN=Test-Root -keyout root.key -out root.pem 2> /dev/null
openssl req $ec -subj /CN=Test-Intermediate -keyout middle.key -out middle.csr 2> /dev/null
openssl x509 -req -in middle.csr -CA root.pem -CAkey root.key -CAcreateserial -days 30 -extfile middle.ext -out middle.pem 2> /dev/null
for name in lab renamed; do
	openssl req $ec -subj /CN=$name -keyout $name.key -out $name.csr 2> /dev/null
	openssl x509 -req -in $name.csr -CA middle.pem -CAkey middle.key -CAcreateserial -days 30 -extfile $name.ext -out $name.pem 2> /dev/null
done
openssl req $ec -subj /CN=other -keyout other.key -out other.csr 2> /dev/null
echo "chain made"
