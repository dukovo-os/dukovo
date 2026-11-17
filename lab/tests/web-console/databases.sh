#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The machine's side of web-console/databases: the databases that the test
# uses, made and taken away with PostgreSQL's own tools, and a session held
# open on one of them for the page to end.
#
#   sh databases.sh reset   no database of the test is left, nor a session
#   sh databases.sh busy    lab_busy is made and a session is held open on it
#   sh databases.sh gone    says "gone" when no database of the test is left
#
# It runs as root, which PostgreSQL's service knows as its administrator.
set -eu

bin=/applications/org.postgresql.postgresql/bin
sql() {
	"$bin/psql" -U postgres -d postgres -X -A -t -c "$1"
}

case "${1:-}" in
reset)
	# A run before may have left either, and the session on one: forcing the
	# removal ends it.
	for name in shop_lab lab_busy; do
		database remove "$name" --role --force > /dev/null 2>&1 || true
	done
	echo reset
	;;
busy)
	database add lab_busy --json > /dev/null
	nohup "$bin/psql" -U postgres -d lab_busy -X -c 'SELECT pg_sleep(1500)' > /dev/null 2>&1 &
	i=0
	until [ "$(sql "SELECT count(*) FROM pg_stat_activity WHERE datname = 'lab_busy' AND state = 'active'")" = 1 ]; do
		i=$((i + 1))
		if [ "$i" -gt 30 ]; then
			echo "no session on lab_busy"
			exit 1
		fi
		sleep 1
	done
	echo "holding lab_busy"
	;;
gone)
	if [ "$(sql "SELECT count(*) FROM pg_database WHERE datname IN ('shop_lab', 'lab_busy')")" = 0 ] \
		&& [ "$(sql "SELECT count(*) FROM pg_roles WHERE rolname IN ('shop_lab', 'lab_busy')")" = 0 ]; then
		echo gone
	else
		echo "a database or a role of the test is left"
		exit 1
	fi
	;;
*)
	echo "usage: databases.sh reset|busy|gone"
	exit 2
	;;
esac
