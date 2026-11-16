#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The machine's side of web-console/websites: the sites the test leaves, taken
# away with the command that makes them, and the hand edits that the page must
# show and keep, which only a person at the machine makes.
#
#   sh websites.sh reset    no website is left, nor its folder
#   sh websites.sh edit     a line is added by hand to the first site's file
#   sh websites.sh break    a line nginx does not know is added to that file
#   sh websites.sh repair   the line that was added by break is taken out
#   sh websites.sh state    says how many sites there are, and whether the folders
#                           of the sites the test removed are left
#   sh websites.sh gone     says "gone" when no website or folder is left
#
# The sites' files are /system/settings/websites/NAME.conf. The PHP site of the
# test is the one that is left out by edit, break and repair: the first site is
# the one made with this machine's own certificate, whose name the script took
# from the machine and this file does not know. It runs as root.
set -eu

files=/system/settings/websites
folders=/shared/www/sites
php=php.acme-lab.example

# The file of the site the hand edits are for: the first that is not the PHP site's.
first() {
	for file in "$files"/*.conf; do
		[ -e "$file" ] || continue
		case "$file" in
		"$files/$php.conf") ;;
		*)
			echo "$file"
			return 0
			;;
		esac
	done
	return 1
}

case "${1:-}" in
reset)
	# A run before may have left sites, with the page's own commands or a hand edit
	# that the command refuses to remove: its file is taken away by hand then.
	for file in "$files"/*.conf; do
		[ -e "$file" ] || continue
		name=$(basename "$file" .conf)
		website remove "$name" --files > /dev/null 2>&1 || rm -f "$file"
	done
	rm -rf "$folders"/*
	service nginx reload > /dev/null 2>&1 || true
	echo reset
	;;
edit)
	file=$(first) || {
		echo "no site file but the PHP site's"
		exit 1
	}
	echo "# lab: kept by hand" >> "$file"
	echo "edited $(basename "$file" .conf)"
	;;
break)
	file=$(first) || {
		echo "no site file but the PHP site's"
		exit 1
	}
	echo "lab_broken_directive on;" >> "$file"
	echo "broken $(basename "$file" .conf)"
	;;
repair)
	file=$(first) || {
		echo "no site file but the PHP site's"
		exit 1
	}
	sed -i '/^lab_broken_directive on;$/d' "$file"
	echo "repaired $(basename "$file" .conf)"
	;;
state)
	sites=0
	for file in "$files"/*.conf; do
		[ -e "$file" ] && sites=$((sites + 1))
	done
	[ -e "$files/$php.conf" ] && there=there || there=missing
	left=0
	for name in plain kept; do
		[ -e "$folders/$name.acme-lab.example" ] && left=$((left + 1))
	done
	echo "sites: $sites, php site: $there, folders left of the removed: $left"
	;;
gone)
	left=""
	for file in "$files"/*.conf; do
		[ -e "$file" ] && left="$left $(basename "$file")"
	done
	for folder in "$folders"/*; do
		[ -e "$folder" ] && left="$left $folder"
	done
	if [ -z "$left" ]; then
		echo gone
	else
		echo "left:$left"
		exit 1
	fi
	;;
*)
	echo "usage: websites.sh reset|edit|break|repair|state|gone"
	exit 2
	;;
esac
