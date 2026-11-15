#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of system/ansible: ansible-core of the release that
# `ob test boot --case ansible` pins, in a Python virtual environment under
# the lab's cache, and the collection dukovo.system from the tarball that
# tools/xtask/src/test/ansible.lock pins, running a playbook of
# tools/xtask/src/test/ansible against the machine over SSH, as the lab
# account with the lab's key.
#
#   ansible.sh MACHINE PORT PLAYBOOK [ansible-playbook option...]
#
# MACHINE is the machine's address and PORT the lab PC's port that reaches
# its SSH, as {machine} and {machine-port:22} give them; PLAYBOOK is the
# name of one of the folder's playbooks, python.yml, site.yml or
# update.yml, or the absolute path of a playbook of another test's own, as
# web-console/ansible writes one for each task the console shows, for the
# machine `dukovo`. The lab account's password, which elevate asks for, is
# read from the standard input. The script prints what ansible-playbook
# printed, then `changed: N`, `unreachable: N` and `failed: N` from its
# recap, and exits with its status. It runs in the folder it is started in,
# where it keeps the inventory, the settings and the playbook's log, so a
# test that runs it again and again gives each run a folder of its own.
#
# A lock may name the collection's tarball by an environment variable, to
# try one built from a checkout of dukovo-os/ansible; when that variable is
# not set, the script exits with 77, which skips the test.
set -eu

machine="$1"
port="$2"
playbook="$3"
shift 3
IFS= read -r password

here=$(cd "$(dirname "$0")" && pwd)
tree=$(cd "$here/../../.." && pwd)
lock="$tree/tools/xtask/src/test/ansible.lock"
playbooks="$tree/tools/xtask/src/test/ansible"
core="ansible-core==2.22.0"
python="/applications/org.python.python/bin/python3"
cache="${XDG_CACHE_HOME:-$HOME/.cache}/dukovo-lab/ansible"
lab_key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"
work=$(pwd)

case "$playbook" in
/*) file="$playbook" ;;
*) file="$playbooks/$playbook" ;;
esac
if [ ! -f "$file" ]; then
	echo "no playbook $playbook (the folder's are in $playbooks)"
	exit 2
fi

# A value of the lock: `key = value` lines, with # comments.
pinned() {
	sed -n "s/^$1[[:space:]]*=[[:space:]]*//p" "$lock"
}

# The collection's tarball: the file that an environment variable names, or
# a URL's, fetched once and checked against the lock's SHA-256.
source=$(pinned source)
case "$source" in
env:*)
	variable="${source#env:}"
	eval "tarball=\${$variable:-}"
	if [ -z "$tarball" ]; then
		echo "ansible: skipped: $variable is not set; it names the tarball of the collection dukovo.system that tools/xtask/src/test/ansible.lock pins"
		exit 77
	fi
	;;
https://*)
	sum=$(pinned sha256)
	tarball="$cache/dukovo-system-$(echo "$sum" | cut -c 1-16).tar.gz"
	if [ ! -f "$tarball" ]; then
		mkdir -p "$cache"
		curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$tarball.partial" "$source"
		mv "$tarball.partial" "$tarball"
	fi
	if [ "$(sha256sum "$tarball" | cut -d ' ' -f 1)" != "$sum" ]; then
		echo "$source has not the pinned SHA-256 $sum"
		exit 1
	fi
	;;
*)
	echo "ansible.lock: source $source is neither env:NAME nor an https URL"
	exit 1
	;;
esac
if [ ! -f "$tarball" ]; then
	echo "no collection tarball: $tarball is no file"
	exit 1
fi

# ansible-core, once, on the first Python at 3.13 or later, which it needs,
# as tools/xtask/src/ansible.rs picks it: Linux Mint 22's python3 is 3.12,
# and a newer one is there only by another name or from uv. Without one the
# test is skipped.
venv="$cache/venv-$core"
if [ ! -f "$venv/.installed" ]; then
	controller=
	for candidate in python3 python3.15 python3.14 python3.13; do
		if "$candidate" -c 'import sys; sys.exit(0 if sys.version_info >= (3, 13) else 1)' 2>/dev/null; then
			controller=$candidate
			break
		fi
	done
	if [ -z "$controller" ] && command -v uv >/dev/null 2>&1; then
		controller=$(uv python find '>=3.13' 2>/dev/null) || controller=
	fi
	if [ -z "$controller" ]; then
		echo "ansible: skipped: $core needs Python 3.13 or later on the lab PC, which has none; on Linux Mint 22, \`uv python install 3.13\` gives one"
		exit 77
	fi
	rm -rf "$venv"
	mkdir -p "$cache"
	"$controller" -m venv "$venv"
	"$venv/bin/python" -m pip install --quiet --disable-pip-version-check "$core"
	echo "$core" > "$venv/.installed"
fi

# The collection, once for each tarball.
tarball_sum=$(sha256sum "$tarball" | cut -d ' ' -f 1 | cut -c 1-16)
collections="$cache/collections-$tarball_sum"
if [ ! -f "$collections/.installed" ]; then
	rm -rf "$collections"
	"$venv/bin/ansible-galaxy" collection install --force -p "$collections" "$tarball" > /dev/null
	: > "$collections/.installed"
fi

# The machine's host key as it shows itself now, the only one the connection
# takes.
hosts="$work/known_hosts"
ssh-keyscan -T 10 -t ed25519 -p "$port" "$machine" > "$hosts" 2> /dev/null
if [ ! -s "$hosts" ]; then
	echo "no host key: ssh-keyscan read none from the machine"
	exit 1
fi

# The inventory and the settings, with the password in single quotes.
quoted_password=$(printf '%s' "$password" | sed "s/'/''/g")
cat > "$work/inventory.yml" <<INVENTORY
all:
  hosts:
    dukovo:
      ansible_host: '$machine'
      ansible_port: $port
      ansible_user: lab
      ansible_ssh_private_key_file: '$lab_key'
      ansible_ssh_common_args: '-F /dev/null -o StrictHostKeyChecking=yes -o UserKnownHostsFile=$hosts -o IdentitiesOnly=yes -o BatchMode=yes -o LogLevel=ERROR -o ConnectTimeout=30'
      ansible_python_interpreter: '$python'
      ansible_become_password: '$quoted_password'
INVENTORY
cat > "$work/ansible.cfg" <<SETTINGS
[defaults]
inventory = $work/inventory.yml
host_key_checking = True
nocolor = True
forks = 1
timeout = 120
local_tmp = $work/tmp
home = $work/home
deprecation_warnings = False

[ssh_connection]
ssh_args = -o ControlMaster=no
retries = 3
SETTINGS
chmod 600 "$work/inventory.yml"

export ANSIBLE_CONFIG="$work/ansible.cfg"
export ANSIBLE_COLLECTIONS_PATH="$collections"
# Setup's other subsets read Linux's /proc and /sys.
export ANSIBLE_GATHER_SUBSET=min
export PYTHONUNBUFFERED=1

log="$work/$(basename "$file").log"
status=0
"$venv/bin/ansible-playbook" "$file" "$@" > "$log" 2>&1 || status=$?
cat "$log"
recap=$(sed -n '/^PLAY RECAP/,$p' "$log" | grep '^dukovo ' | tail -n 1 || true)
for key in changed unreachable failed; do
	count=$(printf '%s\n' "$recap" | sed -n "s/.*[[:space:]]$key=\([0-9][0-9]*\).*/\1/p")
	echo "$key: ${count:-none}"
done
exit "$status"
