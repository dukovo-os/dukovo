#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of services/php-fpm: it asks the machine's nginx for
# the site's pages and prints what came, for the test to look for.
#
#   php-fpm-checks.sh front|info|missing MACHINE PORT
#
# `front` prints the front page, `info` the page of checks, asked with a
# POST field, and `missing` the status of a PHP page that is not there.
# MACHINE is the machine's address and PORT the lab PC's that reaches its
# port 80, as {machine} and {machine-port:80} give them.
set -eu

what="$1"
machine="$2"
port="$3"

case "$what" in
front) curl -s "http://$machine:$port/" ;;
info) curl -s -d a=1 "http://$machine:$port/info.php?n=1" ;;
missing) curl -s -o /dev/null -w 'missing %{http_code}\n' "http://$machine:$port/nope.php" ;;
*)
	echo "usage: php-fpm-checks.sh front|info|missing MACHINE PORT" >&2
	exit 2
	;;
esac
