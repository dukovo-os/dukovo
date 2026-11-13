#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# SQLite's own test suite on Radix (ADR 0511): the machine's side of the
# lab's services/sqlite-suite. It unpacks the sqlite port's test archive,
# which holds testfixture (SQLite's test program, built with the same
# options as the library) and the test scripts, runs SQLite's veryquick
# suite with a process for each processor, and compares the test files that
# failed with sqlite-skips.txt, the ones that fail for a gap of Radix's
# that is in docs/deferred.md. It prints
#
#   sqlite-suite: F test files, N tests: every file passed but the S skipped
#
# when no file failed that is not on the list, and exits 0; else the files
# that failed without being on the list, and exits 1. A file on the list
# that passed, or that the run did not have, is named too, so that the list
# cannot go stale. The list's lines that start with "~" are files the suite
# does not run, as they would never end. A test file that runs for longer
# than a quarter of an hour is ended all the same, and counts as failed:
# the test runner has no limit of its own.
#
#   sqlite-suite.sh
#
# The archive and the list are in /users/lab, where the test's host steps
# copied them.
set -u

home=/users/lab
suite="$home/sqlite-suite"

rm -rf "$suite"
mkdir "$suite" && tar -xzf "$home/sqlite-tests.tar.gz" -C "$suite" || {
	echo "sqlite-suite: the test archive does not unpack"
	exit 1
}
cd "$suite/sqlite-tests" || exit 1
# Tcl's script library, which testfixture does not carry, is a folder here.
TCL_LIBRARY="$PWD/tcl9.1"
export TCL_LIBRARY

# Two lines of the test runner are its own to change for Radix, and no test
# is. It picks its commands by the operating system Tcl names, from a list
# that has no "Dukovo": the case of the BSDs, which run their scripts with
# sh, takes it in. It also keeps its job list in a database it opens in WAL
# mode, which only its own process opens. Radix maps no file shared but
# those in /dev/shm, so the WAL index of a database between connections is
# not there yet (docs/deferred.md): exclusive locking mode, set before WAL,
# keeps the index in memory, which is all one connection needs.
sed -e 's/\*openbsd\* {/*openbsd* - *dukovo* {/' \
	-e 's/PRAGMA journal_mode=WAL;/PRAGMA locking_mode=EXCLUSIVE; PRAGMA journal_mode=WAL;/' \
	test/testrunner.tcl >test/testrunner-radix.tcl
if ! grep -q '\*dukovo\*' test/testrunner-radix.tcl ||
	! grep -q 'locking_mode=EXCLUSIVE' test/testrunner-radix.tcl; then
	echo "sqlite-suite: the test runner's lines to change for Radix are not where they were"
	exit 1
fi

# The watchdog ends the processes of test files that have run too long: the
# test program started by the runner by its full name, as the runner itself
# is started by a relative one.
limit=900
(
	while sleep 60; do
		ps -eo pid,etime,args | awk -v exe="$PWD/testfixture" -v limit="$limit" '
			function seconds(t,   a, b, n, i, s, days) {
				days = 0
				if (index(t, "-")) {
					split(t, a, "-")
					days = a[1]
					t = a[2]
				}
				n = split(t, b, ":")
				s = 0
				for (i = 1; i <= n; i++)
					s = s * 60 + b[i]
				return s + days * 86400
			}
			$3 == exe && seconds($2) > limit { print $1, $4 }
		' | while read -r pid file; do
			echo "$file" >>"$suite/ended"
			kill "$pid"
		done
	done
) >/dev/null 2>&1 &
watchdog=$!

# The files not to run, as the runner's anti-patterns: "~test/NAME.test$"
# matches that file's name and no other.
not_run=$(sed -n 's/^\(~.*\)$/\1$/p' "$home/sqlite-skips.txt")
processors=$(nproc)
# Unquoted: one pattern a word.
./testfixture test/testrunner-radix.tcl --jobs "${processors:-2}" veryquick $not_run \
	>"$suite/output.txt" 2>&1
kill "$watchdog" 2>/dev/null

counts=$(sed -n 's/^[0-9]* errors out of \([0-9]*\) tests.*/\1/p' "$suite/output.txt")
if [ -z "$counts" ]; then
	echo "sqlite-suite: the run did not finish; its output ends:"
	tail -n 30 "$suite/output.txt"
	exit 1
fi

# Each file the runner ran is a line of its log: "### NAME TIMEms (done)" or
# "(failed)".
log=testrunner.log
sed -n 's/^### \(.*\) [0-9]*ms (failed)$/\1/p' "$log" | sort >"$suite/failed"
sed -n 's/^### \(.*\) [0-9]*ms (done)$/\1/p' "$log" | sort >"$suite/passed"
sed -e '/^#/d' -e '/^~/d' -e '/^$/d' "$home/sqlite-skips.txt" | sort >"$suite/skips"
not_run_count=$(grep -c '^~' "$home/sqlite-skips.txt")

unexpected=$(comm -23 "$suite/failed" "$suite/skips" | tr '\n' ' ')
passes=$(comm -12 "$suite/passed" "$suite/skips" | tr '\n' ' ')
missing=$(comm -23 "$suite/skips" "$suite/failed" | comm -23 - "$suite/passed" | tr '\n' ' ')
skipped=$(comm -12 "$suite/failed" "$suite/skips" | wc -l | tr -d ' ')
files=$(cat "$suite/failed" "$suite/passed" | wc -l | tr -d ' ')

status=0
if [ -n "$passes" ]; then
	echo "sqlite-suite: passes now, remove from sqlite-skips.txt: $passes"
	status=1
fi
if [ -n "$missing" ]; then
	echo "sqlite-suite: not in the run, remove from sqlite-skips.txt: $missing"
	status=1
fi
if [ -f "$suite/ended" ]; then
	echo "sqlite-suite: ended after $limit seconds: $(tr '\n' ' ' <"$suite/ended")"
fi
if [ -n "$unexpected" ]; then
	echo "sqlite-suite: unexpected failures: $unexpected"
	echo "sqlite-suite: the runner's output ends:"
	tail -n 40 "$suite/output.txt"
	status=1
else
	echo "sqlite-suite: $files test files, $counts tests: every file passed but the $skipped skipped; $not_run_count not run"
fi
exit "$status"
