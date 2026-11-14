#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of web-console/zap: ZAP's baseline scan, which only
# watches (it spiders the page it is given and runs the passive rules over
# every answer), of the web console signed out and of its API with an API
# token for every scope.
#
#   zap-run.sh RULES MACHINE SSHPORT CONSOLEPORT
#
# RULES is zap-rules.tsv; MACHINE the machine's address, SSHPORT the lab
# PC's port that reaches its SSH and CONSOLEPORT the port that reaches the
# console (9090), as {machine}, {machine-port:22} and {machine-port:9090}
# give them. It runs in the test's own folder.
#
# ZAP is pinned: one release of its image, named below by its version and
# the digest of the image index, which is what docker checks the pull
# against. The lab PC needs Docker; the first run pulls the image, about
# a gigabyte, and later runs use it. The scan runs with the host's network,
# so that it reaches the machine as the lab PC does.
#
# The token is made with `admin token add` (token.sh) and goes to ZAP in a
# replacer rule, which adds it to every request as `Authorization: Bearer`;
# it is a token for this run alone, taken away again, which is why it may
# show in this PC's process list.
#
# The baseline scan exits 0 when no rule warns or fails, 1 when one fails
# and 2 when one warns. A rule that warns on the console is fixed in the
# console or ignored in RULES with its reason, one line each. Exits with 77
# where the lab PC cannot do its part: no Docker, or no way to pull the
# image. That skips the test.
set -eu

IMAGE=ghcr.io/zaproxy/zaproxy:2.17.0@sha256:9624669db72f54d86c4b46a7064518bd9bf79d8c61a4c31befa9923489f328af

rules="$1"
machine="$2"
ssh_port="$3"
port="$4"
here=$(cd "$(dirname "$0")" && pwd)

for program in docker curl; do
	if ! command -v "$program" > /dev/null 2>&1; then
		echo "the lab PC has no $program, which ZAP's baseline scan takes"
		exit 77
	fi
done
if ! docker image inspect "$IMAGE" > /dev/null 2>&1; then
	if ! docker pull --quiet "$IMAGE" > /dev/null 2> "${TMPDIR:-/tmp}/zap-pull.err"; then
		echo "cannot pull $IMAGE: $(tr '\n' ' ' < "${TMPDIR:-/tmp}/zap-pull.err")"
		exit 77
	fi
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/lab-zap.XXXXXX")
token=""
finish() {
	if [ -n "$token" ]; then
		sh "$here/token.sh" remove lab-zap "$machine" "$ssh_port" > /dev/null 2>&1 || true
	fi
	rm -rf "$work"
}
trap finish EXIT
# ZAP runs as a user of its own in the container, and writes its report
# in the folder it is given.
chmod 777 "$work"
cp "$rules" "$work/zap-rules.tsv"

# One scan: its name, the page it starts from, and the options for ZAP
# itself, if any (-z). The report is kept for the failure's words.
scan() {
	name="$1"
	url="$2"
	shift 2
	status=0
	docker run --rm --network host -v "$work:/zap/wrk:rw" "$IMAGE" \
		zap-baseline.py -t "$url" -c zap-rules.tsv -J "$name.json" -m 1 "$@" \
		> "$work/$name.out" 2>&1 || status=$?
	if [ "$status" -ne 0 ]; then
		echo "FAILED: ZAP's baseline scan of $name ended with status $status (1: a rule failed, 2: a rule warned):"
		grep -E '^(WARN|FAIL)' "$work/$name.out" || tail -20 "$work/$name.out"
		exit 1
	fi
	echo "zap: $name: no rule warns or fails that zap-rules.tsv does not ignore"
}

origin="https://$machine:$port"
scan "the console signed out" "$origin/"

token=$(sh "$here/token.sh" make lab-zap "$machine" "$ssh_port")
case "$token" in
dkv_*) ;;
*)
	token=""
	echo "FAILED: admin token add printed no token"
	exit 1
	;;
esac
# ZAP splits what -z holds as a shell does, so the value that has a space in
# it is quoted.
header="-config replacer.full_list(0).description=token -config replacer.full_list(0).enabled=true -config replacer.full_list(0).matchtype=REQ_HEADER -config replacer.full_list(0).matchstr=Authorization -config replacer.full_list(0).regex=false -config 'replacer.full_list(0).replacement=Bearer $token'"
# The API has no page to spider from, so each of its reads is a start.
for route in actions services certificates; do
	scan "the API's $route with a token" "$origin/api/1/$route" -z "$header"
done
