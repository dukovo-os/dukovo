#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's Samba, the far end of the SMB client tests: smbd run as
# the lab's own account, in the test's folder (the working directory), on
# port $1, with nothing of the lab PC's own Samba settings. Its share,
# `share`, holds from-lab-pc.txt and folder/inner.txt; `ro` shares the
# same folder only to read. The account alice, with the sharing password
# alice-share-pass, is the lab's account under another name (a username
# map), so a test signs in as alice on any lab PC. It says "samba ready"
# once it listens, and runs until the lab ends it.
set -eu

port=$1
work=$(pwd)
me=$(id -un)
mkdir -p share/folder private state lock cache pid log

cat > smb.conf <<EOF
[global]
  server role = standalone server
  smb ports = $port
  disable netbios = yes
  pid directory = $work/pid
  lock directory = $work/lock
  state directory = $work/state
  cache directory = $work/cache
  private dir = $work/private
  ncalrpc dir = $work/state/ncalrpc
  log file = $work/log/log.%m
  passdb backend = tdbsam:$work/private/passdb.tdb
  username map = $work/users.map
  load printers = no
  printing = bsd
  printcap name = /dev/null
  disable spoolss = yes
  server min protocol = SMB2_10

[share]
  path = $work/share
  comment = The lab PC's test share
  read only = no

[ro]
  path = $work/share
  comment = The same folder only to read
  read only = yes
EOF

echo "$me = alice" > users.map
printf 'alice-share-pass\nalice-share-pass\n' |
	pdbedit --configfile="$work/smb.conf" -a -u "$me" -t > /dev/null
echo "from the lab PC" > share/from-lab-pc.txt
echo "inside" > share/folder/inner.txt

# In this script's process group, which the lab ends at the test's end:
# without --no-process-group, smbd would make a group of its own.
smbd --foreground --no-process-group --configfile="$work/smb.conf" &
i=0
until nc -z 127.0.0.1 "$port" 2> /dev/null; do
	i=$((i + 1))
	if [ "$i" -gt 200 ]; then
		echo "smbd did not listen on port $port; its log:"
		cat log/log.smbd
		exit 1
	fi
	sleep 0.1
done
echo "samba ready"
wait
