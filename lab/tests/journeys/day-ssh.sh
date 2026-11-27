#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's SSH session of journeys/day-in-the-life: a person signed in over
# SSH for a number of minutes, beside the two browsers, who each round runs
# commands, writes a file and reads it back, and uploads a file and downloads
# it again, every sum held to the one the lab PC made.
#
#   day-ssh.sh work FOLDER ACCOUNT MACHINE PORT
#   day-ssh.sh finish FOLDER
#   day-ssh.sh trend FOLDER [KIB]
#
# FOLDER is the journey's folder on the lab PC, {work}, which holds the key
# of ACCOUNT, as ssh-as.sh takes them. work goes round for the `minutes` that
# the file `info` in it gives, which the browsers read too, says "ssh session
# ready" after its first round, and keeps the result in ssh-result there when
# it ends, then waits to be ended with the test. finish waits for that
# result, up to 10 minutes, and says it: how many rounds, how many failures
# and what each failure was. trend says how far the memory in use, which each
# round read, grew in the second half of the rounds, and holds it to KIB (16384
# by default). MACHINE and PORT are {machine} and {machine-port:22}.
set -u

folder=${2:-.}
result=$folder/ssh-result

case "${1:-}" in
work)
	key_folder=$folder
	account=$3
	machine=$4
	port=$5
	minutes=$(sed -n 's/^minutes=//p' "$folder/info")
	here=$(cd "$(dirname "$0")" && pwd)
	options="-i $key_folder/$account -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR"
	rm -f "$result"
	head -c 3000000 /dev/urandom > "$folder/upload.bin"
	sent=$(sha256sum "$folder/upload.bin" | cut -d ' ' -f 1)
	end=$(($(date +%s) + minutes * 60))
	rounds=0
	failures=0
	: > "$folder/ssh-failures"
	: > "$folder/ssh-memory"
	fail() {
		failures=$((failures + 1))
		echo "failed in round $rounds: $1" | tee -a "$folder/ssh-failures"
	}
	while [ "$(date +%s)" -lt "$end" ]; do
		rounds=$((rounds + 1))
		# A session of commands, which the log keeps a line of.
		said=$(sh "$here/ssh-as.sh" "$key_folder" "$account" "$machine" "$port" "logger journey-ssh-round-$rounds; uptime; ls ~ | wc -l; ps | wc -l; df -k /users | tail -1; snap list /users | tail -1; seq 1 20000 | sort -rn | head -1") || fail "a session of commands ended with an error"
		[ "$(printf '%s\n' "$said" | tail -n 1)" = 20000 ] || fail "its sort of 20000 numbers ended with $(printf '%s\n' "$said" | tail -n 1)"
		# A file written and read back by the account.
		back=$(sh "$here/ssh-as.sh" "$key_folder" "$account" "$machine" "$port" "echo round-$rounds > ~/round.txt; cat ~/round.txt") || fail "writing a file ended with an error"
		[ "$back" = "round-$rounds" ] || fail "a file it wrote read back as $back"
		# An upload with its sum, and the same file downloaded again.
		# shellcheck disable=SC2086
		scp -q -P "$port" $options "$folder/upload.bin" "$account@$machine:upload-$rounds.bin" || fail "scp could not upload"
		there=$(sh "$here/ssh-as.sh" "$key_folder" "$account" "$machine" "$port" "sha256sum upload-$rounds.bin | cut -d ' ' -f 1") || fail "the sum of an uploaded file could not be taken"
		[ "$there" = "$sent" ] || fail "an uploaded file has the sum $there, not $sent"
		# shellcheck disable=SC2086
		scp -q -P "$port" $options "$account@$machine:upload-$rounds.bin" "$folder/download.bin" || fail "scp could not download"
		got=$(sha256sum "$folder/download.bin" | cut -d ' ' -f 1)
		[ "$got" = "$sent" ] || fail "a downloaded file has the sum $got, not $sent"
		sh "$here/ssh-as.sh" "$key_folder" "$account" "$machine" "$port" "rm -f upload-$rounds.bin" || fail "a file could not be removed"
		# The memory in use, as the machine says it, kept for each round so that a day's growth can be told from a plateau.
		mem=$(sh "$here/ssh-as.sh" "$key_folder" "$account" "$machine" "$port" "awk '/^total:/ { t = \$2 } /^free:/ { f = \$2 } /^page_cache:/ { c = \$2 } END { print t - f - c }' /processes/meminfo") || fail "the memory in use could not be read"
		echo "$rounds $mem" >> "$folder/ssh-memory"
		echo "round $rounds done"
		[ "$rounds" -ne 1 ] || echo "ssh session ready"
		sleep 15
	done
	echo "rounds: $rounds, failures: $failures" > "$result"
	cat "$result"
	# Ended with the test, as a server is.
	while true; do sleep 60; done
	;;
finish)
	i=0
	while [ "$i" -lt 600 ] && [ ! -s "$result" ]; do
		i=$((i + 1))
		sleep 1
	done
	if [ ! -s "$result" ]; then
		echo "the SSH session did not end in 10 minutes"
		exit 1
	fi
	cat "$result"
	cat "$folder/ssh-failures"
	;;
trend)
	# The memory in use at the start, in the middle and at the end of the day:
	# a day's use fills caches in its first minutes, and what matters is whether
	# the second half goes on growing.
	awk -v limit="${3:-16384}" '{ kib[NR] = $2 } END {
		if (NR < 4) { print "memory: too few rounds to tell"; exit }
		mid = int(NR / 2)
		growth = kib[NR] - kib[mid]
		verdict = growth <= limit ? "kept" : "too much"
		printf "memory in use: %d KiB after round 1, %d KiB at round %d, %d KiB at round %d; grew %d KiB in the second half (at most %d): %s\n", kib[1], kib[mid], mid, kib[NR], NR, growth, limit, verdict
	}' "$folder/ssh-memory"
	;;
*)
	echo "usage: day-ssh.sh work FOLDER ACCOUNT MACHINE PORT | finish FOLDER | trend FOLDER [KIB]" >&2
	exit 2
	;;
esac
