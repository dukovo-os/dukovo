#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The machine's side of services/php: PHP's test suite on the copy in
# /tmp/php-tests, the tests /tmp/php-list names, with a worker for each
# processor, its output in /tmp/php-suite.log. Every few minutes it says
# how far the suite has got, so that the lab sees a suite at work, not a
# silent step. run-tests.php adds to its log as each test ends, and ends
# a test that is silent for a minute, so a log that stays the same for
# STILL_MAX seconds is a stuck suite. The script then names the tests in
# flight, prints the log's last lines and the lab account's processes
# with what each thread waits for (an orphan has PPID 0), stops the suite
# and says "suite stuck"; it does the same for a suite that runs for
# RUN_MAX seconds. A suite that ends prints the names of the tests that
# failed, the start of each one's difference from what it expects, which
# run-tests.php writes beside the test, and its count of failures last.
#
#   php-suite.sh

php=/applications/net.php.php/bin/php
log=/tmp/php-suite.log
started=/tmp/php-suite.started
ended=/tmp/php-suite.ended
# Past run-tests.php's minute for a silent test, and under the lab's ten
# minutes for a silent step.
STILL_MAX=480
# Under the test's two hours, leaving time to stop and clean up.
RUN_MAX=6300
SAY_EVERY=240

# The tests in flight. run-tests.php writes a test's NAME.php beside its
# NAME.phpt as it starts (NAME.skip.php and NAME.clean.php for those
# parts), and removes them when it passes; a test that failed keeps them,
# with its NAME.diff.
in_flight() {
	find /tmp/php-tests -newer "$started" -name '*.php' | while read -r file; do
		test=${file%.php}
		test=${test%.skip}
		test=${test%.clean}
		if [ -e "$test.phpt" ] && [ ! -e "$test.diff" ]; then
			echo "${test#/tmp/php-tests/}.phpt"
		fi
	done | sort -u
}

# Where the suite is: its last progress line, "TEST N/TOTAL [...]".
progress() {
	tr '\r' '\n' < "$log" | grep '^TEST [0-9]' | tail -n 1
}

# What a stuck suite is doing, then the suite stopped: every process of
# the lab account that runs PHP's program, orphans of its tests among
# them. The brackets keep grep from finding its own command line.
stuck() {
	echo "php suite: $1"
	echo "--- tests in flight"
	in_flight
	echo "--- the log's last lines"
	tr '\r' '\n' < "$log" | grep -v '^ *$' | tail -n 15
	echo "--- the lab account's processes"
	ps -L -u lab -o pid,lwp,ppid,stat,psr,time,wchan,args
	pids=$(ps -u lab -o pid=,args= | grep '[n]et.php.php/bin/php' | awk '{ print $1 }')
	if [ -n "$pids" ]; then
		kill -9 $pids
	fi
	echo "suite stuck"
}

rm -f "$ended"
touch "$started"
cd /tmp/php-tests || exit 1
# The marker, rather than kill -0, says the suite ended: kill -0 finds a
# child that ended but that the shell has not waited for yet.
(
	TEST_PHP_EXECUTABLE=$php "$php" run-tests.php -j"$(nproc)" -q --offline -r /tmp/php-list \
		> "$log" 2>&1
	touch "$ended"
) &
size=0
still=0
ran=0
said=0
while [ ! -e "$ended" ]; do
	sleep 10
	ran=$((ran + 10))
	now=$(wc -c < "$log")
	if [ "$now" -ne "$size" ]; then
		size=$now
		still=0
	else
		still=$((still + 10))
	fi
	if [ "$still" -ge "$STILL_MAX" ]; then
		stuck "stuck: its log has not grown for $((still / 60)) minutes"
		exit 1
	fi
	if [ "$ran" -ge "$RUN_MAX" ]; then
		stuck "still running after $((ran / 60)) minutes"
		exit 1
	fi
	if [ $((ran - said)) -ge "$SAY_EVERY" ]; then
		said=$ran
		echo "php suite, $((ran / 60)) minutes: $(progress)"
	fi
done
wait
sed -n '/^FAILED TEST SUMMARY/,/^=====/p' "$log"
sed -n '/^FAILED TEST SUMMARY/,/^=====/s/.*\[\(.*\)\.phpt\].*/\1/p' "$log" | while read -r test; do
	echo "--- $test.diff"
	head -n 30 "$test.diff"
done
grep '^Tests failed' "$log"
