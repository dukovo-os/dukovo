#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of web-console/testssl: testssl.sh, pinned to one
# release, against the web console's port, and the check of what it found.
#
#   testssl-run.sh ACCEPTED MACHINE CONSOLEPORT
#
# ACCEPTED is testssl-accepted.txt; MACHINE the machine's address and
# CONSOLEPORT the lab PC's port that reaches the console (9090), as
# {machine} and {machine-port:9090} give them. It runs in the test's own
# folder, and needs git, bash, openssl and python3.
#
# testssl.sh is pinned by the tag of its release, and by the commit that
# tag names: the script clones that tag, and refuses a clone whose commit
# is another (a tag can be moved). The clone is kept in
# ${XDG_CACHE_HOME:-$HOME/.cache}/dukovo-lab/testssl-VERSION, or the folder
# DUKOVO_LAB_TESTSSL names, and nothing is installed.
#
# Exits with 77 where the lab PC cannot do its part: no git, bash,
# openssl or python3, or no way to clone the release. That skips the test.
set -eu

VERSION=v3.2.5
COMMIT=4f17be931a9d60f3f987975aea8221f1a8eaf7aa
URL=https://github.com/testssl/testssl.sh.git

accepted="$1"
machine="$2"
port="$3"
here=$(cd "$(dirname "$0")" && pwd)

for program in git bash openssl python3; do
	if ! command -v "$program" > /dev/null 2>&1; then
		echo "the lab PC has no $program, which testssl.sh $VERSION takes"
		exit 77
	fi
done

build=""
work=""
cleanup() {
	[ -z "$build" ] || rm -rf "$build"
	[ -z "$work" ] || rm -rf "$work"
}
trap cleanup EXIT

kit="${DUKOVO_LAB_TESTSSL:-${XDG_CACHE_HOME:-$HOME/.cache}/dukovo-lab/testssl-$VERSION}"
if [ ! -x "$kit/testssl.sh" ] || [ "$(cat "$kit/commit" 2> /dev/null)" != "$COMMIT" ]; then
	build=$(mktemp -d "${TMPDIR:-/tmp}/lab-testssl.XXXXXX")
	if ! git clone --quiet --depth 1 --branch "$VERSION" "$URL" "$build/src" 2> "$build/git.err"; then
		echo "cannot clone testssl.sh $VERSION from $URL: $(tr '\n' ' ' < "$build/git.err")"
		exit 77
	fi
	got=$(git -C "$build/src" rev-parse HEAD)
	if [ "$got" != "$COMMIT" ]; then
		echo "testssl.sh $VERSION is commit $got, not the $COMMIT this script pins: not used"
		exit 1
	fi
	rm -rf "$kit"
	mkdir -p "$(dirname "$kit")"
	mv "$build/src" "$kit"
	echo "$COMMIT" > "$kit/commit"
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/lab-testssl-run.XXXXXX")

# The protocols, the server's defaults (the certificate), the ciphers of
# each protocol and the lists of weak ones, forward secrecy, and the known
# weaknesses. Nothing asks a question, and nothing but a connection to the
# port is made.
status=0
"$kit/testssl.sh" --quiet --color 0 --warnings off \
	--connect-timeout 20 --openssl-timeout 20 \
	--jsonfile "$work/findings.json" \
	--protocols --server-defaults --cipher-per-proto --std --fs --vulnerable \
	"$machine:$port" > "$work/testssl.out" 2>&1 || status=$?
if [ ! -s "$work/findings.json" ]; then
	echo "FAILED: testssl.sh ended with status $status and wrote no findings:"
	tail -20 "$work/testssl.out"
	exit 1
fi
python3 -I "$here/testssl-check.py" "$work/findings.json" "$accepted"
