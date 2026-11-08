#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of the NFS server test: its own kernel's NFS client,
# as root, mounts the root of the machine's nfsd, at $1 port $2, on mnt
# in the test's folder (the working directory) over NFS 4.2, with the
# options Dukovo's nfsd boot test uses. It says "mounted" once the mount
# is there, and keeps it until the lab ends it, which unmounts it, so a
# test that fails half way leaves no mount behind. A kernel with no NFS 4
# client (a container, say) cannot do the test: it exits with 77, which
# skips it.
set -eu

server=$1
port=$2
mkdir -p mnt
if ! grep -qw nfs4 /proc/filesystems && ! modprobe -q nfsv4 2> /dev/null; then
	echo "the lab PC's kernel has no NFS 4 client"
	exit 77
fi
if ! mount -t nfs4 -o "vers=4.2,proto=tcp,port=$port,soft,timeo=100,retrans=3" "$server:/" mnt; then
	echo "the lab PC's kernel did not mount $server:/ through port $port"
	exit 1
fi
trap 'umount -f -l mnt; exit 0' TERM INT
echo "mounted"
while :; do
	sleep 1
done
