#!/system/commands/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# Moves the record of a certificate that certd asks for again, for the boot
# test admin-certificates, which cannot wait the hour between two tries or the
# day before an alert. Run it as root.
#
#   wanted.sh NAME due           the next try is long past
#   wanted.sh NAME aged HOURS    the first try was HOURS hours ago, and the
#                                next is long past
#   wanted.sh NAME junk          the record is not a record

set -e
name="${1:?wanted.sh NAME due|aged|junk}"
file="/system/state/certd/wanted/$name.toml"
case "${2:?wanted.sh NAME due|aged|junk}" in
due)
	sed -i 's/^next_ms = .*/next_ms = 1000/' "$file"
	;;
aged)
	first=$(($(date +%s) - ${3:?wanted.sh NAME aged HOURS} * 3600))
	sed -i "s/^first_ms = .*/first_ms = ${first}000/; s/^next_ms = .*/next_ms = 1000/" "$file"
	;;
junk)
	echo 'not = = toml' > "$file"
	;;
*)
	echo "wanted.sh: due, aged or junk" >&2
	exit 2
	;;
esac
