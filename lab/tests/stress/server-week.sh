#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of stress/server-week: the files it keeps, the days it
# watches the machine for, the check after a fault and the verdict.
#
#   server-week.sh prepare SITE ACCOUNT PASSWORD
#   server-week.sh page MACHINE HTTP-PORT
#   server-week.sh day N MINUTES MACHINE SSH-PORT
#   server-week.sh back MACHINE SSH-PORT HTTP-PORT
#   server-week.sh verdict
#   server-week.sh finish
#
# MACHINE is the machine's address and the ports the lab PC's that reach its
# ports 22 and 80, as {machine} and {machine-port:N} give them. Everything
# the test keeps on the lab PC is in /tmp/lab-server-week, which prepare
# makes and finish takes away: info (what prepare was told), acked.txt and
# minutes.csv (server-week-load.py's, whose header says what they hold),
# and for each day N, day-N.start and day-N.end (seconds since 1970) and
# day-N.checks.
#
# prepare makes the folder. page waits up to 120 seconds for the site's page
# to answer and says `page answers`.
#
# day watches a day of MINUTES minutes, 1440 for a real one: 24 times,
# evenly spaced, it has the machine check itself (server-week-machine.sh
# check) and says what it found. The day fails at the first check where a
# service is not running, the items are not as setup made them, the machine
# does not answer in 3 tries a minute apart, or, once two hours of the day
# have gone, the last backup is over two hours old. At its end the memory in use is held as `ob soak`
# holds a soak (tools/xtask/src/test/soak.rs): the mean of the checks of the
# last quarter may be at most the mean of the second quarter, plus the
# larger of a twentieth of it and 8 MiB; the processes may be at most 2
# more; and more than 2 SSH or console sign-ins of the load in a row that
# failed fail the day. It says `day N held` or why not.
#
# back follows a fault: it waits up to 120 seconds for the page to answer
# and then reads the writes PostgreSQL holds. Every write the load had
# acknowledged before it began must be among them. It says `back in S s, N
# acknowledged writes all kept`, or which are missing.
#
# verdict adds up the requests of day 1 and day 7 and holds the 99th
# percentile of day 7's latencies to 120% of day 1's, and says `week held`.
# The latencies are the page's own time on the machine, which the load takes
# from the X-Took-Us header: the lab PC may run other machines' tests during
# the week, so its load must not move the verdict. The round trip the lab PC
# measured is kept beside it and shown, never held to anything.
# shellcheck disable=SC2154 # site, account and password come from the info file
set -eu

dir=/tmp/lab-server-week
key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"
password_file="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_password"
# The first two hours of a day have no backup to be held to, and after that
# the last one may be this old, since the machine makes one an hour.
backup_grace=7200
backup_limit=7200

# Runs a command on the machine as the lab account.
on_machine() {
	ssh -p "$port" -i "$key" -o IdentitiesOnly=yes -o BatchMode=yes \
		-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR \
		-o ConnectTimeout=30 -o ServerAliveInterval=30 "lab@$machine" "$@"
}

# Runs the machine script as root: elevate takes the lab password from its
# standard input, as the lab's own root steps give it.
as_root() {
	on_machine "elevate -S sh /users/lab/server-week-machine.sh $*" <"$password_file"
}

# Asks the site for item 1 and prints the status curl got, 000 for none.
ask_page() {
	curl -s -o /dev/null -w '%{http_code}' --max-time 10 -H "Host: $site" \
		"http://$machine:$http_port/server-week.php?op=read&key=1" || true
}

wait_for_page() {
	began=$(date +%s)
	while [ "$(ask_page)" != 200 ]; do
		if [ $(($(date +%s) - began)) -ge 120 ]; then
			return 1
		fi
		sleep 2
	done
}

prepare() {
	rm -rf "$dir"
	mkdir "$dir"
	: >"$dir/acked.txt"
	: >"$dir/minutes.csv"
	printf 'site=%s\naccount=%s\npassword=%s\n' "$1" "$2" "$3" >"$dir/info"
	echo "prepared"
}

# Prints the means of the second and the last quarter of the numbers in
# column $2 of file $1, as the soak takes them, in two words.
quarters() {
	awk -v column="$2" '
		{ value[NR] = $column }
		END {
			first = int(NR / 4) + 1; middle = int(NR / 2); last = NR - int(NR / 4) + 1
			for (i = first; i <= middle; i++) { warm += value[i]; warm_count++ }
			for (i = last; i <= NR; i++) { late += value[i]; late_count++ }
			printf "%d %d\n", int(warm / warm_count), int(late / late_count)
		}' "$1"
}

# Holds a day's checks and the load's sign-ins to what a day may do.
judge_day() {
	n=$1
	start=$2
	end=$3
	# shellcheck disable=SC2046 # the answer is numbers in words
	set -- $(quarters "$dir/day-$n.checks" 2)
	warm=$1
	late=$2
	slack=$((warm / 20))
	[ "$slack" -ge 8192 ] || slack=8192
	if [ "$late" -gt $((warm + slack)) ]; then
		echo "day $n failed: the memory in use crept from $warm KiB to $late KiB, more than $slack KiB"
		return 1
	fi
	# shellcheck disable=SC2046 # the answer is numbers in words
	set -- $(quarters "$dir/day-$n.checks" 3)
	if [ "$2" -gt $(($1 + 2)) ]; then
		echo "day $n failed: the processes grew from $1 to $2, more than 2"
		return 1
	fi
	awk -F, -v from="$start" -v to="$end" -v day="$n" '
		$1 >= from && $1 < to {
			requests += $2; errors += $3; acked += $4
			if ($5 > 0) { ssh_run = ($6 > 0) ? ssh_run + 1 : 0 }
			if ($7 > 0) { console_run = ($8 > 0) ? console_run + 1 : 0 }
			if (ssh_run > 2) { bad = "SSH" }
			if (console_run > 2) { bad = "console" }
		}
		END {
			if (bad != "") {
				print "day " day " failed: more than 2 " bad " sign-ins in a row failed"
				exit 1
			}
			print "day " day ": " requests + 0 " requests, " errors + 0 " errors, " acked + 0 " writes acknowledged"
		}' "$dir/minutes.csv"
}

# Has the machine check itself and leaves its answer in $answer. A lab PC
# busy with other machines' tests may drop an SSH connection, so a check
# is tried 3 times a minute apart; a machine that is down stays down for
# all three.
ask_check() {
	tries=1
	until answer=$(as_root check 2>&1) && printf '%s\n' "$answer" | grep -q '^check '; do
		if [ "$tries" -ge 3 ]; then
			return 1
		fi
		tries=$((tries + 1))
		sleep 60
	done
}

# The value of the word NAME=VALUE named $1 in the check's line, $result.
word() {
	printf '%s\n' "$result" | tr ' ' '\n' | sed -n "s/^$1=//p"
}

day() {
	n=$1
	minutes=$2
	start=$(date +%s)
	echo "$start" >"$dir/day-$n.start"
	: >"$dir/day-$n.checks"
	length=$((minutes * 60))
	i=1
	while [ "$i" -le 24 ]; do
		due=$((start + i * length / 24))
		now=$(date +%s)
		[ "$due" -le "$now" ] || sleep $((due - now))
		if ! ask_check; then
			echo "day $n failed: check $i got no answer in 3 tries a minute apart, the last: $answer"
			exit 1
		fi
		result=$(printf '%s\n' "$answer" | grep '^check ' | tail -n 1)
		case $(word services) in
		up) ;;
		*)
			echo "day $n failed: check $i found $(word services)"
			exit 1
			;;
		esac
		if [ "$(word items)" != same ]; then
			echo "day $n failed: check $i found the items changed"
			exit 1
		fi
		age=$(word backup_age)
		if [ $(($(date +%s) - start)) -ge "$backup_grace" ] && { [ "$age" -lt 0 ] || [ "$age" -gt "$backup_limit" ]; }; then
			echo "day $n failed: check $i found the last backup $age s old"
			exit 1
		fi
		echo "$i $(word memory) $(word processes) $(word files) $(word disk)" >>"$dir/day-$n.checks"
		echo "day $n check $i of 24: memory $(word memory) KiB, processes $(word processes), files open $(word files), disk used $(word disk) KiB, writes kept $(word rows), last backup $age s ago"
		i=$((i + 1))
	done
	end=$(date +%s)
	echo "$end" >"$dir/day-$n.end"
	judge_day "$n" "$start" "$end" || exit 1
	echo "day $n held"
}

back() {
	known=$(wc -l <"$dir/acked.txt" | tr -d ' ')
	head -n "$known" "$dir/acked.txt" | LC_ALL=C sort >"$dir/acked.sorted"
	began=$(date +%s)
	if ! wait_for_page; then
		echo "the page did not answer within 120 s"
		exit 1
	fi
	seconds=$(($(date +%s) - began))
	# PostgreSQL may take a moment longer than a page Valkey answers.
	tries=0
	until as_root dump 2>/dev/null | grep '^[0-9][0-9]*$' | LC_ALL=C sort >"$dir/kept.sorted" &&
		[ -s "$dir/kept.sorted" ] || [ "$known" -eq 0 ]; do
		tries=$((tries + 1))
		if [ "$tries" -ge 24 ]; then
			echo "the machine's writes could not be read"
			exit 1
		fi
		sleep 5
	done
	LC_ALL=C comm -23 "$dir/acked.sorted" "$dir/kept.sorted" | sort -n >"$dir/missing.txt"
	lost=$(wc -l <"$dir/missing.txt" | tr -d ' ')
	if [ "$lost" -eq 0 ]; then
		echo "back in $seconds s, $known acknowledged writes all kept"
		return 0
	fi
	echo "back in $seconds s, $lost of $known acknowledged writes missing, the first: $(head -n 10 "$dir/missing.txt" | tr '\n' ' ')"
	exit 1
}

# Prints the requests, the errors, the 50th and the 99th percentile of the
# latencies, in milliseconds, of the minutes of day $1, from the buckets of
# every minute added up: those in column 9 of minutes.csv, the page's own
# time, or with $2 as 10 the round trip.
percentiles() {
	awk -F, -v column="$2" -v from="$(cat "$dir/day-$1.start")" -v to="$(cat "$dir/day-$1.end")" '
		$1 >= from && $1 < to {
			requests += $2; errors += $3
			count = split($column, pairs, " ")
			for (i = 1; i <= count; i++) {
				split(pairs[i], pair, "=")
				bucket[pair[1]] += pair[2]
				if (pair[1] + 0 > top) { top = pair[1] + 0 }
				answered += pair[2]
			}
		}
		# The upper edge of a bucket: the first is up to 0.5 ms and each
		# is 5% wider than the one before.
		function edge(number) { return 0.5 * exp(number * log(1.05)) }
		function percentile(share,    wanted, seen, i) {
			wanted = int(answered * share); if (wanted < answered * share) { wanted++ }
			for (i = 0; i <= top; i++) {
				seen += bucket[i]
				if (seen >= wanted) { return edge(i) }
			}
			return 0
		}
		END { printf "%d %d %.2f %.2f\n", requests, errors, percentile(0.5), percentile(0.99) }' "$dir/minutes.csv"
}

verdict() {
	for n in 1 7; do
		if [ ! -s "$dir/day-$n.end" ]; then
			echo "week failed: day $n was not watched"
			exit 1
		fi
	done
	# shellcheck disable=SC2046 # the answer is numbers in words
	set -- $(percentiles 1 9)
	echo "day 1: $1 requests, $2 errors, the page took a median of $3 ms and 99th percentile $4 ms"
	first=$4
	# shellcheck disable=SC2046
	set -- $(percentiles 1 10)
	echo "day 1: round trip on the lab PC, median $3 ms, 99th percentile $4 ms (not held to anything)"
	# shellcheck disable=SC2046
	set -- $(percentiles 7 9)
	echo "day 7: $1 requests, $2 errors, the page took a median of $3 ms and 99th percentile $4 ms"
	last=$4
	# shellcheck disable=SC2046
	set -- $(percentiles 7 10)
	echo "day 7: round trip on the lab PC, median $3 ms, 99th percentile $4 ms (not held to anything)"
	if ! awk -v first="$first" -v last="$last" 'BEGIN { exit !(first > 0 && last <= first * 1.2) }'; then
		echo "week failed: day 7's 99th percentile, $last ms, is over 120% of day 1's, $first ms"
		exit 1
	fi
	echo "week held"
}

command=${1:-}
case "$command" in
prepare) prepare "${2:?site}" "${3:?account}" "${4:?password}" ;;
finish)
	rm -rf "$dir"
	echo "gone"
	exit 0
	;;
page | day | back | verdict) ;;
*)
	echo "say prepare, page, day, back, verdict or finish" >&2
	exit 2
	;;
esac
[ "$command" = prepare ] && exit 0

# shellcheck disable=SC1091 # written by prepare
. "$dir/info"
case "$command" in
page)
	machine=${2:?machine}
	http_port=${3:?port}
	if wait_for_page; then
		echo "page answers"
	else
		echo "the page did not answer within 120 s"
		exit 1
	fi
	;;
day)
	machine=${4:?machine}
	port=${5:?port}
	day "${2:?day}" "${3:?minutes}"
	;;
back)
	machine=${2:?machine}
	port=${3:?port}
	http_port=${4:?port}
	back
	;;
verdict) verdict ;;
esac
