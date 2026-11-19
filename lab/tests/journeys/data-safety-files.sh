#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# A person's files for journeys/data-safety, made, changed, restored and
# checked as the person, in their own home: `sh -s ACTION [ARGUMENT]` reads this
# file on its standard input over SSH.
#
#   make              docs/ holds a note, a long list of numbers, a table and two
#                     photos of known content, and .docs.sums their sums
#   change            the note and the photos are deleted, the numbers are
#                     added to and the table is written over
#   restore SNAPSHOT  each of those is copied back from the snapshot with
#                     `snap restore`, which never writes over a file, beside
#                     the one there, and then takes its place
#   restore-backup    as restore, from the newest snapshot that `backup
#                     restore` made, which is named restored-TIME
#   check             says how many files match their sums, and any that do not
#   big               a large file of the person's own, with its sum in .big.sum
#   bigcheck          says whether the large file matches its sum
set -eu

cd "$HOME"
name=$(id -un)

restore_from() {
	for part in notes.txt numbers.txt report.csv photos; do
		snap restore --from "$1" --to "$HOME/docs/$part.restored" "$HOME/docs/$part"
		rm -rf "docs/$part"
		mv "docs/$part.restored" "docs/$part"
	done
}

case "${1:-}" in
make)
	rm -rf docs .docs.sums
	mkdir -p docs/photos
	printf 'Notes of %s, first draft.\nBuy milk, back up the server.\n' "$name" > docs/notes.txt
	seq 1 100000 > docs/numbers.txt
	seq 1 5000 | sed "s/\$/,$name/" > docs/report.csv
	yes "$name-photo-a" | head -c 1048576 > docs/photos/a.bin
	yes "$name-photo-b" | head -c 1048576 > docs/photos/b.bin
	(cd docs && find . -type f | sort | xargs sha256sum > ../.docs.sums)
	wc -l < .docs.sums
	;;
change)
	rm docs/notes.txt
	rm -r docs/photos
	echo 100001 >> docs/numbers.txt
	echo "changed,$name" > docs/report.csv
	find docs -type f | sort
	;;
restore)
	restore_from "$2"
	;;
restore-backup)
	snapshot=$(snap list /users | grep -o 'restored-[0-9-]*' | tail -1)
	echo "restoring from $snapshot"
	restore_from "$snapshot"
	;;
check)
	cd docs
	sha256sum -c ../.docs.sums > ../.docs.check 2>&1 || true
	echo "matching: $(grep -c ': OK$' ../.docs.check)"
	grep -v ': OK$' ../.docs.check || true
	;;
big)
	head -c 300000000 /dev/urandom > docs/big.bin
	(cd docs && sha256sum big.bin > ../.big.sum)
	sync
	wc -c < docs/big.bin
	;;
bigcheck)
	(cd docs && sha256sum -c ../.big.sum)
	;;
*)
	echo "usage: sh -s make | change | restore SNAPSHOT | check | big | bigcheck" >&2
	exit 2
	;;
esac
