#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's NFS server, the far end of the NFS client tests:
# NFS-Ganesha, which runs in user space (so on any lab PC, its kernel's
# own NFS server or none), as root, in the test's folder (the working
# directory), on port $1, NFS 4.1 and 4.2 only, with no grace period and
# nothing of the lab PC's own NFS settings. It exports two folders:
# /lab, owned by number 1000 (the lab account's on Dukovo) to read and
# write, holding from-lab-pc.txt, with root served as no one; and
# /lab-ro, only to read. It says "ganesha ready" once it listens, and
# runs until the lab ends it.
set -eu

port=$1
work=$(pwd)
mkdir -p export/folder export-ro recovery
echo "from the lab PC" > export/from-lab-pc.txt
echo "inside" > export/folder/inner.txt
echo "only to read" > export-ro/read-only.txt
chown -R 1000:1000 export
chmod 755 export

cat > ganesha.conf <<CONF
NFS_CORE_PARAM {
  NFS_Port = $port;
  Protocols = 4;
  Enable_NLM = false;
  Enable_RQUOTA = false;
}
NFSV4 {
  Graceless = true;
  Minor_Versions = 1, 2;
  # Owners as numbers, as Linux's own server sends them to AUTH_SYS
  # clients: the lab PC has no account for Dukovo's numbers.
  Allow_Numeric_Owners = true;
  Only_Numeric_Owners = true;
  RecoveryBackend = fs;
  RecoveryRoot = "$work/recovery";
}
EXPORT_DEFAULTS {
  SecType = sys;
  Protocols = 4;
  Transports = TCP;
}
EXPORT {
  Export_Id = 1;
  Path = "$work/export";
  Pseudo = /lab;
  Access_Type = RW;
  Squash = Root_Squash;
  FSAL { Name = VFS; }
}
EXPORT {
  Export_Id = 2;
  Path = "$work/export-ro";
  Pseudo = /lab-ro;
  Access_Type = RO;
  Squash = Root_Squash;
  FSAL { Name = VFS; }
}
CONF

# In this script's process group, which the lab ends at the test's end.
# Its log goes to the script's output, which the lab keeps.
ganesha.nfsd -F -f "$work/ganesha.conf" -L /dev/stderr -p "$work/ganesha.pid" -N NIV_EVENT &
i=0
until nc -z 127.0.0.1 "$port" 2> /dev/null; do
	i=$((i + 1))
	if [ "$i" -gt 300 ]; then
		echo "ganesha.nfsd did not listen on port $port"
		exit 1
	fi
	sleep 0.1
done
echo "ganesha ready"
wait
